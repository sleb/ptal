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
| Move around the file | n/a (the agent reads the file itself) | `j`/`k`, `ctrl+d`/`ctrl+u`, `g`/`G`; aliases `↓`/`↑`, `PgDn`/`PgUp`, `End`/`Home`. In Visual mode the same keys (and `]`/`[`) extend the selection | none |
| Quit | n/a (process exits) | `q`, or `ctrl+c` in Normal mode (in Visual mode they only leave the selection) | none |
| List open threads | `ptal comments <file>` | Gutter glyph on covered lines: `●` open, `◆` waiting on you (last comment not by `$USER`), `○` resolved only | `view(include_resolved=false)` |
| Read thread text | `comments` output: id, status, `state`, range, quote, comments | `enter` on a threaded line opens the thread panel: status, `outdated`, quote, every comment with author, time and body | `view` |
| Open a thread's conversation | n/a | In the panel: `tab`/`shift+tab` select among the line's threads (wraps), `j`/`k` (`↓`/`↑`) scroll, `r` replies, `x` resolves, `o` reopens the selected thread, `esc` (or `q`, `ctrl+c`) closes | none |
| Jump between threads | n/a | `]` / `[`: next / previous threaded line, no wrap | none |
| List resolved threads | `ptal comments <file> --all` | Shown in the panel (`resolved by <name>`, dimmed) and as `○` in the gutter | `view(include_resolved=true)` |
| Comment on one line | `ptal add <file> --lines N "text" --as <name>` | `enter` on an unthreaded line (or `a` in the thread panel, which adds a thread beside the existing ones), type, `ctrl+s` saves (leading blank lines and trailing whitespace dropped), `esc` (or `ctrl+c`) cancels; with text in the box the first press asks and a second discards, any other key keeps the draft | `add_thread` |
| Edit the draft | n/a | `←`/`→` move by grapheme cluster, `↑`/`↓` by screen row (keeping the column), `Home`/`End` to the line's start/end; `backspace`/`delete` remove (and join lines); `enter` starts a new line. Text wraps at the box width; the box grows with it to 10 rows (at most half the terminal), then scrolls to follow the cursor | none |
| Comment on a range | `ptal add <file> --lines A-B "text" --as <name>` | `v` (or `V`) starts a line selection at the cursor; the movement keys and `]`/`[` extend it either way (footer `visual A-B`); `enter` opens the box (`Comment on lines A-B`), even on threaded lines (the new thread sits beside any it overlaps); `ctrl+s` saves; `esc` (or `ctrl+c`) in the box returns to the selection (twice with text in the box); `esc`, `q`, `ctrl+c`, `v` or `V` leave the selection without writing | `add_thread` |
| Resolve a thread | `ptal resolve <file> <id> --as <name> [--note "..."]` (`already_resolved` if resolved) | `x` in the thread panel resolves the selected thread (no note; already resolved: footer says so) | `resolve` |
| Reply to a thread | `ptal reply <file> <id> "text" --as <name>` (reopens a resolved thread) | `r` in the thread panel replies to the selected thread (the box says `Reply to <id>`); `ctrl+s` saves, `esc` (or `ctrl+c`) cancels as for a new comment | `reply` |
| Reopen a thread | `ptal reopen <file> <id> --as <name> [--note "..."]` (`already_open` if open) | `o` in the thread panel reopens the selected resolved thread (no note; already open: footer says so) | `reopen` |
| See whether a thread is stale | `state: "current" \| "outdated"` in `comments` | `outdated` in the thread panel; the footer counts open outdated threads (`N outdated`) | `view` |
| Learn the tool | `ptal --help` (workflow, conversation model, JSON shape, exit codes); `ptal <command> --help` has an example | n/a | none |

## Cross-cutting

| Concern | Agent (CLI) | Human (TUI) |
|---|---|---|
| Author | `--as <name>` or `PTAL_AUTHOR`; never invented | `$USER` |
| Output | JSON when stdout is not a TTY or with `--json`; readable on a TTY | Rendered screen |
| Errors | `{"error": {"code", "message"}}` on stderr when not a TTY or with `--json`; exit codes below | A failed write shows its message in the footer and keeps the draft; startup failures print as the CLI does, before the screen is taken over |
| Seeing the other side's writes | Every call reads from disk | Re-reads after its own writes; reopen to see the agent's |
| Reviewed file | Read-only | Read-only |
| Markdown highlighting | n/a (raw text) | `.md`, `.markdown` and `.mdx` files show headings, emphasis, code, lists, quotes and links highlighted; off when `NO_COLOR` is set and non-empty. No key toggles it |

## Exit codes

Success prints data on stdout and exits `0`. On failure stdout is empty and
stderr carries one error. The table below is pinned by a unit test in
`src/cli.rs`.

| Exit | Meaning | Error codes |
|---|---|---|
| `0` | Success (also `--help`, `--version`) | none |
| `2` | The caller can fix the invocation | `usage`, `missing_author`, `not_found`, `invalid_range`, `empty_body`, `already_open`, `already_resolved` |
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
- **The TUI reads threads, adds threads on a line or a line range, replies,
  resolves and reopens.** Notes on resolve/reopen are CLI-only for now. A thread covers its stored
  range whatever its state, so outdated threads carry the same gutter glyph
  and open from it; the panel labels them `outdated`.
- **Ranges and long lines.** The TUI comments on one line or a range of
  whole lines (Visual mode); there is no column selection. Long
  file lines wrap at word boundaries onto continuation rows (the number and
  thread glyph show on the first row only); `j`/`k` move by file line, and
  anchors stay file lines. The TUI shows well-formed markdown tables with
  aligned columns; a table too wide for the window wraps cells inside their
  columns, still one cursor stop per file line. The agent reads the raw
  file; a comment on a table row anchors to that file line and quotes its
  raw text.
