# ADR 0001 — `EDIT` is executed by `oip`

| | |
|---|---|
| **Status** | Accepted (2026-10-12) |
| **Decision owner** | Project owner (directive decision D-5, resolved for `EDIT` only) |
| **Relates to** | SPEC §3, §7, §11, §21, §31 · `XACT_COMPLETION_DIRECTIVE.md` M4.1, M4.2 · Sprint S1 |

## Context

SPEC §7 lists `EDIT` in the imperative vocabulary, and SPEC §11's own example is `£ SEE MY ~/project/README.md` followed by `£ EDIT THAT`. Until now `EDIT` had no execution plan. SPEC §3/§31 require Xact to compose an ELci tool wherever one owns the capability. The owner has decided that `oip` (Overwrite In Place, `github.com/elci-group/oip`) is that tool.

`oip <file>` replaces a file's content with the current clipboard text. It first copies the old content to `<file>.oip.backup`, prints a short before/after preview, and exits non-zero (leaving the file untouched) when the clipboard is empty or not text. Four properties of `oip` constrain the integration:

1. It brace-expands its target argument (`a{1,2}.rs` edits two files) and has no flag to turn that off.
2. If the target does not exist, it creates it.
3. `oip --revert` restores *every* `*.oip.backup` in the current directory, not one file.
4. `oip -i` guesses which file to overwrite.

## Decision

`£ EDIT <target>` means **replace the target file's content with the current clipboard text, through `oip`, which keeps a backup of the previous content.**

- Xact invokes exactly `oip -- <absolute path>`, never `-i` or `--revert`.
- The target must already exist as a regular file. Directories and symlinks are refused, as `COPY` refuses symlinks. A missing file is refused: creating files belongs to `CREATE`/`bank` (SPEC §6), so the answer is `£ CREATE` first.
- A target path containing `{` or `}` is refused before `oip` is spawned, because `oip` would brace-expand it into several files.
- `EDIT` requires the capabilities `process-spawn`, `filesystem-write` and `clipboard`. `! WITHOUT` any of them rejects the command during validation.
- `oip` runs under the session's `ResourceBudget` like every other spawned tool: `SPEND`/`SAVE` cgroup limits, network isolation under `WITHOUT network`, and CTRL-C cancellation.
- The outcome reports the backup path and `oip`'s own preview. `THIS` and `THAT` both name the edited file afterwards.
- Revert is not exposed. `oip --revert` is directory-wide, so mapping it to a per-command undo would restore unrelated files. A per-file revert in `oip` is a prerequisite for the recovery work in directive M4.8.

All `oip`-specific knowledge lives in the `xact-oip` crate. Nothing above it knows `oip` exists (MESUT §29, no semantic leakage).

## Consequences

- `EDIT` now executes; 7 of 10 verbs run.
- `EDIT` depends on a desktop session. Without one, `oip` fails cleanly and `! WITH clipboard` is refused at acceptance.
- Edits are reversible by hand from `<file>.oip.backup`, which holds one level of history.
- `oip` writes in place after taking the backup, so a crash mid-write could truncate the file (the backup survives).

## Follow-ups for `oip` (upstream, not in this sprint)

1. A per-file revert (`oip --revert <file>`).
2. Atomic writes (write to a temporary file, then rename).
3. A flag that disables brace expansion, so Xact can pass any literal path.
4. Align `--version` (reports 0.1.0) with the crate version (0.1.1).
