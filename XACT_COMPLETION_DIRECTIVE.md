# Xact Completion Technical Directive

| | |
|---|---|
| **Document** | XACT-TD-001 — Programme directive for driving Xact to 1.0 |
| **Status** | Issued for adoption |
| **Date** | 2026-10-12 |
| **Assessed baseline** | `elci-group/xact@3e94d9f` (identical to local `HEAD` `b4f024d` for `xact/`) **plus** the uncommitted local working tree |
| **Normative sources** | [`AGENTS.md`](./AGENTS.md) (language specification, cited as **SPEC §n**), [`MESUT_INTEGRATION.md`](./MESUT_INTEGRATION.md) (execution directive, cited as **MESUT §n**) |
| **Relationship** | This document does not amend SPEC or MESUT. It sets out how to bring the implementation into conformance with them, in what order, and what proof is needed at each step. Where it identifies a gap in SPEC, it raises a decision (§8) and does not resolve it on its own. |

The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, and **MAY** are to be read as described in RFC 2119.

---

## 0. Executive summary

Xact's architectural core is sound. The deterministic pipeline (lexer → parser → semantic → policy → planner → Mesut-backed executor) is real. It is split across 25 single-purpose crates and is honest about its own limits: an unimplemented path reports `Unsupported` rather than faking success. Six of ten verbs execute for real (CREATE, SEE, RUN, BOUND, and, uncommitted, COPY and MOVE). Scheduling, cgroup v2 resource caps, lifecycle telemetry, CTRL-C cancellation and single-hop dependencies are real. Live per-keystroke completion is derived from the parser itself, as SPEC §14 requires.

Even so, the project is **not in a releasable or even a stable state**. The assessment found:

1. **The working tree does not compile.** `xact-cli` has 5 errors (reedline/nu-ansi-term API mismatch), and the tests of `xact-completion-graph` have 2 type errors. About 1,900 lines of new work across 21 modified files plus 11 untracked files (multi-provider TELL, TEAM, COPY/MOVE, script mode) **exist only on one machine**, are not committed, and are not pushed.
2. **Hard policy is accepted but not enforced.** `WITH`/`WITHOUT` are parsed and conflict-checked, then ignored at execution time (`! WITHOUT network` followed by an API-backed `@ TELL` will send data off-host). This is the most serious conformance gap, because SPEC §27's policy invariant is a safety property, not a feature.
3. **Semantic core gaps.** `THIS` and `THAT` resolve to the same single slot (SPEC §11). `THEIR` only checks that an identity exists and never resolves against it (SPEC §10). `OUR` has meaning only for `RUN`. There are no block semantics (SPEC §17). In script mode a multi-line program executes line by line, so an invalid line 3 does not stop lines 1–2 (SPEC §18).
4. **No delivery infrastructure.** There is no CI. The GitHub repository cannot build standalone because of `../mesut`, `../padagonia` and `../bound` path dependencies. Code is not rustfmt-clean (239 diffs at HEAD, no `rustfmt.toml`). Tests are not hermetic (they need `bank`/`gls`/`bat`/`bound`/`ollama` and a delegated cgroup; about 2 minutes of wall time are spent mostly on live tools). Module documentation has drifted from behaviour in at least 7 places.
5. **Unbuilt spec surface.** EDIT, DELETE, CUT and PASTE are missing. Multi-target cardinality (SPEC §5), the full `BoundSourceSet` (SPEC §12), operator metadata beyond schedule singletons (SPEC §16), the `Capability` adapter contract (SPEC §26), the typed execution IR (MESUT §5), the Bash compatibility escape (SPEC §29), and agent-produced semantic intents (SPEC §9) are all absent.

**Directive:** stop feature work, restore a green, pushed and reproducible baseline (M0–M1), then close the safety-critical semantic gaps (M2–M3) **before** adding verbs or agent capability (M4–M6). Then harden for 1.0 (M7). Each milestone is phase-gated: it is built, tested, verified live, documented and reported before the next one starts.

**Overall conformance estimate:** about 45% of SPEC's normative surface is implemented and enforced. A further ~20% is parsed or validated but not enforced. About 35% is absent (see §2.2).

---

## 1. Assessment method and evidence

Every claim in this document comes from one of the following steps, run on 2026-10-12. None is recalled from earlier sessions.

| Check | Command / method | Result |
|---|---|---|
| Working-tree build | `cargo build --workspace` | **FAIL**: `xact-cli` E0432 `reedline::MenuSettings` not re-exported at the crate root in reedline 0.38.0; 4× E0599 `Style::bg` (nu-ansi-term 0.50 uses `.on(Color)`) |
| Working-tree tests | `cargo test --workspace` | **FAIL**: `xact-completion-graph` tests compare `Option<String>` to `Option<&str>` (lib.rs:337–338) |
| Committed HEAD build+tests | Isolated `git archive HEAD` copy with sibling deps symlinked, `cargo test --workspace` | **PASS**: 190 tests, 0 failures; `xact-executor` 64.6 s and `xact-tell` 51.3 s, both live-tool dominated |
| GitHub parity | Fresh clone of `elci-group/xact`, `diff -rq` against local | Remote == local HEAD; every difference is uncommitted local work |
| Formatting | `cargo fmt --all --check` | 239 diffs at HEAD, 335 in the working tree; no `rustfmt.toml` committed |
| Lints | `cargo clippy --workspace --all-targets` (HEAD) | About 20 warnings (`should_implement_trait` ×7, doc-list indentation ×8, `result_large_err` ×2, others) |
| Non-interactive use (HEAD binary) | `printf '...' \| xact` | Piped input is **silently ignored**, exit 0 (HEAD has no `-c`/script mode; the working tree adds one) |
| Temperature audit | `tempcheq --explain .` | **0 inference actions detected**, although the working tree contains 8 HTTP LLM backends and 4 CLI harnesses, none of which sets a temperature. This is a tempcheq detection gap; see §6.7 |
| Spec conformance | Line-by-line reading of AST, parser index, semantic, reference, policy, planner, executor, core, CLI, resolve, tell, team, resource, cancel, process | §2.2 |

---

## 2. Current-state assessment

### 2.1 Delivery health

| Dimension | State | Rating |
|---|---|---|
| Build (HEAD) | Green | 🟢 |
| Build (working tree) | **Red**: does not compile | 🔴 |
| Version control | About 1.9k lines plus 11 files uncommitted and unpushed; `xact/` is developed inside the `/home/sal` monorepo (whose `origin` is `elci-group/deliver`) and mirrored to `elci-group/xact` | 🔴 |
| Reproducible build | GitHub repo cannot build alone: path deps on `../mesut`, `../padagonia` (which itself path-depends on `../bound`) | 🔴 |
| CI | None | 🔴 |
| Test suite | 190 (HEAD) / 232 (working tree) unit tests; good coverage of the deterministic pipeline; non-hermetic live tests mixed in with unit tests; no property tests, fuzzing or integration-test crate | 🟠 |
| Formatting / lint | Not enforced | 🟠 |
| Documentation | Two strong directive documents; module docs and CLI strings drifted (§2.4) | 🟠 |
| Licensing | MIT, NOTICE, THIRD_PARTY_NOTICES generated by `lwoodz` | 🟢 |
| `unsafe` | 2 sites (`libc::kill` in `xact-cancel`, `pre_exec` cgroup join in `xact-resource`), both narrow and documented | 🟢 |

### 2.2 Specification conformance matrix (AGENTS.md)

Legend: ✅ conformant · 🟡 partial · 🟥 absent · ⛔ behaviour contradicts a SHALL

| SPEC § | Requirement | State | Evidence / gap |
|---|---|---|---|
| 2 | Rust-only core; no Bash/Python/Node required | ✅ | `RUN` spawns directly, without a shell |
| 3, 31 | Compose ELci tools, don't reimplement | 🟡 | CREATE→`bank`, BOUND→`bound`, SEE→`gls`/`bat` ✅. COPY/MOVE are implemented natively (uncommitted, justified in MESUT status but with no ADR). `bound` is spawned instead of `bound-core` (SPEC §12 / MESUT §18 SHOULD) |
| 4, 12 | `BoundSourceSet {roots, filters, dependency_policy, depth/size/token limits, output_mode}` | 🟡 | Single root plus optional destination only |
| 5 | Cardinality-aware multi-target operations | 🟥 | `Operand` is singular everywhere; no list syntax |
| 6, 13 | CREATE via `bank` | ✅ | |
| 7 | Verbs SEE EDIT MOVE COPY PASTE CUT DELETE RUN CREATE | 🟡 | 6/10 plan (CREATE, SEE, RUN, BOUND; COPY/MOVE uncommitted). EDIT, PASTE, CUT, DELETE → `Unsupported` |
| 8 | Policy operators; hard before soft | 🟡 | All 9 parse. Real: SPEND, SAVE, CONCURRENTLY, CONSECUTIVELY. Conflict-checked but **unenforced**: WITH, WITHOUT, PREFER, DODGE. `! WHEN` is stored as opaque text |
| 8, 27 | No plan may violate a hard policy | ⛔ | `WITH`/`WITHOUT` have no effect on planning or execution. The CLI prints a stale "(not yet enforced — the planner/executor are not implemented yet)" |
| 9 | `@ TELL`/`TEAM` with BE/READING/POPULATING/THINK | 🟡 | TELL real (Ollama at HEAD; 8 API backends + 4 harnesses uncommitted). TEAM real but uncommitted. Agents return text only; **"agents produce semantic intents"** is not implemented |
| 10 | Ownership MY/OUR/THEY/THEM/THEIR; validated before execution | 🟡 | MY real (bare-relative → `~/`). OUR has meaning only for RUN (system bin dirs). THEIR only checks that `THEY` was declared; it is never resolved to those users' resources. THEM is absent from the vocabulary. There is no permission/ownership check against the filesystem |
| 11 | THIS/THAT typed, distinct references; `ObjectRef → ResolvedObject → TypedResource` | 🟡 | One "last object" slot; `resolve(_kind)` ignores its argument; no `TypedResource` |
| 14, 15 | Live grammatical awareness; completion uses the same grammar | ✅ | `xact-completion` reflects the parser's `expected`; per-keystroke menu, highlighter and validator in the CLI |
| 16 | Operator metadata: repeatable / singleton / mutually-exclusive / ordered / compositional | 🟡 | Only the schedule singleton/exclusion and agent-clause singletons are modelled |
| 17 | Block context (policies, references, ownership, … scoped per block) | 🟥 | All state is session-global; no block boundary exists |
| 18 | An invalid block never executes | ⛔ (script mode) | Line-level is correct. Batch/script mode (`run_batch`) executes each line as it validates, so earlier lines of an invalid program run |
| 19 | Ten validation levels, each able to prevent execution | 🟡 | Lexical → reference present; capability and resource-availability levels absent |
| 20 | Explicit plan; composite plans (pipelines, branches, rollback) | 🟡 | `ExecutionPlan` is a flat per-command enum; no plan graph |
| 21 | Native vs tool-backed selection | ✅ | For every implemented verb |
| 22 | SPEND/SAVE enforced by kernel controls | 🟡 | cgroup v2 for RUN, BOUND and local-process TELL. Not applied to CREATE, SEE, COPY or MOVE. CPU is capped at ≤1 core. Requires a systemd-delegated subtree. No session-aggregate cap |
| 23 | CONCURRENTLY/CONSECUTIVELY in an execution graph | 🟡 | Real branches and single-hop `WHEN`; no fan-in; queued branches cannot be cancelled |
| 24 | Contextual, actionable diagnostics | 🟡 | Good messages and continuation lists; no stable diagnostic codes; no span-caret rendering standard |
| 25 | Workspace layout | 🟡 | All crates exist; `xact-terminal` is a one-line stub, and the live-editing surface lives in a 1,452-line `xact-cli/src/main.rs` |
| 26 | Typed `Capability` adapter contract | 🟥 | Adapters are free functions with bespoke error types |
| 27 | Invariant test suite | 🟡 | Invariants are argued in docs and covered piecemeal by unit tests; no dedicated invariant/property suite |
| 28 | Interactive parsing/completion local and deterministic | ⛔ (working tree) | Typing an `@ TELL` target triggers **synchronous** provider model-list HTTP calls from the completion path (120 s timeout per provider, 300 s cache). A slow network freezes the line editor |
| 29 | Explicit Bash compatibility mechanism | 🟥 | |
| 30 | Success scenario | 🟡 | `! SAVE … / £ SEE … / £ COPY THAT to OUR …` cannot be shown end to end until COPY lands and OUR has filesystem semantics |

### 2.3 MESUT directive status

Phases 1–7 are delivered as documented. Phase 8 is partial: single-hop dependencies and CTRL-C are real, while fan-in aggregation, queued-branch cancellation (`MesuT::cancel`) and retry/cleanup/rollback are open. Further MESUT findings:

- **MESUT §5 (typed IR)** is not met in spirit: `ExecutionPlan` carries no `inputs/outputs/dependencies/constraints/capabilities/scheduling` structure. Dependencies and schedule live as CLI loop state.
- **MESUT §26/§31 Inv. 6 (Xact owns authorization)**: the capability-validation stage in the required sequence does not exist.
- **MESUT §31 Inv. 8 (long-running work never blocks the loop)** conflicts with the default `CONSECUTIVELY` foreground behaviour. This needs a decision (§8, D-4).
- **MESUT §31 Inv. 9 (cancellation propagates through Mesut)**: only OS children are signalled. Signals go to the PID, not the process group, so grandchildren survive. A PID-reuse race exists between reap and deregistration.

### 2.4 Documentation drift (violates the "keep documentation in sync" policy)

| Location | Stale claim |
|---|---|
| `xact-core/src/lib.rs` module doc and `SessionOutcome` docs | "Only `CREATE` has a real plan so far"; "not yet implemented planner/executor"; "agent providers not run yet" |
| `xact-cli/src/main.rs` module doc | "`@ TEAM` … has no execution plan yet"; verb list omits COPY/MOVE |
| `xact-cli` startup banner | "CREATE, SEE, and RUN actually run" |
| `xact-cli` policy acknowledgement | "(not yet enforced — the planner/executor are not implemented yet.)" — the planner and executor exist; the true reason is that capability enforcement is missing |
| `xact-executor/src/lib.rs` module doc | "Resource control … is not implemented" (it is, since Phase 5) |
| `xact-planner/src/lib.rs` module doc | "`TEAM` is reported unsupported" |
| `xact-agent/src/lib.rs` module doc | "still a stub in `xact-planner`" |

### 2.5 Risk register

| ID | Risk | Likelihood | Impact | Treatment (milestone) |
|---|---|---|---|---|
| R1 | Loss of ~1.9k uncommitted lines (single machine, non-compiling) | High | High | Commit and push in M0 |
| R2 | Hard policy silently unenforced, so users trust `! WITHOUT network` and leak data to third-party APIs via `TELL READING` | High | **Critical** | Fail closed in M0; real capability engine in M3 |
| R3 | "Text-only" harness claim for `claude -p`, `gemini -p`, `kimi -p`, `cursor-agent -p` is unverified. These CLIs carry their own tool permissions and settings and may read files or run commands | Medium | High | Containment in M5; gated behind an explicit capability until then |
| R4 | Interactive freeze from network calls in completion | High (offline/slow networks) | Medium | Async/background catalog refresh in M0 |
| R5 | No reproducible build or CI, so regressions land unnoticed (as happened in the current tree) | High | High | M1 |
| R6 | Script mode partially executes invalid programs | Medium | High | M2 (whole-program validation) |
| R7 | Orphaned grandchildren / wrong-process signal on cancel | Low | Medium | M4 (process groups + pidfd) |
| R8 | `RUN` splits on whitespace, so quoted arguments (`git commit -m "a b"`) are mangled | High | Medium | M2 (argument-vector grammar) |
| R9 | Cross-repo API drift (mesut, padagonia, bound-core) breaks builds | Medium | Medium | M1 (pinned revisions/versions) |
| R10 | Destructive verbs (DELETE, MOVE, CUT) shipped before references and ownership are trustworthy | Medium | **Critical** | Sequencing rule §4.3 |

---

## 3. Governing principles for completion

These add to SPEC §31 and MESUT §34. They are not substitutes for either.

- **P1 — Fail closed.** A policy, ownership or reference statement that Xact cannot enforce MUST cause rejection, at acceptance time where possible and otherwise before the workload begins. "Accepted but not enforced" is not a valid state for a hard constraint.
- **P2 — Green trunk.** `main` MUST build, pass `cargo test --workspace`, and be fmt- and clippy-clean at every commit. Work in progress lives on branches.
- **P3 — Whole-program validation.** No line of a block executes until every line of that block has passed every validation level that can be checked statically (SPEC §18, §19).
- **P4 — Safety before surface.** Destructive or exfiltrating capabilities (DELETE, MOVE, CUT, network-backed agents, side-effecting harnesses) MUST NOT ship before the reference, ownership and capability machinery they depend on (§4.3).
- **P5 — One fact, one place.** The rule already established by `xact-resolve` (one Padagonia-backed source for resolution) extends to capabilities, operator metadata and diagnostics codes.
- **P6 — Evidence over assertion.** Every milestone exit is proved by commands that are run and recorded in the status log (§9.3), the same standard MESUT's phase log already applies.
- **P7 — Docs move with code.** A change that alters behaviour updates the affected module docs, README, SPEC status notes and CLI strings in the same commit.

---

## 4. Programme structure

### 4.1 Milestones at a glance

| M | Name | Theme | Size | Depends on |
|---|---|---|---|---|
| **M0** | Stabilise | Green, committed, pushed, fail-closed | S | — |
| **M1** | Engineering foundation | Reproducible build, CI, hermetic tests, lint/format gates | M | M0 |
| **M2** | Semantic core | Execution IR, references, ownership, blocks, argument grammar | L | M1 |
| **M3** | Policy and capability engine | WITH/WITHOUT/PREFER/DODGE/WHEN enforced; Capability contract | L | M2 |
| **M4** | Verbs and execution graph | EDIT/DELETE/CUT/PASTE, multi-target, full BOUND, fan-in, recovery, cancellation | XL | M3 |
| **M5** | Agent platform | Provider/harness containment, agent→intent, cost/egress control | L | M3 |
| **M6** | Shell completeness | Bash escape, terminal crate, script mode, login-shell readiness | M | M2 (parallel to M4/M5) |
| **M7** | Hardening and 1.0 | Fuzzing, property suite, threat model, performance budgets, packaging | M | M4–M6 |

Sizes are relative effort classes (S < M < L < XL), not calendar commitments. Calendar planning is the owner's call once D-1…D-9 are decided.

### 4.2 Phase-gating rule

Each milestone MUST be run as: implement → `cargo test --workspace` green → live verification of every user-visible behaviour → docs updated → status entry written (§9.3) → **report to owner and stop**. "Execute all milestones" means this loop repeated once per milestone. It never means implementing several milestones before testing any of them.

### 4.3 Sequencing constraints (hard)

- DELETE, MOVE-overwrite, CUT and any irreversible operation MUST NOT ship before M2 (typed THIS/THAT) **and** M3 (capability gate) are complete.
- Any agent backend that sends local content off-host MUST NOT be enabled by default before M3 provides an enforceable `network`/egress capability.
- Harness backends that can take actions MUST NOT be reachable before M5's containment work.

---

## 5. Milestone specifications

### M0 — Stabilise

**Objective:** return to a green, pushed baseline with no silent safety gaps.

| ID | Work item | Acceptance criteria |
|---|---|---|
| M0.1 | Fix `xact-cli` against reedline 0.38 / nu-ansi-term 0.50: replace `Style::bg(c)` with `.on(c)`; drop the `MenuSettings` import and use the menu builder (`MenuBuilder`) methods, **or** upgrade reedline deliberately and pin it | `cargo build --workspace` passes |
| M0.2 | Fix `xact-completion-graph` tests (`Some("gp".to_string())`) and the unused `rest` binding | `cargo test --workspace` passes |
| M0.3 | Split the uncommitted work into reviewable commits: (a) native COPY/MOVE, (b) TEAM, (c) multi-provider TELL catalog, (d) harness backends, (e) CLI script/`-c` mode and menus. Push to `elci-group/xact` | Remote == local; each commit builds and tests green on its own |
| M0.4 | **Fail closed on unenforced hard policy:** `PolicyAccepted` for WITH/WITHOUT MUST be rejected with a diagnostic ("WITHOUT is not yet enforceable in this build"), not accepted with a warning. PREFER/DODGE MAY be accepted as advisory, but the message MUST say so precisely | Test: `! WITHOUT network` is rejected; an existing session cannot reach a state where a hard constraint is recorded but ignored |
| M0.5 | Gate network-backed agent backends and side-effect-capable harnesses behind explicit opt-in (env flag or config) until M3/M5 land; local Ollama stays the default | Test: an API model name with the opt-in absent yields a clear refusal |
| M0.6 | Take network I/O out of the completion path: catalog refresh runs on a background thread; completion reads only what is cached; the first keystroke never blocks on HTTP. Model-list fetches MUST use a short timeout (≤2 s), separate from the inference timeout | Test with an unroutable base URL: completion returns within the interactive budget (§7.4) |
| M0.7 | Correct every drifted doc and string in §2.4 | Reviewer checklist signed in the status log |
| M0.8 | Record tempcheq findings (§6.7) alongside the TELL work and file the detection gap upstream with tempcheq | Status-log entry with tempcheq output and issue link |

**Exit:** M0.1–M0.8 met; status log entry "M0 complete" includes test counts and the live CLI transcript.

### M1 — Engineering foundation

| ID | Work item | Acceptance criteria |
|---|---|---|
| M1.1 | **Reproducible dependencies.** Replace `../mesut`, `../padagonia` and `../bound` path deps with git deps pinned by `rev`, or with published crate versions. A local `[patch]` override is permitted for co-development but MUST NOT be committed as the default | A fresh `git clone elci-group/xact && cargo build` succeeds on a clean machine |
| M1.2 | **Repository topology.** Make `elci-group/xact` the single source of truth (as was done for `deliver`, `defail` and `ogma`): untrack `xact/` from the home monorepo and develop in its own checkout | No xact commits land in the home repo after cut-over |
| M1.3 | Commit `rustfmt.toml` (the codebase's de facto width is 120) and `clippy.toml`; format the tree in one mechanical commit; fix or explicitly `allow` each clippy lint with a reason (e.g. rename `from_str` → `parse` or implement `FromStr`) | `cargo fmt --check` and `cargo clippy -- -D warnings` pass |
| M1.4 | **Test tiers.** Unit tests MUST be hermetic. Tests that need `bank`, `gls`, `bat`, `bound`, `ollama`, cgroup delegation or network move behind `#[cfg(feature = "live")]` or `#[ignore]` plus a `cargo xtask live-test` runner that first probes for prerequisites and reports which are skipped | `cargo test --workspace` passes in a container with none of those tools in < 30 s |
| M1.5 | **CI** (GitHub Actions): fmt, clippy `-D warnings`, hermetic tests, `cargo deny` (licenses/advisories/bans), MSRV check; live tier on a self-hosted runner or a nightly schedule | Required checks protect `main` |
| M1.6 | Declare MSRV (`rust-version`) in workspace metadata | CI matrix includes it |
| M1.7 | Release plumbing: version via kaptaind (with its push configuration verified), install via `baby` (`.baby.toml` exists), CHANGELOG | Tag → reproducible release artifact |

**Exit:** a clean-machine clone builds and tests green in CI; branch protection is on.

### M2 — Semantic core

**Objective:** make the semantic model as strong as SPEC claims, before any more power is added.

| ID | Work item | Acceptance criteria |
|---|---|---|
| M2.1 | **Typed execution IR** (MESUT §5): replace the flat `ExecutionPlan` enum with `Plan { nodes: Vec<PlanNode>, edges: Vec<Dependency>, constraints: ExecutionConstraints, capabilities: RequiredCapabilities, scheduling: SchedulingPolicy }`, where `PlanNode { operation, inputs: Vec<TypedResource>, outputs: Vec<TypedResource> }`. Move dependency and schedule state out of the CLI loop and into the plan | `xact-cli` contains no scheduling or dependency logic; the planner emits one `Plan` per block |
| M2.2 | **References** (SPEC §11): `ReferenceContext` becomes an ordered history of `TypedResource { kind: File \| Directory \| Process \| Text \| BoundSet \| AgentOutput, path/handle, ownership, produced_by }`. The semantics of THIS vs THAT MUST be decided (D-2) and implemented distinctly. `resolve(kind)` MUST use `kind` | Property test: THIS and THAT resolve differently whenever ≥2 objects exist; type mismatches (e.g. `EDIT THAT` where THAT is a process) are rejected before execution |
| M2.3 | **Ownership** (SPEC §10): define OUR's filesystem domain (D-3); resolve `THEIR <path>` against the declared `THEY` members (home directory via NSS/`getpwnam`, never string concatenation); add THEM; validate access permissions **before** execution (ownership validation level, SPEC §19) | `THEIR ~/x` with `THEY are "alice,bob"` resolves to two concrete, permission-checked targets, or is rejected with a diagnostic |
| M2.4 | **Block semantics** (SPEC §17): introduce an explicit block boundary (D-1). Policies, references and identity scope to the block; the session holds blocks | Policies stated in block A do not leak into block B unless explicitly session-scoped |
| M2.5 | **Whole-program validation** (SPEC §18): the interactive REPL and script mode both validate an entire block before planning any of it | Test: a 3-line script whose third line is invalid executes nothing and exits non-zero |
| M2.6 | **RUN argument grammar:** quoted arguments become an argument vector at parse time (no shell); `split_whitespace` is removed from `xact-process` | `£ RUN 'git commit -m "two words"'` passes exactly 4 argv entries |
| M2.7 | **Operator metadata** (SPEC §16): a single declarative table (P5) with repeatable / singleton / exclusive-with / ordered / compositional for every policy operator, agent clause and command clause, consumed by both parser and completion | Completion property test (§7.1) passes over the table |
| M2.8 | **Diagnostics** (SPEC §24): stable codes (`XA0001…`), a severity, a primary span plus caret rendering, and continuation lists. One renderer is shared by the REPL, script mode and highlighter | Golden-file tests for every diagnostic code |

**Exit:** the invariant suite (§7.1) covers the reference, ownership, execution and completion invariants and passes.

### M3 — Policy and capability engine

| ID | Work item | Acceptance criteria |
|---|---|---|
| M3.1 | **Capability contract** (SPEC §26): implement the `Capability` trait (`validate`/`plan`/`execute`, typed intent/plan/error) and port every adapter to it (bank, see, bound, process, transfer, tell, team) | The executor dispatches only through the trait |
| M3.2 | **Capability registry:** a closed, documented vocabulary of capabilities (initial set: `network`, `filesystem-write`, `filesystem-delete`, `process-spawn`, `agent-local`, `agent-remote`, `agent-tools`, `clipboard`). Every adapter declares what it requires; this is data, held in one place (P5) | `xact capabilities` lists them; unknown names in `! WITH` are rejected with suggestions |
| M3.3 | **Hard enforcement:** the planner refuses any plan whose required capabilities intersect `WITHOUT` or omit something `WITH` demands. This is the capability validation level of SPEC §19 | Test matrix: every capability × WITH/WITHOUT × every verb |
| M3.4 | **Soft optimisation:** PREFER/DODGE choose between valid alternative plans (e.g. local vs remote agent, `bound-core` library vs binary) and never override hard constraints | Test: PREFER cannot select an alternative that violates WITHOUT |
| M3.5 | **`! WHEN`:** either define a condition language (D-6) and enforce it, or remove it from the grammar. It MUST NOT remain accepted and inert | Grammar and docs agree |
| M3.6 | **Resource policy completion:** apply budgets to every process-spawning or blocking verb (CREATE, SEE, COPY, MOVE included, or a documented reason per verb); multi-core CPU caps (`SPEND 200%CPU` or `SPEND 2CPU`, D-7); an aggregate session/block cap across concurrent branches; a clear diagnostic when cgroup delegation is unavailable (fail closed) | Live test: three concurrent branches under `! SPEND 30%CPU` together stay ≤30% |
| M3.7 | **Agent egress:** `agent-remote` requires `network`; `READING` content sent off-host is subject to the same capability check. Network backends become enableable through policy, replacing the M0.5 opt-in | `! WITHOUT network` + `@ TELL '<api-model>' READING MY ~/x` is rejected before any byte leaves the host |

**Exit:** SPEC §27's policy invariant is proved by tests that cover every policy operator.

### M4 — Verbs and execution graph

| ID | Work item | Acceptance criteria |
|---|---|---|
| M4.1 | **ELci ownership review (ADR):** for each of EDIT, DELETE, CUT, PASTE, COPY and MOVE, record whether an ELci tool owns the capability (SPEC §3). If no tool owns it, record the decision to implement it natively, or to create a new ELci tool first (D-5) | ADR committed per verb |
| M4.2 | **EDIT** → `$VISUAL`/`$EDITOR` (or an ELci editor tool), as a foreground TTY handoff; THIS/THAT typed as File | Live: `£ SEE MY ~/x.md` then `£ EDIT THAT` opens the same file |
| M4.3 | **DELETE**: default to recoverable deletion (trash, per D-5); hard delete requires an explicit capability (`filesystem-delete`) and confirmation in interactive mode; never follows symlinks out of the target | Tests for symlink, non-empty directory, permission-denied and cross-ownership cases |
| M4.4 | **CUT/PASTE**: define clipboard semantics (session object register vs system clipboard, D-8); CUT is a deferred MOVE, materialised by PASTE, never a delete-then-hope | A CUT with no PASTE leaves the filesystem untouched |
| M4.5 | **Multi-target cardinality** (SPEC §5): grammar for explicit collections (e.g. `MY ~/a, MY ~/b` or a bracketed set, D-9); plan nodes fan out per target; BOUND is the only verb that *aggregates* a collection | Completion and diagnostics distinguish single, multiple, bounded set and destination |
| M4.6 | **Full `BoundSourceSet`** (SPEC §12): roots, filters, dependency policy, depth/size/token limits, output mode; adapter targets the `bound-core` library (MESUT §18), and the binary remains only as fallback | Parity tests against `bound` CLI output for the same set |
| M4.7 | **Execution graph:** fan-in dependencies (`WHEN ALL OF … SUCCEED`); adopt `mesut::Pipeline` once a plan has fan-in (as MESUT Phase 8 itself anticipates); queued-branch cancellation through `MesuT::cancel` | Live: A ∥ B → C waits for both; cancelling while C is queued cancels C without a process ever starting |
| M4.8 | **Recovery** (MESUT §23): retry with bounded attempts, cleanup, and rollback for plan nodes that declare an inverse (CREATE ↔ remove-if-created, MOVE ↔ move-back) | Rollback tests on induced mid-plan failure |
| M4.9 | **Cancellation correctness:** spawn every child in its own process group and signal the group; use `pidfd` (`pidfd_open`/`pidfd_send_signal`) to remove the PID-reuse race; escalate SIGTERM → SIGKILL after a grace period | Test: `RUN 'sh -c "sleep 100 & sleep 100"'` leaves no survivors after CTRL-C |

**Exit:** all ten SPEC §7 verbs plan and execute, or are formally deferred by ADR; MESUT Phase 8 is closed.

### M5 — Agent platform

| ID | Work item | Acceptance criteria |
|---|---|---|
| M5.1 | **Provider tiers** declared as capabilities: `agent-local` (Ollama), `agent-remote` (HTTP APIs), `agent-tools` (harnesses). Each backend's tier is data in `xact-tell` | `@ TELL` refuses a backend whose tier the policy forbids |
| M5.2 | **Harness containment:** verify each harness's non-interactive tool behaviour against its current documentation and the user's own settings files. Run each harness in a sandbox (read-only bind of `READING`, no write access, network as policy allows), or remove it. The "text-only" claim MUST be backed by a test that tries a side effect and observes it fail | Containment test per harness |
| M5.3 | **Agents produce semantic intents** (SPEC §9): an agent may return an Xact program as structured output; Xact parses and validates it through the identical pipeline and presents it for confirmation; it never executes implicitly | Test: an agent-proposed `£ DELETE` that violates policy is rejected exactly as a typed one would be |
| M5.4 | **THINK budget:** one documented, per-provider mapping from the numeric budget to each provider's native control (token budget / effort level), held in one table, with the mapping shown to the user | Table plus per-provider tests |
| M5.5 | **Sampling determinism:** set an explicit temperature for every inference call (configurable; default chosen per action type and justified by a tempcheq audit). No implicit provider defaults | tempcheq (once its detection gap is fixed) reports 0 implicit actions |
| M5.6 | **Cost and quota:** `SPEND` gains a token/cost dimension for remote agents (D-7) instead of the current "budget against an API backend fails fast" | `! SPEND 50000TOKENS` enforced across a TEAM |
| M5.7 | **HTTP cancellation:** in-flight requests become abortable (async client or a cancellable agent thread), joining the CTRL-C path | CTRL-C during a remote TELL returns within 1 s |
| M5.8 | **Secrets hygiene:** keys come only from env or a secret manager (as today); errors and telemetry MUST never include headers; a redaction test covers every backend | Test with a sentinel key value |

**Exit:** no agent path can exceed policy, and every agent output that could act is validated as Xact.

### M6 — Shell completeness

| ID | Work item | Acceptance criteria |
|---|---|---|
| M6.1 | **Bash escape** (SPEC §29): an explicit, visually distinct construct (D-10). It is policy-checked as `process-spawn` + `bash`, and is never the fallback for an unparsable line | Grammar + completion + test |
| M6.2 | **`xact-terminal`:** move highlighter, validator, prompt, menus and completion keybindings out of `xact-cli/src/main.rs`; `xact-cli` becomes argument parsing plus wiring | `main.rs` under ~300 lines |
| M6.3 | **Non-interactive contract:** `-c`, script file and stdin; exit status reflects the block outcome (0 / validation-failure / execution-failure, as distinct codes); diagnostics go to stderr | HEAD's silent ignore of piped input is gone; tested |
| M6.4 | **Login-shell readiness:** `/etc/shells` registration doc, TTY/job-control handoff for foreground programs (D-4), `SIGTSTP`/`SIGWINCH` handling, environment inheritance, `~/.config/xact/` config | Documented install-as-login-shell procedure verified on a test account |
| M6.5 | Vi-mode parity for the live completion overlay | Tests or recorded demo |

### M7 — Hardening and 1.0

| ID | Work item | Acceptance criteria |
|---|---|---|
| M7.1 | **Threat model** (STRIDE-style) covering agents, harnesses, ownership/permissions, cgroup escape, clipboard, and script injection via THIS/THAT text objects | Document committed; each threat mapped to a control or accepted risk |
| M7.2 | **Fuzzing:** `cargo-fuzz` targets for lexer, parser and completion; 24 h with no crash before release | Corpus committed |
| M7.3 | **Invariant suite** (§7.1) complete for all SPEC §27 invariants | CI-required |
| M7.4 | **Performance budgets** (§7.4) enforced by benchmarks in CI | Criterion benches, regression threshold |
| M7.5 | **Security review** of the two `unsafe` sites and every subprocess boundary | Review notes committed |
| M7.6 | **User documentation:** language reference generated from the grammar and operator tables (P5), tutorial, man page | Docs build in CI |
| M7.7 | **1.0 release:** semver commitment for the language surface; CHANGELOG; signed tag; `baby` install verified on a clean machine | Release checklist |

---

## 6. Cross-cutting requirements

### 6.1 Definition of Done (every change)

1. Builds; `cargo test --workspace` (hermetic tier) green; fmt and clippy clean.
2. New behaviour has tests at the lowest viable crate, plus an invariant/property test if it touches §7.1.
3. User-visible behaviour is verified live, with the transcript in the PR or status log.
4. Module docs, README, CLI strings and the SPEC/MESUT status notes updated (P7).
5. No new path dependency, network call in an interactive path, or unenforced policy state.
6. Commits authored through the project's normal kaptaind flow; no secrets.

### 6.2 Definition of Done (every milestone)

The DoD for every change, plus: the exit criteria are met; a status-log entry (§9.3) with test counts, live evidence and open items; and a report to the owner before the next milestone starts (§4.2).

### 6.3 Crate ownership boundaries (normative)

| Concern | Sole owner |
|---|---|
| Grammar, operator metadata | `xact-parser` (+ declarative table, M2.7) |
| Resolution of paths/binaries/homes | `xact-resolve` |
| References / typed resources | `xact-reference` |
| Ownership and identity | `xact-semantic` |
| Policy and capability decisions | `xact-policy` |
| Plan construction | `xact-planner` |
| Talking to Mesut | `xact-mesut` (no other crate depends on `mesut`) |
| Process spawn, groups, cgroups, cancel | `xact-process`, `xact-resource`, `xact-cancel` |
| Tool adapters | one crate per tool, behind `Capability` |
| Live terminal UX | `xact-terminal` |
| Wiring only | `xact-cli` |

### 6.4 Dependency policy

Sibling ELci crates are consumed at pinned revisions or versions (M1.1). Third-party crates are added only with a `cargo deny`-clean license and advisory status. TLS for remote agents uses the platform verifier, with no `danger_accept_invalid_certs`.

### 6.5 Observability

Mesut lifecycle events stay execution-oriented and separate from semantic diagnostics (MESUT §19). Add a `--trace` flag that emits structured (JSON-lines) plan, policy-decision and lifecycle events for debugging and audit. It must never include secrets or `READING` contents.

### 6.6 Data handling

`READING` content is user data. It may leave the host only through an `agent-remote` capability that the active policy permits (M3.7). It is never logged or cached on disk by Xact.

### 6.7 TempCheq findings (per project policy)

`tempcheq --explain /home/sal/xact` (run 2026-10-12, before and after the M0 work) reported **0 inference actions**. That is a tempcheq detection gap, filed as [elci-group/tempcheq#1](https://github.com/elci-group/tempcheq/issues/1). Manual inventory of inference call sites:

| Call site | Backends | Temperature |
|---|---|---|
| `xact-tell/src/anthropic.rs` (`POST /v1/messages`) | Anthropic | Provider default (not sent) |
| `xact-tell/src/openai_compatible.rs` (`POST /chat/completions`) | OpenAI, Groq, xAI, OpenRouter, Moonshot, Zhipu | Provider default (not sent) |
| `xact-tell/src/ollama.rs` (`ollama run`) | Ollama | Not controllable from the CLI |
| `xact-tell/src/harness.rs` (`<cli> -p`) | claude, kimi, gemini, cursor-agent | Not controllable |

Using provider defaults is now a recorded interim decision (D-12), documented in `xact-tell`'s module docs, rather than an unexamined omission. Several current reasoning models reject any non-default temperature, and two of the four backend kinds expose no control at all, so one explicit value cannot be applied uniformly. Until tempcheq#1 is fixed, each release's audit MUST repeat this manual inventory.

---

## 7. Verification framework

### 7.1 Invariant suite (SPEC §27, mapped to tests)

| Invariant | Test form | Milestone |
|---|---|---|
| Every suggested continuation is valid | Property: for random prefixes from a grammar-driven generator, appending any suggestion yields `Incomplete` or `Complete`, never `Invalid` | M2.7 |
| No invalid AST reaches the executor | Type-level: executor accepts only `ValidatedPlan`, constructible only by the planner after validation; plus a fuzz target | M2.1 |
| No unresolved ownership reaches execution | Property over generated identity contexts | M2.3 |
| No unresolved THIS/THAT reaches execution | Property over generated histories | M2.2 |
| No plan violates a hard policy | Exhaustive matrix: capability × operator × verb | M3.3 |
| Tool composition: no silent substitution | Test that each tool-owned verb's plan names its tool; ADR list for native verbs | M4.1 |
| `bound` / `bank` semantics preserved | Parity tests against the real tools | M4.6 |
| A block with any invalid line executes nothing | Script-mode and REPL tests | M2.5 |

### 7.2 Test tiers

| Tier | Runs | Requirements |
|---|---|---|
| Unit (hermetic) | Every push, CI-required | None |
| Invariant/property | Every push, CI-required | None |
| Live-tool | Nightly / self-hosted | `bank`, `gls`, `bat`, `bound`, `ollama`, cgroup v2 delegation |
| Live-remote | Manual, pre-release | Provider API keys (never in CI logs) |
| Fuzz | Nightly; 24 h pre-release | `cargo-fuzz` |

### 7.3 Live verification standard

Each user-visible feature is demonstrated through the real CLI against real tools, with the transcript captured. This is the bar the MESUT phase log already applies, and it remains mandatory.

### 7.4 Performance budgets (SPEC §28)

| Operation | Budget (p99, release build) |
|---|---|
| Parse + validate one line | ≤ 1 ms |
| Static completion (keyword) | ≤ 5 ms |
| Dynamic completion (path/binary, warm cache) | ≤ 20 ms |
| Any completion with network unavailable | ≤ 50 ms (cached or empty, never blocking) |
| REPL startup to first prompt | ≤ 150 ms |

---

## 8. Decisions required from the owner

These are product decisions that this directive deliberately does not make. Each blocks the work items listed against it.

| ID | Decision | Options (non-exhaustive) | Blocks |
|---|---|---|---|
| D-1 | What delimits a **block** (SPEC §17)? | Blank line; explicit `END`/`;;`; one submission = one block | M2.4, M2.5 |
| D-2 | **THIS vs THAT** semantics | THIS = object of the current/most recent command, THAT = the one before; or THIS = output, THAT = input of the last command | M2.2 |
| D-3 | **OUR** filesystem meaning | A shared group directory (e.g. `/srv/<group>`); the project root; system paths only | M2.3 |
| D-4 | Foreground vs MESUT Inv. 8 | Foreground blocks the loop as in conventional shells (amend MESUT Inv. 8), or all work is backgrounded, with explicit `WAIT`/job control | M6.4, M4.7 |
| D-5 | Ownership of COPY/MOVE/DELETE/EDIT/clipboard | Native in Xact (with ADR), or new ELci tools first, per SPEC §3/§31 | M4.1–M4.4 |
| D-6 | `! WHEN` policy statement | Define a condition language; or remove it in favour of the command-level `WHEN THIS/THAT` clause | M3.5 |
| D-7 | Units for `SPEND`/`SAVE` beyond % | Multi-core CPU (`2CPU`/`200%CPU`); `TOKENS`; currency | M3.6, M5.6 |
| D-8 | Clipboard model | Xact-internal register; system clipboard; both | M4.4 |
| D-9 | Multi-target syntax | Comma list; bracketed set; glob as a typed collection | M4.5 |
| D-10 | Bash escape syntax | `£ BASH '…'`; a distinct sigil; `£ RUN BASH '…'` | M6.1 |
| D-11 | Harness backends | Keep with sandboxing; or remove until a containment design exists | M5.2 |
| D-12 | Sampling temperature policy | Provider defaults (interim, documented); per-action explicit values where the backend supports them; a user-settable override | M5.5 |

**Resolved by the owner, 2026-10-12:** D-1 → a block is the unit of all-or-nothing validation (a REPL submission, or a whole `-c` string, script file, or piped input); policies, identity and references stay session-wide. D-2 → `THAT` is the last command's result (destination, else target) and `THIS` its input (source). D-3 → `OUR` paths resolve under the shared root `XACT_OUR_ROOT` and are refused when it is unset; `THEIR` paths resolve under the declared user's real home. Agent default → network-backed agents stay allowed by default, and `! WITHOUT network`/`agent-remote`/`agent-tools` refuse them.

**Resolved by the owner, 2026-10-12 (later):** D-5, for `EDIT` only → `EDIT` is executed by the ELci tool `oip` ([ADR 0001](docs/adr/0001-edit-via-oip.md)). COPY, MOVE, DELETE and the clipboard verbs remain open. Repository topology (M1.2) → `/home/sal/xact` is its own repository on `elci-group/xact`, released through kaptaind.

---

## 9. Governance

### 9.1 Roles

| Role | Responsibility |
|---|---|
| Owner | Decides D-*, accepts milestone exits, owns SPEC and MESUT |
| Implementer (human or agent) | Executes one milestone at a time under §4.2; never skips a gate |
| Reviewer | Checks DoD (§6.1/6.2) and docs sync before merge |

### 9.2 Change control

- SPEC (`AGENTS.md`) and MESUT change only by explicit owner decision. The implementation conforms to them, never the reverse.
- Each D-* decision is recorded as an ADR under `docs/adr/` and referenced from the work item it unblocks.
- A deviation from SPEC discovered during implementation is raised as a new D-* entry, not decided on the spot.

### 9.3 Status reporting

This document carries a **status log** (below), maintained the way MESUT's phase log is: each entry is dated, names the milestone and work items, gives the test count and the live-verification evidence, and states open items plainly. The README's status claims must never be ahead of this log.

---

## 10. Status log

> **2026-10-12 — Directive issued.** Baseline assessed as described in §1–§2. HEAD green (190 tests); working tree red (7 compile errors across 2 crates); ~1.9k uncommitted lines. Next action: **M0**.
>
> **2026-10-12 — M0 complete; M1.1, M1.3–M1.5 complete; M2.5, M2.2 (partial), M2.3 (partial), M3.2/M3.3/M3.5/M3.7 (core), M6.3 complete.** Done:
>
> - **M0.1–M0.3:** the build is fixed (reedline 0.38 `Menu` delegation, nu-ansi-term `.on()`, test types). The uncommitted work is committed, but **not pushed**: the session was not permitted to attach the `elci-group/xact` history to the local checkout (M1.2), so pushing is left to the owner.
> - **M0.4/M3.2/M3.3:** fail-closed policy. A closed capability vocabulary (`network`, `process-spawn`, `filesystem-write`, `clipboard`, `agent-local`, `agent-remote`, `agent-tools`) is parsed, completed and enforced. `WITHOUT` rejects any command needing the capability at validation time. `WITH` is accepted only after a real availability probe. Unknown capability names are refused. `PREFER`/`DODGE` are labelled advisory.
> - **M3.7 / R2:** under `! WITHOUT network`, every process an executed command spawns (`bank`, `gls`/`bat`, `RUN`, `bound`, harnesses; not `ollama run`) runs in a private user+network namespace, which is kernel-enforced. API-backed and harness agents are refused before any context is read. Verified by `strace`: 0 non-loopback `connect()` calls versus 10 without the policy.
> - **M0.5:** by owner decision, remote agents stay allowed by default and are blocked by `WITHOUT` (see §8).
> - **M3.5:** `! WHEN` is rejected by the grammar and never offered by completion.
> - **M0.6 / R4:** completion never blocks on providers. Catalog snapshots refresh in the background, model-list requests have a 10 s ceiling, and remote lookups are skipped entirely while policy forbids them.
> - **M0.7:** every drifted doc and string listed in §2.4 is corrected.
> - **M0.8:** tempcheq findings are recorded (§6.7) and the gap is filed as tempcheq#1.
> - **M1.1:** `mesut`/`padagonia` are pinned git dependencies; a copy of the tree with no sibling checkouts builds.
> - **M1.3:** `rustfmt.toml` is committed; fmt and `clippy -D warnings` are clean.
> - **M1.4:** 31 live tests are `#[ignore = "live: …"]`; `cargo live-test` runs every tier. An `ETXTBSY` race in the process-test stubs is fixed (0/10 failures after, versus 2/3 before).
> - **M1.5:** a GitHub Actions workflow runs fmt, clippy and the hermetic tests.
> - **M2.5 / M6.3 / R6:** `-c`, script files and piped stdin run as one block: every statement is validated against a throwaway session clone and nothing runs if any is invalid. Exit codes are 0/1/2. COPY/MOVE without `to`, and a destination on a verb that takes none, are now validation errors.
> - **M2.2 (partial):** `THIS`/`THAT` are distinct (input/result), recorded as resolved absolute paths. This also fixed a latent bug: `£ COPY THAT to …` resolved `THAT` to its own destination, because references were updated before planning. Accepted commands now carry their validation-time reference context.
> - **M2.3 (partial):** `THEY` members must be real users (NSS `getpwnam_r`). `THEIR` resolves to that user's home, and several members are refused (fan-out not supported). `OUR` resolves under `XACT_OUR_ROOT`. Completion lists under the same roots.
> - **M3.6 (partial):** `SPEND`/`SAVE` now also apply to `bank`, `gls` and `bat`.
>
> Evidence: hermetic tier 233 passed / 0 failed / 31 ignored with `PATH=/usr/bin:/bin`. Live tier 261 passed / 0 failed, with the two Ollama-generation tests run separately (`real_ollama_end_to_end_and_cancellation` passed in 135 s). Every user-visible behaviour above was verified through the real binary.
>
> **Still open:** M1.2 (repo split and push; owner action), M1.6–M1.7, typed resources (rest of M2.2), multi-target `THEIR` fan-out (M4.5, SPEC §5), M2.1, M2.4 (block-scoped policy, deliberately not adopted per D-1), M2.6–M2.8, M3.1, M3.4, M3.6 (aggregate caps, multi-core), and M4–M7. Network isolation of completion's own `gls` listings is out of scope (they are not executions). Next action: owner pushes; then **M2.1** (typed execution IR).
>
> **2026-10-12 — Sprint S1 complete: `EDIT` via `oip`; M1.2 done; M4.1/M4.2 done for `EDIT`.** Plan of record: [`docs/sprints/S1-edit-via-oip.md`](docs/sprints/S1-edit-via-oip.md); decision record: [ADR 0001](docs/adr/0001-edit-via-oip.md).
>
> - **M1.2:** `/home/sal/xact` is now its own repository. `git subtree split --prefix=xact` reproduced GitHub's tip `3e94d9f` exactly, so the 5 local commits were carried over as a fast-forward and nothing was rewritten. The home repository untracks and ignores `/xact/`.
> - **`EDIT` executes:** the new `xact-oip` crate owns the whole `oip` boundary. It always invokes `oip -- <absolute path>` and never `-i` or `--revert`. Before spawning, it refuses missing files (use `CREATE`), directories, symlinks, and paths containing `{`/`}` (which `oip` would brace-expand). The planner reuses the same check, so such targets plan as `Unsupported` with the fix named.
> - **Policy:** `EDIT` requires `process-spawn`, `filesystem-write` and `clipboard`. `oip` runs under `SPEND`/`SAVE`, network isolation and CTRL-C cancellation like every other spawned tool.
> - **Pipeline:** an `xact.edit` Mesut workload (sync and concurrent). `ExecutionOutcome::Edited` carries the backup path and `oip`'s preview, and the CLI renders both.
> - **Verb count:** 7 of 10 verbs now execute.
>
> Evidence:
>
> - Hermetic tier: 240 passed / 0 failed / 33 ignored. Live tier: 270 passed / 0 failed. Both include stub-`oip` tests (argument shape, no spawn on refused targets, error mapping, budget refusal) and real-`oip`/real-clipboard tests, one of them inside the network-isolated namespace.
> - Verified through the real binary: under `! WITHOUT network`, `£ SEE MY …/notes.md`, `£ EDIT THAT`, `£ RUN 'cat …' WHEN THAT SUCCEEDS` replaced the file from the clipboard and kept `notes.md.oip.backup`.
> - Exit codes: 1 for a missing-file edit, 2 for `EDIT` under `! WITHOUT clipboard`.
>
> **Upstream follow-ups for `oip`** (ADR 0001): per-file revert, atomic write, a flag to disable brace expansion, version string alignment. **Next action:** M2.1 (typed execution IR).
