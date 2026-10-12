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

## Key idea: revisions and anchoring

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

- `Review` — one per reviewed file: identity, ordered revision list, threads.
- `Revision` — `{ id: sha256, seq: u32, seen_at }`. `seq` is 1-based, dense.
- `Thread` — a root `Comment` plus `anchor` and `status`.
- `Comment` — `{ id, parent: Option<id>, author, body, created_at, edited_at?,
  deleted }`. Replies nest to arbitrary depth via `parent`.
- `Anchor` — `{ revision, start_line, end_line, quote }` (1-based, inclusive).
- `Status` — `Open | Resolved { by, at }`.

**Invariants**
- Comment ids are short, human-typeable, and unique within a review (`c1`,
  `c2`, …), assigned from a monotonic counter that is never reused.
- Comments form a forest of trees; `parent` always refers to an existing
  comment in the same thread; no cycles. A thread's id is its root comment's id.
- `anchor.revision` refers to a revision in the review; `start <= end`; both
  lie within that revision's line count; `quote` is exactly those lines.
- Status lives on the thread root only. A reply cannot be "resolved" alone.
- Deleting a comment that has replies tombstones it (`deleted: true`, body
  cleared); the tree shape is preserved.

**Contract:** serde (de)serialization is the on-disk and on-wire format.
Types derive `JsonSchema`; `ptal schema` prints it. Every persisted file
carries `schema_version`; unknown future versions are refused, not guessed at.

### `anchor`
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
**Role:** persistence. Reads and writes `Review`s and revision snapshots.

**Layout** — `$PTAL_DATA_DIR` or `$XDG_DATA_HOME/ptal` or `~/.local/share/ptal`:

```
<data>/<slug>-<hash8>/        slug = file name; hash8 = of canonical abs path
  review.json                 Review (schema_version, path, revisions, threads)
  revisions/<sha256>.txt      immutable snapshots
```

**Invariants**
- Review identity is the canonicalized absolute path of the file. Moving the
  file starts a new review (a future `ptal move` may migrate).
- Writes are atomic (write temp file, fsync, rename). A crash leaves the old
  or the new `review.json`, never a partial one.
- Snapshots are immutable and write-once; a snapshot is written before any
  `review.json` that references it.
- Every mutation is read-modify-write under an exclusive advisory lock on the
  review directory. No caller holds state across a lock release and writes it
  back. This is what lets the TUI and a CLI-driving agent run concurrently.

**Contract:** `with_review(path, |&mut Review| -> R) -> Result<R>` is the only
mutation primitive; `load(path)` is a shared-lock read. Nothing else writes.

### `review`
**Role:** the application core and the **only write path**. Each operation is
a transaction through `store`; each re-checks the `model` invariants before
committing.

Operations:

- `sync(path)` — read the file; if its hash differs from the latest revision,
  snapshot it and append a revision. Idempotent. Called at the start of every
  other operation, so any entry point observes the current file.
- `view(path, as_of: Option<Revision>)` — the review resolved against the
  latest revision: threads with mapped ranges and `Current | Outdated`, plus
  changed lines since `as_of` (default: the revision last acknowledged).
- `add_thread`, `reply`, `edit`, `delete`, `resolve`, `reopen`.
- `acknowledge(path)` — record that the human has seen the latest revision;
  advances the baseline for "changed since last review" markers. Called by the
  TUI on quit, never implicitly by the CLI.

**Invariants**
- Revisions are append-only; `seq` is dense and increasing.
- New threads anchor to the **latest** revision. Line numbers supplied by a
  caller are always interpreted against the latest revision.
- A mutation referencing an unknown comment id fails without changing anything.
- Replying to a resolved thread is allowed and leaves it resolved (resolve /
  reopen is an explicit act). Reopen requires no particular author.
- Every message has a non-empty `author`.
- All operations are idempotent where reasonable (`resolve` on a resolved
  thread is a no-op success).

**Contract:** returns typed results and typed errors (`NotFound`,
`InvalidRange`, `FileUnreadable`, `Locked`, …). No printing, no terminal
awareness. This is the API both front ends share.

### `cli`
**Role:** the agent-facing front end. Parses args (clap), calls `review`,
prints.

Commands: `comments`, `add`, `reply`, `resolve`, `reopen`, `schema`; bare
`ptal <file>` launches `tui`.

**Contract**
- **stdout** is data only: JSON when not a TTY (or `--json`), a readable
  rendering when a TTY. **stderr** is diagnostics. Never mixed.
- `comments` output is self-contained: each thread carries id, range, `state`,
  quoted text, status, and the full nested reply tree, so an agent never has to
  re-derive context. Default is open threads; `--all` includes resolved.
- Exit codes: `0` success, `1` runtime error (file unreadable, locked), `2`
  usage or not-found. Errors are JSON on stderr when not a TTY.
- Writes require `--as <name>` or `PTAL_AUTHOR`; the CLI never invents an
  author.
- `--help` is part of the product: every command shows an example and the
  top-level help includes a short "using ptal as an agent" workflow.
- Output shapes are the `model` types; `ptal schema` is the source of truth.
  Changing them is a `schema_version` bump.

### `tui`
**Role:** the human front end (ratatui + crossterm).

Internally: `App` state (cursor, selection, scroll, focused thread, mode),
`Mode` = `Normal | Visual | Input(comment draft) | Help`, an `update(state,
event)` function, and a pure `render(state, view)`.

**Invariants**
- All keys are handled in `update`; `render` is pure over state and holds no
  logic. The state machine can be tested without a terminal.
- The TUI owns no persistent state. Every write goes through `review`; after a
  write it re-reads `view` rather than patching local state.
- External change is expected: the agent edits the file or writes comments
  while the TUI is open. The TUI polls the file and `review.json` mtimes and
  re-syncs, preserving cursor position by mapping it through `anchor`.
- Unsent comment drafts survive a re-sync.
- Terminal state is always restored on exit, including panic.
- Display only; never mutates the reviewed file.

### `highlight`
**Role:** syntax highlighting of the reviewed text (syntect), producing styled
spans per line. Pure; language chosen from file extension, plain text
fallback. Depends on nothing else in the crate.

### `diffview`
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
- **Lines are 1-based and inclusive** everywhere in the model and on the wire.
- **Time and randomness are injected** (clock, id counter) so core logic is
  testable deterministically.
- **Forward compatibility.** `schema_version` on every persisted file; refuse
  newer versions, migrate older.

## Testing strategy

- `anchor`: property tests (identity, in-bounds, `Current` ⇒ identical text).
- `model`/`review`: invariant tests over operation sequences, including
  tombstones, nested replies, reopen, and multi-revision drift.
- `store`: crash-safety (kill between temp write and rename), concurrent
  writers under the lock.
- `cli`: golden JSON output tests plus a schema-conformance check.
- `tui`: drive `update` with scripted key events and assert on state and on
  `TestBackend` snapshots.

## Deferred

Moving or renaming a reviewed file; MCP server (a thin wrapper over `review`,
should the CLI prove insufficient); inline and side-by-side diff views;
severity or categories; multi-file reviews.
