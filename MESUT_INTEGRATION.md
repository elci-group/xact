# Xact–Mesut Integration Technical Directive

> **Implementation status (updated as phases land):**
> **Phase 1 — done.** `xact-mesut` exists, depends on `mesut` (path dependency on the sibling
> `/home/sal/mesut` checkout, referenced as `../mesut/crates/mesut` from this workspace), and
> proves the dependency edge is live (constructs a `MesuT` runtime, submits `Work` through it in
> a test). It is not wired into `xact-planner`/`xact-executor`/`xact-core`/`xact-cli` — no
> behavioural change to the language, exactly as this directive's section 32 scopes Phase 1.
>
> **Phase 2 — done.** Phase 1 found Mesut's executors were simulation stubs (sleep-and-discard)
> with no way for `Work` to carry real executable content. Mesut has since been extended
> (`2e94d38`, "Add real task execution, coordination scheduling, adaptive scheduling, and
> observability"): `Work::with_job`/`with_future` now carry a real closure/future that a compute,
> blocking, or async executor actually runs, returning a real result. `£ RUN`, `£ CREATE`, and
> `£ SEE` all now execute through `xact-mesut`'s adapter (`run_process`, `establish_path`,
> `view_directory`, `view_file`), each submitting its real external-tool call (`xact-process`'s
> launch, `bank -p`, `gls`/`bat`) as `WorkKind::Blocking` work to a shared `MesuT` runtime and
> reporting back the real result — verified live (real stdout/exit codes for `RUN`, a real file
> created on disk for `CREATE`, `gls`'s real animated listing and `bat`'s real rendering reaching
> the terminal for `SEE`) and by the full test suite with zero regressions. `xact-bank`/
> `xact-see`/`xact-process` still own *how* each tool is invoked; `xact-executor` no longer calls
> them directly — every plan variant goes through the adapter. See `crates/xact-mesut/src/lib.rs`
> module docs for the full detail.
>
> **Phase 3 — done.** Spec section 23: `! CONCURRENTLY` "creates independent execution branches";
> `! CONSECUTIVELY` creates an explicit dependency (`A → B`). `CONSECUTIVELY` needed no new
> mechanism — blocking on each plan's real result before the next command is even read already is
> `A → B`, and is also today's default with no schedule stated. `CONCURRENTLY` is now real: each
> adapter function (`run_process`, `establish_path`, `view_directory`, `view_file`) has an
> `_async` counterpart that admits the work onto Mesut and returns a `PendingTask` handle
> immediately instead of blocking, and `xact-executor::execute_concurrent` wraps these into a
> `Pending` handle typed as an `ExecutionOutcome`. `xact-cli` checks `session.schedule()` and, once
> `! CONCURRENTLY` has been established, queues each subsequent command as an independent branch
> instead of waiting on it, printing already-finished branches between prompts and joining
> whatever's left at session end. Verified as real, not just deferred: three `£ RUN 'sleep 1'`
> under `! CONCURRENTLY` finished in ~1.01s wall-clock (vs. ~3.02s for the same three sequentially)
> — genuine parallelism on Mesut's blocking-executor thread pool (`mesut-blocking`'s
> `num_cpus::get().max(2)` real OS threads), not simulated. Full test suite green, zero
> regressions. See `crates/xact-mesut/src/lib.rs` module docs for the full detail.
>
> **Phase 4 — done.** Section 20: "Xact's live interface SHALL consume Mesut lifecycle events";
> section 19 draws the line kept here: "Xact's user-facing diagnostic model SHALL remain
> semantic. Mesut's telemetry SHALL remain execution-oriented... Xact may expose selected Mesut
> telemetry through its dynamic terminal interface." `xact-mesut` now registers a `BranchObserver`
> on the shared `MesuT` runtime (`MesuT::with_observer`, replacing Mesut's own animation observer
> so nothing is duplicated per section 20's closing note) that captures every real
> `Submitted`/`Routed`/`Queued`/`Started`/`Completed`/`Failed`/`Cancelled` event and routes it to
> whichever `PendingTask` subscribed for that task, translated into a Xact-owned `LifecycleEvent`
> so no crate outside `xact-mesut` needs a `mesut`/`mesut-observe` dependency. `Pending::
> drain_events` (non-blocking, best-effort — the authoritative result still comes from
> `join`/`try_join`) surfaces these; `xact-cli` prints them only for `! CONCURRENTLY` branches
> (a `CONSECUTIVELY` command blocks until done, so there's no gap to narrate), draining both
> before and immediately after a branch is found finished so a `Completed`/`Failed` event that
> arrives on its own channel at nearly the same moment as the result isn't lost. Also fixed along
> the way: the adapter's `Work` job now mirrors a real domain failure (e.g. a missing binary) into
> Mesut's own result as `TaskError::ExecutionFailed`, so Mesut's `Completed`/`Failed` telemetry
> agrees with Xact's outcome instead of Mesut always seeing "completed" because the wrapper
> closure itself never panics. Verified live: `! CONCURRENTLY` branches now show real transitions
> (`submitted` → `routed to Blocking(...)` → `queued` → `started on ...` → `completed in Nms`)
> ahead of Xact's own outcome line, and a genuine failure shows `failed: ...` from both layers in
> agreement. Full test suite green, zero regressions. See `crates/xact-mesut/src/lib.rs` module
> docs for the full detail.

---

## 1. Mission

Integrate **Mesut** as the native execution-orchestration substrate of **Xact**.

Xact SHALL remain responsible for:

* language interpretation;
* semantic resolution;
* object/reference resolution;
* ownership semantics;
* capability evaluation;
* user-facing policy;
* execution-plan construction;
* interactive diagnostics;
* determining whether an operation is legally executable.

Mesut SHALL be responsible for:

* workload classification;
* execution-substrate selection;
* scheduling;
* runtime coordination;
* executor abstraction;
* concurrent execution;
* blocking-work isolation;
* compute-oriented execution;
* I/O-oriented execution;
* execution lifecycle;
* execution telemetry.

Mesut is not an Xact language component.

Xact is not a replacement for Mesut's execution scheduler.

The integration SHALL preserve this separation.

---

# 2. Existing Mesut Contract

The current Mesut architecture defines a Rust-native unified execution orchestration layer for heterogeneous workloads.

Its documented execution substrates are:

* Tokio for I/O-oriented asynchronous work;
* Rayon for compute-oriented work;
* blocking workers for blocking workloads.

Its architecture is explicitly organised around:

```text
Application
    ↓
Mesut API
    ↓
Work Description
    ↓
Classification + Scheduling + Policy
    ↓
┌──────────┬──────────┬──────────┐
│  ASYNC   │ COMPUTE  │ BLOCKING │
│  Tokio   │  Rayon   │ workers  │
└──────────┴──────────┴──────────┘
    ↓
Result
```

Mesut currently separates its implementation into:

```text
mesut
mesut-core
mesut-router
mesut-scheduler
mesut-runtime
mesut-executor
mesut-tokio
mesut-rayon
mesut-blocking
mesut-observe
```

Xact SHALL integrate against these existing abstractions rather than reimplementing their functionality.

---

# 3. Architectural Position

Mesut SHALL occupy the execution-orchestration layer immediately below Xact's execution planner.

```text
                         USER
                          │
                          ▼
                    XACT LANGUAGE
                          │
                          ▼
                   PARSER / AST
                          │
                          ▼
                 SEMANTIC RESOLUTION
                          │
                          ▼
                 POLICY / CAPABILITY
                       ENGINE
                          │
                          ▼
                 EXECUTION PLANNER
                          │
                          ▼
              ┌─────────────────────┐
              │        MESUT        │
              │ Execution Runtime   │
              │ Classification      │
              │ Scheduling          │
              │ Routing             │
              └──────────┬──────────┘
                         │
            ┌────────────┼────────────┐
            ▼            ▼            ▼
          Tokio        Rayon       Blocking
            │            │            │
            └────────────┼────────────┘
                         ▼
                    OS / Services
```

Xact SHALL NOT directly select Tokio, Rayon, or blocking workers for ordinary workloads.

That decision belongs to Mesut.

---

# 4. Fundamental Boundary

The governing distinction SHALL be:

> **Xact determines WHAT may happen. Mesut determines HOW permitted work is executed.**

For example:

```text
! SPEND 40%CPU 10%RAM
£ RUN 'build'
```

Xact SHALL:

1. parse the command;
2. resolve `build`;
3. evaluate the resource policy;
4. determine the operation is executable;
5. construct an execution description;
6. submit that description to Mesut.

Mesut SHALL then:

1. classify the workload;
2. select the appropriate execution substrate;
3. schedule it;
4. execute it;
5. observe its lifecycle;
6. return its result.

---

# 5. Xact Execution IR

Xact SHALL introduce a stable, typed execution representation between semantic validation and Mesut.

Conceptually:

```rust
struct ExecutionPlan {
    operation: Operation,
    inputs: Vec<Resource>,
    outputs: Vec<Resource>,
    dependencies: Vec<Dependency>,
    constraints: ExecutionConstraints,
    capabilities: RequiredCapabilities,
    scheduling: SchedulingPolicy,
}
```

The exact structure SHALL be determined by the implementation, but the principle is mandatory:

> **Mesut MUST receive a validated execution description, not raw Xact syntax.**

Mesut SHALL never parse Xact source.

Mesut SHALL never resolve Xact pronouns such as:

```text
THIS
THAT
MY
OUR
THEY
THEIR
```

Mesut SHALL never interpret Xact operators such as:

```text
WITH
WITHOUT
PREFER
DODGE
SPEND
SAVE
```

Those belong to Xact.

---

# 6. Mesut Work Description Adapter

Xact SHALL implement an adapter converting its `ExecutionPlan` into the Mesut work model.

Conceptually:

```text
Xact ExecutionPlan
        │
        ▼
MesutWorkAdapter
        │
        ▼
Mesut Work Description
```

The adapter SHALL translate:

* operation identity;
* workload characteristics;
* dependencies;
* concurrency requirements;
* resource constraints;
* cancellation semantics;
* priority;
* observability metadata;
* execution context.

The adapter SHALL contain no business logic beyond translation.

---

# 7. Work Classification

Mesut SHALL remain authoritative for execution-substrate classification.

Xact MAY provide classification hints when semantic information makes them obvious.

For example:

```text
FILE READ
NETWORK REQUEST
PROCESS WAIT
```

may provide I/O characteristics.

Likewise:

```text
HASH LARGE DATASET
COMPRESS DATA
TRANSFORM FILES
```

may provide compute characteristics.

However, Xact SHALL NOT hard-code:

```text
FILE READ → Tokio
HASH → Rayon
COMMAND → blocking
```

as an execution rule.

Mesut SHALL make the final routing decision.

This preserves Mesut's purpose as the heterogeneous execution router.

---

# 8. Scheduling

Xact scheduling operators SHALL become execution-plan constraints.

For example:

```text
! CONCURRENTLY
```

SHALL produce a concurrency policy.

```text
! CONSECUTIVELY
```

SHALL produce a sequential dependency policy.

Xact SHALL express the semantic requirement.

Mesut SHALL determine how that requirement is realised.

Example:

```text
! CONCURRENTLY {
    £ RUN 'task-a'
    £ RUN 'task-b'
    £ RUN 'task-c'
}
```

becomes conceptually:

```text
ExecutionPlan
    scheduling = Concurrent
    tasks = [A, B, C]
```

Mesut determines the actual scheduling and execution mechanics.

---

# 9. Dependencies

Xact SHALL represent explicit execution dependencies.

Example:

```text
£ RUN 'compile'
£ RUN 'test' WHEN THAT succeeds
£ RUN 'package' WHEN THAT succeeds
```

SHALL become a dependency graph rather than a sequence of immediately executed commands.

```text
compile
   │
   ▼
 test
   │
   ▼
package
```

Mesut SHALL execute the graph according to the resulting scheduling constraints.

Xact SHALL therefore construct the **semantic dependency graph**.

Mesut SHALL construct the **runtime schedule**.

---

# 10. Resource Policies

Xact resource policies SHALL be represented explicitly in the execution plan.

For example:

```text
! SPEND 40%CPU 10%RAM
£ RUN 'chrome'
```

means the execution plan contains a resource budget.

```text
! SAVE 40%CPU 10%RAM
£ RUN 'chrome'
```

means the execution plan contains a system-reservation constraint.

The Xact policy engine SHALL determine the semantic meaning of these policies.

Mesut SHALL receive the resulting execution constraints.

Neither layer SHALL silently discard a constraint.

If Mesut cannot honour a mandatory constraint, execution SHALL fail before the workload begins.

---

# 11. Hard Constraints vs Scheduling Optimisation

Xact SHALL distinguish between:

```text
HARD
```

and:

```text
PREFERRED
```

constraints.

The semantic hierarchy remains:

```text
Xact hard constraints
        ↓
Capability validation
        ↓
Resource feasibility
        ↓
Mesut scheduling
        ↓
Mesut optimisation
```

A Mesut optimisation SHALL never violate an Xact hard constraint.

For example:

```text
! WITHOUT NETWORK
£ RUN 'foo'
```

must not become executable merely because Mesut can find a network-based execution route.

---

# 12. Cancellation

Cancellation SHALL be first-class.

When the user interrupts an Xact operation:

```text
CTRL-C
```

Xact SHALL propagate cancellation through the Mesut execution context.

Conceptually:

```text
Xact
 ↓
CancellationToken
 ↓
Mesut
 ↓
Executor
```

Mesut SHALL propagate cancellation to the selected execution substrate where supported.

Cancellation SHALL NOT require killing the entire Xact process unless the workload has become irrecoverably unresponsive.

---

# 13. Process Execution

External processes SHALL be treated as Mesut workloads rather than as ad-hoc `Command` invocations scattered throughout Xact.

Conceptually:

```text
£ RUN 'cargo build'
```

becomes:

```text
RunProcess {
    executable: "cargo",
    arguments: ["build"],
    environment: ...,
    working_directory: ...,
    constraints: ...,
}
```

Xact constructs the semantic operation.

Mesut manages execution lifecycle.

The implementation SHALL avoid creating an independent process-management subsystem inside Xact where Mesut can provide the appropriate abstraction.

---

# 14. Agent Execution

Agent operations SHALL also pass through the execution architecture.

For example:

```text
@ TELL 'GPT-5.6-luna'
    READING MY ~/project/
    "Review this project."
```

shall resolve approximately as:

```text
AgentIntent
    ↓
Capability validation
    ↓
ExecutionPlan
    ↓
Mesut
    ↓
Agent executor
```

Agent execution SHALL therefore receive the same lifecycle semantics as other workloads:

* cancellation;
* scheduling;
* dependencies;
* observability;
* resource constraints;
* concurrency;
* failure propagation.

Mesut SHALL not become responsible for deciding what the agent is allowed to read or write.

That remains Xact's responsibility.

---

# 15. ELci Tool Execution

Xact SHALL use the same execution pathway when invoking ELci tools.

For example:

```text
£ BANK ...
£ BOUND ...
```

SHALL NOT require bespoke execution machinery for every tool.

Conceptually:

```text
Xact Intent
    ↓
ELci Capability Adapter
    ↓
ExecutionPlan
    ↓
Mesut
    ↓
Tool execution
```

This creates a common execution lifecycle for:

* native Xact operations;
* ELci utilities;
* external processes;
* agent invocations;
* future execution providers.

---

# 16. `bank` Integration

Xact SHALL continue to use the actual `bank` utility/library for its documented filesystem-establishment semantics.

Mesut SHALL orchestrate its execution where appropriate.

The architecture SHALL therefore be:

```text
Xact BANK intent
       ↓
bank integration
       ↓
Mesut execution
       ↓
bank
       ↓
filesystem
```

Xact SHALL NOT reproduce `bank`'s implementation internally.

Mesut SHALL NOT reproduce `bank`'s implementation internally.

---

# 17. `bound` Integration

Xact SHALL continue to use the actual `bound` capabilities for source aggregation/bounding workflows.

Where appropriate:

```text
Xact
 ↓
BOUND semantic operation
 ↓
bound-core / bound integration
 ↓
Mesut orchestration
```

Mesut SHALL treat `bound` as a workload/capability rather than redefining its semantics.

`BOUND` SHALL NOT be represented internally as a generic filesystem-copy primitive.

---

# 18. Native Library Preference

Where Mesut exposes library APIs suitable for direct integration, Xact SHOULD prefer library integration over spawning the Mesut executable.

Likewise, Xact SHOULD prefer:

```text
bound-core
```

or other documented library interfaces where appropriate rather than unnecessary subprocess invocation.

The general ELci principle SHALL be:

> **Compose existing Rust capabilities rather than reproduce them behind a subprocess boundary.**

---

# 19. Observability

Mesut's observation facilities SHALL feed Xact's execution diagnostics.

The user should be able to understand:

```text
what is running
why it is running
where it is running
how much resource it is consuming
what it is waiting for
what failed
what completed
what was cancelled
```

Xact's user-facing diagnostic model SHALL remain semantic.

Mesut's telemetry SHALL remain execution-oriented.

For example:

```text
Xact:
"Building project"

Mesut:
executor = rayon
workers = 8
queue = 0
elapsed = 2.4s
```

Xact may expose selected Mesut telemetry through its dynamic terminal interface.

---

# 20. Interactive Terminal Integration

Xact's live interface SHALL consume Mesut lifecycle events.

This enables:

```text
£ RUN 'build'
```

to transition through states such as:

```text
accepted
    ↓
planning
    ↓
queued
    ↓
classified
    ↓
executing
    ↓
progressing
    ↓
completed
```

The terminal presentation SHALL be driven from actual execution state.

Mesut's existing lifecycle-driven terminal animation behaviour SHALL remain Mesut-owned rather than being duplicated in Xact.

Xact may provide a higher-level semantic presentation over those events.

---

# 21. Error Model

Errors SHALL preserve their originating layer.

Conceptually:

```text
XactError
MesutError
ExecutorError
ToolError
ProcessError
AgentError
```

Xact SHALL not flatten every failure into:

```text
command failed
```

The user-facing diagnostic should distinguish:

```text
semantic failure
policy failure
capability failure
scheduling failure
executor failure
process failure
external-tool failure
```

The underlying cause SHALL remain inspectable.

---

# 22. Failure Semantics

A Mesut workload failure SHALL propagate through the Xact execution graph.

For sequential dependencies:

```text
A → B → C
```

if:

```text
A fails
```

then B and C SHALL NOT execute unless the Xact program explicitly defines recovery semantics.

For concurrent execution:

```text
A ─┐
B ─┼→ aggregate
C ─┘
```

Mesut SHALL return sufficient information for Xact to determine the aggregate semantic result.

---

# 23. Recovery

Recovery SHALL be represented explicitly.

Xact SHALL eventually support execution constructs capable of expressing:

```text
retry
fallback
cleanup
rollback
ignore
```

Mesut SHALL execute those resulting workloads.

Mesut SHALL not invent recovery behaviour that changes Xact semantics.

---

# 24. Runtime Ownership

Mesut SHALL own runtime lifecycle for work submitted to it.

Xact SHALL own the lifetime of the interactive shell itself.

Therefore:

```text
Xact process
    │
    └── Mesut runtime
          ├── Tokio
          ├── Rayon
          └── blocking workers
```

Xact SHALL avoid creating competing global runtimes unless technically unavoidable.

The integration SHOULD establish one well-defined Mesut runtime lifecycle for the Xact process.

---

# 25. Threading Model

Xact's parser and interactive state SHALL remain responsive while Mesut executes workloads.

Long-running work SHALL never block the interactive command loop.

The intended model is:

```text
Terminal/UI
    │
    ├── parser
    ├── semantic state
    └── interaction
           │
           ▼
        Mesut
           │
      ┌────┼────┐
      ▼    ▼    ▼
    async compute blocking
```

---

# 26. Security Boundary

Mesut SHALL never be treated as an authorization boundary.

Authorization remains an Xact concern.

The required sequence is:

```text
User input
   ↓
Xact parse
   ↓
Xact semantic validation
   ↓
Xact policy validation
   ↓
Xact capability validation
   ↓
ExecutionPlan
   ↓
Mesut
   ↓
Executor
```

No Mesut optimization, routing decision, or executor selection may bypass earlier Xact validation.

---

# 27. Networking

Networking SHALL initially be treated as an execution capability rather than automatically introducing a second implementation language or network daemon.

For example:

```text
network request
```

may become a Mesut-managed I/O workload.

If Xact later requires a separately deployed network control plane, that SHALL be designed as an explicit architectural boundary rather than being introduced merely because Go or another language is convenient.

The initial Xact/Mesut integration SHOULD remain Rust-native.

---

# 28. API Design

The Xact integration SHALL expose a narrow internal interface.

Conceptually:

```rust
trait ExecutionRuntime {
    fn submit(
        &self,
        plan: ExecutionPlan,
    ) -> ExecutionHandle;

    fn cancel(
        &self,
        id: ExecutionId,
    ) -> Result<()>;

    fn status(
        &self,
        id: ExecutionId,
    ) -> Result<ExecutionStatus>;
}
```

The exact API SHALL conform to the actual Mesut API rather than forcing Mesut into an invented interface.

An adapter layer SHOULD isolate Xact from Mesut's internal crate topology.

---

# 29. No Semantic Leakage

Mesut SHALL NOT acquire dependencies on:

* Xact syntax;
* Xact parser structures;
* Xact terminal UI;
* Xact-specific pronouns;
* Xact-specific policy keywords.

The dependency direction SHALL remain:

```text
Xact
  ↓
Mesut
```

not:

```text
Xact ↔ Mesut
```

Mesut must remain independently reusable.

---

# 30. Testing

The integration SHALL include tests for:

### Routing

```text
I/O workload → appropriate async execution
compute workload → appropriate compute execution
blocking workload → blocking isolation
```

### Concurrency

```text
CONCURRENTLY
CONSECUTIVELY
dependency ordering
```

### Cancellation

```text
running → cancellation → terminated
```

### Resource constraints

```text
hard constraint accepted
hard constraint rejected
```

### Failure propagation

```text
executor failure → Xact diagnostic
dependency failure → dependent suppression
```

### ELci integration

```text
BANK → bank
BOUND → bound
```

without duplicating their underlying semantics.

---

# 31. Architectural Invariants

The following SHALL be enforced as project invariants.

### Invariant 1

> Xact never executes raw user syntax directly.

### Invariant 2

> Every executable operation passes through semantic validation.

### Invariant 3

> Every executable workload is represented as an explicit execution plan.

### Invariant 4

> Mesut receives validated work, never unvalidated Xact syntax.

### Invariant 5

> Mesut owns execution classification and scheduling.

### Invariant 6

> Xact owns user-facing authorization and policy semantics.

### Invariant 7

> Mesut executor selection is opaque to Xact semantics.

### Invariant 8

> Long-running work cannot block the interactive Xact loop.

### Invariant 9

> Cancellation propagates from Xact through Mesut to the executor.

### Invariant 10

> Existing ELci capabilities SHALL be composed rather than reimplemented.

---

# 32. Migration Strategy

Integration SHALL proceed incrementally.

## Phase 1 — Mesut dependency

Add Mesut as an Xact workspace dependency.

Establish:

```text
Xact → Mesut
```

with no behavioural changes to the language.

## Phase 2 — Execution adapter

Implement the Xact-to-Mesut execution adapter.

Move ordinary external-process execution behind the adapter.

## Phase 3 — Scheduling

Translate:

```text
CONCURRENTLY
CONSECUTIVELY
```

into Mesut execution constraints.

## Phase 4 — Lifecycle

Connect Mesut execution events to Xact's terminal state model.

## Phase 5 — Resource policies

Translate Xact resource policies into Mesut execution constraints.

## Phase 6 — ELci composition

Route appropriate:

```text
BANK
BOUND
```

operations through the unified execution path.

## Phase 7 — Agents

Route agent workloads through Mesut.

## Phase 8 — Advanced execution graphs

Introduce dependency graphs, cancellation trees, aggregation, recovery, and richer scheduling.

---

# 33. Success Criteria

The integration is successful when Xact can execute heterogeneous workloads through one coherent runtime without knowing which execution substrate ultimately performs the work.

For the user:

```text
£ RUN 'build'
```

should simply mean:

> Run this.

Internally:

```text
Xact
 ↓
intent
 ↓
semantic validation
 ↓
policy
 ↓
execution plan
 ↓
Mesut
 ↓
classification
 ↓
scheduling
 ↓
appropriate executor
 ↓
result
```

The user should not need to know whether Mesut selected Tokio, Rayon, or a blocking worker.

That is precisely the abstraction Mesut exists to provide.

---

# 34. Governing Principle

The Xact/Mesut relationship SHALL be governed by one rule:

> **Xact decides whether work should happen. Mesut decides how permitted work should happen.**

Xact is the semantic authority.

Mesut is the execution authority.

Neither subsystem should absorb responsibilities belonging to the other.

The result SHALL be a Rust-native shell in which:

```text
human intent
      ↓
semantic certainty
      ↓
policy certainty
      ↓
execution certainty
      ↓
Mesut orchestration
      ↓
optimal available executor
```

becomes the fundamental execution path of Xact.
