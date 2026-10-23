# ptal

[![crates.io](https://img.shields.io/crates/v/ptal.svg)](https://crates.io/crates/ptal)

*Please take a look.* A keyboard-driven terminal UI for reviewing a single text
file, usually a throwaway plan or design doc (`.md`) that you and a coding
agent are iterating on together and that will never be committed.

You read the file, leave threaded comments on lines, and hand off. The agent
reads your comments through a small JSON CLI, edits the file, replies,
resolves threads, and hands back. ptal's screen follows along as the agent
writes and marks what changed since you handed off, and when the plan is done
you close the review as `approved` or `dropped`.
Comments, replies, resolved state and whose turn it is persist across rounds
and revisions.

> **Status:** the agent CLI (`comments`, `add`, `reply`, `resolve`, `reopen`,
> `handoff`, `wait`) and the TUI are built. In the TUI: scroll a file (long
> lines wrap, markdown tables align, markdown structure is highlighted),
> search, comment on a line or a selected range (multi-line, paste works),
> read, reply to, resolve and reopen threads, hand the turn to the agent,
> close a review with a verdict and reopen it; `?` lists the keys. Threads
> follow the file as the agent edits it, the gutter marks what changed since
> your last look, and the screen refreshes live. Agents block on `ptal wait`
> for their turn. See [ARCHITECTURE.md](ARCHITECTURE.md).

## Why not a PR?

For code, open a PR. ptal is for the things you wouldn't check in: scratch
plans, specs-in-progress, notes. It is inspired by
[crit](https://github.com/kevindutra/crit) and deliberately smaller:

- One file per review. No code-review mode, no multi-file tabs.
- No built-in multiplexer integration: ptal never creates panes. Its help
  tells your agent to open `ptal plan.md` in a pane beside it, or you run
  it yourself in any terminal.
- Agent-agnostic. The contract is JSON on stdout, not a plugin.
- Threads are flat conversations that can be resolved and reopened (nested replies are deferred).
- Comments live in your user data directory, never in your repo.

## Install

```sh
cargo install ptal
```

## Usage

```sh
ptal plan.md            # open the TUI
```

### Keys

Keys typed with AltGr work as the same key pressed plain; `ctrl+` keys need ctrl without alt.

**Available**, by mode:

Normal mode:

| Key | Action |
|---|---|
| `j` / `k` (or `↓` / `↑`) | Down / up |
| `ctrl+d` / `ctrl+u` (or `PgDn` / `PgUp`) | Half page down / up |
| `g` / `G` (or `Home` / `End`) | Top / bottom |
| `]` / `[` | Next / previous threaded line (no wrap) |
| `}` / `{` | Next / previous change since your last hand-off or close (no wrap) |
| `/` | Search the file (see Search below) |
| `n` / `N` | Next / previous line matching the active search, wrapping (the footer says so); matches are highlighted in reverse video, one stop per line however many matches it has |
| `esc` | Clear the active search and its highlight |
| `v` (or `V`) | Select lines from the cursor (see Visual select below) |
| `enter` | On a threaded line: open the thread panel. Otherwise comment on the line |
| `H` | Hand off to the agent: type an optional note, `ctrl+s` (even blank) hands off |
| `C` | Close the review: `tab` picks `approved` or `dropped`, type an optional note, `ctrl+s` closes (approving with open threads asks once more) |
| `O` | Reopen a closed review |
| `i` | Show the review full-screen: whose turn it is (or the verdict), the latest hand-off, close or reopen, and its whole note |
| `?` | Show the key listing (Normal and thread-panel keys), grouped by mode; the footer ends with `?:help` when it fits |
| `q` (or `ctrl+c`) | Quit |

Visual select:

| Key | Action |
|---|---|
| movement keys, `]` / `[`, `}` / `{`, `n` / `N` | Extend the selection either way |
| `/` | Search; the live jumps extend the selection, `enter` and `esc` return to it |
| `enter` | Comment on the selected lines; `esc` in the box returns to the selection (twice with text in the box) |
| `esc` (or `q`, `ctrl+c`, `v`, `V`) | Leave the selection without commenting (does not quit) |

Search:

| Key | Action |
|---|---|
| (typing) | The cursor jumps live to the next matching line (smart-case: case-insensitive unless the query has an uppercase letter); `backspace` edits, and leaves an empty prompt |
| `enter` | Keep the position and make the query the active search |
| `esc` (or `ctrl+c`) | Go back to where you were |

Thread panel:

| Key | Action |
|---|---|
| `tab` / `shift+tab` | Next / previous thread on the line |
| `j` / `k` (or `↓` / `↑`) | Scroll |
| `a` | Add a thread on this line |
| `r` | Reply to the selected thread; `ctrl+s` saves, `esc` cancels (twice with text in the box) |
| `x` / `o` | Resolve / reopen the selected thread (no note) |
| `?` | Show the key listing |
| `esc` (or `q`, `ctrl+c`) | Close the panel |

Comment box:

| Key | Action |
|---|---|
| `enter` | New line |
| `ctrl+s` | Save |
| `esc` (or `ctrl+c`) | Cancel; with text in the box, press it twice (the first asks, any key in between keeps the draft) |
| `←` / `→`, `↑` / `↓`, `Home` / `End` | Move by character, by screen row, to the line's start / end; `backspace` / `delete` remove (and join lines) |
| Paste | Insert the pasted text, line breaks and tabs included |

Help and review overlays:

| Key | Action |
|---|---|
| `j` / `k` (or `↓` / `↑`), `ctrl+d` / `ctrl+u` (or `PgDn` / `PgUp`), `g` / `G` (or `Home` / `End`) | Scroll the listing |
| `?` for help, `i` for the review (or `esc`, `q`, `ctrl+c`) | Close |

The top row is the review's banner: `your turn`, `agent's turn` or
`closed: approved`, the latest note's first line, and the keys for what you
can do now.

Long lines wrap at word boundaries onto continuation rows; the line number and
thread marker show on a line's first row, and `j` / `k` move by file line,
not by screen row. Markdown tables show with aligned columns; one too wide
for the window wraps its cells inside their columns, still one cursor stop
per file line. A malformed table, or a window too narrow for it, shows as
written. In a markdown file (`.md`, `.markdown` or `.mdx`) headings,
emphasis, strikethrough, inline code, code blocks, list markers, block
quotes and links are highlighted, in your terminal theme's colors and never
hiding a character; set `NO_COLOR` (to any non-empty value) to turn it off.
Other files show plain.
One-row text too wide for the window — the footer, a box's title or border
message — is cut and ends in `…`; the search prompt instead keeps the end of
a long query, after a leading `…`. In the footer the search and key hints go first; a
path too long to leave room for the line position and counts keeps its end,
after a leading `…`, so those stay visible.

A comment can span several paragraphs: in the comment box `enter` starts a
new line and `ctrl+s` saves (leading blank lines and trailing whitespace are
dropped). With text in the box, `esc` asks first; a second `esc` discards the
draft and any other key keeps it. Text wraps inside the box, which grows with
it up to 10 rows (and never past half the terminal), then scrolls to follow
the cursor. Pasting into the box keeps the text's line breaks and tab
indentation (a tab shows as four spaces and is saved as a tab); a paste
never saves or closes the box, and outside the box a paste is ignored. If a
save fails, the box stays open with your draft and shows the error on its
bottom border.

Comments are written as `$USER`. The gutter marks lines inside a thread: `●`
open, `◆` open and waiting on you (its last comment is not yours), `○`
resolved. The thread panel shows each thread's status, `outdated` state,
quoted text and comments; resolved threads are dimmed. Threads follow the
file as it changes: lines inserted or deleted above a thread move it, and it
becomes `outdated` once one of its own lines is edited or removed, shown at a
best guess with its original quote. The footer counts open outdated threads
(`N outdated`).

The gutter also shows what changed since your last look, which is your
latest hand-off or close: `┃` (green) on lines added or edited since then,
`-` (red) where text was removed. The footer counts them (`N changes`) and
`}` / `{` jump between them. Opening ptal is not a look, so the markers stay
until you hand off or close again; nothing is marked before your first one.

The screen keeps itself current: a few times a second ptal checks whether the
file or its review changed, and shows the agent's replies, resolves, new
threads, hand-backs and edits to the file without a restart. The cursor
stays on the text it was on, and a half-typed comment or note is kept. If
the file changed after the screen last showed it, a new comment, hand-off
or close is refused (`the file changed: look again`), the screen refreshes,
and `ctrl+s` again saves against what you now see.

The TUI needs an interactive terminal: run without one it
exits `1` with a `not_a_terminal` JSON error.

**Planned**

| Key | Action |
|---|---|
| `e` | Edit your own comment |
| `s` | Toggle the comment sidebar |
| `R` | Show / hide resolved threads |

### For agents

Everything an agent needs to use the tool is on the CLI. `ptal --help` is the
one page to read: the turn loop, the concepts (threads, turns and the verdict,
revisions), exit codes and environment. Each `ptal <command> --help` has that
command's output shape, the errors it can return, and examples. How you want reviews to run (when
to resolve, when to ask, when to publish) is yours to put in your own skill or
prompt. Output is JSON when stdout is not a terminal (or always, with
`--json`).

```sh
ptal comments plan.md          # open threads: id, status, state, line range, quote, comments
ptal comments plan.md --all    # include resolved
ptal add plan.md --lines 12-14 "Why not X?" --as claude
ptal reply plan.md c3 "Which of the two do you mean?" --as claude
ptal resolve plan.md c3 --as claude --note "Done, see step 2."
ptal reopen plan.md c3 --as scott --note "Not fixed."
ptal handoff plan.md --as claude --note "Addressed all but c4."   # your turn, human
ptal wait plan.md              # block until it's the agent's turn or the review closes
ptal wait plan.md --timeout 300   # ...or give up after 5 minutes: reason "timeout"
ptal reopen plan.md --as claude   # a closed review, when asked to revise again
ptal --help                    # the agent guide: the turn loop, concepts, exit codes
ptal comments --help           # one command's output shape, errors and examples
```

The loop `ptal --help` walks through:

0. Once per review, open it for the human: in a terminal multiplexer, a
   pane beside the agent running `exec ptal '<absolute file>'`; otherwise
   ask the human to run `ptal <file>`.
1. `ptal wait <file>`: on `"turn"`, read the human's note; on `"closed"`, act
   on the verdict (`approved`: take the next step; `dropped`: stop) and stop;
   on `"timeout"`, wait again.
2. `ptal comments <file>`, edit the file, `ptal comments` again (edits can
   leave threads outdated).
3. `ptal reply` / `ptal resolve` / `ptal reopen` each thread.
4. `ptal handoff <file>`, then back to 1.

Run `ptal wait` in the background if your agent's command tool can; otherwise
pass a `--timeout` below the tool's time limit and loop.

Writes require `--as <name>` (or `PTAL_AUTHOR`) so every message is attributed.
Failures print nothing on stdout and one JSON error line on stderr; exit `2`
means the invocation can be fixed, `1` is a runtime failure (see
[docs/interfaces.md](docs/interfaces.md#exit-codes)). A skill or prompt for
your agent of choice is just: *"open `ptal <file>` for me in a pane, run
`ptal wait <file>`, then `ptal comments`,
edit the file to address each thread, `ptal resolve` it, `ptal handoff`, and
wait again."*

**Planned, not built:** `ptal schema` (JSON Schema for all output).

The full command and key map, agent and human side by side, is in
[docs/interfaces.md](docs/interfaces.md).
How a review runs between you and the agent (threads, revisions, turns,
closing with a verdict) is in [docs/protocol.md](docs/protocol.md).

**Turns.** A review is on your turn or the agent's, or closed. You hand off
with `H` in the TUI; the agent's `ptal wait` returns with your note (and
`ptal comments` shows `"turn": "agent"`), and it hands back with `ptal handoff`. You close with `C` and a
verdict (`approved`: go ahead; `dropped`: don't), and either side can reopen
a closed review. The turn is a signal, not a lock: both sides can comment at
any time.

## Troubleshooting

**The agent keeps answering itself, or never sees a thread as waiting.** A
thread is waiting on you when its last comment's author is not your `--as`
name (exact match). Use one stable name per agent (set `PTAL_AUTHOR`), and
don't reuse the human's `$USER`.

## Where data lives

`$XDG_DATA_HOME/ptal/` (default `~/.local/share/ptal/`, on macOS too). Override
with `PTAL_DATA_DIR`. Nothing is written next to the reviewed file.

## Building

```sh
cargo build --release
cargo test
cargo install --path .   # install the checkout's binary
```

Requires Rust 1.89 or newer.

Maintainers: see [docs/releasing.md](docs/releasing.md) for cutting a release.

## License

MIT. See [LICENSE](LICENSE).
