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
| Open a review | `ptal comments <file>` (no session) | `ptal <file>` (interactive terminal only; see below) | `view` |
| Move around the file | n/a (the agent reads the file itself) | `j`/`k`, `ctrl+d`/`ctrl+u`, `g`/`G`; aliases `↓`/`↑`, `PgDn`/`PgUp`, `End`/`Home` | none |
| Quit | n/a (process exits) | `q`, or `ctrl+c` in Normal mode | none |
| List open threads | `ptal comments <file>` | Gutter marker on commented lines | `view(include_resolved=false)` |
| Read thread text | `comments` output: id, status, `state`, range, quote, comments | n/a | `view` |
| List resolved threads | `ptal comments <file> --all` | n/a | `view(include_resolved=true)` |
| Comment on one line | `ptal add <file> --lines N "text" --as <name>` | `enter` on the line, type, `enter` saves, `esc` (or `ctrl+c`) cancels | `add_thread` |
| Edit the draft | n/a | `←`/`→`, `Home`/`End` move; `backspace`/`delete` remove; single line | none |
| Comment on a range | `ptal add <file> --lines A-B "text" --as <name>` | n/a | `add_thread` |
| Resolve a thread | `ptal resolve <file> <id> --as <name> [--note "..."]` | n/a | `resolve` |
| Reply to a thread | `ptal reply <file> <id> "text" --as <name>` (reopens a resolved thread) | n/a | `reply` |
| Reopen a thread | `ptal reopen <file> <id> --as <name> [--note "..."]` (`already_open` if open) | n/a | `reopen` |
| See whether a thread is stale | `state: "current" \| "outdated"` in `comments` | n/a | `view` |
| Learn the tool | `ptal --help` (workflow, conversation model, JSON shape, exit codes); `ptal <command> --help` has an example | n/a | none |

## Cross-cutting

| Concern | Agent (CLI) | Human (TUI) |
|---|---|---|
| Author | `--as <name>` or `PTAL_AUTHOR`; never invented | `$USER` |
| Output | JSON when stdout is not a TTY or with `--json`; readable on a TTY | Rendered screen |
| Errors | `{"error": {"code", "message"}}` on stderr when not a TTY or with `--json`; exit codes below | A failed save shows its message in the footer and keeps the draft; startup failures print as the CLI does, before the screen is taken over |
| Seeing the other side's writes | Every call reads from disk | Re-reads after its own writes; reopen to see the agent's |
| Reviewed file | Read-only | Read-only |

## Exit codes

Success prints data on stdout and exits `0`. On failure stdout is empty and
stderr carries one error. The table below is pinned by a unit test in
`src/cli.rs`.

| Exit | Meaning | Error codes |
|---|---|---|
| `0` | Success (also `--help`, `--version`) | none |
| `2` | The caller can fix the invocation | `usage`, `missing_author`, `not_found`, `invalid_range`, `empty_body`, `already_open` |
| `1` | Runtime failure | `file_unreadable`, `locked`, `unsupported_schema`, `io`, `corrupt`, `no_data_dir`, `not_a_terminal`, `terminal` |

`no_data_dir` is reported only by commands that read or write review data.
`--help`, `--version` and usage errors never touch the data directory, so they
behave the same with `PTAL_DATA_DIR`, `XDG_DATA_HOME` and `HOME` all unset.

`not_a_terminal` is `ptal <file>` run without a terminal on stdin and
stdout (an agent running it by mistake gets this JSON error instead of a hung
raw-mode process). `terminal` is an I/O failure of the terminal itself while
the TUI runs.

## Bare `ptal <file>` and subcommand names

`ptal <file>` opens the TUI. A first argument that is exactly `comments`,
`add`, `resolve`, `reply`, or `reopen` is the subcommand; to open a file with one of those names,
write `./comments`. With no arguments, `ptal` prints help and exits `2`. The
`--json` flag is accepted with a file and has no effect on the TUI. Unlike
the agent commands, the TUI takes its author from `$USER` only (never
`PTAL_AUTHOR`); an unset or blank `$USER` is `missing_author`, reported before
the terminal is taken over.

## Deliberate asymmetries

- **Navigation and quitting** are TUI-only; the agent has no cursor.
- **The MVP TUI only adds single-line comments and shows where threads are.**
  Reading thread text, ranges, resolving, replying, reopening, and seeing
  resolved or outdated state are CLI-only for now. Threads whose quoted text no longer matches are not
  marked in the gutter; the footer counts them (`N outdated`).
- **Ranges and long lines.** The TUI comments on one line at a time, and long
  file lines are truncated at the screen edge (no wrapping yet).
