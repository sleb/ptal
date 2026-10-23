# Interface map: agent CLI and human TUI

ptal has two front ends over one core (`review`). This page lists each
operation once, with the way an agent does it and the way a human does it, so
the CLI and the TUI cannot drift apart. It covers what is built; anything not
listed is not built.

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
| Move around the file | n/a (the agent reads the file itself) | `j`/`k`, `ctrl+d`/`ctrl+u`, `g`/`G`; aliases `↓`/`↑`, `PgDn`/`PgUp`, `End`/`Home`. In Visual mode the same keys (and `]`/`[`, `}`/`{`, `n`/`N`) extend the selection | none |
| See what changed since your last look | n/a (deliberate; see asymmetries) | Gutter markers `┃` (green, added or edited) and `-` (red, removed); footer `N changes`; `}`/`{` jump between changes. See [Changes since your last look](#changes-since-your-last-look) | `view` (`View.changes`) |
| Search the file | n/a (the agent greps the file itself) | `/` opens a prompt and the cursor jumps live; `enter` keeps it, `esc` goes back; `n`/`N` step through matches. See [Search](#search) | none |
| Quit | n/a (process exits) | `q`, or `ctrl+c` in Normal mode (in Visual mode they only leave the selection) | none |
| List open threads | `ptal comments <file>` | Gutter glyph on covered lines: `●` open, `◆` waiting on you (last comment not by `$USER`), `○` resolved only | `view(include_resolved=false)` |
| Read thread text | `comments` output: `revision`, the review's state (`review`), and per thread id, status, `state`, range, quote, comments; `revision` fields say which version of the file each part referred to | `enter` on a threaded line opens the thread panel: status, `outdated`, quote, every comment with author, time and body | `view` |
| Open a thread's conversation | n/a | In the panel: `tab`/`shift+tab` select among the line's threads (wraps), `j`/`k` (`↓`/`↑`) scroll, `r` replies, `x` resolves, `o` reopens the selected thread, `esc` (or `q`, `ctrl+c`) closes | none |
| Jump between threads | n/a | `]` / `[`: next / previous threaded line, no wrap | none |
| List resolved threads | `ptal comments <file> --all` | Shown in the panel (`resolved by <name>`, dimmed) and as `○` in the gutter | `view(include_resolved=true)` |
| Comment on one line | `ptal add <file> --lines N "text" --as <name>` | `enter` on an unthreaded line (or `a` in the thread panel), type, `ctrl+s` saves, `esc` (or `ctrl+c`) cancels. See [The comment box](#the-comment-box) | `add_thread` |
| Edit the draft | n/a | Arrow keys, `Home`/`End`, `backspace`/`delete`, `enter` for a new line; AltGr characters and pastes insert as text. See [The comment box](#the-comment-box) | none |
| Comment on a range | `ptal add <file> --lines A-B "text" --as <name>` | `v` (or `V`) starts a line selection; movement extends it; `enter` opens the box (`Comment on lines A-B`); `ctrl+s` saves; `esc` leaves. See [Visual select](#visual-select) | `add_thread` |
| Resolve a thread | `ptal resolve <file> <id> --as <name> [--note "..."]` (`already_resolved` if resolved) | `x` in the thread panel resolves the selected thread (no note; already resolved: footer says so) | `resolve` |
| Reply to a thread | `ptal reply <file> <id> "text" --as <name>` (reopens a resolved thread) | `r` in the thread panel replies to the selected thread (the box says `Reply to <id>`); `ctrl+s` saves, `esc` (or `ctrl+c`) cancels as for a new comment | `reply` |
| Reopen a thread | `ptal reopen <file> <id> --as <name> [--note "..."]` (`already_open` if open) | `o` in the thread panel reopens the selected resolved thread (no note; already open: footer says so) | `reopen` |
| See whose turn it is | `review` in `comments`: `turn`, `verdict` and `latest` (the last hand-off, close or reopen, with its note); on a TTY, a `turn:` or `closed:` header and the latest note | A banner on the top row (`your turn`, `agent's turn`, `closed: approved`) with the latest note's first line; `i` opens the whole review. See [The review banner and overlay](#the-review-banner-and-overlay) | `view` |
| Hand off | `ptal handoff <file> --as <name> [--note "..."]`; prints `{"file", "review", "waiting"}`; `not_your_turn` out of turn or when closed | `H` opens a note box (optional); `ctrl+s` hands off. See [Hand-off and close](#hand-off-and-close) | `hand_off` |
| Wait for your turn | `ptal wait <file> [--timeout SECS]` blocks until it is the agent's turn or the review is closed, then prints `{"file", "reason", "review"}` (`turn`, `closed` or `timeout`; all exit `0`). Never writes. See [Waiting](#waiting) | n/a: the TUI refreshes live (see [Seeing the other side's writes](#seeing-the-other-sides-writes)) | `wait` |
| Close a review | n/a: only the human closes; `comments` shows the `verdict` and the note in `latest` | `C` opens a box; `tab` picks `approved` or `dropped`; the text is an optional note; `ctrl+s` closes. See [Hand-off and close](#hand-off-and-close) | `close` |
| Reopen a review | `ptal reopen <file> --as <name> [--note "..."]` (no thread id), as the agent (`--side human` to act as the human); prints `{"file", "review"}`; back to the human's turn; `not_closed` if active | `O` in Normal reopens the review at once (no note); on an active one the footer says `review is not closed` | `reopen_review` |
| See whether a thread is stale | `state: "current" \| "outdated"` in `comments`; the range is where the anchored text is now (edits above move it), `outdated` once an anchored line is edited or removed, at a best guess with the original quote | `outdated` in the thread panel; the footer counts open outdated threads (`N outdated`); the gutter and panel follow the same mapped range | `view` |
| Learn the tool | `ptal --help` is the one page an agent must read (the turn loop, concepts, conventions, exit codes); each `ptal <command> --help` has that command's output shape, errors and examples | `?` opens a full-screen key listing, grouped by mode. See [Learn the tool](#learn-the-tool) | none |
| Get the agent skill | `ptal skill` prints the review-loop skill as markdown (not JSON, with or without `--json`); save it where your agent loads skills | n/a | none |

## Cross-cutting

| Concern | Agent (CLI) | Human (TUI) |
|---|---|---|
| Author | `--as <name>` or `PTAL_AUTHOR`; never invented | `$USER` |
| Output | JSON when stdout is not a TTY or with `--json` (`file` is the canonical path, non-UTF-8 bytes shown as U+FFFD); readable on a TTY | Rendered screen |
| Errors | `{"error": {"code", "message"}}` on stderr when not a TTY or with `--json`; exit codes below | A failed write shows its message in the footer (on the comment box's bottom border while writing) and keeps the draft; startup failures print as the CLI does, before the screen is taken over |
| Seeing the other side's writes | Every call reads from disk; `ptal wait` blocks until the human hands off or closes | Live refresh: the agent's writes and edits to the file appear without a restart. See [Seeing the other side's writes](#seeing-the-other-sides-writes) |
| Keys typed with AltGr | n/a | A character typed with AltGr, which terminals report as ctrl+alt, acts as that key pressed plain in every mode; ctrl chords such as `ctrl+c`, `ctrl+d` and `ctrl+s` need ctrl without alt |
| Reviewed file | Read-only | Read-only |
| Markdown highlighting | n/a (raw text) | `.md`, `.markdown` and `.mdx` files show headings, emphasis, strikethrough, code, lists, quotes and links highlighted; off when `NO_COLOR` is set and non-empty. No key toggles it |

## Details

Detail that outgrew a table cell. Each section is the full description of the
cell that links to it.

### Changes since your last look

Gutter column 2 shows `┃` (green) on lines added or edited since your last
hand-off or close, and `-` (red) where text was removed; the footer says
`N changes` (hunks). `}`/`{` jump to the next / previous change start, without
wrapping. Nothing is marked before your first hand-off or close; opening `ptal`
is not a look, so markers stay until your next hand-off or close.

### Search

`/` opens a prompt in Normal or Visual mode. Typing searches incrementally from
where `/` was pressed and moves the cursor to the first matching line after it;
`backspace` edits; `enter` keeps the position and makes the query the active
search; `esc` or `ctrl+c` restores the cursor and view. Matching is smart-case.
`n` / `N` jump to the next / previous matching line, wrapping; `esc` in Normal
clears the search. Matches are highlighted and the footer shows `/query`.

### The comment box

On a line, `enter` (an unthreaded line) or `a` (in the thread panel, which adds
a thread beside the existing ones) opens the box; type, `ctrl+s` saves (leading
blank lines and trailing whitespace dropped), `esc` (or `ctrl+c`) cancels. With
text in the box the first press asks and a second discards; any other key keeps
the draft.

Editing: `←`/`→` move by grapheme cluster, `↑`/`↓` by screen row (keeping the
column), `Home`/`End` to the line's start / end; `backspace`/`delete` remove
(and join lines); `enter` starts a new line. Characters typed with AltGr insert
as text, so `[ ] { } \| @ ~ \` and the backtick work on AltGr layouts (see "Keys typed with
AltGr"). Pasting inserts the text at the cursor in one step: line breaks kept,
tabs kept (shown as four spaces), other control characters dropped, so a paste
never saves, cancels or leaves the box; outside the comment box a paste does
nothing. Text wraps at the box width; the box grows with it to 10 rows (at most
half the terminal), then scrolls to follow the cursor.

### Visual select

`v` (or `V`) starts a line selection at the cursor. The movement keys, `]`/`[`,
`}`/`{` and `n`/`N` extend it either way (footer `visual A-B`). `enter` opens
the box (`Comment on lines A-B`), even on threaded lines (the new thread sits
beside any it overlaps); `ctrl+s` saves; `esc` (or `ctrl+c`) in the box returns
to the selection (twice with text in the box). `esc`, `q`, `ctrl+c`, `v` or `V`
leave the selection without writing.

### The review banner and overlay

A banner on the top row reads `your turn`, `agent's turn` or `closed: approved`,
then ` · <author>: <note>` (the note's first line, `…` if cut or longer), and on
the right the keys for what you can do now. `i` in Normal opens the review
full-screen: the state, the latest hand-off, close or reopen with its author,
side, time and revision, and the whole note (`j`/`k`, `g`/`G` scroll; `i`,
`esc`, `q` or `ctrl+c` close). Live refresh keeps both current.

On the CLI, `review` in `comments` is `turn` (`human`, `agent`, or `null` when
closed), `verdict` (`approved`, `dropped`, or `null`), and `latest`: the last
hand-off, close or reopen with its side, author, time, revision, `next_id` and
note.

### Hand-off and close

**Hand off.** The CLI hands off as the agent (`--side human` to act as the
human); `waiting` lists open threads whose last comment isn't yours;
`not_your_turn` if the side doesn't hold the turn or the review is closed. In
the TUI, `H` opens a box titled `Hand off to the agent (note optional)`;
`ctrl+s` hands off, even blank; `esc` (or `ctrl+c`) cancels as for a comment.
The footer then reads `handed off`, with `· waiting on you: c3, c5` when open
threads' last comment isn't yours. Out of turn, `not your turn` shows on the
box's border and the draft is kept.

**Close.** `C` opens a box titled `Close as ? (tab: approved / dropped)`; `tab`
picks `approved`, then `dropped`, then `approved` again, and the title follows;
the text is the note (optional). `ctrl+s` closes; with no verdict chosen, the
core's `missing_verdict` message shows on the border and the draft is kept.
Approving with open threads asks first (`N open threads stay open: ctrl+s
again to approve`): a second `ctrl+s` closes, any other key cancels the ask.
Works during either turn.

### Waiting

`ptal wait` blocks until it is the agent's turn or the review is closed, then
prints `{"file", "reason", "review"}`. `reason` is `turn`, `closed` (with the
`verdict`) or `timeout` (still the human's turn); all exit `0`. `review.latest`
is the hand-off or close that ended it for `turn` and `closed`; on `timeout`
it is `null` for a fresh review. It returns at once if already the
agent's turn or closed; `--timeout 0` checks once; a wait that never read the
review fails with that read's error (`io`, exit `1`). On a TTY: the
`turn: agent` / `closed: approved` header and the note, or
`timeout: still the human's turn`.

### Learn the tool

`ptal --help` holds the turn loop (once per review, open `ptal <file>` for the
human in a multiplexer pane or ask them to run it, then `wait` → `comments` →
edit → reply/resolve/reopen → `handoff` → `wait`) with what each `wait` reason
and verdict means, the concepts (threads, waiting on you, turns, revisions,
`outdated`), author, output and error conventions, the exit-code table and
environment variables. Each `ptal <command> --help` has that command's output
shape, the errors it can return beyond the common ones, and examples.

In the TUI, `?` in Normal mode or the thread panel opens a full-screen key
listing, grouped by mode (`?`, `esc`, `q` or `ctrl+c` close it; `j`/`k`,
`ctrl+d`/`ctrl+u`, `PgDn`/`PgUp`, `g`/`G`, `Home`/`End` scroll it); the footer
ends with `?:help` when it fits.

### Seeing the other side's writes

A few times a second the TUI checks whether the file or its review changed on
disk (by content hash) and, if so, re-reads them: the agent's replies,
resolves, new threads, hand-offs and edits to the file appear without a
restart. The cursor stays on the text it was on (lines inserted above move it
down with them; an edited line keeps a best guess), a selection's ends follow
the same way, an open thread panel keeps the selected thread, and a half-typed
comment, reply or note is never touched (a close waiting for its second
`ctrl+s` asks again).

A check that can't read the file (an editor mid-save) is skipped; if several
checks in a row fail (a corrupt `review.json`, a file that isn't UTF-8), the
footer says `can't refresh: …` once.

A new comment, hand-off or close is made against the content on screen: if the
file changed since, it is refused with `the file changed: look again` on the
box's border, the draft is kept, the screen refreshes at once, and a second
`ctrl+s` saves against what you now see, so a hand-off or close always records
the content you were shown as your last look. Replies, resolves and reopens
are by thread id and aren't refused.

## Exit codes

Success prints data on stdout and exits `0`. On failure stdout is empty and
stderr carries one error. The table below is pinned by a unit test in
`src/cli.rs`.

| Exit | Meaning | Error codes |
|---|---|---|
| `0` | Success (also `--help`, `--version`) | none |
| `2` | The caller can fix the invocation | `usage`, `missing_author`, `not_found`, `invalid_range`, `empty_body`, `already_open`, `already_resolved`, `not_your_turn`, `missing_verdict`, `not_closed`, `already_closed`, `file_changed` |
| `1` | Runtime failure | `file_unreadable`, `locked`, `unsupported_schema`, `io`, `corrupt`, `no_data_dir`, `not_a_terminal`, `terminal` |

`no_data_dir` is reported only by commands that read or write review data.
`--help`, `--version` and usage errors never touch the data directory, so they
behave the same with `PTAL_DATA_DIR`, `XDG_DATA_HOME` and `HOME` all unset.

`not_your_turn` is a hand-off by the side that doesn't hold the turn, or on
a closed review. `missing_verdict` is a close without a verdict; only the TUI
closes, so no CLI command produces it. `file_changed` is TUI-only too: a
new comment, hand-off or close from the TUI is made against the content on
screen, and is refused when the file has changed since (the agent reads the
file itself, so the CLI never passes a hash). `not_closed` is reopening an active
review, and `already_closed` is closing a closed one.

`not_a_terminal` is `ptal <file>` run without a terminal on stdin and
stdout (an agent running it by mistake gets this JSON error instead of a hung
raw-mode process). `terminal` is an I/O failure of the terminal itself while
the TUI runs. A failed write of the result to stdout (including `--help` and
`--version`) is `io`, exit `1`; a closed pipe is not a failure.

## Bare `ptal <file>` and subcommand names

`ptal <file>` opens the TUI. A first argument that is exactly `comments`,
`add`, `resolve`, `reply`, `reopen`, `handoff`, `wait` or `help` is the subcommand; to open a file with one of those names,
write `./comments`. With no arguments, `ptal` prints help and exits `2`. The
`--json` flag is accepted with a file and has no effect on the TUI. Unlike
the agent commands, the TUI takes its author from `$USER` only (never
`PTAL_AUTHOR`); an unset or blank `$USER` is `missing_author`, reported before
the terminal is taken over.

## Deliberate asymmetries

- **Changes since the last look are TUI-only.** They answer the human's "what did the agent change?"; the agent made the edits and reads the file itself, so `comments` carries no changes.
- **Only the agent waits.** `ptal wait` blocks the agent until its turn; the
  human's side of waiting is the TUI's live refresh, so `wait` has no
  `--side`, and no `--as`, since it only reads.
- **Only the human closes a review.** The CLI has no close command; the
  agent reads the verdict and note from `comments`.
- **Navigation and quitting** are TUI-only; the agent has no cursor.
- **The TUI reads threads, adds threads on a line or a line range, replies,
  resolves and reopens.** Notes on resolve/reopen are CLI-only for now, and so is a note on reopening a review (`O` takes none); hand-offs and closes take a note on both sides. A thread covers its range
  as mapped to the file now, whatever its state, so an outdated thread
  carries the same gutter glyph at its best-guess lines and opens from them;
  the panel labels it `outdated` and shows its original quote.
- **Ranges and long lines.** The TUI comments on one line or a range of
  whole lines (Visual mode); there is no column selection. Long
  file lines wrap at word boundaries onto continuation rows (the number and
  thread glyph show on the first row only); `j`/`k` move by file line, and
  anchors stay file lines. The TUI shows well-formed markdown tables with
  aligned columns; a table too wide for the window wraps cells inside their
  columns, still one cursor stop per file line. The agent reads the raw
  file; a comment on a table row anchors to that file line and quotes its
  raw text.
