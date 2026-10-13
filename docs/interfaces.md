# Interface map: agent CLI and human TUI

ptal has two front ends over one core (`review`). This page lists each
operation once, with the way an agent does it and the way a human does it, so
the CLI and the TUI cannot drift apart. It covers what is in the MVP; anything
not listed is not built.

> **Rule: every change to the user-facing surface updates this table.** Adding,
> renaming, or removing a CLI command or flag, or a TUI key or mode, edits the
> table in the same change, for both columns. When one side has no equivalent,
> the cell says so and the asymmetry is listed under "Deliberate asymmetries".
> A change that touches the surface without touching this file is incomplete.

This table owns the command and key surface. Story issues describe scope and
acceptance criteria and link here instead of restating commands.

## Operations

| Operation | Agent (CLI) | Human (TUI) | Core call |
|---|---|---|---|
| Open a review | `ptal comments <file>` (no session) | `ptal <file>` | `view` |
| Move around the file | n/a (the agent reads the file itself) | `j`/`k`, `ctrl+d`/`ctrl+u`, `g`/`G` | none |
| Quit | n/a (process exits) | `q` | none |
| List open threads | `ptal comments <file>` | Gutter marker on commented lines | `view(include_resolved=false)` |
| Read thread text | `comments` output: id, status, `state`, range, quote, comments | n/a | `view` |
| List resolved threads | `ptal comments <file> --all` | n/a | `view(include_resolved=true)` |
| Comment on one line | `ptal add <file> --lines N "text" --as <name>` | `enter` on the line, type, `enter` saves, `esc` cancels | `add_thread` |
| Comment on a range | `ptal add <file> --lines A-B "text" --as <name>` | n/a | `add_thread` |
| Resolve a thread | `ptal resolve <file> <id> --as <name> [--note "..."]` | n/a | `resolve` |
| See whether a thread is stale | `state: "current" \| "outdated"` in `comments` | n/a | `view` |
| Learn the tool | `ptal --help` (workflow, JSON shape, exit codes); `ptal <command> --help` has an example | n/a | none |

## Cross-cutting

| Concern | Agent (CLI) | Human (TUI) |
|---|---|---|
| Author | `--as <name>` or `PTAL_AUTHOR`; never invented | `$USER` |
| Output | JSON when stdout is not a TTY or with `--json`; readable on a TTY | Rendered screen |
| Errors | `{"error": {"code", "message"}}` on stderr when not a TTY or with `--json`; exit codes below | Shown in the UI |
| Seeing the other side's writes | Every call reads from disk | Re-reads after its own writes; reopen to see the agent's |
| Reviewed file | Read-only | Read-only |

## Exit codes

Success prints data on stdout and exits `0`. On failure stdout is empty and
stderr carries one error. The table below is pinned by a unit test in
`src/cli.rs`.

| Exit | Meaning | Error codes |
|---|---|---|
| `0` | Success (also `--help`, `--version`) | none |
| `2` | The caller can fix the invocation | `usage`, `missing_author`, `not_found`, `invalid_range`, `empty_body` |
| `1` | Runtime failure | `file_unreadable`, `locked`, `unsupported_schema`, `io`, `corrupt`, `no_data_dir` |

## Deliberate asymmetries

- **Navigation and quitting** are TUI-only; the agent has no cursor.
- **The MVP TUI only adds single-line comments and shows where threads are.**
  Reading thread text, ranges, resolving, and seeing resolved or outdated state
  are CLI-only for now.
