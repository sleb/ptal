# `tui`

**Role:** the human front end (ratatui; crossterm via `ratatui::crossterm`). It depends on [`review`](review.md) and `model` (and the crate-wide `error`), never on `store`. Everything but the driver is pure: state in, state or rows out. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the layering and [docs/interfaces.md](../interfaces.md) for the keys.

The types, fields and variants are in rustdoc (`cargo doc --open`, start at `ptal::tui::app::App`). This doc keeps what each part owns, the contract others rely on, and the invariants. The markdown highlighter is [highlight.md](highlight.md).

## Modules

- `tui::app`: the state machine. See [App state and modes](#app-state-and-modes).
- `tui::draft`: the comment being typed, pure, no widgets.
  - `text` holds printable characters, `\t` and `\n` only, and rows are byte ranges of it.
  - The cursor always sits on a grapheme cluster boundary.
  - Width is passed in, never cached, so `Draft` stays plain data. `scroll` is clamped by `App` after every event.
  - `paste` inserts in one step: line breaks normalized, tabs kept, other control characters dropped.
- `tui::panel`: `layout(threads, selected, width)`, pure.
  - Each thread's header, quote and comments become rows wrapped by `tui::wrap`, newlines and blank lines kept.
  - `update` clamps scroll from the row count; `render` draws a slice.
- `tui::wrap`: the only place the TUI wraps text, styled (`rows`) or plain (`plain_rows`, `row_count`, `row_ranges`, `clusters`).
  - `cut` and `tail` are the only places one-row text is cut to fit. `…` marks the cut, and a wide cluster is dropped whole.
  - It breaks after the last whitespace that fits and hard-breaks a word wider than a row.
  - It keeps whitespace at a break hanging so rows join back to the line, and never splits a grapheme cluster.
  - Widths are measured as ratatui draws.
- `tui::table`: `align(lines, width)`, pure, the only place tables are parsed.
  - Per file line, it returns its screen rows if the line belongs to a well-formed GFM table that lays out in `width`, else `None`.
  - A table is a header, a delimiter and a body up to the first blank, pipeless or fence line.
  - A table too wide shares the width fairly and wraps cells in their columns.
  - Every row is equally wide, so a table is shown laid out or raw as a whole.
- `tui::keys`: the help listing.
  - `HELP` is a `const` table of every key bound in Normal mode and the thread panel; `rows(width)` lays it out.
  - The handlers are the behavior and `HELP` the documentation.
  - A probe test in `tui::app` presses every candidate key and fails on an unlisted key or a dead listed one. `tests/docs.rs` ties the listing to README and the interface map.
- `tui::search`: `find` and `emphasize`, pure.
  - Matching is smart-case and folds one `char` at a time, so ranges are valid byte ranges.
  - The `/` prompt re-searches from where it was opened, never from the live cursor. `esc` restores that place and `enter` keeps the position and sets the active search.
  - `n`/`N` search raw file lines (a table line on its laid-out rows), one stop per line, so every jump lands on something highlighted.
- `tui::render`: `render(&App, &mut Frame)`, pure over state, with no logic of its own.
  - It draws the rows `App::screen_rows` yields: one file line is one or more rows, and the number, glyphs and highlight show on the first.
  - It also draws the banner, footer, panel and comment box. The module doc lists the layout contract.
  - Text that must fit is cut with `wrap::cut`/`tail`, never clipped by the buffer.
- `tui` (module root): the driver, the only part that does I/O. See [Events and effects](#events-and-effects) and [Live refresh](#live-refresh).

## App state and modes
- `App::open(view, author, size)` is the one constructor, from a single `review::View`.
- A mode is a variant of `Mode`: Normal, Visual, Search, Panel, Input, Overlay. The handlers are not table-driven.
- `update(&mut App, Event) -> Option<Effect>` is the only place keys are handled. It does no I/O and reads no clock.
- Keys read modifiers through one `Mods` helper. Terminals report AltGr as ctrl+alt, so a character typed with it is that plain character in every mode and no ctrl chord fires from it.
- Scroll intent is read once (`Scroll::of`); each mode maps it to its own move. `]`/`[` and `}`/`{` share one `nearest` helper.
- A thread covers its range as `view` maps it to the file as last re-read, whatever its state. The gutter marker is the strongest over its threads: waiting, open, resolved.
- A hand-off or close box saves even when blank; a comment does not.
- A close with no verdict is still sent, so the core's `MissingVerdict` is the one rule.
- The TUI never checks the turn itself: its state can be a refresh behind, so the core decides and a refusal shows like any failed write.
- Submodules each add an `impl App` block and their own tests: `state`, `layout`, `normal`, `find`, `threads`, `input`, `overlay`. Fixtures shared across `tui` modules are in `tui::test_support`.

## Events and effects
- Effects are requests, never performed in `update`. A thread id for `r`, `x` or `o` is captured when the key is pressed, never an index.
- A write's result comes back as an event carrying the one `View` the driver re-read. The app takes its lines, hash, threads, review and changes.
- A re-read keeps you in place: the cursor, `top`, a selection's anchor and the search prompt's origin are mapped through an `anchor::Diff` of the old lines to the new, then clamped.
- A re-read also keeps the panel on its thread by id, never touches a draft, and clears a close box's confirmation.
- The driver's `load` returns the one `Reviews::view` before the terminal is touched. `open` builds the `App` from it and highlights only markdown, with color wanted.
- `step(app, event, reviews, file, author)` is the only place effects are performed.
- Each write goes through `review`, against `app.seen()` for new threads, hand-offs and closes. It is followed by one `view` re-read and comes back as an event.
- A write refused with `FileChanged` re-reads at once and then reports the error, so the draft stays and the next save is made against what is now shown.
- `run` draws, polls events until the next check is due (`POLL_INTERVAL` after the last), and `step`s an event if one came. It redraws only after something that changed the screen.
- Setup turns on raw mode, the alternate screen and bracketed paste (best-effort), so a paste is one `Event::Paste`, not keys.

## Live refresh
- `refresh` takes `Reviews::stamp`. Only when that differs from the last one does it run a full `view` and send `Refreshed`.
- `Refreshed` (the other side's writes) applies the `View` the same way as a write's result and changes nothing else.
- A failing check changes nothing and is retried. After `REFRESH_FAILURES_SHOWN` in a row it sends `RefreshFailed` once.
- `refresh` never writes.
- `run` refreshes when due whether or not keys arrived, so typing can't starve it.
- The file and its review are re-read on open, after each of the TUI's own writes, and whenever the stamp changes (within `POLL_INTERVAL`).

## Layout and rendering
- The rows `render` draws come from `App::screen_rows`.
- The cursor line is whole on screen except when taller than the view; see the scroll invariants below.
- Gutter glyphs mark threads and, separately, the changes since the human's last look; see Change markers in [ARCHITECTURE.md](../../ARCHITECTURE.md#change-markers).
- Tables are laid out by `tui::table`, wrapped lines by `tui::wrap`, and markdown styling by [`highlight`](highlight.md).

## Invariants
- All keys are handled in `update`; `render` holds no logic. The state machine is tested without a terminal.
- The TUI owns no persistent state. Every write goes through `review`; after a write it re-reads `view` rather than patching local state.
- If that re-read fails, the threads and change markers are dropped from the screen (a retry would duplicate a comment). The review state is kept: the one the screen had for a thread write, the write's own for a hand-off, close or reopen.
- After every event `top <= cursor` and lines `top..=cursor` fit in the view's height in screen rows (wrapped rows counted), so the whole cursor line is on screen.
- The one exception is a cursor line taller than the view: `top == cursor` and its tail is cut off. `top` is a file line; there is no sub-line scroll.
- `top` never leaves blank rows below the last line while earlier lines could fill them. The panel never moves the cursor.
- In `Input`, after every event (resizes included) the draft's `scroll` shows its cursor row and leaves no blank rows below the text.
- A failed write keeps the draft and mode and shows the error (in `Input`, on the box's bottom border; elsewhere in the footer). Nothing is lost silently.
- The terminal is restored on every exit path: raw mode, the alternate screen and bracketed paste are undone in the reverse of setup, each step tried even if one fails.
- `TerminalSession` restores on drop (normal return, `?`, unwinding), and a chained panic hook restores before the panic message prints. `SIGTERM`/`SIGHUP` skip both and are not handled.
- Display only; never mutates the reviewed file.
- A new thread, hand-off or close is written against the content on screen (`App::seen`), so a hand-off or close records what the human was shown as their last look, and a thread lands on the lines they saw.

## Contract
- The keys and modes are the table in [docs/interfaces.md](../interfaces.md), listed in the help overlay from `tui::keys::HELP`.
- Writes go only through `review`, and the TUI's author is `$USER`, passed in by `cli`.

## Tests
Unit-test fixtures (key events, apps, views, threads, comments, review statuses) are defined once, in `tui::test_support`. `tests/tui.rs` can't see `cfg(test)` items, and the few helpers it has of its own (`key`, `draw`, `screen`) are named like their unit counterparts.

### By module
- `tui::app`: scripted key events through `update` assert on state, including the search prompt, `n`/`N` and Visual search.
- Re-reads are applied for lines inserted above the cursor, an edited cursor line, a shrink mid-selection, drafts in every box kept, the close confirm cleared, the panel kept by thread id, banner, markers, tables and styling recomputed, and the search origin mapped.
- Screen rows, the clamp and key effects are identical with highlighting on and off. The help probe ties `tui::keys::HELP` to the key handlers.
- `tui::render`: `TestBackend` snapshots (inline expected strings) cover Normal, Visual, Panel and Input rendering, and gutter glyphs and styles.
- The snapshots include the selection highlight on wrapped and table rows before and after a resize, and the comment box wrapping, growing and scrolling its draft.
- The banner and the review overlay are covered in `tui::render` and `tui::app`. The footer hints are pinned by the `tui::render` snapshots.
- `tui::draft`: examples of its rows and cursor cell by width, of cluster and row moves and of paste normalization (tabs as 4-cell clusters).
- A `tui::draft` property over a fixed multi-line draft (one line tab-indented) checks that every cluster boundary is reachable by `↓` then `→` and editable there.
- `tui::wrap`: examples plus a property test over a fixed corpus (ASCII, CJK, emoji, tabs, long words) and widths 1..=12. Rows join back to the line and each fits the width.
- `tui::panel`: examples plus a property over a fixed corpus (ASCII, CJK, emoji ZWJ, combining marks, tabs, newlines) as quotes, authors and bodies, and widths 5..=20.
- The panel property: every row, trailing whitespace trimmed, fits the panel by cells unless it holds a single cluster wider than the room after its indent.
- `tui::table`: examples cover the GFM subset, fences, indentation, ragged blocks, alignment, wide characters, wrapped cells and column-width allocation.
- `tui::table` properties: allocated widths fit, respect the minimums and use all the space when short.
- Over a fixed corpus and view widths 1..=60: one entry per line, every row of a table equally wide and within the view with `│` in the same columns, and no cell text lost.
- `tui::highlight`: see [highlight.md](highlight.md).
- `tests/docs.rs` ties the help listing to README's key tables and the interface map, and keeps every interface-map cell short.
- Driver: `enter_screen`/`leave_screen` are checked for the bracketed-paste escape sequences (`ESC[?2004h`/`ESC[?2004l`, off before leaving the alternate screen) written to a `Vec<u8>`.
- The terminal guard and panic hook are tested with injected `restore` functions. `tests/panic_hook.rs` is its own process because the panic hook is global.

### `tests/tui.rs`
- Scripted rounds through `step` against a real `Reviews`.
- Single-line and Visual range comments are checked through the real `ptal comments`.
- A thread moves with lines inserted above it, then is outdated once its line is edited.
- Turns: a hand-off, a hand-back seen after the next write, closing with and without a verdict, the approve confirmation, reopening, refusals keeping the draft, and the state after a reload.
- Live refresh: the agent's replies, resolves, threads and file edits shown by `refresh` with the cursor and a draft kept; a CLI hand-back flipping the banner; no refresh and no write when nothing changed; a refresh that can't read changing nothing.
- A hand-off, comment or close against an unseen edit is refused with `file_changed` and then made against what is shown.
- Both front ends writing at once: the TUI adds comments through `step` while separate `ptal reply` processes answer and the file is rewritten between them.
- Afterwards every comment is there exactly once, ids are unique, `review.json` parses and every revision has its snapshot.

### Manual
A real-terminal check (pty) is manual.
