//! The single, real-world resolution engine both execution
//! (`xact-process`, `xact-planner`) and autocomplete (`xact-cli`, via
//! `xact-completion-graph`) call into. Neither of those ever re-derives a
//! search-directory list or a fallback rule on its own — they ask this
//! crate, which asks the same Padagonia-backed ontology graph either way.
//! Before this crate existed, `xact-process` had its own `SYSTEM_BIN_DIRS`
//! + directory scan, `xact-planner` had its own `~` expansion, and an
//! earlier draft of autocomplete had a *third*, independent copy of both —
//! three places that had to be kept in sync by hand, and weren't
//! guaranteed to be. That's exactly the failure mode this crate closes:
//! there is now exactly one fact ("what directories does `OUR` search, in
//! what order") and exactly one algorithm per operation ("does this
//! directory list contain a match"), and every caller — whether it wants
//! one definitive answer (execution) or every matching candidate
//! (completion) — reads the same graph and calls the same functions.
//!
//! The ontology graph itself only ever holds two kinds of real fact:
//! which [`ResolverKind`] applies to which [`xact_ast::Verb`], and which
//! real directories `OUR`'s system-binary search consults, in what order.
//! It does not encode the grammar (that stays `xact-parser`'s job, per
//! `xact-completion`'s module doc) and it does not encode *when* a
//! fallback fires (that's real control flow — "try `$PATH` first, only
//! fall back to `OUR`'s directories on a genuine miss" — which lives in
//! [`search_dirs_for`]/[`list_binaries`] as ordinary Rust, reading facts
//! out of the graph rather than hardcoding them a second time).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use padagonia::{NodeId, Provenance, QueryEngine, Scalar, Store};
use xact_ast::{OwnershipKind, Verb};

/// The real-world sources this build knows how to resolve against. Every
/// variant has an actual implementation below (`$PATH`/filesystem scans) —
/// this is the exhaustive set of resolvers this build genuinely has, not
/// an extensible plugin list.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ResolverKind {
    InstalledBinary,
    FilesystemPath,
}

impl ResolverKind {
    const ALL: [ResolverKind; 2] = [ResolverKind::InstalledBinary, ResolverKind::FilesystemPath];

    fn label(self) -> &'static str {
        match self {
            ResolverKind::InstalledBinary => "installed_binary",
            ResolverKind::FilesystemPath => "filesystem_path",
        }
    }
}

/// The standard POSIX/Linux system binary directories, in search order —
/// the same list most Linux `sudo` configurations use as `secure_path`.
/// This is the one place this literal list is written down; everything
/// else reads it back out of the graph via [`system_bin_dirs`].
const SYSTEM_BIN_DIRS: &[&str] = &["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"];

fn provenance(evidence: &str) -> Provenance {
    Provenance::new("xact-resolve", "hand-authored", 1.0, 0.0, 0, vec![evidence.to_string()])
}

/// Builds the ontology graph fresh: a couple dozen nodes/edges derived
/// entirely from [`xact_ast::Verb::ALL`] and [`SYSTEM_BIN_DIRS`] — cheap
/// enough to rebuild per call, and sidesteps needing `Store` to be shared
/// across threads or kept alive between calls.
fn build_graph() -> (Store, HashMap<&'static str, NodeId>) {
    let mut store = Store::new();

    let mut resolver_nodes = HashMap::new();
    for resolver in ResolverKind::ALL {
        let id = store.add_node(
            "Resolver",
            vec![("kind", Scalar::String(resolver.label().to_string()))],
            None,
            provenance("Resolver catalog: the real-world sources this build can query."),
        );
        resolver_nodes.insert(resolver.label(), id);
    }

    let mut verb_nodes = HashMap::new();
    for verb in Verb::ALL {
        let id = store.add_node(
            "Verb",
            vec![("name", Scalar::String(verb.as_str().to_string()))],
            None,
            provenance("Verb catalog: xact_ast::Verb::ALL."),
        );
        verb_nodes.insert(verb.as_str(), id);
    }

    let filesystem_path = resolver_nodes[ResolverKind::FilesystemPath.label()];
    for verb in [
        Verb::See,
        Verb::Edit,
        Verb::Move,
        Verb::Copy,
        Verb::Paste,
        Verb::Cut,
        Verb::Delete,
        Verb::Create,
        Verb::Bound,
    ] {
        store.add_edge(
            verb_nodes[verb.as_str()],
            filesystem_path,
            "resolves_via",
            vec![],
            None,
            provenance("This verb's operand/destination is a filesystem path (xact-ast::Operand::Owned)."),
        );
    }

    let installed_binary = resolver_nodes[ResolverKind::InstalledBinary.label()];
    store.add_edge(
        verb_nodes[Verb::Run.as_str()],
        installed_binary,
        "resolves_via",
        vec![],
        None,
        provenance("RUN's operand names a real binary — MY searches $PATH, OUR the system directories below."),
    );

    for (order, dir) in SYSTEM_BIN_DIRS.iter().enumerate() {
        let dir_node = store.add_node(
            "Directory",
            vec![("path", Scalar::String((*dir).to_string())), ("order", Scalar::I64(order as i64))],
            None,
            provenance("OUR's real system binary directories, in search order."),
        );
        store.add_edge(
            installed_binary,
            dir_node,
            "searches",
            vec![("order", Scalar::I64(order as i64))],
            None,
            provenance("OUR's real system binary directories, in search order."),
        );
    }

    (store, verb_nodes)
}

/// The resolver kinds registered for `verb`, per the ontology graph — the
/// same lookup `xact-cli`'s autocomplete uses to decide whether a verb's
/// trailing operand is worth resolving at all.
pub fn resolvers_for(verb: Verb) -> Vec<ResolverKind> {
    let (store, verb_nodes) = build_graph();
    let Some(&node) = verb_nodes.get(verb.as_str()) else {
        return Vec::new();
    };
    let query = QueryEngine::new(&store);
    query
        .outgoing(node, None)
        .into_iter()
        .filter_map(|edge| store.nodes().get(&edge.dst))
        .filter_map(|resolver_node| scalar_string(&store, resolver_node, "kind"))
        .filter_map(|kind| ResolverKind::ALL.into_iter().find(|r| r.label() == kind))
        .collect()
}

/// `OUR`'s real system binary search directories, in order, straight out
/// of the graph — the single source both [`search_dirs_for`] (execution
/// and completion's shared fallback rule) and [`list_binaries`] read.
pub fn system_bin_dirs() -> Vec<PathBuf> {
    let (store, _) = build_graph();
    let query = QueryEngine::new(&store);
    let Some(installed_binary) = find_resolver_node(&store, ResolverKind::InstalledBinary) else {
        return Vec::new();
    };

    let mut dirs: Vec<(i64, PathBuf)> = query
        .outgoing(installed_binary, None)
        .into_iter()
        .filter_map(|edge| {
            let dir_node = store.nodes().get(&edge.dst)?;
            let path = scalar_string(&store, dir_node, "path")?;
            let order = scalar_i64(&store, dir_node, "order").unwrap_or(0);
            Some((order, PathBuf::from(path)))
        })
        .collect();
    dirs.sort_by_key(|(order, _)| *order);
    dirs.into_iter().map(|(_, path)| path).collect()
}

fn find_resolver_node(store: &Store, kind: ResolverKind) -> Option<NodeId> {
    store.nodes().iter().find_map(|(id, node)| {
        let s = scalar_string(store, node, "kind")?;
        (s == kind.label()).then_some(*id)
    })
}

fn scalar_string(store: &Store, node: &padagonia::Node, key: &str) -> Option<String> {
    node.properties.iter().find_map(|(k, v)| {
        let name = store.string_table().resolve(k.0)?;
        if name != key {
            return None;
        }
        match v {
            Scalar::String(s) => Some(s.clone()),
            _ => None,
        }
    })
}

fn scalar_i64(store: &Store, node: &padagonia::Node, key: &str) -> Option<i64> {
    node.properties.iter().find_map(|(k, v)| {
        let name = store.string_table().resolve(k.0)?;
        if name != key {
            return None;
        }
        match v {
            Scalar::I64(n) => Some(*n),
            _ => None,
        }
    })
}

/// The real search directories `domain` consults for a binary lookup, in
/// priority order — the exact same list `xact-process`'s `RUN` fallback
/// and this crate's own [`list_binaries`] both read: `THEIR` never
/// consults a directory list ($PATH only, no fallback), `MY` and `OUR`
/// both fall back to (or, for `OUR`, search directly) [`system_bin_dirs`].
pub fn search_dirs_for(domain: OwnershipKind) -> Vec<PathBuf> {
    match domain {
        OwnershipKind::My | OwnershipKind::Our => system_bin_dirs(),
        OwnershipKind::Their => Vec::new(),
    }
}

/// The first directory in `dirs` containing an executable file named
/// exactly `program`, if any — a real filesystem check, shared by
/// execution (looking for one definitive answer) and, transitively via
/// [`list_binaries`], completion (enumerating every match).
pub fn find_binary_in_dirs(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(program)).find(|candidate| candidate.is_file())
}

/// Expands a leading `~/` or bare `~` against the real `$HOME`; every
/// other (relative or absolute) path is returned as-is, matching normal
/// shell convention. The one real implementation — `xact-planner` calls
/// this for every path it resolves before executing anything, and
/// `xact-completion-graph` calls it for the same path before listing what
/// real filesystem entries match.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

/// `MY`'s real default anchor for a bare relative path (spec section 10):
/// `£ SEE MY project/README.md` means `~/project/README.md`, the same as
/// if `~/` had actually been typed — not a path relative to whatever
/// directory Xact happens to be running from. Only `RUN` is excluded
/// (`home_relative_default: false`): its `MY` path names a program for
/// `$PATH` search (see [`search_dirs_for`]/`xact-process`), not a
/// filesystem location, so treating `£ RUN MY chrome` as `~/chrome` would
/// break real binary resolution rather than default it sensibly. `OUR`
/// and `THEIR` are never touched here — this default is `MY`-specific.
///
/// Only applies to a genuinely bare reference: a path already starting
/// with `/` (explicitly absolute) or `~` (already home-relative, which is
/// this same default spelled out by hand) is returned unchanged. The
/// result is still an ordinary path string, not yet expanded — callers
/// that need a real `PathBuf` still call [`expand_tilde`] on it after
/// (`xact-completion-graph` instead feeds it straight into
/// [`list_path_entries`], since that keeps `~/`-style candidate text
/// human-readable rather than the fully expanded absolute form).
pub fn anchor_my_default(path: &str, domain: OwnershipKind, home_relative_default: bool) -> String {
    if home_relative_default && domain == OwnershipKind::My && !path.starts_with('/') && !path.starts_with('~') {
        format!("~/{path}")
    } else {
        path.to_string()
    }
}

/// Real installed binaries whose name starts with `prefix`, searched under
/// exactly the directories `domain` would use to resolve *one* program
/// (see [`search_dirs_for`]) — plus `$PATH` itself for `MY`/`THEIR`, since
/// that's genuinely part of their real search surface (an OS `execvp`-style
/// `$PATH` search doesn't enumerate — it only ever answers "does this one
/// name resolve" — so listing candidates has to walk `$PATH`'s directories
/// itself; `OUR` deliberately never consults `$PATH` at all, matching
/// `xact-process`'s `our_domain_never_consults_path` behavior).
pub fn list_binaries(prefix: &str, domain: OwnershipKind) -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if domain != OwnershipKind::Our {
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
    }
    for dir in search_dirs_for(domain) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    let mut matches: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !name.starts_with(prefix) || !is_executable_file(&entry) {
                continue;
            }
            matches.push(name);
        }
    }
    matches.sort();
    matches.dedup();
    matches.truncate(50);
    matches
}

/// Real matching entries under whatever directory `partial` names so far —
/// a genuine `read_dir` against `expand_tilde(partial)`'s parent, not a
/// fabricated listing. Directories get a trailing `/` so the next
/// keystroke can keep descending.
pub fn list_path_entries(partial: &str) -> Vec<String> {
    let (dir_part, name_prefix) = match partial.rfind('/') {
        Some(idx) => (&partial[..=idx], &partial[idx + 1..]),
        None => ("", partial),
    };
    let dir = expand_tilde(dir_part);
    let dir = if dir_part.is_empty() { PathBuf::from(".") } else { dir };

    let mut matches: Vec<String> = gls_listing(&dir)
        .into_iter()
        .filter(|(name, _)| name.starts_with(name_prefix))
        .map(|(name, is_dir)| {
            let mut candidate = format!("{dir_part}{name}");
            if is_dir {
                candidate.push('/');
            }
            candidate
        })
        .collect();
    matches.sort();
    matches.truncate(50);
    matches
}

/// How long [`run_gls`] waits for one real `gls` invocation before giving
/// up and reporting no candidates. `gls` ("filesystem meaning,
/// progressively revealed") is not a fast lister — even with
/// `--no-context` (the default), it runs a real discovery + BART sizing +
/// type-classification + Padagonia-indexing pipeline for every entry.
/// Measured directly: `~400` real entries in `$HOME` finished in ~0.8s;
/// `~2600` entries in `/usr/bin` took ~21s. A live, per-keystroke REPL
/// cannot block on the latter, so a slow directory degrades to "no
/// dynamic candidates this time" rather than freezing input — the static
/// `<path>` placeholder from `xact-completion` is still shown either way.
const GLS_TIMEOUT: Duration = Duration::from_millis(1500);

/// One real `gls` listing, cached per resolved directory and reused as
/// long as the directory's own mtime (which the kernel bumps whenever an
/// entry is added, removed, or renamed inside it) hasn't changed — the
/// same "fetch once when a new directory is entered, then filter locally
/// as more characters narrow it" behavior a real interactive listing
/// should have, and the only way `gls`'s real latency (see
/// [`GLS_TIMEOUT`]) stays off the hot path of every keystroke.
static GLS_CACHE: LazyLock<Mutex<HashMap<PathBuf, CachedListing>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct CachedListing {
    mtime: SystemTime,
    entries: Vec<(String, bool)>,
}

fn gls_listing(dir: &Path) -> Vec<(String, bool)> {
    let mtime = std::fs::metadata(dir).and_then(|m| m.modified()).ok();

    if let Some(mtime) = mtime {
        if let Some(cached) = GLS_CACHE.lock().unwrap().get(dir) {
            if cached.mtime == mtime {
                return cached.entries.clone();
            }
        }
    }

    let entries = run_gls(dir);
    if let Some(mtime) = mtime {
        GLS_CACHE.lock().unwrap().insert(dir.to_path_buf(), CachedListing { mtime, entries: entries.clone() });
    }
    entries
}

/// Real direct children of `dir` (name, is-directory), via the real `gls`
/// binary — the same tool `£ SEE` itself uses to show a directory,
/// composed rather than reimplemented (spec section 3/4/6's "Xact
/// composes real sibling tools" principle). `--output json` is the only
/// way to get `gls`'s real classification back as structured data instead
/// of an ANSI-formatted grid meant for a human terminal.
///
/// `gls`'s JSON stream has a real, confirmed quirk this parsing works
/// around: a `discovery` event's JSON object contains two `"kind"` keys —
/// the outer event-type tag (`"discovery"`) and, later in the same
/// object, the entry's own type (`"file"`/`"directory"`). Verified
/// directly (both Rust's `serde_json::Value` and Python's `json` module
/// silently keep only the *second* occurrence when parsing a JSON object
/// with a duplicate key), so `"kind"` on a parsed event always reads as
/// the entry type here, never the event tag. `"basename"`+`"relative"`
/// (present only on `discovery` events, confirmed against a real `gls
/// --output json` run) is what actually identifies the line as one.
fn run_gls(dir: &Path) -> Vec<(String, bool)> {
    let mut child = match std::process::Command::new("gls")
        .arg(dir)
        .args(["--no-animate", "--output", "json", "--depth", "1", "--hidden"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };

    let Some(mut stdout) = child.stdout.take() else {
        return Vec::new();
    };
    // Drained on a dedicated thread rather than after `wait()`: a large
    // directory's JSON output can exceed the OS pipe buffer, and `gls`
    // would then block writing to a full pipe while this side blocks
    // waiting for it to exit -- a real deadlock, not a hypothetical one,
    // for exactly the large-directory case `GLS_TIMEOUT` exists to bound.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let result = match rx.recv_timeout(GLS_TIMEOUT) {
        Ok(output) => parse_discovery_events(&output),
        Err(_) => Vec::new(),
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn parse_discovery_events(output: &str) -> Vec<(String, bool)> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|event| {
            let basename = event.get("basename")?.as_str()?;
            event.get("relative")?.as_str()?;
            let is_dir = event.get("kind").and_then(|k| k.as_str()) == Some("directory");
            Some((basename.to_string(), is_dir))
        })
        .collect()
}

#[cfg(unix)]
fn is_executable_file(entry: &std::fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = entry.metadata() else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_file(entry: &std::fs::DirEntry) -> bool {
    entry.metadata().map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// `std::env::set_var("PATH", ...)` mutates real process-wide state —
    /// shared across every test thread, not just the test that set it.
    /// Confirmed the hard way: adding `gls`-backed tests below made
    /// `our_domain_never_searches_path_even_for_a_real_path_only_binary`'s
    /// temporary `$PATH` narrowing race against them under `cargo test`'s
    /// default parallel execution — a concurrently-running test's
    /// `Command::new("gls")` would spawn-fail with "No such file or
    /// directory" because `$PATH` had been swapped out from under it.
    /// Every test here that touches `$PATH`, directly or by spawning a
    /// real `$PATH`-resolved binary (`gls` included), takes this lock
    /// first so at most one of them runs at a time; every other test in
    /// this module is untouched by it and stays fully parallel.
    static PATH_SENSITIVE: Mutex<()> = Mutex::new(());

    #[test]
    fn resolvers_for_run_is_installed_binary_only() {
        assert_eq!(resolvers_for(Verb::Run), vec![ResolverKind::InstalledBinary]);
    }

    #[test]
    fn resolvers_for_see_is_filesystem_path_only() {
        assert_eq!(resolvers_for(Verb::See), vec![ResolverKind::FilesystemPath]);
    }

    #[test]
    fn system_bin_dirs_matches_the_real_standard_locations_in_order() {
        let dirs = system_bin_dirs();
        assert_eq!(dirs, SYSTEM_BIN_DIRS.iter().map(PathBuf::from).collect::<Vec<_>>());
    }

    #[test]
    fn our_never_includes_path_in_search_dirs() {
        // search_dirs_for itself only ever returns system_bin_dirs or
        // nothing — $PATH inclusion is list_binaries' job, and only for
        // non-OUR domains.
        assert_eq!(search_dirs_for(OwnershipKind::Our), system_bin_dirs());
    }

    #[test]
    fn their_never_falls_back_to_a_directory_list() {
        assert!(search_dirs_for(OwnershipKind::Their).is_empty());
    }

    #[test]
    fn find_binary_in_dirs_locates_a_real_executable() {
        let dir = std::env::temp_dir().join(format!("xact-resolve-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("xact-resolve-stub");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&script, perms).unwrap();

        let found = find_binary_in_dirs("xact-resolve-stub", &[dir.clone()]);
        assert_eq!(found, Some(script));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_tilde_resolves_against_real_home() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~/x"), PathBuf::from(home).join("x"));
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn anchor_my_default_anchors_a_bare_relative_my_path_at_home() {
        assert_eq!(anchor_my_default("project/x", OwnershipKind::My, true), "~/project/x");
    }

    #[test]
    fn anchor_my_default_leaves_an_absolute_path_unchanged() {
        assert_eq!(anchor_my_default("/tmp/project", OwnershipKind::My, true), "/tmp/project");
    }

    #[test]
    fn anchor_my_default_leaves_an_already_tilde_path_unchanged() {
        assert_eq!(anchor_my_default("~/project", OwnershipKind::My, true), "~/project");
    }

    #[test]
    fn anchor_my_default_never_applies_to_our_or_their() {
        assert_eq!(anchor_my_default("project", OwnershipKind::Our, true), "project");
        assert_eq!(anchor_my_default("project", OwnershipKind::Their, true), "project");
    }

    #[test]
    fn anchor_my_default_never_applies_when_disabled() {
        // The RUN case: MY's path names a $PATH-searched program, not a
        // filesystem location, so `home_relative_default: false` must
        // leave it exactly as typed.
        assert_eq!(anchor_my_default("chrome", OwnershipKind::My, false), "chrome");
    }

    #[test]
    fn list_binaries_finds_a_real_system_binary() {
        let _guard = PATH_SENSITIVE.lock().unwrap_or_else(|e| e.into_inner());
        // "ls" (not the single letter "l") stays well under the 50-match
        // cap even on a machine with a very broad $PATH.
        let matches = list_binaries("ls", OwnershipKind::My);
        assert!(matches.contains(&"ls".to_string()), "{matches:?}");
    }

    /// `PATH` is process-global mutable state, and cargo runs tests in
    /// parallel by default, so both assertions live in one test (same
    /// rationale as `xact-see`'s `see_adapter_dispatches_by_tool_name`)
    /// rather than risking one test observing another's temporary `PATH`
    /// override.
    #[test]
    fn our_domain_never_searches_path_even_for_a_real_path_only_binary() {
        let _guard = PATH_SENSITIVE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("xact-resolve-list-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("xact-resolve-list-stub");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&script, perms).unwrap();

        let original_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);

        let my_matches = list_binaries("xact-resolve-list-stub", OwnershipKind::My);
        let our_matches = list_binaries("xact-resolve-list-stub", OwnershipKind::Our);

        match original_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        fs::remove_dir_all(&dir).ok();

        assert!(my_matches.contains(&"xact-resolve-list-stub".to_string()), "{my_matches:?}");
        assert!(our_matches.is_empty(), "OUR must never search $PATH: {our_matches:?}");
    }

    #[test]
    fn list_path_entries_finds_real_filesystem_matches() {
        let _guard = PATH_SENSITIVE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("xact-resolve-path-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("alpha")).unwrap();
        fs::write(dir.join("beta.txt"), b"x").unwrap();

        let input = format!("{}/al", dir.display());
        let matches = list_path_entries(&input);
        assert_eq!(matches, vec![format!("{}/alpha/", dir.display())]);

        fs::remove_dir_all(&dir).ok();
    }

    /// The exact behavior described in the request: an empty trailing
    /// segment lists everything real in the directory, and each further
    /// character collapses that same real listing by prefix — without
    /// re-invoking `gls` a second time. Proven, not just asserted: the
    /// second call (same directory, unchanged since the first) has to be
    /// dramatically faster than the first real `gls` invocation for this
    /// to be true, since an uncached `gls` call is real subprocess work
    /// (tens of milliseconds at minimum).
    #[test]
    fn narrows_a_cached_real_listing_by_prefix_without_a_second_gls_call() {
        let _guard = PATH_SENSITIVE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("xact-resolve-narrow-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("alpha.txt"), b"x").unwrap();
        fs::write(dir.join("apricot.txt"), b"x").unwrap();
        fs::write(dir.join("beta.txt"), b"x").unwrap();

        let base = format!("{}/", dir.display());
        let first_call = std::time::Instant::now();
        let mut everything = list_path_entries(&base);
        let first_elapsed = first_call.elapsed();
        everything.sort();
        assert_eq!(
            everything,
            vec![
                format!("{base}alpha.txt"),
                format!("{base}apricot.txt"),
                format!("{base}beta.txt"),
            ]
        );

        let narrowed_input = format!("{base}a");
        let second_call = std::time::Instant::now();
        let mut narrowed = list_path_entries(&narrowed_input);
        let second_elapsed = second_call.elapsed();
        narrowed.sort();
        assert_eq!(narrowed, vec![format!("{base}alpha.txt"), format!("{base}apricot.txt")]);

        assert!(
            second_elapsed < first_elapsed || second_elapsed < std::time::Duration::from_millis(5),
            "the second call (same, unchanged directory) should reuse the cached real gls \
             listing instead of paying for another real subprocess call: first={first_elapsed:?} second={second_elapsed:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hidden_entries_are_included_matching_the_pre_gls_behavior() {
        let _guard = PATH_SENSITIVE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("xact-resolve-hidden-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".hidden"), b"x").unwrap();

        let input = format!("{}/.hid", dir.display());
        let matches = list_path_entries(&input);
        assert_eq!(matches, vec![format!("{}/.hidden", dir.display())]);

        fs::remove_dir_all(&dir).ok();
    }
}
