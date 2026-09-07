<img src="assets/xact-logo.svg" alt="" width="96" height="96" align="left">

# Xact

Xact is a deterministic, semantic shell for the ELCi ecosystem. Instead of asking a
user to construct executable shell commands by hand, it accepts a small, typed
imperative language and turns each accepted line into a real, executed action —
composing real sibling tools (`bank`, `gls`, `bat`, `bound`) and a real local agent
provider (`ollama`) rather than reimplementing what they already do.

<br clear="left">

![xact demo: £ CREATE, £ SEE, ! SPEND, and £ RUN, all executing for real](assets/xact-demo.gif)

## What it does

Xact understands three kinds of input, each parsed by a single deterministic
grammar (no LLM involved in parsing, ownership resolution, or policy evaluation):

- **`£` imperative commands** — `£ CREATE MY ~/project/`, `£ SEE MY ~/project/README.md`,
  `£ RUN 'cargo build'`, `£ BOUND MY ~/project/ to MY ~/bundle.txt`. Ownership
  (`MY`/`OUR`/`THEIR`) and references (`THIS`/`THAT`) are resolved before anything
  runs, and a command can depend on the outcome of a previous one:
  `£ RUN 'test' WHEN THAT SUCCEEDS`.
- **`!` policy statements** — hard constraints (`WITH`/`WITHOUT`/`SPEND`/`SAVE`),
  soft preferences (`PREFER`/`DODGE`), and scheduling (`CONCURRENTLY`/
  `CONSECUTIVELY`), accumulated for the session and enforced for real.
- **`@` agent blocks** — `@ TELL '<model>' BE "..." READING MY ~/project/
  POPULATING MY ~/review.txt THINK 80 "..."` runs a real local model via
  `ollama`, with `READING` a directory aggregated through `bound` first.

Every accepted command is planned, then actually executed — `£ CREATE` really
creates the file or directory via `bank`, `£ SEE` really renders it via `gls` or
`bat`, `£ RUN` really spawns and waits on the process, `£ BOUND` really
aggregates the source set via `bound`, and `@ TELL` really calls the configured
model. Nothing is simulated: a plan that can't be honestly executed is reported
as unsupported rather than faked.

## Execution architecture

Xact owns semantics: language interpretation, ownership/reference resolution,
policy evaluation, and capability validation. Real execution is delegated to
[Mesut](https://github.com/elci-group/mesut), a general-purpose execution
orchestration runtime, and to the real ELCi tools themselves. Concretely:

- `£ RUN`/`£ BOUND`/`@ TELL` submit real work to a shared Mesut runtime, which
  schedules it across Tokio, Rayon, or a dedicated blocking thread pool.
- `! SPEND`/`! SAVE` resource policy is enforced with real Linux cgroup v2
  controls, not just displayed.
- `! CONCURRENTLY` runs independent branches in parallel for real, on Mesut's
  thread pool; `! CONSECUTIVELY` (and the default) waits for each command to
  finish before the next one starts.
- A real `CTRL-C` stops whatever is currently running (via a real `SIGTERM` to
  the spawned process) without killing the Xact session itself.
- `WHEN THIS/THAT SUCCEEDS`/`FAILS` gates a command on the real outcome of the
  thing it depends on, including waiting on a still-running concurrent branch
  before deciding.

See [`MESUT_INTEGRATION.md`](./MESUT_INTEGRATION.md) for the full integration
directive and an honest, phase-by-phase account of what's real today and what
remains open (queued-branch cancellation, multi-branch dependency aggregation,
and recovery constructs beyond a plain `WHEN ... FAILS` fallback).

## Workspace layout

Xact is a Cargo workspace of small, single-purpose crates. The language pipeline
(lexer → parser → AST → semantic validation → policy → planner) is fully
deterministic and covered by unit tests; execution adapters
(`xact-bank`, `xact-see`, `xact-bound`, `xact-process`, `xact-tell`) each wrap
exactly one real external tool; `xact-mesut` is the single boundary crate that
talks to Mesut; `xact-cli` is the interactive REPL that ties it all together.

## Building and running

```sh
cargo build --workspace
cargo test --workspace
cargo run --bin xact
```

Real `£ CREATE`/`£ SEE`/`£ BOUND`/`@ TELL` execution requires `bank`, `gls`,
`bat`, `bound`, and `ollama` (with at least one model pulled) to be installed
and reachable on `$PATH`.

## Specification

The full language specification lives in [`AGENTS.md`](./AGENTS.md).

## License

MIT — see [`LICENSE`](./LICENSE).
