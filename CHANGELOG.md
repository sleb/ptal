# Changelog

Newest first. Each release gets a `## X.Y.Z - YYYY-MM-DD` section; the release
workflow publishes that section as the GitHub Release notes. See
[docs/releasing.md](docs/releasing.md).

## Unreleased

## 0.3.1 - 2026-10-23

Fixes and cleanup. The review store format is unchanged. A few CLI edge cases
changed (below); the shapes of successful JSON output are the same.

**CLI behavior changes** (edge cases, not the normal path):

- A failed write to stdout now fails with error code `io` (exit 1). It was
  `terminal`, a code documented for TUI failures. A closed pipe
  (`ptal comments | head`) is no longer a failure.
- `ptal wait --timeout` that expires before the first read of `review.json`
  now fails with that read's error (for example a missing file), instead of
  a plain timeout.
- `ptal reopen <file> <thread> --side ...` is now rejected by clap as a
  `usage` conflict (exit 2). It was already rejected, by hand in `dispatch`,
  with the same code.
- Parse errors print as JSON only when `--json` is a real flag. `-- --json`,
  where it is a positional argument, no longer counts.
- A file path that is not UTF-8 is printed lossily in JSON instead of
  panicking.

**Other changes**

- `comments` indents continuation lines of a multi-line comment under its
  body.
- TUI: a long path gives way so the footer's counts show. AltGr characters are
  the plain key in every mode, and text in the comment box.
- Build: a curated `[lints.clippy]` table in `Cargo.toml`.
- Internals: one definition each for the waiting rule, write kinds, blank text
  and line numbers; store I/O helpers deduplicated; `tui::app` split up.
- Docs: shorter tables, drift tests for the interface map, and corrections.

## 0.3.0 - 2026-10-21

Turns release: the human and the agent take turns on a review, threads follow
edits to the file, and each side can see what changed while it waited.
Existing `review.json` files still load (`schema_version` stays `1`).

**Breaking (CLI):** `reply`, `resolve` and `reopen` now fail with
`file_unreadable` (exit 1) when the file is missing or not UTF-8. `reopen` with
no thread id now reopens the review.

- Threads follow edits: each write records the file revision it was made
  against, and threads are mapped onto the current file. A thread whose lines
  changed is marked outdated. `comments` gains a top-level `revision`.
- Turns and closing: `ptal handoff` passes the turn, `C` in the TUI closes the
  review with a verdict, and `reopen` (or `O`) reopens it. `comments` gains a
  `review` object (`turn`, `verdict`, `latest`). New errors `not_your_turn`,
  `missing_verdict`, `not_closed` and `already_closed` exit 2.
- TUI: a top banner shows the review's state and the keys you can use, `i`
  opens the review overlay, and `H` hands off.
- `ptal wait <file> [--timeout SECS]` blocks until it is your turn, the review
  closes, or the timeout passes.
- The TUI refreshes live when the file or review changes. The cursor and
  selection follow their text, and a write made against content you haven't
  seen fails with `file_changed` and refreshes the screen, keeping your draft.
- Change markers: the gutter shows lines changed (`┃`) or removed (`-`) since
  your last hand-off or close, the footer counts them, and `}`/`{` jump between
  them.
- Bracketed paste in the comment box; pasted tabs are kept.
- `ptal --help` walks the turn loop, and each `<command> --help` documents its
  output shape, errors and examples.

## 0.2.0 - 2026-10-18

TUI-focused release: richer commenting and a much better reading view.

**Breaking (TUI keys):** in the comment box, `enter` now inserts a newline and
`ctrl+s` saves. `esc` or `ctrl+c` cancels; with text in the box the first
press asks and a second discards.

- Comment on a range of lines: `v` (or `V`) starts a selection, the movement
  keys and `]`/`[` extend it, `enter` opens the box. In a selection, `q` and
  `ctrl+c` only leave the selection.
- Multi-line comments and replies.
- Incremental `/` search with `n`/`N` and highlighted matches.
- `?` help overlay listing every key.
- Long lines wrap instead of being cut off; one-row text that is cut off is
  marked with an ellipsis; panel comments wrap by cell width.
- Markdown tables render aligned in place, and wide tables wrap their cells.
- Markdown structure (headings, emphasis, code, lists, quotes, links) is
  highlighted in `.md`, `.markdown` and `.mdx` files. Set `NO_COLOR` to a
  non-empty value to turn it off.
- A failed save's error shows while the comment box is open.

## 0.1.0 - 2026-10-13

First release. ptal is a terminal tool for reviewing a single throwaway text
file (usually a `.md` plan): you comment in a TUI, an agent reads and resolves
the comments through a JSON CLI.

- TUI: scroll the file, comment on a line, read threads and the agent's
  replies, jump between threaded lines with `]`/`[`, reply, resolve, reopen.
- Agent CLI: `comments`, `add`, `resolve`, `reply`, `reopen`, with JSON on
  stdout and documented exit codes (`already_resolved` exits 2).
- Comments live in your user data directory, never in your repo; threads
  persist across rounds and file revisions.
- Install with `cargo install ptal`, or download a binary from the GitHub
  Release.
- 0.x policy: a minor bump may break the CLI JSON, exit codes, store format or
  TUI keys. See [docs/releasing.md](docs/releasing.md).
