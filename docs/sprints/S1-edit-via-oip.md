# Sprint S1 — `EDIT` via `oip`

| | |
|---|---|
| **Sprint goal** | `£ EDIT` executes for real through `oip`, safely, under every existing policy, with full test coverage, and ships through kaptaind to `elci-group/xact`. |
| **Dates** | 2026-10-12 (single-session sprint) |
| **Baseline** | `elci-group/xact@3e94d9f` plus 5 local commits (`…3f73f87`), hermetic tier 233 passed / 31 ignored |
| **Decision record** | [ADR 0001](../adr/0001-edit-via-oip.md) |
| **Directive items** | M4.1 and M4.2 (`EDIT` only), M1.2 (repository split), M1.7 (release via kaptaind, partial) |

## 1. Scope

**In scope**

- Repository topology: `/home/sal/xact` becomes its own repository on `elci-group/xact` history, untracked from the home repository, matching deliver, defail and ogma.
- A new `xact-oip` adapter crate that owns the entire `oip` boundary.
- `EDIT` through the whole pipeline: capability requirements, planning, Mesut workload, executor outcome, CLI rendering, completion (already path-aware).
- Hermetic tests (stubbed `oip`), live tests (real `oip`, real clipboard), and live verification through the shell binary.
- Documentation: ADR, README, SPEC §21 table, directive status log, Mesut status.
- Release: kaptaind configuration, commit and push.

**Out of scope**

- Changes to `oip` itself (follow-ups are listed in ADR 0001).
- An undo or revert verb (blocked on a per-file revert in `oip`).
- `PASTE`, `CUT`, `DELETE` (decisions D-5 and D-8 remain open for them).

## 2. Stories

Points use a 1/2/3/5/8 scale for relative effort.

| ID | Story | Pts | Acceptance criteria |
|---|---|---|---|
| S1-01 | **Standalone repository** | 2 | `/home/sal/xact/.git` tracks `origin` = `elci-group/xact`; `main` fast-forwards from `origin/main`; the home repository ignores `/xact/` and no longer tracks it; no local commit is lost. |
| S1-02 | **Decision record** | 1 | ADR 0001 states semantics, preconditions, capabilities, non-goals and upstream follow-ups. |
| S1-03 | **`xact-oip` adapter** | 5 | Invokes exactly `oip -- <abs path>`. Refuses, *before spawning*: a missing path, a directory, a symlink, and a path containing `{` or `}`. Captures `oip`'s output and maps a non-zero exit to an error carrying `oip`'s message. Reports the backup path. Applies `ResourceBudget` through `xact-resource` and registers the child with `xact-cancel`. |
| S1-04 | **Capability model** | 1 | `EDIT` requires `process-spawn`, `filesystem-write`, `clipboard`. `Capability::Clipboard` documents both reading and writing. `! WITHOUT clipboard` rejects `EDIT` at validation. |
| S1-05 | **Planner** | 2 | `£ EDIT <owned path or THIS/THAT>` plans `ExecutionPlan::Edit { path }` with the owner-resolved absolute path; directory, symlink, missing and brace paths plan as `Unsupported` with a reason that names the fix. |
| S1-06 | **Execution path** | 3 | `xact-mesut` submits an `xact.edit` blocking workload (sync and async); `xact-executor` returns `ExecutionOutcome::Edited { path, backup, preview }`; works under `! CONCURRENTLY` with lifecycle events. |
| S1-07 | **Shell output** | 1 | The CLI prints `oip`'s preview and the backup location; a successful edit satisfies `WHEN THAT SUCCEEDS`; failures count toward batch exit status 1. |
| S1-08 | **Verification** | 5 | Hermetic adapter tests against a stub `oip` (argument shape, preconditions not spawning, error mapping, budget denial). Planner and core tests. Live tests with real `oip` and a real clipboard, including under `! WITHOUT network`. Full hermetic and live tiers green; fmt and `clippy -D warnings` clean. |
| S1-09 | **Documentation** | 2 | README, SPEC §21 (`EDIT → oip`), directive status log (M4.2 done for `EDIT`, D-5 partially resolved), Mesut status note. |
| S1-10 | **Release** | 2 | `kaptaind.toml` (actuate, push enabled, `network_push`, unprotected `main`, `patch = 0.0`, test gate), root `VERSION`, `.kaptaind/` ignored. kaptaind commits the sprint and pushes; `git ls-remote origin` shows the new tip; GitHub CI triggered. |

**Total: 24 points.**

## 3. Sequencing

S1-01 → S1-02 → S1-03 → (S1-04, S1-05) → S1-06 → S1-07 → S1-08 → S1-09 → S1-10.

The adapter comes first because every later story depends on its contract. Release is last and is gated on S1-08.

## 4. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `oip` brace-expands a target and edits several files | Medium | High | Refuse `{`/`}` paths before spawning; upstream flag requested. |
| `EDIT` silently creates files, overlapping `bank` | Medium | Medium | Require an existing regular file. |
| Symlink edits escape the owner's domain | Low | High | Refuse symlinks, consistent with `COPY`. |
| Clipboard unreachable inside the network-isolated namespace | Medium | Medium | Verify live under `! WITHOUT network`; if it fails, refuse with a clear reason rather than run unisolated. |
| Test flakiness from writing stub executables in a threaded test binary (`ETXTBSY`) | Medium | Low | Write stubs through a child shell, as `xact-process` tests now do. |
| Live clipboard tests disturb the user's clipboard | High | Low | Live tests restore the previous clipboard text afterwards; they are opt-in (`cargo live-test`). |
| kaptaind pushes nothing (`chore_commit`, protected `main`) | Medium | Medium | `patch = 0.0`, `protect_branches = []`, verify with `git ls-remote`, not kaptaind's log. |
| Running cargo during kaptaind's test gate causes spurious failures | Medium | Low | Run no cargo commands while kaptaind is running. |

## 5. Definition of done

1. Every story's acceptance criteria are met and evidenced in the directive's status log.
2. `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
3. Hermetic tier and live tier both green.
4. `£ EDIT` verified through the real `xact` binary against a real clipboard.
5. Docs updated in the same change.
6. Committed and pushed by kaptaind; remote tip verified.

## 6. Rollback

The sprint is additive. If `EDIT` misbehaves after release, the planner change alone can be reverted to return `EDIT` to `Unsupported`. `kaptaind-cli rollback` reverts the most recent kaptaind commit. File-level damage from an edit is recoverable from `<file>.oip.backup`.

## 7. Outcome (2026-10-12)

All ten stories are done. Every acceptance criterion was met, except that S1-10's GitHub CI result is only visible after the push.

| Measure | Before | After |
|---|---|---|
| Verbs that execute | 6 / 10 | 7 / 10 |
| Hermetic tier | 233 passed / 31 ignored | 240 passed / 33 ignored |
| Live tier | 261 passed | 270 passed |
| Repository | inside the home repo, 5 commits unpushed | standalone `elci-group/xact`, released by kaptaind |

Risk outcome: the clipboard *is* reachable from inside the network-isolated namespace. `oip` uses X11 through XWayland's filesystem socket, which a network namespace does not cut off. Verified live, both by a test and through the shell.

