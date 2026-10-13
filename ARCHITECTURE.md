# Architecture

ptal reviews **one file at a time**. A human uses the TUI; an agent uses the
CLI. Both are thin front ends over one core that owns all rules and all
persistence. This document defines the components, what each is responsible
for, and the invariants and contracts between them.

```
        ┌─────────┐      ┌─────────┐
        │   tui   │      │   cli   │        front ends (no business rules)
        └────┬────┘      └────┬────┘
             └───────┬────────┘
               ┌─────▼─────┐
               │  review   │   the only write path; enforces invariants
               └──┬─────┬──┘
          ┌───────▼┐   ┌▼────────┐
          │ anchor │   │  store  │          pure logic │ disk I/O
          └───┬────┘   └────┬────┘
              └──────┬──────┘
               ┌─────▼─────┐
               │   model   │   plain data types, serde, JSON Schema
               └───────────┘
        (tui also uses: highlight, diffview)
```

Dependencies point downward only. `model` depends on nothing in the crate.
`tui` and `cli` never touch `store` or `anchor` directly.

## Status

| Component | State |
|---|---|
| `model` | Built: `Review`, `Thread`, `Status`, `Comment`, `Anchor`, `State` |
| `store` | Built: data dir, atomic `review.json`, lock, schema check |
| `review` | Built (MVP subset): `add_thread`, `view`, `resolve`, `reply`, `reopen` |
| `cli` | Built (MVP subset): `comments`, `add`, `resolve`, `reply`, `reopen` |
| `tui` | Built (MVP subset): `Normal`/`Input` modes, gutter markers, add a one-line comment |
| `anchor`, `highlight`, `diffview` | Deferred |

Deferred within built components: revisions and snapshots, anchor mapping
between file versions, nested replies, edit, delete, `acknowledge`, and
`ptal schema` in the CLI. In the MVP, `state` compares the
quote against the file at the stored lines, with no mapping. Sections below describe deferred design where
they say so.

## Key idea: revisions and anchoring

*Deferred.* The MVP stores no revisions: a thread keeps the line range and
quote from when it was created, and `review` reports `outdated` whenever the
file no longer matches. The design below replaces that without changing the
output shape.

A plan file is mutable and untracked, so there are no commits to anchor to.
ptal supplies its own: every distinct content of the file is a **revision**,
snapshotted and content-addressed (SHA-256) in the data dir.

A thread is anchored to a line range **in the revision it was created on**.
Showing it against a newer revision means mapping that range forward by
diffing the two snapshots (the same approach GitHub uses between PR commits):

- All anchored lines are unchanged → the thread maps to the new line numbers
  (`current`).
- Any anchored line was edited or removed → the thread is `outdated`. It is
  still listed, shown at the best-guess position, with its original quoted text.

No fuzzy text search. Mapping is deterministic and explainable, and outdated
threads are never silently dropped or silently re-pointed at the wrong text.

## Components

### `model`
**Role:** the data types that are persisted and exposed to agents.

- `Review` — one per reviewed file: `schema_version`, `path`, `next_id`,
  `threads`.
- `Thread` — `{ id, anchor, status, comments }`. Its id is its first
  comment's id.
- `Status` — `open` or `resolved`; `resolved_by` and `resolved_at` appear on
  the wire only when resolved. Reopening, or replying to a resolved thread,
  makes the thread `open` and drops them.
- `Comment` — `{ id, author, body, created_at }`. Timestamps are RFC 3339 UTC.
- `Anchor` — `{ start_line, end_line, quote }` (1-based, inclusive); `quote`
  is the anchored lines joined by `\n`.
- `State` — `current | outdated`. Computed by `review` when viewing, never
  persisted.

**Invariants**
- Comment ids are short, human-typeable, and unique within a review (`c1`,
  `c2`, …), assigned from `next_id`, a monotonic counter that is never reused.
  Threads, replies and notes draw from the same counter.
- `start_line <= end_line`, both within the file when the thread is created;
  `quote` is exactly those lines.
- Status lives on the thread.

**Contract:** serde (de)serialization is the on-disk and on-wire format. The
golden-JSON test in `model` pins it. Every persisted file carries
`schema_version`; unknown future versions are refused, not guessed at.

*Deferred:* nested replies (`parent`), edit/delete, `JsonSchema` derives and
`ptal schema`, revisions.

### `anchor`
*Deferred.* Not part of the MVP.

**Role:** pure functions mapping line ranges between revisions.

```
map_range(old_text, new_text, range) -> Mapped { range, state: Current | Outdated }
changed_lines(old_text, new_text)    -> Vec<Range>      // for gutter markers
```

**Invariants**
- Pure: no I/O, no clock, deterministic for the same inputs.
- `Current` implies the mapped lines are byte-identical to the originals.
- `Outdated` always carries a best-guess range clamped to the new file, never
  out of bounds.
- Identity: mapping a range between identical texts returns it unchanged.

### `store`
**Role:** persistence. Reads and writes `Review`s.

**Layout** — `$PTAL_DATA_DIR` or `$XDG_DATA_HOME/ptal` or `~/.local/share/ptal`
(empty variables count as unset):

```
<data>/<slug>-<hash8>/        slug = file name sanitized to [A-Za-z0-9._-];
                              hash8 = of the canonical absolute path
  review.json                 Review (schema_version, path, next_id, threads)
  review.lock                 advisory lock file; never renamed or replaced
```

**Invariants**
- Review identity is the canonicalized absolute path of the file, so a
  symlink or `./` path reaches the same review. Moving the file starts a new
  review.
- Writes are atomic (write `review.json.tmp`, fsync, rename). A crash leaves
  the old or the new `review.json`, never a partial one; a stale temp file is
  overwritten by the next write.
- Every mutation is read-modify-write under an exclusive advisory lock on
  `review.lock`. The lock is a separate file because `review.json` is replaced
  by rename, which would orphan a lock held on it. Acquisition retries for ~5s,
  then fails with `Locked`.
- A `review.json` with a newer `schema_version` is refused (`UnsupportedSchema`)
  and left untouched.
- An unchanged review is not rewritten.

**Contract:** `with_review(file, |&mut Review| -> Result<R>) -> Result<R>` is
the only mutation primitive; an `Err` from the closure writes nothing.
`load(file)` takes no lock: rename makes each read a consistent snapshot. A
review with no `review.json` loads as empty without touching disk.

*Deferred:* `revisions/` snapshots.

### `review`
**Role:** the application core and the **only write path**. Each operation is
a transaction through `store`; each validates its inputs before committing.

Operations (on `Reviews`, which holds a `Store` and an injected `Clock`):

- `add_thread(file, start, end, body, author)` — anchors a new open thread to
  lines `start..=end` of the file as it is now, storing the quote.
- `view(file, include_resolved)` — threads in id order, open only by default,
  each with a computed `state`.
- `resolve(file, id, author, note?)` — marks the thread resolved; a note is
  appended as a second comment by the resolver.
- `reply(file, id, body, author)` — appends a comment to the thread. A reply
  to a resolved thread sets it `open` in the same write; no extra comment is
  synthesised.
- `reopen(file, id, author, note?)` — makes a resolved thread `open`; a note
  is appended as a comment by the reopener. Reopening an open thread is
  `AlreadyOpen` and writes nothing, so the note is dropped.

Free function `file_lines(file)` returns the file's lines under the rule
below, so a front end that displays the file shows exactly what
`add_thread` quotes.

**Invariants**
- Lines are read with `str::lines()` (`\n` or `\r\n`, final newline
  ignored); the file must be valid UTF-8. `file_lines` is the one helper: it
  supplies the quote at `add_thread`, the check in `view`, and the TUI's
  display.
- `state` is `outdated` iff the stored range no longer exists in the file or
  its text differs from `quote`. There is no mapping between versions of the
  file: inserting lines above an anchor makes it `outdated`.
- A mutation that fails (unknown id, bad range, empty author or body) changes
  nothing.
- Every message has a non-empty `author` and `body`.
- `resolve` on a resolved thread is a no-op success: the new note is ignored
  and nothing is written. (`reopen` on an open thread is the opposite:
  `AlreadyOpen`.)
- Error precedence: `reply` checks author, body, then thread id; `reopen`
  checks author, thread id, then status. An id is allocated only after the
  thread is found, so a failed write never burns one.

**Contract:** returns typed results and typed errors (`NotFound`,
`AlreadyOpen`, `InvalidRange`, `FileUnreadable`, `Locked`,
`UnsupportedSchema`, `EmptyAuthor`, `EmptyBody`, `Io`, `Corrupt`). No printing, no terminal awareness. This is the
API both front ends share. Mapping errors to exit codes belongs to `cli`.

*Deferred:* `sync` and revisions, `edit`, `delete`, `acknowledge`, `as_of`
views.

### `cli`

**Role:** the agent-facing front end (`src/cli.rs`, clap derive). Parses args,
calls `review`, prints. The whole flow is `run(args, &Env, out, err) -> u8`,
with the process facts (store, clock, `PTAL_AUTHOR`, TTY flags) injected via
`Env`, so every path is testable in-process; `main` only fills `Env` from the
real process.

Commands: `comments`, `add`, `resolve`, `reply`, `reopen`. Bare `ptal <file>` launches the TUI
(`args_conflicts_with_subcommands`; a file named like a subcommand is written
`./name`). `run` splits into `run_command` and `launch_tui`; the latter
refuses everything it can before the terminal is touched: no TTY on stdin and
stdout (`not_a_terminal`), blank `$USER` (`missing_author`), unreadable file.
The TUI's author is `$USER` (via `Env.user`), never `PTAL_AUTHOR`.

**Contract**
- **stdout** is data only: JSON when not a TTY (or `--json`), a readable
  rendering when a TTY. **stderr** is diagnostics. Never mixed; on failure
  stdout is empty. `--help` and `--version` go to stdout with exit `0`.
- `comments` prints `{"file", "threads": [...]}`. Each thread is self-contained:
  id, `status`, `state`, `anchor` with the quote, and its flat list of
  `comments` (resolved threads add `resolved_by`/`resolved_at`). Default is
  open threads; `--all` includes resolved. Threads are in id order. Replies are
  flat: there is no reply tree yet.
- `add`, `reply`, `resolve` and `reopen` print `{"file", "thread"}` (no
  `state`: it is a read-time concept). Resolving a resolved thread succeeds
  and prints it unchanged; reopening an open thread fails with `already_open`
  (exit `2`).
- There is no `waiting_on` field. A thread is waiting on an agent when its
  last comment's `author` is not the agent's `--as` name (exact match); the
  help says so and tells agents to use one stable name.
- Errors are one line on stderr, `{"error": {"code", "message"}}` (readable
  text on a TTY stderr unless `--json`). The code-to-exit mapping is the
  `Failure::classify` table, pinned by a unit test and documented in
  [docs/interfaces.md](docs/interfaces.md#exit-codes): `2` means the caller
  can fix the invocation, `1` is a runtime failure.
- Writes require `--as <name>` or `PTAL_AUTHOR` (blank counts as unset); the
  CLI never invents an author and never reads `$USER`.
- `--lines` is `N` or `A-B`; syntax errors are `usage`, range validity
  against the file is the core's `InvalidRange`.
- `--help` is part of the product: the top-level help carries the agent
  workflow, the JSON shape, exit codes, and env vars; every command has an
  example.

*Deferred:* `ptal schema`; output shapes are the `model`
types, so changing them is a `schema_version` bump.

### `tui`

**Role:** the human front end (ratatui; crossterm via `ratatui::crossterm`).
Depends on `review` and `model` only.

Three parts:

- `tui::app`: `App` state (`lines`, `cursor`, `top`, terminal `rows`, `mode`,
  gutter `markers`, `outdated` count, footer `status`), `Mode = Normal |
  Input { draft, cursor }`, and `update(&mut App, Event) -> Option<Effect>`.
  `Event` is `Key | Resize | Saved { id, threads } | SaveFailed`; `Effect` is
  `Save { line, body } | Quit`. `update` does no I/O and reads no clock.
- `tui::render`: `render(&App, &mut Frame)`, pure over state. One file line
  is one screen row (long lines are truncated, tabs shown as four spaces).
  Gutter: `●` on lines inside an open, current thread, then the line number,
  then `│`. Footer: path (or the last save message), `L<line>/<total>`,
  `N outdated`, key hints. In `Input` the bordered 3-row box replaces the
  footer and the view shrinks to stay above it; the draft scrolls sideways
  instead of wrapping, and the cursor is a reversed cell.
- `tui` (module root): the driver. `open` loads lines (`review::file_lines`)
  and open threads; `step(app, event, reviews, file, author)` is the only
  place effects are performed (`Save` → `Reviews::add_thread`, then
  `Reviews::view` → `Event::Saved`); `run` is the blocking loop (read event →
  `step` → draw) on a `TerminalSession`.

**Invariants**
- All keys are handled in `update`; `render` holds no logic. The state
  machine is tested without a terminal.
- The TUI owns no persistent state. Every write goes through `review`; after
  a write it re-reads `view` rather than patching local state.
- `top <= cursor < top + height` after every event, where height is the rows
  left after the footer (or input box).
- A failed save keeps the draft and mode and shows the error; nothing is lost
  silently.
- The terminal is restored on every exit path: `TerminalSession` restores on
  drop (normal return, `?`, unwinding), and a chained panic hook restores
  before the panic message prints. `SIGTERM`/`SIGHUP` skip both and are not
  handled.
- Display only; never mutates the reviewed file (it is read once on open).

*Deferred within `tui`:* `Visual` and `Help` modes; wrapping long lines in the
file view; polling the file and `review.json` mtimes and re-syncing; draft
survival across a re-sync; mapping the cursor through `anchor`; viewing,
editing, and resolving threads; thread navigation. Until live refresh exists,
the screen shows the file as read at open, while `add_thread` quotes the file
as it is at save time.

### `highlight`
*Deferred.*

**Role:** syntax highlighting of the reviewed text (syntect), producing styled
spans per line. Pure; language chosen from file extension, plain text
fallback. Depends on nothing else in the crate.

### `diffview`
*Deferred.*

**Role:** the seam that keeps room for other diff presentations. A trait the
TUI renders against:

```rust
trait DiffView {
    /// Per-line decoration for the gutter given changed lines.
    fn gutter(&self, line: usize) -> GutterMark;
    /// Extra rows to inject around a line (e.g. removed lines in an inline view).
    fn virtual_rows(&self, line: usize) -> Vec<Row>;
}
```

v1 ships `GutterMarks` (green marker on changed lines, no virtual rows). Inline
and side-by-side views implement the same trait later without touching `tui`
state or `review`.

## Cross-cutting rules

- **Single write path.** If it changes data, it goes through `review`.
- **The file is read-only to us.** ptal never modifies the reviewed file.
- **Core has no UI knowledge; front ends have no rules.**
- **One interface map.** [docs/interfaces.md](docs/interfaces.md) pairs each
  CLI command with its TUI key. Any change to either front end's surface
  updates it in the same change.
- **Lines are 1-based and inclusive** everywhere in the model and on the wire.
- **Time and randomness are injected** (clock, id counter) so core logic is
  testable deterministically.
- **Forward compatibility.** `schema_version` on every persisted file; refuse
  newer versions, migrate older.

## Testing strategy

- `model`: round-trip and a golden-JSON test pinning the wire shape.
- `store`: data-dir and review-dir naming, stale-temp recovery, schema
  refusal, lock contention.
- `review`: one test per rule, with an injected clock; editing the file between
  calls to exercise `state`.
- `tests/core.rs`: concurrent writers, cross-handle reads, a full review round.
- `anchor` (deferred): property tests (identity, in-bounds, `Current` ⇒
  identical text).
- `cli`: in-process tests (golden JSON, TTY rendering, error table, help) in
  `src/cli.rs`; process tests in `tests/cli.rs` for streams, exit status, and
  env. Schema conformance is deferred with `ptal schema`.
- `tui`: scripted key events through `update` assert on state; `TestBackend`
  snapshots (inline expected strings) cover Normal and Input rendering;
  `tests/tui.rs` runs scripted rounds through `step` against a real
  `Reviews`; the terminal guard and panic hook are tested with injected
  `restore` functions (`tests/panic_hook.rs` is its own process because the
  panic hook is global). A real-terminal check (pty) is manual.

## Deferred

Moving or renaming a reviewed file; MCP server (a thin wrapper over `review`,
should the CLI prove insufficient); inline and side-by-side diff views;
severity or categories; multi-file reviews.
