# ptal

*Please take a look.* A keyboard-driven terminal UI for reviewing a single text
file, usually a throwaway plan or design doc (`.md`) that you and a coding
agent are iterating on together and that will never be committed.

You read the file, leave threaded comments on lines, and quit. The agent reads
your comments through a small JSON CLI, edits the file, replies, and resolves
threads. You re-open ptal and see what changed. Comments, replies, and resolved
state persist across rounds and revisions.

> **Status:** core library, the agent CLI (`comments`, `add`, `resolve`), and
> a minimal TUI (scroll a file, comment on a line, see where threads are)
> implemented. Replies, ranges, resolving from the TUI, and live refresh are
> planned. See [ARCHITECTURE.md](ARCHITECTURE.md).

## Why not a PR?

For code, open a PR. ptal is for the things you wouldn't check in: scratch
plans, specs-in-progress, notes. It is inspired by
[crit](https://github.com/kevindutra/crit) and deliberately smaller:

- One file per review. No code-review mode, no multi-file tabs.
- No tmux integration. Open a pane, run `ptal plan.md`, go back to the agent.
- Agent-agnostic. The contract is JSON on stdout, not a plugin.
- Threads nest arbitrarily and can be resolved and reopened.
- Comments live in your user data directory, never in your repo.

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
| `enter` | Comment on the current line; `enter` saves, `esc` cancels |
| `q` (or `ctrl+c`) | Quit |

Comments are written as `$USER`. A `●` in the gutter marks lines inside an
open thread; the footer counts threads whose quoted text has since changed
(`N outdated`). The TUI needs an interactive terminal: run without one it
exits `1` with a `not_a_terminal` JSON error.

**Planned**

| Key | Action |
|---|---|
| `v` | Visual select, then `enter` to comment on the range |
| `r` | Reply to the focused comment |
| `e` | Edit your own comment |
| `x` | Resolve / reopen the focused thread |
| `s` | Toggle the comment sidebar |
| `[` / `]` | Previous / next thread |
| `R` | Show / hide resolved threads |
| `n` / `N` | Next / previous line changed since you last reviewed |

### For agents

Everything an agent needs is on the CLI. Output is JSON when stdout is not a
terminal.

```sh
ptal comments plan.md          # open threads: id, status, state, line range, quote, comments
ptal comments plan.md --all    # include resolved
ptal add plan.md --lines 12-14 "Why not X?" --as claude
ptal resolve plan.md c3 --as claude --note "Done, see step 2."
ptal --help                    # the full agent guide: workflow, JSON shape, exit codes
```

Writes require `--as <name>` (or `PTAL_AUTHOR`) so every message is attributed.
Failures print nothing on stdout and one JSON error line on stderr; exit `2`
means the invocation can be fixed, `1` is a runtime failure (see
[docs/interfaces.md](docs/interfaces.md#exit-codes)). A skill or prompt for
your agent of choice is just: *"run `ptal comments <file>`, edit the file to
address each thread, then `ptal resolve` it."*

**Planned, not built:** `ptal reply` (threaded replies) and `ptal schema`
(JSON Schema for all output).

The full command and key map, agent and human side by side, is in
[docs/interfaces.md](docs/interfaces.md).

## Where data lives

`$XDG_DATA_HOME/ptal/` (default `~/.local/share/ptal/`, on macOS too). Override
with `PTAL_DATA_DIR`. Nothing is written next to the reviewed file.

## Building

```sh
cargo build --release
cargo test
```
