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
> `reopen`), and a TUI (scroll a file, comment on a line or a range, read threads and the
> agent's replies, jump between threads, reply, resolve, reopen) implemented. Live refresh is planned. See [ARCHITECTURE.md](ARCHITECTURE.md).

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

**Available**

| Key | Action |
|---|---|
| `j` / `k` (or `↓` / `↑`) | Down / up |
| `ctrl+d` / `ctrl+u` (or `PgDn` / `PgUp`) | Half page down / up |
| `g` / `G` (or `Home` / `End`) | Top / bottom |
| `enter` | On a threaded line: open the thread panel. Otherwise comment on the line; `ctrl+s` saves, `enter` starts a new line, `esc` cancels |
| `]` / `[` | Next / previous threaded line |
| `v` (or `V`) | Select lines from the cursor; the movement keys and `]` / `[` extend the selection either way |
| `enter` | In a selection: comment on the selected lines; `ctrl+s` saves, `esc` returns to the selection |
| `esc` (or `q`, `ctrl+c`, `v`, `V`) | In a selection: leave it without commenting (does not quit) |
| `tab` / `shift+tab` | In the panel: next / previous thread on the line |
| `j` / `k` | In the panel: scroll |
| `a` | In the panel: add a thread on this line |
| `r` | In the panel: reply to the selected thread; `ctrl+s` saves, `esc` cancels |
| `x` / `o` | In the panel: resolve / reopen the selected thread (no note) |
| `esc` (or `q`, `ctrl+c`) | In the panel: close it |
| `q` (or `ctrl+c`) | Quit |
| `←` / `→`, `↑` / `↓`, `Home` / `End` | In a comment box: move by character, by screen row, to the line's start / end; `backspace` / `delete` remove (and join lines) |

Long lines wrap at word boundaries onto continuation rows; the line number and
thread marker show on a line's first row, and `j` / `k` move by file line,
not by screen row. Markdown tables show with aligned columns; one too wide
for the window wraps its cells inside their columns, still one cursor stop
per file line. A malformed table, or a window too narrow for it, shows as
written. In a markdown file (`.md`, `.markdown` or `.mdx`) headings,
emphasis, inline code, code blocks, list markers, block quotes and links are
highlighted, in your terminal theme's colors and never hiding a character;
set `NO_COLOR` (to any non-empty value) to turn it off. Other files show
plain.

A comment can span several paragraphs: in the comment box `enter` starts a
new line and `ctrl+s` saves. Text wraps inside the box, which grows with it
up to 10 rows (and never past half the terminal), then scrolls to follow the
cursor.

Comments are written as `$USER`. The gutter marks lines inside a thread: `●`
open, `◆` open and waiting on you (its last comment is not yours), `○`
resolved. The thread panel shows each thread's status, `outdated` state,
quoted text and comments; resolved threads are dimmed. The footer counts open
threads whose quoted text has since changed (`N outdated`). The TUI needs an interactive terminal: run without one it
exits `1` with a `not_a_terminal` JSON error.

**Planned**

| Key | Action |
|---|---|
| `e` | Edit your own comment |
| `s` | Toggle the comment sidebar |
| `R` | Show / hide resolved threads |
| `n` / `N` | Next / previous line changed since you last reviewed |

### For agents

Everything an agent needs to use the tool is on the CLI: `ptal --help` says what
each command does and what the output means. How you want reviews to run (when
to resolve, when to ask, when to publish) is yours to put in your own skill or
prompt. Output is JSON when stdout is not a terminal.

```sh
ptal comments plan.md          # open threads: id, status, state, line range, quote, comments
ptal comments plan.md --all    # include resolved
ptal add plan.md --lines 12-14 "Why not X?" --as claude
ptal reply plan.md c3 "Which of the two do you mean?" --as claude
ptal resolve plan.md c3 --as claude --note "Done, see step 2."
ptal reopen plan.md c3 --as scott --note "Not fixed."
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
