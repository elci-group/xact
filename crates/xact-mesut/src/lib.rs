//! Xact/Mesut integration boundary (Xact–Mesut Technical Directive,
//! sections 1–34). This crate is the *only* place Xact code may depend on
//! `mesut`'s crate topology (directive section 28: "An adapter layer
//! SHOULD isolate Xact from Mesut's internal crate topology"; section 29:
//! the dependency direction is `Xact -> Mesut`, never the reverse, and
//! Mesut must never acquire dependencies on Xact syntax, pronouns, or
//! policy keywords).
//!
//! # Phase 1 (this crate, right now)
//!
//! Directive section 32 scopes Phase 1 as: "Add Mesut as an Xact workspace
//! dependency. Establish `Xact -> Mesut` with no behavioural changes to
//! the language." That is all this crate does. It is **not** wired into
//! `xact-planner`, `xact-executor`, `xact-core`, or `xact-cli` — `£ RUN`,
//! `£ CREATE`, and `£ SEE` still execute exactly as before, via
//! `xact-process`/`xact-bank`/`xact-see`. [`runtime`] exists only to prove
//! the dependency edge is live (constructible, submittable), not to be
//! called from anywhere yet.
//!
//! # A blocking finding for Phase 2
//!
//! Directive section 32's Phase 2 says: "Implement the Xact-to-Mesut
//! execution adapter. Move ordinary external-process execution behind the
//! adapter." As of this writing, **Mesut's three executors do not execute
//! real work**: `mesut_tokio::TokioExecutor::submit`,
//! `mesut_rayon::RayonExecutor::submit`, and
//! `mesut_blocking::BlockingExecutor::submit` each spawn a task whose body
//! is a comment-labelled `// Simulate work execution` / `// Simulate
//! compute work` / `// Simulate blocking work` — a fixed sleep, then the
//! submitted [`mesut::prelude::Work`] is dropped. `Work` itself has no
//! closure, future, or process-spec field to carry real executable
//! content (only `payload: Arc<Vec<u8>>`, opaque bytes with no defined
//! meaning to any executor).
//!
//! Concretely: Mesut can classify and route work and emit believable
//! lifecycle telemetry, but it cannot yet run a subprocess, a closure, or
//! anything else — and there is no way to get a real result back out.
//! Moving `£ RUN`'s real `xact-process` invocation "behind the adapter" as
//! Phase 2 literally describes would not add orchestration — it would
//! silently replace Xact's currently-working process execution with a
//! no-op sleep, a regression dressed as an architecture improvement. This
//! needs a decision (extend Mesut with real work execution first, or shape
//! Phase 2 differently) before that phase can proceed honestly.

use mesut::{MesuT, RuntimeConfig};

/// Constructs a default Mesut runtime. Exists only to prove the
/// `xact-mesut -> mesut` dependency edge compiles and runs — nothing in
/// Xact calls this yet (see the module docs: this is Phase 1, not Phase 2).
pub fn runtime() -> MesuT {
    MesuT::new(RuntimeConfig::new().with_animations(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesut::prelude::*;

    #[test]
    fn runtime_constructs() {
        let runtime = runtime();
        assert_eq!(runtime.active_tasks(), 0);
    }

    /// Proves the dependency edge is not just present in `Cargo.toml` but
    /// actually functional at the API level: submitting a unit of work
    /// through the real Mesut runtime succeeds. This does not prove Mesut
    /// runs real work — see the module docs' blocking finding — only that
    /// Xact can reach Mesut's routing/classification/telemetry layer.
    #[tokio::test]
    async fn submits_work_through_the_real_mesut_runtime() {
        let runtime = runtime()
            .with_async_executor(std::sync::Arc::new(TokioExecutor::new(Default::default())))
            .with_compute_executor(std::sync::Arc::new(
                RayonExecutor::new(Default::default()).expect("rayon executor should build"),
            ))
            .with_blocking_executor(std::sync::Arc::new(BlockingExecutor::new(Default::default())));

        let result = runtime.submit(Work::new(WorkKind::Io)).await;

        assert!(result.is_ok(), "expected Mesut to accept and route the work: {result:?}");
    }
}
