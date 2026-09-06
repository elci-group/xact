# Xact

**Xact** is the default interactive shell for the ELci ecosystem.

Its purpose is to replace the historical assumption that users should construct executable shell commands manually with a stronger model:

> **The user constructs intent; Xact constructs and validates execution.**

Xact shall be implemented entirely in **Rust**.

Bash shall remain available as a compatibility shell. Xact is not Bash with a new parser, nor a natural-language wrapper around Bash.

---

# 1. Core Objective

Xact shall provide a dynamically assisted, semantically validated shell environment in which:

1. the user writes human-oriented commands;
2. the shell continuously understands the partially constructed program;
3. the shell deterministically exposes valid next steps;
4. target and ownership definitions are assisted and validated;
5. invalid grammatical structures cannot be executed;
6. semantic inconsistencies cannot be executed;
7. execution policies are enforced before execution;
8. existing ELci tools are composed rather than unnecessarily reimplemented.

The interactive experience should feel closer to:

* `fish` for immediate shell assistance;
* an IDE for structural awareness;
* a typed language for semantic correctness;
* an operating-system shell for direct execution.

It must remain a shell rather than becoming a full-screen IDE.

---

# 2. Non-Negotiable Implementation Requirement

Xact shall be written in Rust.

Native Xact components shall use Rust for:

* lexical analysis;
* parsing;
* incremental parsing;
* AST construction;
* semantic analysis;
* completion;
* diagnostics;
* ownership resolution;
* reference resolution;
* policy evaluation;
* execution planning;
* process supervision;
* resource control;
* agent orchestration;
* terminal rendering;
* history;
* configuration;
* integration adapters.

External programs may be implemented in other languages.

Xact must not require Bash, Python, Node, or another scripting language for its core operation.

---

# 3. ELci Tool Composition

Xact shall preferentially **compose existing ELci-group tools** rather than duplicate their responsibilities.

The implementation process for every Xact primitive must begin by asking:

> Does an existing ELci tool already own this capability?

If yes, Xact shall provide a semantic interface to that tool.

If no, Xact may implement the capability natively.

Xact therefore becomes an orchestration and semantic layer over an increasingly coherent ELci tool ecosystem.

The shell must not invent alternate implementations merely because invoking an existing ELci binary is slightly less convenient.

---

# 4. `bound` Integration

Xact shall integrate the existing `bound` utility for source aggregation.

`bound` is a Rust CLI utility for recursively aggregating file contents from directories. It provides:

* recursive traversal;
* `.boundignore`;
* extension filtering;
* dependency resolution;
* token limits;
* size limits;
* depth limits;
* metadata;
* hashes;
* tree output;
* JSON output;
* clipboard/file output;
* progress telemetry;
* estimated bounding time.

Xact shall therefore **not reimplement source-bundling semantics inside `SEE`, `COPY`, or `TELL` merely for convenience**.

Where a user requests a multi-source operation requiring aggregation of file contents, Xact shall construct and execute an appropriate `bound` operation.

For example, an Xact-level multi-source context operation may resolve to the equivalent of:

```text
BOUND
    sources = [...]
    filters = [...]
    dependency_resolution = ...
    limits = ...
    output = ...
```

The precise Xact surface syntax shall be defined by the semantic grammar rather than exposing raw `bound` flags directly.

### Important distinction

`BOUND` means **source aggregation/bounding**, not generic filesystem copying.

Xact shall preserve this distinction internally.

The Xact semantic layer may invoke `bound` as part of a larger multi-target operation, but shall never falsely model `bound` as a filesystem copy primitive.

---

# 5. Multi-target Operations

Xact shall support multi-target operations through explicit cardinality-aware semantics.

The shell must distinguish:

```text
single target
multiple targets
bounded source collection
destination
```

rather than attempting to infer cardinality from whitespace.

Where a multi-target operation requires `bound`'s source aggregation functionality, Xact shall use `bound` rather than recreating its traversal, filtering, dependency-resolution or aggregation implementation.

The execution planner shall therefore be capable of producing composite plans such as:

```text
Xact operation
    ↓
target resolution
    ↓
bound aggregation
    ↓
policy validation
    ↓
execution
```

rather than translating every operation into a Bash command.

---

# 6. `bank` Integration

Xact shall use **`bank` for creation of directories and empty files**.

The existing `bank` utility explicitly combines `mkdir` and `touch`, creates missing parent paths, sets permissions, and prompts when a path is ambiguous.

Therefore Xact shall not independently reproduce:

```text
mkdir
+
touch
```

semantics when the requested operation is within `bank`'s responsibility.

The semantic operation:

```text
£ BANK MY ~/project/
```

shall resolve to an Xact `BankIntent`, which is then executed through the `bank` integration.

For example:

```text
£ BANK MY ~/project/src/main.rs
```

means:

> establish this filesystem object using the ELci `bank` capability.

The shell should not care whether the underlying implementation requires directory creation, file creation, parent creation, permission handling, or ambiguity resolution.

That responsibility belongs to `bank`.

---

# 7. Shell Language

The initial imperative vocabulary shall include:

```text
SEE
EDIT
MOVE
COPY
PASTE
CUT
DELETE
RUN
```

Command execution begins with:

```text
£
```

Example:

```text
£ SEE MY ~/Documents
```

The shell shall parse this into semantic intent rather than directly constructing a process invocation.

---

# 8. Policy Language

Policy shall use:

```text
!
```

Initial policy operators:

```text
WITH
WITHOUT
PREFER
DODGE
SPEND
SAVE
CONCURRENTLY
CONSECUTIVELY
WHEN
```

These shall be represented as typed policy nodes.

Hard constraints must be evaluated before preferences.

Conceptually:

```text
WITH/WITHOUT
      ↓
SPEND/SAVE
      ↓
execution capabilities
      ↓
PREFER/DODGE
      ↓
planning optimisation
```

No soft preference may override a hard constraint.

---

# 9. Agent Language

Agent delegation shall use:

```text
@
```

The initial agent primitives are:

```text
TELL
TEAM
READING
POPULATING
BE
THINK
```

Example:

```text
@ TELL 'GPT-5.6-luna'
    BE "a meticulous senior Rust engineer"
    READING MY ~/project/
    POPULATING MY ~/project/review/
    THINK 80
    "Review this project."
```

Agents shall produce semantic intents rather than directly receiving unrestricted shell access.

The Xact policy engine remains authoritative.

---

# 10. Ownership

Xact shall treat ownership as a semantic property.

User/ownership vocabulary:

```text
MY
OUR
THEY
THEM
THEIR
```

Object/reference vocabulary:

```text
THIS
THAT
```

The two categories must remain distinct.

```text
THEIR
```

refers to resources associated with users.

```text
THIS / THAT
```

refer to resolved objects.

Ownership shall be validated before execution.

For example:

```text
£ THEY are "alice,bob"
```

establishes an identity context against which:

```text
THEIR
```

may subsequently resolve.

---

# 11. References

`THIS` and `THAT` shall represent semantic object references rather than textual substitutions.

Example:

```text
£ SEE MY ~/project/README.md
£ EDIT THAT
```

The second command shall reference the result of the first operation.

References shall be typed.

Conceptually:

```text
ObjectRef
    ↓
ResolvedObject
    ↓
TypedResource
```

An unresolved reference is a semantic error and cannot execute.

---

# 12. `BOUND` as a Language Concept

Xact shall reserve `BOUND` for operations involving a deliberately established bounded collection/context.

This is not synonymous with "multiple paths."

A bounded source set may contain:

* files;
* directories;
* filtered files;
* dependency-resolved sources;
* content subject to token limits;
* content subject to byte limits;
* content subject to traversal-depth limits.

Those semantics are inherited from the actual `bound` tool rather than reinvented by Xact.

Xact's semantic IR should therefore model:

```text
BoundSourceSet {
    roots
    filters
    dependency_policy
    depth_limit
    size_limit
    token_limit
    output_mode
}
```

The adapter converts that representation into an invocation of `bound` or, where appropriate, directly into its Rust library interface.

`bound-core` should be preferred over spawning the `bound` executable when doing so is architecturally appropriate, because the project explicitly exposes `bound-core` as a library.

---

# 13. `BANK` as a Language Concept

`BANK` represents resource establishment.

It shall be backed by the existing `bank` tool rather than implemented as:

```text
mkdir(...)
touch(...)
```

inside Xact.

This creates a deliberate ELci composition boundary:

```text
Xact
  │
  ├── bank → resource establishment
  │
  └── bound → source aggregation
```

Xact owns **intent and orchestration**.

The specialist tool owns its domain implementation.

---

# 14. Dynamic Interactive Environment

Xact's defining UX characteristic shall be **live grammatical awareness**.

At every cursor position the shell shall know:

```text
What has already been established?
What can legally come next?
What cannot come next?
What semantic information is missing?
What resources are valid targets?
What ownership contexts exist?
What references exist?
What operators have already been consumed?
```

Completion must be generated from the same grammar/semantic machinery used for validation.

There shall be no independent "autocomplete grammar."

---

# 15. Deterministic Valid-Next-Step Model

For every parser state:

```text
CompletionSet(state)
```

shall contain only valid continuations.

If the current state requires a target:

```text
£ COPY MY █
```

the shell must suggest targets.

It must not suggest:

```text
SEE
EDIT
MOVE
COPY
```

because those are operators rather than valid continuations of the current production.

Operators shall never be suggested directly after another operator where an operand or clause is required.

---

# 16. Operator State

Operators shall have explicit grammar metadata:

```text
repeatable
singleton
mutually-exclusive
ordered
compositional
```

The completion engine shall use this metadata.

If a singleton operator has already been established within the current block, it shall not be suggested again.

If two operators are mutually exclusive, selection of one removes the other from the valid completion set.

This is semantic completion rather than simple keyword completion.

---

# 17. Block Semantics

Xact shall treat multiline input as a structured program.

Example:

```text
! SAVE 20%RAM 30%CPU

£ RUN 'chrome'
£ SEE MY ~/Documents
```

The shell maintains a block context containing:

```text
policies
references
ownership
operators
targets
dependencies
results
execution mode
```

Each subsequent line is parsed against the accumulated context.

---

# 18. Impossible-to-Execute Invalid Blocks

The shell may permit incomplete text while the user is composing it.

It shall never execute an invalid block.

Therefore:

```text
£ COPY MY
```

is an incomplete draft.

It is not an executable program.

The shell should indicate what is required next and provide deterministic completions.

Likewise:

```text
£ COPY THAT
```

must be rejected if no valid `THAT` exists.

The shell should not defer these errors until after Enter.

---

# 19. Semantic Validation

Validation shall occur at multiple levels:

```text
Lexical
   ↓
Grammatical
   ↓
Structural
   ↓
Semantic
   ↓
Ownership
   ↓
Reference
   ↓
Policy
   ↓
Capability
   ↓
Resource
   ↓
Execution
```

Every level must be capable of preventing execution.

---

# 20. Execution Planner

Xact shall construct an explicit execution plan.

```text
Intent
   ↓
ResolvedIntent
   ↓
ExecutionPlan
```

An execution plan may contain:

```text
native operation
ELci tool invocation
external executable
agent invocation
bound operation
bank operation
pipeline
concurrent branch
sequential dependency
resource policy
rollback/cleanup action
```

The planner must understand that an Xact command may be a **composition of ELci capabilities**.

---

# 21. Native vs Tool-backed Execution

The planner should choose between:

```text
native Xact execution
```

and:

```text
ELci tool integration
```

according to capability ownership.

Examples:

```text
BANK
    → bank

BOUND
    → bound / bound-core

RUN
    → process execution

TELL
    → agent provider

TEAM
    → Xact orchestration runtime
```

Xact must not duplicate an ELci tool's domain logic merely to avoid an integration boundary.

---

# 22. Resource Controls

`SPEND` and `SAVE` shall be actual execution constraints.

Example:

```text
! SAVE 20%RAM 30%CPU
£ RUN 'chrome'
```

must result in an execution plan whose resource constraints are enforced by the runtime/platform rather than merely displayed to the user.

Linux implementations should use appropriate kernel-native resource controls.

---

# 23. Scheduling

`CONCURRENTLY` and `CONSECUTIVELY` shall be represented in the execution graph.

```text
! CONCURRENTLY
£ RUN A
£ RUN B
```

creates independent execution branches where dependencies allow.

```text
! CONSECUTIVELY
£ RUN A
£ RUN B
```

creates an explicit dependency:

```text
A → B
```

The scheduler must combine scheduling policy with resource and capability policy.

---

# 24. Diagnostics

Diagnostics must be contextual and actionable.

Instead of:

```text
syntax error
```

Xact should expose:

```text
COPY requires a source target.

Valid continuations:
  MY
  OUR
  THEIR
  THIS
  THAT
```

The same semantic state must power:

* diagnostic;
* syntax highlighting;
* completion;
* validation.

---

# 25. Rust Architecture

Recommended workspace:

```text
xact/
├── Cargo.toml
├── crates/
│   ├── xact-cli/
│   ├── xact-core/
│   ├── xact-lexer/
│   ├── xact-parser/
│   ├── xact-ast/
│   ├── xact-semantic/
│   ├── xact-completion/
│   ├── xact-diagnostics/
│   ├── xact-policy/
│   ├── xact-planner/
│   ├── xact-executor/
│   ├── xact-process/
│   ├── xact-resource/
│   ├── xact-reference/
│   ├── xact-agent/
│   ├── xact-team/
│   ├── xact-terminal/
│   ├── xact-bound/
│   └── xact-bank/
```

The `xact-bound` and `xact-bank` crates should own the integration boundaries rather than leaking tool-specific details throughout the compiler/runtime.

---

# 26. Integration Contracts

ELci integrations shall be treated as typed adapters.

Conceptually:

```rust
trait Capability {
    type Intent;
    type Plan;
    type Error;

    fn validate(&self, intent: &Self::Intent) -> Result<(), Self::Error>;
    fn plan(&self, intent: Self::Intent) -> Result<Self::Plan, Self::Error>;
    fn execute(&self, plan: Self::Plan) -> Result<ExecutionResult, Self::Error>;
}
```

The precise trait design may differ, but the architectural principle is mandatory:

> **Xact owns semantic composition; specialist ELci tools own specialist execution.**

---

# 27. Testing Invariants

The project shall establish property-level guarantees.

### Completion invariant

```text
Every suggested continuation must be valid.
```

### Execution invariant

```text
No invalid AST reaches the executor.
```

### Ownership invariant

```text
No unresolved ownership expression reaches execution.
```

### Reference invariant

```text
No unresolved THIS/THAT reference reaches execution.
```

### Policy invariant

```text
No execution plan may violate a hard policy.
```

### Tool composition invariant

```text
Xact must not silently substitute an independent implementation
where an authoritative ELci capability exists.
```

### `bound` invariant

```text
Source aggregation owned by bound remains bound semantics.
```

### `bank` invariant

```text
Bank-owned creation semantics remain bank semantics.
```

---

# 28. Performance

Interactive grammar parsing and completion must be local and deterministic.

LLM inference must never be required to determine:

* whether a token is legal;
* whether an operator is available;
* whether a command is grammatically complete;
* whether ownership syntax is valid;
* whether a reference exists.

These must remain Rust-native deterministic operations.

Agentic inference occurs only where the language explicitly delegates to it.

---

# 29. Compatibility

Bash remains available.

Xact shall provide an explicit mechanism for invoking Bash when compatibility is required.

Xact must not attempt to become a Bash compatibility parser.

The two environments should coexist:

```text
Default:
    Xact

Compatibility:
    Bash
```

This allows Xact to remain aggressively opinionated without sacrificing the Unix ecosystem.

---

# 30. Definition of Success

Xact succeeds when a user can interact with the operating system without needing to understand Bash's implementation vocabulary while still retaining precise control.

The user should be able to construct:

```text
! SAVE 20%RAM 30%CPU

£ SEE MY ~/project
£ COPY THAT to OUR ~/backup
```

with the shell continuously explaining and completing the valid structure.

For ELci-native capabilities:

```text
£ BANK MY ~/project/src/main.rs
```

shall use `bank`.

For source aggregation:

```text
BOUND
```

semantics shall use `bound`.

For agentic work:

```text
@ TELL ...
```

shall use the agent runtime.

For every operation:

```text
human intent
    ↓
Xact semantics
    ↓
ELci capability
    ↓
validated execution
```

---

# 31. Governing Principle

Xact shall follow one architectural rule above all others:

> **Do not make Xact know how to do something that an ELci tool already knows how to do. Make Xact know how to ask that tool to do it correctly.**

That gives the ecosystem a powerful division of responsibility:

```text
                 XACT
        semantic composition
        policy + references
        live language + UX
                 │
        ┌────────┼────────┐
        │        │        │
      BANK     BOUND    other ELci tools
        │        │        │
 resource    aggregation specialist
 creation                 capabilities
        │        │        │
        └────────┼────────┘
                 │
                OS
```

Xact consequently becomes not merely a shell, but the **semantic command surface through which the ELci tool ecosystem becomes composable**.
