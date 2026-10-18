# Changelog

Newest first. Each release gets a `## X.Y.Z - YYYY-MM-DD` section; the release
workflow publishes that section as the GitHub Release notes. See
[docs/releasing.md](docs/releasing.md).

## Unreleased

## 0.2.0 - 2026-10-18

TUI-focused release: richer commenting and a much better reading view.

- Comment on a range of lines with visual select, and write multi-line
  comments in the comment box.
- Incremental `/` search with `n`/`N` and highlighted matches.
- `?` help overlay listing every key.
- Long lines wrap instead of being cut off; one-row text that is cut off is
  marked with an ellipsis; panel comments wrap by cell width.
- Markdown tables render aligned in place, and wide tables wrap their cells.
- Markdown structure is highlighted.
- A failed save's error shows while the comment box is open.
- Docs reviewed against the built behavior.

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
