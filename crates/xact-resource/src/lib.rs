//! Real, kernel-native resource enforcement for `SPEND`/`SAVE` (spec
//! section 22; Xact–Mesut Integration Phase 5), via Linux cgroup v2. This
//! is the only crate that touches `/sys/fs/cgroup`; `xact-process` calls
//! [`apply`] before spawning a process, and nothing else needs to know
//! cgroups exist.
//!
//! # Scope and honesty
//!
//! - Percentages are interpreted directly in cgroup v2's own native
//!   terms: `cpu_percent` is a percentage of *one* CPU core (matching
//!   `cpu.max`'s quota/period definition — `40` means 40% of one core,
//!   not 40% of total system capacity across all cores); `ram_percent`
//!   is a percentage of total system RAM (from `/proc/meminfo`).
//! - Each call to [`apply`] creates its own independent cgroup. Two
//!   concurrent `£ RUN`s under the same `! SPEND 40%CPU` each get their
//!   *own* 40%-of-one-core cap — the cap is per invocation, not a shared
//!   budget aggregated across everything running under that policy. Real
//!   aggregate budgeting (one shared parent cgroup enforcing a combined
//!   cap across sibling branches) is future work, not implemented here —
//!   a real limitation, stated plainly rather than hidden.
//! - Requires a writable, delegated cgroup v2 subtree with the needed
//!   controllers enabled — true on a modern systemd-managed Linux session
//!   (systemd delegates a `user@<uid>.service` scope to the user), which
//!   this crate detects by walking `/proc/self/cgroup` rather than
//!   assuming a UID-based path. If that delegation isn't available, or
//!   the requested resource isn't `CPU`/`RAM`, or a needed controller
//!   isn't enabled there, [`apply`] returns `Err`. Per Xact–Mesut
//!   Integration section 10 — "If Mesut cannot honour a mandatory
//!   constraint, execution SHALL fail before the workload begins,"
//!   generalized here to whichever layer is actually responsible for
//!   enforcement — nothing in this crate ever runs a constrained command
//!   unconstrained while pretending otherwise.
//!
//! # Network isolation (`! WITHOUT network`)
//!
//! When the budget denies [`Capability::Network`], [`apply`] also moves the
//! child into a fresh, unprivileged user namespace plus a new, empty network
//! namespace before `exec` (`unshare(CLONE_NEWUSER | CLONE_NEWNET)`, with the
//! caller's own uid/gid mapped through so file ownership is unchanged). The
//! new namespace has no interfaces except a down loopback, so the program
//! can reach nothing, not even `localhost`; the kernel enforces this, not
//! Xact. If the host forbids unprivileged user namespaces, `spawn` fails
//! and the workload never starts — it is never run with network access
//! instead. [`network_isolation_available`] checks this ahead of time.

use std::fmt;
use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use xact_ast::{Capability, ResourceBudget};

#[derive(Debug)]
pub struct ResourceError(String);

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ResourceError {}

impl From<io::Error> for ResourceError {
    fn from(err: io::Error) -> Self {
        Self(err.to_string())
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Holds a spawned process's dedicated cgroup alive until it exits, then
/// removes it. Cleanup is best-effort: a cgroup can only be removed once
/// it has no live processes, which is true by the time a caller drops
/// this (after waiting for the child) — a failure here would mean
/// something else is oddly still using it, not worth panicking over.
pub struct ResourceGuard {
    path: PathBuf,
}

impl ResourceGuard {
    /// The real cgroup path this budget was enforced through — exposed
    /// for tests (and diagnostics) that want to verify the actual
    /// `cpu.max`/`memory.max` file contents, not just that `apply`
    /// returned `Ok`.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

/// Applies `budget` to `command`, so the process it's about to spawn is
/// kernel-enforced, not just described. Returns `Ok(None)` when `budget`
/// states no constraint — `command` is left completely untouched,
/// identical to pre-Phase-5 behavior. The returned [`ResourceGuard`]
/// (when `Some`) should be kept alive until the spawned child has been
/// waited on; dropping it earlier only affects cleanup timing, not
/// enforcement (enforcement belongs to the kernel, tied to the cgroup
/// file existing, not to this guard's lifetime).
pub fn apply(command: &mut Command, budget: &ResourceBudget) -> Result<Option<ResourceGuard>, ResourceError> {
    // Registered after the cgroup join below (pre_exec hooks run in
    // registration order), so the child is already in its cgroup when it
    // enters the new user namespace.
    let guard = apply_quotas(command, budget)?;
    if budget.denies(Capability::Network) {
        isolate_network(command);
    }
    Ok(guard)
}

fn apply_quotas(command: &mut Command, budget: &ResourceBudget) -> Result<Option<ResourceGuard>, ResourceError> {
    if budget.is_empty() {
        return Ok(None);
    }
    if !budget.unenforceable.is_empty() {
        return Err(ResourceError(format!(
            "cannot enforce a resource policy for {} — Xact only enforces CPU and RAM today.",
            budget.unenforceable.join(", ")
        )));
    }

    let base = delegated_cgroup_base()?;
    let path = base.join(format!("xact-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&path)?;
    let guard = ResourceGuard { path: path.clone() };

    let available = fs::read_to_string(path.join("cgroup.controllers"))?;

    if let Some(cpu_percent) = budget.cpu_percent {
        require_controller(&available, "cpu", &path)?;
        let period_us: u64 = 100_000;
        let quota_us = period_us * u64::from(cpu_percent) / 100;
        fs::write(path.join("cpu.max"), format!("{quota_us} {period_us}"))?;
    }
    if let Some(ram_percent) = budget.ram_percent {
        require_controller(&available, "memory", &path)?;
        let cap_bytes = total_ram_bytes()? * u64::from(ram_percent) / 100;
        fs::write(path.join("memory.max"), cap_bytes.to_string())?;
    }

    // Opened before `spawn`'s fork, so the forked child inherits this
    // exact file descriptor. Moved into the closure (rather than just its
    // raw fd) so the `File` — and the fd it owns — stays open for as long
    // as `command` does, spanning the fork/exec `pre_exec` runs between;
    // otherwise it would close here, at the end of `apply`, well before
    // the caller ever calls `spawn`. Reading its fd back out inside the
    // closure is a plain field access, not a syscall, so this is still
    // sound to run between fork and exec — no path lookups, no
    // allocation, after forking.
    let procs_file = fs::OpenOptions::new().write(true).open(path.join("cgroup.procs"))?;

    unsafe {
        command.pre_exec(move || {
            let pid = std::process::id();
            let mut buf = [0u8; 10];
            let bytes = format_u32(pid, &mut buf);
            if libc::write(procs_file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    Ok(Some(guard))
}

/// Adds a `pre_exec` hook that unshares a new user + network namespace in
/// the child and maps the caller's uid/gid through unchanged. Everything
/// that allocates happens here, before `fork`; the hook itself only makes
/// raw syscalls on preformatted buffers.
fn isolate_network(command: &mut Command) {
    // SAFETY: geteuid/getegid cannot fail and have no preconditions.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    let uid_map = format!("{uid} {uid} 1\n");
    let gid_map = format!("{gid} {gid} 1\n");

    // SAFETY: the closure runs in the forked child before exec, where only
    // async-signal-safe operations are allowed. It calls `unshare`, `open`,
    // `write`, and `close` on NUL-terminated literals and buffers allocated
    // before the fork, and allocates nothing itself.
    unsafe {
        command.pre_exec(move || {
            if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                return Err(io::Error::last_os_error());
            }
            // `setgroups` must be denied before an unprivileged process
            // may write its own gid_map (user_namespaces(7)).
            write_proc(c"/proc/self/setgroups", b"deny")?;
            write_proc(c"/proc/self/uid_map", uid_map.as_bytes())?;
            write_proc(c"/proc/self/gid_map", gid_map.as_bytes())?;
            Ok(())
        });
    }
}

/// `open` + `write` + `close` with raw syscalls, for use between fork and exec.
fn write_proc(path: &std::ffi::CStr, data: &[u8]) -> io::Result<()> {
    // SAFETY: `path` is NUL-terminated and `data` is a live slice.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let written = libc::write(fd, data.as_ptr().cast(), data.len());
        let err = io::Error::last_os_error();
        libc::close(fd);
        if written < 0 {
            return Err(err);
        }
    }
    Ok(())
}

/// Whether this host lets Xact run a process in its own network namespace
/// — i.e. whether `! WITHOUT network` can be enforced for spawned
/// processes. Checked once, for real, by spawning `true` under isolation.
pub fn network_isolation_available() -> Result<(), String> {
    static AVAILABLE: OnceLock<Result<(), String>> = OnceLock::new();
    AVAILABLE
        .get_or_init(|| {
            let mut command = Command::new("true");
            isolate_network(&mut command);
            match command.status() {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(format!("an isolated test process exited with {status}")),
                Err(err) => Err(format!("unprivileged network namespaces are unavailable here ({err})")),
            }
        })
        .clone()
}

/// Whether this host currently has a default route (IPv4 or IPv6) — the
/// availability check behind `! WITH network`.
pub fn has_default_route() -> bool {
    let ipv4 = fs::read_to_string("/proc/net/route")
        .is_ok_and(|table| table.lines().skip(1).any(|line| line.split_whitespace().nth(1) == Some("00000000")));
    let ipv6 = fs::read_to_string("/proc/net/ipv6_route").is_ok_and(|table| {
        table.lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next() == Some("00000000000000000000000000000000")
                && fields.next() == Some("00")
                && fields.last() != Some("lo")
        })
    });
    ipv4 || ipv6
}

fn require_controller(available: &str, controller: &str, path: &Path) -> Result<(), ResourceError> {
    if available.split_whitespace().any(|c| c == controller) {
        Ok(())
    } else {
        Err(ResourceError(format!(
            "the '{controller}' cgroup controller isn't available under {} — cannot enforce this resource.",
            path.display()
        )))
    }
}

/// Finds this process's own delegated cgroup v2 subtree by walking
/// `/proc/self/cgroup` rather than assuming a UID-based path — the
/// current process is already running somewhere under it, so the
/// delegated ancestor is a real, current prefix of that path, ending at
/// the `user@<uid>.service` component systemd creates and delegates to
/// the user.
fn delegated_cgroup_base() -> Result<PathBuf, ResourceError> {
    let contents = fs::read_to_string("/proc/self/cgroup")?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("0::"))
        .ok_or_else(|| ResourceError("this system is not using the cgroup v2 unified hierarchy".into()))?;
    let cgroup_path = &line["0::".len()..];

    let mut prefix = PathBuf::from("/sys/fs/cgroup");
    for component in cgroup_path.split('/').filter(|c| !c.is_empty()) {
        prefix.push(component);
        if component.starts_with("user@") && component.ends_with(".service") {
            return Ok(prefix);
        }
    }

    Err(ResourceError(
        "no delegated cgroup v2 subtree found (expected a systemd user@<uid>.service scope) — \
         cannot enforce SPEND/SAVE without one."
            .into(),
    ))
}

fn total_ram_bytes() -> Result<u64, ResourceError> {
    let contents = fs::read_to_string("/proc/meminfo")?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .ok_or_else(|| ResourceError("could not find MemTotal in /proc/meminfo".into()))?;
    let kb: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| ResourceError(format!("could not parse /proc/meminfo line: {line}")))?;
    Ok(kb * 1024)
}

/// Formats `n` into `buf` without allocating — safe to call from a
/// `pre_exec` closure running in a forked child before `exec`.
fn format_u32(mut n: u32, buf: &mut [u8; 10]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &buf[i..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_budget_leaves_the_command_untouched() {
        let mut command = Command::new("true");
        let guard = apply(&mut command, &ResourceBudget::default()).expect("empty budget should never fail");
        assert!(guard.is_none());
    }

    #[test]
    fn unenforceable_resource_fails_before_spawning() {
        let budget = ResourceBudget { unenforceable: vec!["GPU".to_string()], ..Default::default() };
        let mut command = Command::new("true");
        assert!(apply(&mut command, &budget).is_err());
    }

    #[test]
    #[ignore = "live: requires cgroup v2 delegation"]
    fn cpu_budget_writes_a_real_cpu_max_file_and_cleans_up_after() {
        let budget = ResourceBudget { cpu_percent: Some(40), ..Default::default() };
        let mut command = Command::new("true");

        let guard = apply(&mut command, &budget)
            .expect("cgroup enforcement should be available on this system")
            .expect("a non-empty budget should return a guard");
        let cgroup_path = guard.path().to_path_buf();

        let cpu_max = fs::read_to_string(cgroup_path.join("cpu.max")).expect("cpu.max should exist");
        assert_eq!(cpu_max.trim(), "40000 100000");

        let mut child = command.spawn().expect("true should launch");
        child.wait().expect("true should exit");
        drop(guard);

        assert!(!cgroup_path.exists(), "the cgroup should be removed once the guard is dropped");
    }

    #[test]
    #[ignore = "live: requires cgroup v2 delegation"]
    fn ram_budget_writes_an_absolute_byte_cap() {
        let budget = ResourceBudget { ram_percent: Some(50), ..Default::default() };
        let mut command = Command::new("true");

        let guard = apply(&mut command, &budget)
            .expect("cgroup enforcement should be available")
            .expect("a non-empty budget should return a guard");
        let memory_max: u64 = fs::read_to_string(guard.path().join("memory.max"))
            .expect("memory.max should exist")
            .trim()
            .parse()
            .expect("memory.max should be a plain byte count");

        let total = total_ram_bytes().unwrap();
        // `/proc/meminfo` can change by one memory page between `apply`
        // taking its snapshot and this assertion taking another (notably
        // under memory hotplug/ballooning). The kernel-written cgroup value
        // must therefore be the requested half within one page, rather than
        // requiring two independently sampled volatile values to be bitwise
        // identical.
        assert!(
            memory_max.abs_diff(total / 2) <= 4096,
            "memory.max {memory_max} was not approximately half of sampled total RAM {total}"
        );

        let mut child = command.spawn().expect("true should launch");
        child.wait().expect("true should exit");
    }

    /// Proves the cap is real, not just a file write nobody honours: a
    /// process capped at 20% of one CPU core, spinning a busy loop for a
    /// fixed number of iterations, must take meaningfully longer wall
    /// clock than the same loop uncapped — the kernel is really throttling
    /// it. Uses `sh -c` with a shell counting loop so this doesn't depend
    /// on any extra binary being installed.
    #[test]
    #[ignore = "live: requires cgroup v2 delegation"]
    fn cpu_cap_actually_throttles_a_real_process() {
        let busy_loop = "i=0; while [ $i -lt 300000 ]; do i=$((i+1)); done";

        let uncapped_start = std::time::Instant::now();
        Command::new("sh").arg("-c").arg(busy_loop).status().expect("sh should launch");
        let uncapped = uncapped_start.elapsed();

        let budget = ResourceBudget { cpu_percent: Some(10), ..Default::default() };
        let mut command = Command::new("sh");
        command.arg("-c").arg(busy_loop);
        let guard = apply(&mut command, &budget)
            .expect("cgroup enforcement should be available")
            .expect("a non-empty budget should return a guard");

        let capped_start = std::time::Instant::now();
        command.status().expect("sh should launch under the cap");
        let capped = capped_start.elapsed();
        drop(guard);

        assert!(
            capped > uncapped * 2,
            "a 10%-of-one-core cap should make the same busy loop take well over 2x longer \
             (uncapped: {uncapped:?}, capped: {capped:?}) — the cap doesn't look real"
        );
    }

    #[test]
    #[ignore = "live: requires unprivileged user namespaces"]
    fn denied_network_puts_the_process_in_an_empty_network_namespace() {
        let budget = ResourceBudget { denied: vec![Capability::Network], ..Default::default() };
        // Inside the new namespace /proc/net/route has only its header
        // line: there is no interface to route through.
        let mut command = Command::new("sh");
        command.args(["-c", "test $(wc -l < /proc/net/route) -le 1 && test $(id -u) = \"$EXPECTED_UID\""]);
        command.env("EXPECTED_UID", unsafe { libc::geteuid() }.to_string());
        let guard = apply(&mut command, &budget).unwrap();
        assert!(guard.is_none(), "a capability denial alone needs no cgroup");
        assert!(command.status().unwrap().success(), "the child must see no routes and keep its own uid");
    }

    #[test]
    fn network_is_untouched_when_not_denied() {
        let mut command = Command::new("true");
        assert!(apply(&mut command, &ResourceBudget::default()).unwrap().is_none());
        assert!(command.status().unwrap().success());
    }
}
