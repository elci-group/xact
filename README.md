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
  `£ COPY MY ~/draft.md to MY ~/backup.md`, `£ MOVE MY ~/draft.md to MY ~/archive.md`,
  `£ RUN 'cargo build'`, `£ BOUND MY ~/project/ to MY ~/bundle.txt`. COPY handles ordinary
  files/directories recursively and never overwrites; MOVE is atomic and refuses cross-filesystem
  fallbacks. Ownership and references are resolved and validated before anything
  runs: `~` means `$HOME` under `MY`, the shared root `$XACT_OUR_ROOT` under `OUR`
  (refused if unset), and the declared user's real home under `THEIR` (after
  `£ THEY are "alice"`). After a command, `THAT` is what it produced (its
  destination, else its target) and `THIS` is what it consumed (its source).
  A command can depend on the outcome of a previous one:
  `£ RUN 'test' WHEN THAT SUCCEEDS`.
- **`!` policy statements** — accumulated for the session:
  - hard constraints, enforced: `WITHOUT <capability>` refuses any command that
    needs it before it is accepted (and isolates spawned processes from the
    network under `WITHOUT network`); `WITH <capability>` is accepted only if
    the capability is actually available on this host; `SPEND`/`SAVE` cap CPU
    and RAM. Capabilities form a closed vocabulary: `network`, `process-spawn`,
    `filesystem-write`, `clipboard`, `agent-local`, `agent-remote`, `agent-tools`.
  - soft preferences (`PREFER`/`DODGE`), validated against the same vocabulary
    and never allowed to contradict a hard constraint, but advisory today: no
    plan Xact makes yet has an alternative for them to choose between.
  - scheduling (`CONCURRENTLY`/`CONSECUTIVELY`).
  - A standalone `! WHEN <condition>` is rejected — there is no condition
    language to enforce it with; attach `WHEN THIS/THAT SUCCEEDS|FAILS` to a
    command instead.
- **`@` agent blocks** — `@ TELL '<model>' BE "..." READING MY ~/project/
  POPULATING MY ~/review.txt THINK 80 "..."` runs one real model. `@ TEAM
  '<model-a>, <model-b>' ...` runs an explicit roster concurrently and returns
  one labelled report. `READING` a directory is aggregated through `bound`
  first; `POPULATING` writes the single final response/report.

Every accepted command is planned, then actually executed — `£ CREATE` really
creates the file or directory via `bank`, `£ SEE` really renders it via `gls` or
`bat`, `£ RUN` really spawns and waits on the process, `£ BOUND` really
aggregates the source set via `bound`, and `@ TELL`/`@ TEAM` really call their
configured model or roster. Nothing is simulated: a plan that can't be honestly
executed is reported as unsupported rather than faked.

## Execution architecture

Xact owns semantics: language interpretation, ownership/reference resolution,
policy evaluation, and capability validation. Real execution is delegated to
[Mesut](https://github.com/elci-group/mesut), a general-purpose execution
orchestration runtime, and to the real ELCi tools themselves. Concretely:

- `£ RUN`/`£ BOUND`/`@ TELL`/`@ TEAM` submit real work to a shared Mesut runtime, which
  schedules it across Tokio, Rayon, or a dedicated blocking thread pool.
- `! SPEND`/`! SAVE` resource policy is enforced with real Linux cgroup v2
  controls, not just displayed, for every process a command spawns.
- `! WITHOUT network` runs every spawned process (except `ollama run`, which
  only talks to the local daemon) in a private, unconnected network namespace
  — kernel-enforced — and refuses API-backed and harness agents before any
  request is made.
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
(`xact-bank`, `xact-see`, `xact-bound`, `xact-process`, `xact-tell`, `xact-team`) each wrap
exactly one real external tool; `xact-mesut` is the single boundary crate that
talks to Mesut; `xact-cli` is the interactive REPL and script runner that ties it all
together. [`XACT_COMPLETION_DIRECTIVE.md`](./XACT_COMPLETION_DIRECTIVE.md) is the
programme directive for driving the project to 1.0, with a dated status log.

## Building and running

```sh
cargo build --workspace
cargo test --workspace     # hermetic: needs no external tools
cargo live-test            # also runs tests that need real tools / cgroups
cargo run --bin xact
```

A fresh clone builds on its own: sibling ELci crates (`mesut`, `padagonia`) are
pinned git dependencies (see the comment in `Cargo.toml` for local
co-development overrides).

Besides the interactive REPL, `xact -c '...'`, `xact script.xact`, and input
piped to `xact` each run as one block: every statement is validated first, and
nothing runs unless all of them pass. The exit status is 0 when every command
succeeded, 1 when one did not, and 2 when the block was rejected.

Real `£ CREATE`/`£ SEE`/`£ BOUND`/`@ TELL`/`@ TEAM` execution requires `bank`, `gls`,
`bat`, `bound`, and `ollama` (with at least one model pulled) to be installed
and reachable on `$PATH`. `! WITHOUT network` requires unprivileged user
namespaces; where the host forbids them, isolated processes fail to start
rather than run with network access.

## Specification

The full language specification lives in [`AGENTS.md`](./AGENTS.md).

## License

MIT — see [`LICENSE`](./LICENSE).
