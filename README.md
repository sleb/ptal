# ptal

[![crates.io](https://img.shields.io/crates/v/ptal.svg)](https://crates.io/crates/ptal)

*Please take a look.* A keyboard-driven terminal UI for reviewing a single text
file, usually a throwaway plan or design doc (`.md`) that you and a coding
agent are iterating on together and that will never be committed.

You read the file, leave threaded comments on lines, and quit. The agent reads
your comments through a small JSON CLI, edits the file, replies, and resolves
threads. You re-open ptal and see what changed. Comments, replies, and resolved
state persist across rounds and revisions.

> **Status:** core library, the agent CLI (`comments`, `add`, `resolve`, `reply`,
> `reopen`, `handoff`), and a TUI implemented: scroll a file (long lines wrap, markdown
> tables align, markdown structure is highlighted), comment on a line or a
> selected range, read threads and the agent's replies, jump between threads,
> search, write multi-line comments, reply, resolve, reopen, `?` lists the
> keys. Live refresh is planned. See [ARCHITECTURE.md](ARCHITECTURE.md).

## Why not a PR?

For code, open a PR. ptal is for the things you wouldn't check in: scratch
plans, specs-in-progress, notes. It is inspired by
[crit](https://github.com/kevindutra/crit) and deliberately smaller:

- One file per review. No code-review mode, no multi-file tabs.
- No tmux integration. Open a pane, run `ptal plan.md`, go back to the agent.
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

**Available**, by mode:

Normal mode:

| Key | Action |
|---|---|
| `j` / `k` (or `↓` / `↑`) | Down / up |
| `ctrl+d` / `ctrl+u` (or `PgDn` / `PgUp`) | Half page down / up |
| `g` / `G` (or `Home` / `End`) | Top / bottom |
| `]` / `[` | Next / previous threaded line (no wrap) |
| `/` | Search the file (see Search below) |
| `n` / `N` | Next / previous line matching the active search, wrapping (the footer says so); matches are highlighted in reverse video, one stop per line however many matches it has |
| `esc` | Clear the active search and its highlight |
| `v` (or `V`) | Select lines from the cursor (see Visual select below) |
| `enter` | On a threaded line: open the thread panel. Otherwise comment on the line |
| `i` | Show the review full-screen: whose turn it is (or the verdict), the latest hand-off, close or reopen, and its whole note |
| `?` | Show the key listing (Normal and thread-panel keys), grouped by mode; the footer ends with `?:help` when it fits |
| `q` (or `ctrl+c`) | Quit |

Visual select:

| Key | Action |
|---|---|
| movement keys, `]` / `[`, `n` / `N` | Extend the selection either way |
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
a long query, after a leading `…`.

A comment can span several paragraphs: in the comment box `enter` starts a
new line and `ctrl+s` saves (leading and trailing blank lines are dropped). With text in
the box, `esc` asks first; a second `esc` discards the draft and any other
key keeps it. Text wraps inside the box, which grows with it
up to 10 rows (and never past half the terminal), then scrolls to follow the
cursor. Pasting into the box keeps the text's line breaks and tab
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
(`N outdated`). The TUI needs an interactive terminal: run without one it
exits `1` with a `not_a_terminal` JSON error.

**Planned**

| Key | Action |
|---|---|
| `e` | Edit your own comment |
| `s` | Toggle the comment sidebar |
| `R` | Show / hide resolved threads |
| (unassigned) | Next / previous line changed since you last reviewed |

### For agents

Everything an agent needs to use the tool is on the CLI: `ptal --help` says what
each command does and what the output means. How you want reviews to run (when
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
ptal reopen plan.md --as claude   # a closed review, when asked to revise again
ptal --help                    # the full agent guide: workflow, JSON shape, exit codes
```

Writes require `--as <name>` (or `PTAL_AUTHOR`) so every message is attributed.
Failures print nothing on stdout and one JSON error line on stderr; exit `2`
means the invocation can be fixed, `1` is a runtime failure (see
[docs/interfaces.md](docs/interfaces.md#exit-codes)). A skill or prompt for
your agent of choice is just: *"run `ptal comments <file>`, edit the file to
address each thread, then `ptal resolve` it."*

**Planned, not built:** `ptal schema` (JSON Schema for all output).

The full command and key map, agent and human side by side, is in
[docs/interfaces.md](docs/interfaces.md).
How a review runs between you and the agent (threads, revisions, turns,
closing with a verdict) is in [docs/protocol.md](docs/protocol.md).

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
