# ptal

*Please take a look.* A keyboard-driven terminal UI for reviewing a single text
file, usually a throwaway plan or design doc (`.md`) that you and a coding
agent are iterating on together and that will never be committed.

You read the file, leave threaded comments on lines, and quit. The agent reads
your comments through a small JSON CLI, edits the file, replies, and resolves
threads. You re-open ptal and see what changed. Comments, replies, and resolved
state persist across rounds and revisions.

> **Status:** design phase. See [ARCHITECTURE.md](ARCHITECTURE.md). Nothing
> below is implemented yet; keybindings and commands are the intended shape.

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

### Keys (proposed)

| Key | Action |
|---|---|
| `j` / `k` | Down / up |
| `ctrl+d` / `ctrl+u` | Half page down / up |
| `g` / `G` | Top / bottom |
| `enter` | Comment on the current line |
| `v` | Visual select, then `enter` to comment on the range |
| `r` | Reply to the focused comment |
| `e` | Edit your own comment |
| `x` | Resolve / reopen the focused thread |
| `s` | Toggle the comment sidebar |
| `[` / `]` | Previous / next thread |
| `R` | Show / hide resolved threads |
| `n` / `N` | Next / previous line changed since you last reviewed |
| `q` | Quit |

Gutter markers show lines changed since the revision you last reviewed.

### For agents

Everything an agent needs is on the CLI. Output is JSON when stdout is not a
terminal.

```sh
ptal comments plan.md                 # open threads, with line ranges, quoted text, replies
ptal comments plan.md --all           # include resolved
ptal reply   plan.md c3 "Done, see step 2." --as claude
ptal resolve plan.md c3 --as claude
ptal add     plan.md --lines 12-14 "Why not X?" --as claude
ptal schema                           # JSON Schema for all output
ptal --help                           # includes a short "using ptal as an agent" guide
```

Writes require `--as <name>` (or `PTAL_AUTHOR`) so every message is attributed.
A skill or prompt for your agent of choice is just: *"run `ptal comments
<file>`, address each thread, reply and resolve."*

## Where data lives

`$XDG_DATA_HOME/ptal/` (default `~/.local/share/ptal/`, on macOS too). Override
with `PTAL_DATA_DIR`. Nothing is written next to the reviewed file.

## Building

```sh
cargo build --release
```
