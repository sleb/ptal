# ptal

[![crates.io](https://img.shields.io/crates/v/ptal.svg)](https://crates.io/crates/ptal)

*Please take a look.* A keyboard-driven terminal UI for reviewing a text
file, usually a throwaway plan or design doc (`.md`) that you and a coding
agent iterate on together. Comments are stored outside your repo (see
[Where data lives](#where-data-lives)).

You read the file, leave threaded comments on lines, and hand off. The agent
reads your comments through a small JSON CLI, edits the file, replies,
resolves threads, and hands back. When the plan is done you close the review
as `approved` or `dropped`. Comments, replies, resolved state and whose turn
it is persist across rounds and revisions.

## Install

### crates.io

```sh
cargo install ptal --locked
```

`--locked` builds with the dependency versions ptal was tested with.

### Prebuilt binaries

Each [GitHub Release](https://github.com/sleb/ptal/releases) has a
`ptal-<version>-<target>.tar.gz` archive per target, and a `SHA256SUMS` file
covering them:

| Target | Platform |
|---|---|
| `aarch64-apple-darwin` | macOS, Apple silicon |
| `x86_64-apple-darwin` | macOS, Intel |
| `x86_64-unknown-linux-gnu` | Linux, x86-64 |
| `aarch64-unknown-linux-gnu` | Linux, arm64 |

Download the archive for your target and `SHA256SUMS` into one directory,
then verify, extract and put `ptal` on your `PATH`:

```sh
shasum -a 256 -c SHA256SUMS --ignore-missing   # expect: OK for your archive
tar -xzf ptal-X.Y.Z-<target>.tar.gz
install ptal-X.Y.Z-<target>/ptal ~/.local/bin/
```

The Linux binaries link the glibc of the build machine and may not run on
older distributions; use crates.io there. There is no Windows build.

To build from a checkout, see [Building](#building). What changed between
versions is in [CHANGELOG.md](CHANGELOG.md).

## Usage

```sh
ptal plan.md            # open the TUI
```

### Keys

Keys are listed by mode. Keys typed with AltGr work as the same key pressed
plain; `ctrl+` keys need ctrl without alt.

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

### Reading and navigation

- Long lines wrap at word boundaries onto continuation rows. The line number
  and thread marker show on a line's first row, and `j` / `k` move by file
  line, not by screen row.
- Markdown tables show with aligned columns, still one cursor stop per file
  line. One too wide for the window wraps its cells inside their columns. A
  malformed table, or a window too narrow for it, shows as written.
- In a markdown file (`.md`, `.markdown` or `.mdx`) headings, emphasis,
  strikethrough, inline code, code blocks, list markers, block quotes and
  links are highlighted in your terminal theme's colors, never hiding a
  character. Set `NO_COLOR` (to any non-empty value) to turn it off. Other
  files show plain.
- One-row text too wide for the window (the footer, a box's title or border
  message) is cut and ends in `…`. The search prompt instead keeps the end
  of a long query, after a leading `…`.
- In the footer the search and key hints go first. A path too long to leave
  room for the line position and counts keeps its end, after a leading `…`.
- The top row is the review's banner: `your turn`, `agent's turn` or
  `closed: approved`, the latest note's first line, and the keys for what you
  can do now.
- The TUI needs an interactive terminal. Without one it exits `1` with a
  `not_a_terminal` JSON error.

### Comments and threads

- A comment can span several paragraphs: `enter` starts a new line and
  `ctrl+s` saves. Leading blank lines and trailing whitespace are dropped.
- With text in the box, `esc` asks first; a second `esc` discards the draft
  and any other key keeps it.
- Text wraps inside the box, which grows with it up to 10 rows (and never
  past half the terminal), then scrolls to follow the cursor.
- Pasting keeps the text's line breaks and tab indentation (a tab shows as
  four spaces and is saved as a tab). A paste never saves or closes the box,
  and outside the box it is ignored.
- If a save fails, the box stays open with your draft and shows the error on
  its bottom border.
- Comments are written as `$USER`.
- The gutter marks lines inside a thread: `●` open, `◆` open and waiting on
  you (its last comment is not yours), `○` resolved.
- The thread panel shows each thread's status, `outdated` state, quoted text
  and comments. Resolved threads are dimmed.
- Threads follow the file as it changes: lines inserted or deleted above a
  thread move it. It becomes `outdated` once one of its own lines is edited
  or removed, and shows at a best guess with its original quote.
- The footer counts open outdated threads (`N outdated`).

### Change markers

- The gutter shows what changed since your last look, which is your latest
  hand-off or close: `┃` (green) on lines added or edited, `-` (red) where
  text was removed.
- The footer counts them (`N changes`), and `}` / `{` jump between them.
- Opening ptal is not a look, so the markers stay until you hand off or
  close again. Nothing is marked before your first hand-off or close.

### Live refresh

- A few times a second ptal checks whether the file or its review changed. It
  shows the agent's replies, resolves, new threads, hand-backs and edits to
  the file without a restart.
- The cursor stays on the text it was on, and a half-typed comment or note
  is kept.
- If the file changed after the screen last showed it, a new comment,
  hand-off or close is refused (`the file changed: look again`). The screen
  refreshes, and `ctrl+s` again saves against what you now see.

### Hand-off and close

- A review is on your turn or the agent's, or closed.
- `H` hands the turn to the agent with an optional note. The agent's
  `ptal wait` returns with it, and `ptal comments` shows `"turn": "agent"`.
- The agent hands back with `ptal handoff`.
- `C` closes with a verdict: `approved` (go ahead) or `dropped` (don't).
  Either side can reopen a closed review (`O` in the TUI).
- The turn is a signal, not a lock: both sides can comment at any time.
- The agent's side of this is in [For agents](#for-agents).

### For agents

Everything an agent needs is on the CLI, and the contract is JSON on stdout,
so any agent works. ptal never creates panes. For the full reference see
`ptal --help` (the turn loop, concepts, exit codes, environment), each
`ptal <command> --help` (output shape, errors, examples),
[docs/interfaces.md](docs/interfaces.md) and
[docs/protocol.md](docs/protocol.md).

```sh
ptal comments plan.md          # open threads (--all includes resolved)
ptal add plan.md --lines 12-14 "Why not X?" --as claude
ptal reply plan.md c3 "Which of the two do you mean?" --as claude
ptal resolve plan.md c3 --as claude --note "Done, see step 2."
ptal reopen plan.md c3 --as claude --note "Not fixed."   # a resolved thread
ptal reopen plan.md --as claude   # a closed review, when asked to revise again
ptal handoff plan.md --as claude --note "Addressed all but c4."
ptal wait plan.md --timeout 300   # block until the agent's turn, closed, or "timeout"
```

The loop:

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

Writes require `--as <name>` (or `PTAL_AUTHOR`) so every message is
attributed. Output is JSON when stdout is not a terminal, or always with
`--json`. Failures print nothing on stdout and one JSON error line on stderr;
exit `2` means the invocation can be fixed, `1` is a runtime failure (see
[docs/interfaces.md](docs/interfaces.md#exit-codes)).

## Troubleshooting

**The agent keeps answering itself, or never sees a thread as waiting.** A
thread is waiting on you when its last comment's author is not your `--as`
name (exact match). Use one stable name per agent (set `PTAL_AUTHOR`), and
don't reuse the human's `$USER`.

**`not_a_terminal`: the interactive view needs a terminal; agents should use `ptal comments`.**
An agent ran `ptal <file>`, which opens the TUI. Agents use the commands under
[For agents](#for-agents); the human runs `ptal <file>` in a terminal.

**`locked`: review is locked by another process.** Another ptal, the TUI or a
command, held the review's lock for more than 5 seconds. Retry. If nothing is
running, find and end the stuck ptal process; the lock is released when its
process exits.

**`file_changed`: the file changed: look again.** The file was edited after
the screen or the agent last read it, so the write was refused. Read again
(`ptal comments`, or let the TUI refresh) and repeat the write.

**`missing_author`: no author: pass --as <name> or set PTAL_AUTHOR.** The TUI
signs comments with `$USER` and exits with this when it is unset. Run
`export USER=<name>`, or `export PTAL_AUTHOR=<name>`.

**My comments vanished after I renamed or moved the file.** A review belongs
to the file's path, so the renamed file started a new, empty review. The old
one is still on disk; see [Where data lives](#where-data-lives).

## Where data lives

`$XDG_DATA_HOME/ptal/` (default `~/.local/share/ptal/`, on macOS too). Override
with `PTAL_DATA_DIR`. Nothing is written next to the reviewed file.

Each reviewed file gets its own directory there, `<slug>-<hash8>/`: `<slug>`
is the file's name, and `<hash8>` is a hash of its canonical absolute path.
Inside:

- `review.json`: the comments, replies, turns and the review's full path.
- `review.lock`: guards writes; safe to ignore.
- `revisions/`: a copy of each version of the file that a comment was made
  against.

The canonical path is the file's identity, so a symlink and its target share
one review. A rename or move gives the file a new path and so a new, empty
review. A copy is a new file too.

ptal never deletes a review, so throwaway ones pile up. To remove one, delete
its directory: `grep -l plan.md <data dir>/*/review.json` finds it, then
`rm -r <data dir>/<slug>-<hash8>`. To remove them all, `rm -r <data dir>`.

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
