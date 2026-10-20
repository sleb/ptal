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
        (tui also uses: diffview, deferred; highlight lives under tui)
```

Dependencies point downward only. `model` depends on nothing in the crate.
`tui` and `cli` never read or write through `store` or `anchor` directly;
`cli` only constructs the `Store` (in `main` and in tests) to build
`Reviews`.

## Status

| Component | State |
|---|---|
| `model` | Built: `Review`, `Revision`, `Thread`, `Status`, `Comment`, `Anchor`, `State`, the review log (`Entry`, `Event`, `Side`, `Verdict`), its derived `ReviewState`, and the human's last look (`Review::last_look`) |
| `store` | Built: data dir, atomic `review.json`, lock, schema check, `revisions/` snapshots |
| `review` | Built: `add_thread`, `view` (anchors mapped across revisions, the review's state), `resolve`, `reply`, `reopen`, `hand_off`, `close`, `reopen_review` |
| `cli` | Built: `comments` (with the review's state), `add`, `resolve`, `reply`, `reopen` (a thread, or the review), `handoff` |
| `tui` | Built: `Normal`/`Visual`/`Search`/`Panel`/`Input`/`Overlay` modes, a review banner (whose turn, the latest note), an `i` review overlay, hand off (`H`), close with a verdict (`C`), reopen the review (`O`), incremental line search with highlighted matches, gutter markers, word-wrapped long lines, aligned markdown tables (long cells wrap in their columns), thread panel, thread navigation, add a comment on a line or a selected line range, reply, resolve, reopen, multi-line comment box (wraps, grows, scrolls), bracketed paste into the comment box, `?` help overlay, `…` marks cut-off one-row text |
| `highlight` | Built (markdown structure, `tui::highlight`) |
| `anchor` | Built: `map_range`, `Diff` (with `Diff::changes`), `clamp` (Myers line diff via `similar`) |
| `diffview` | Deferred |

Deferred within built components: nested replies, edit, delete,
`ptal schema` in the CLI, and printing the review log's history (only the
latest entry is shown; the whole log is kept on disk for rounds and "changes
since your last look", #46). Sections below describe deferred design where they
say so.

## Key idea: revisions and anchoring

A plan file is mutable and untracked, so there are no commits to anchor to.
ptal supplies its own: every distinct content of the file is a **revision**,
snapshotted and content-addressed (SHA-256) in the data dir. Snapshots are
taken only on writes (see [docs/protocol.md § Revisions](docs/protocol.md#revisions)),
so edits made between two writes collapse into one revision, and viewing
never writes.

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
  `revisions`, `threads`, `log`.
- `Revision` — `{ id, sha256 }`: one kept version of the file, `r1`, `r2`, …
  in the order first written against. `Review::revision_for(sha)` returns the
  known id or appends the next one; there is no separate counter, because
  revisions are never pruned.
- `Thread` — `{ id, anchor, status, comments }`. Its id is its first
  comment's id.
- `Status` — `open` or `resolved`; `resolved_by`, `resolved_at` and
  `resolved_revision` appear on the wire only when resolved. Reopening, or replying to a resolved thread,
  makes the thread `open` and drops them.
- `Comment` — `{ id, author, body, created_at, revision }`. Timestamps are
  RFC 3339 UTC.
- `Anchor` — `{ revision, start_line, end_line, quote }` (1-based,
  inclusive); `quote` is the anchored lines joined by `\n`, and the lines
  refer to `revision`. Anchors are immutable; `review` maps them to the
  current file when viewing.
- Every `revision` field (`anchor.revision`, `comments[].revision`,
  `resolved_revision`) is skipped when `None`, which only data written before
  revisions existed has. `revisions` defaults to empty, so such a
  `review.json` still loads.
- `State` — `current | outdated`. Computed by `review` when viewing, never
  persisted.
- `Entry` — one hand-off, close or reopen in `Review.log`:
  `{ event, side, author, created_at, revision, next_id, note }`. On the wire
  `event` is `"hand_off" | "close" | "reopen"`, a close also carries
  `verdict` (`Verdict`: `approved | dropped`), and `note` is skipped when
  `None`. `side` (`Side`: `human | agent`) is who wrote it; `next_id` is the
  review's counter at that moment, so every comment id below it came first.
  `log` defaults to empty, so a `review.json` from before turns still loads.
- `ReviewState` — `Turn(Side) | Closed(Verdict)`, from `Review::state()`:
  an empty log is the human's turn; a hand-off by one side gives the turn to
  the other, a close sets the verdict, a reopen gives the turn to the human.
  Each entry sets the state outright, so it is the latest entry's. Never
  persisted.
- `Review::last_look()` — the latest log entry written by the human side
  that is a hand-off or a close; its revision is what the human last saw.
  Matched by side, not author. A reopen is not a look, and neither is opening
  the TUI (reads never write). `None` before the first one.

**Invariants**
- Comment ids are short, human-typeable, and unique within a review (`c1`,
  `c2`, …), assigned from `next_id`, a monotonic counter that is never reused.
  Threads, replies and notes draw from the same counter.
- `start_line <= end_line`, both within the file when the thread is created;
  `quote` is exactly those lines.
- Status lives on the thread.
- The log is append-only: entries are only ever pushed, and the review's
  state is derived from it, never stored beside it.

**Contract:** serde (de)serialization is the on-disk and on-wire format. The
golden-JSON test in `model` pins it. Every persisted file carries
`schema_version`; unknown future versions are refused, not guessed at.

*Deferred:* nested replies (`parent`), edit/delete, `JsonSchema` derives and
`ptal schema`.

### `anchor`
**Role:** pure functions mapping line ranges between revisions.

```
map_range(old: &[&str], new: &[&str], range: LineRange) -> Mapped { range, state }
Diff::new(old, new).map(range) -> Mapped     // one diff, many ranges
Diff::new(old, new).changes() -> Vec<Change { range, kind }>
clamp(range, len) -> LineRange               // into 1..=max(len, 1), no diff
```

`LineRange { start, end }` is 1-based and inclusive; `state` is the model's
`State`. It takes lines, not text, so the caller splits them (`review` uses
`str::lines`). The diff is `similar`'s Myers over whole lines; `map_range`
is `Diff::new(old, new).map(range)`. Checking a range costs one pass over
the diff ops, not one per line.

- `Current` iff every anchored line is in an unchanged run of the diff **and**
  the mapped lines are contiguous: a line inserted inside the range makes it
  `Outdated`, since the anchored text no longer reads the same.
- `Outdated` best guess: each end follows the diff; an end on an edited or
  deleted line goes to where that change starts in the new text. The result
  is clamped to `1..=max(new_len, 1)` with `end >= start`, so an empty file
  gives `1-1`.
- `changes()` lists what differs in the new text, one `Change` per hunk (a
  maximal run of adjacent non-equal diff ops). A hunk with new lines is
  `ChangeKind::Changed` over them; a pure deletion is `Removed` at the line
  that now follows the removed text, or at the last line if it was removed at
  the end. Deleting everything marks nothing, and removals before and after a
  sole remaining line are one mark.

**Invariants**
- Pure: no I/O, no clock, deterministic for the same inputs.
- `changes()` are sorted, disjoint and within `1..=new_len`; identical texts
  have none.
- `Current` implies the mapped lines are byte-identical to the originals.
- `Outdated` always carries a best-guess range clamped to the new file, never
  out of bounds. A range outside the old text is `Outdated`, never a panic.
- Identity: mapping a range between identical texts returns it unchanged.

### `store`
**Role:** persistence. Reads and writes `Review`s.

**Layout** — `$PTAL_DATA_DIR` or `$XDG_DATA_HOME/ptal` or `~/.local/share/ptal`
(empty variables count as unset):

```
<data>/<slug>-<hash8>/        slug = file name sanitized to [A-Za-z0-9._-];
                              hash8 = of the canonical absolute path
  review.json                 Review (schema_version, path, next_id, revisions, threads, log)
  review.lock                 advisory lock file; never renamed or replaced
  revisions/<sha256>          raw bytes of each revision written against
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
- A snapshot is named by the SHA-256 of the file's raw bytes (so a change of
  line endings alone is a new revision), written as `<sha>.tmp`, fsynced, then
  renamed into place. An existing snapshot is never rewritten, and none is
  ever removed. The reviewed file itself is only ever read.
- Snapshots kept by a transaction are written after its closure returns `Ok`
  and **before** `review.json` is renamed, all under the lock: a reader that
  sees `rN` in `review.json` always finds its snapshot.

**Contract:** `with_review(file, |&mut Review, &mut Snapshots| -> Result<R>)
-> Result<R>` is the only mutation primitive; `Snapshots::keep(bytes)` stages
a snapshot and returns its hash, and an `Err` from the closure writes nothing,
snapshots included. `load(file)` and `load_revision(file, sha) ->
Option<bytes>` take no lock: rename makes each read a consistent snapshot. A
review with no `review.json` loads as empty without touching disk.
`content_hash(bytes)` is the one hashing rule, shared with `review`.

### `review`
**Role:** the application core and the **only write path**. Each operation is
a transaction through `store`; each validates its inputs before committing.

Operations (on `Reviews`, which holds a `Store` and an injected `Clock`):

- `add_thread(file, start, end, body, author)` — anchors a new open thread to
  lines `start..=end` of the file as it is now, storing the quote.
- `view(file, include_resolved) -> View { revision, lines, threads, review }`
  — threads in id order, open only by default, each with its anchor's lines
  mapped to the file as it is now and a computed `state`; `revision` is the
  current content's id, or `None` if nothing has been written against it
  yet; `review` is the `ReviewStatus`. One read of the file serves all of
  them.
- `resolve`, `reply` and `reopen` return the thread with its lines mapped to
  the content they read (the stored anchor is unchanged); `add_thread`'s
  range is as given, since it anchors to the current content.
- `resolve(file, id, author, note?)` — marks the thread resolved; a note is
  appended as a second comment by the resolver. Resolving a resolved thread
  is `AlreadyResolved` and writes nothing, so the note is dropped.
- `reply(file, id, body, author)` — appends a comment to the thread. A reply
  to a resolved thread sets it `open` in the same write; no extra comment is
  synthesised.
- `reopen(file, id, author, note?)` — makes a resolved thread `open`; a note
  is appended as a comment by the reopener. Reopening an open thread is
  `AlreadyOpen` and writes nothing, so the note is dropped.
- `hand_off(file, side, author, note?) -> HandedOff { review, waiting }` —
  appends a `hand_off` entry by `side` to the log and passes the turn to the
  other side. `review` is the resulting `ReviewStatus`; `waiting` lists the
  open threads whose last comment isn't by `author` (exact match), the same
  "waiting on you" rule the TUI's gutter uses. A hand-off by the side
  without the turn, or on a closed review, is `NotYourTurn`.
- `close(file, author, verdict?, note?) -> ReviewStatus` — the human ends
  the review with a `close` entry, during either side's turn. Open threads
  stay open. `verdict` is an `Option` so a front end can pass on an
  unchosen one: `None` is `MissingVerdict`. Closing a closed review is
  `AlreadyClosed`, so a verdict never changes under an agent acting on it.
- `reopen_review(file, side, author, note?) -> ReviewStatus` — either side
  brings a closed review back, on the human's turn. On an active review it
  is `NotClosed`.

`ReviewStatus { state, latest }` is the review's state as front ends see
it: the `ReviewState` and the last log entry, if any. It serializes as
`{"turn", "verdict", "latest"}`: `turn` is `null` exactly when the review is
closed, and `verdict` is set exactly then.

`View.lines` is the file's lines from the same read the threads were mapped
against, so a front end that displays them shows exactly what `add_thread`
quotes and where each thread is.

**Invariants**
- Lines are read with `str::lines()` (`\n` or `\r\n`, final newline
  ignored); the file must be valid UTF-8. One rule supplies the quote at
  `add_thread`, the mapping in `view`, and `View.lines` for the TUI's display.
- Anchors are never rewritten on disk. `view` maps each one from its own
  revision's snapshot to the current file with an `anchor::Diff`, so an edit
  that is later reverted maps back to `current`. Each revision is loaded and
  diffed against the current file at most once per call.
- `state` is `current` iff the mapping is `Current` **and** the lines at the
  mapped range read as `quote` (the second check keeps an adopted legacy
  anchor whose quote no longer matched from turning `current`). Otherwise it
  is `outdated`, at the best-guess range, with the original `quote`.
- An anchor with no revision (from before revisions, not yet adopted) is
  checked in place against the current file. One whose snapshot is missing
  or unreadable (deleted or damaged by hand) is `outdated` at its stored
  range clamped to the file; it is still listed, and mapping never fails a
  read or a write.
- A mutation that fails (unknown id, bad range, empty author or body) changes
  nothing, snapshots included.
- Every write reads the file once, outside the lock, and records the revision
  it was made against, inside the lock: it keeps the snapshot and assigns the
  next ordinal if the content is new. `add_thread` sets `anchor.revision` and
  the comment's `revision`; `reply` sets the comment's; `resolve` sets
  `resolved_revision` and the note's; `reopen` sets only its note's (a reopen
  itself is not recorded, so one without a note keeps no snapshot and
  assigns no ordinal); `hand_off`, `close` and `reopen_review` set their log
  entry's. Reads hash the file but never write.
- A thread anchored before revisions existed (`anchor.revision` absent) is
  anchored to the revision of the first write after the upgrade, "the file as
  first seen"; its old comments keep no revision.
- Every message has a non-empty `author` and `body`.
- The turn is a signal, not a lock: no thread write reads it or moves it.
  Comments, replies, resolves and reopens succeed during either side's turn,
  or on a closed review, and leave the log alone; they never reopen a
  closed review.
- A log entry records the revision of the file as it is now, like any other
  write, and the review's `next_id`. A blank note is no note.
- `resolve` on a resolved thread is `AlreadyResolved` and `reopen` on an open
  thread is `AlreadyOpen`: both write nothing, so a note is dropped.
- Error precedence: `reply` checks author, body, file, then thread id;
  `resolve` and `reopen` check author, file, thread id, then status. A file
  that is missing or not UTF-8 fails every write with `FileUnreadable`. An id
  is allocated only after the thread is found, so a failed write never burns
  one. `hand_off` checks author, file, then the turn under the lock;
  `close` checks author, verdict, file, then whether it is closed;
  `reopen_review` checks author, file, then whether it is closed.

**Contract:** returns typed results and typed errors (`NotFound`,
`AlreadyOpen`, `AlreadyResolved`, `NotYourTurn`, `MissingVerdict`,
`NotClosed`, `AlreadyClosed`, `InvalidRange`, `FileUnreadable`, `Locked`,
`UnsupportedSchema`, `EmptyAuthor`, `EmptyBody`, `Io`, `Corrupt`). No printing, no terminal awareness. This is the
API both front ends share. Mapping errors to exit codes belongs to `cli`.

*Deferred:* `sync`, `edit`, `delete`, `as_of` views.

### `cli`

**Role:** the agent-facing front end (`src/cli.rs`, clap derive). Parses args,
calls `review`, prints. The whole flow is `run(args, &Env, out, err) -> u8`,
with the process facts (store, clock, `PTAL_AUTHOR`, TTY flags) injected via
`Env`, so every path is testable in-process; `main` only fills `Env` from the
real process.

Commands: `comments`, `add`, `resolve`, `reply`, `reopen`, `handoff`. Bare `ptal <file>` launches the TUI
(`args_conflicts_with_subcommands`; a file named like a subcommand is written
`./name`). `run` splits into `run_command` and `launch_tui`; the latter
refuses everything it can before the terminal is touched: no TTY on stdin and
stdout (`not_a_terminal`), blank `$USER` (`missing_author`), unreadable file.
The TUI's author is `$USER` (via `Env.user`), never `PTAL_AUTHOR`.

**Contract**
- **stdout** is data only: JSON when not a TTY (or `--json`), a readable
  rendering when a TTY. **stderr** is diagnostics. Never mixed; on failure
  stdout is empty. `--help` and `--version` go to stdout with exit `0`.
- `comments` prints `{"file", "revision", "threads": [...]}`; `revision` is
  the file's current revision id, or `null` if nothing has been written
  against this content. Each thread is self-contained: id, `status`, `state`,
  `anchor` (its `revision`, its lines mapped to the file now, and the
  original quote), and its flat list of `comments`, each with its `revision`
  (resolved threads add `resolved_by`/`resolved_at`/`resolved_revision`). Default is
  open threads; `--all` includes resolved. Threads are in id order. Replies are
  flat: there is no reply tree yet. A top-level `review` is the
  `ReviewStatus`: `turn`, `verdict`, and `latest`, the last log entry in its
  on-disk shape. Only the latest entry is printed; the full log stays on
  disk.
- `add`, `reply`, `resolve` and `reopen` print `{"file", "thread"}` (no
  `state`: it is a read-time concept). The printed anchor's lines are mapped
  to the file as it is now, like `comments`, so the CLI never shows unmapped
  lines; `anchor.revision` stays the revision the thread was created on. Resolving a resolved thread fails with
  `already_resolved` and reopening an open thread with `already_open` (both
  exit `2`).
- `handoff <file> [--side human|agent] [--note]` prints `{"file", "review",
  "waiting"}`; `reopen <file>` with no thread id reopens the review and
  prints `{"file", "review"}`. The CLI acts as the agent; `--side human`
  overrides it on these two (a flag, not an environment variable, so an
  agent can't inherit it). `--side` on a thread reopen is `usage`: threads
  have no side. There is no CLI close; only the human closes, in the TUI.
- Threads carry no `waiting_on` field. A thread is waiting on an agent when
  its last comment's `author` is not the agent's `--as` name (exact match);
  the help says so and tells agents to use one stable name. A hand-off's
  `waiting` applies the same rule to the author handing off.
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
  example. It states facts about the tool (what a command does, what a field
  means), not policy: when to resolve, what to ask, how to hand over to the
  human belong to the user's own skill or prompt, which ptal does not ship.

*Deferred:* `ptal schema`; output shapes are the `model`
types, so changing them is a `schema_version` bump.

### `tui`

**Role:** the human front end (ratatui; crossterm via `ratatui::crossterm`).
Depends on `review` and `model` (and the crate-wide `error`), never on
`store`.

Its parts:

- `tui::app`: `App` state (`lines`, their `tables` rows from `tui::table`
  at `text_width()`, recomputed on open and on resize, their `stylings` from
  `tui::highlight`, computed once by `App::highlighted(true)` since the file
  never changes in a session, `cursor`, `top`, terminal `cols` and
  `rows`, `mode`, `author`, every thread of the file including resolved and
  outdated ones, the `review`'s `ReviewStatus`, footer `status`, the active
  `search` query, the `help` and review overlay listings laid out for
  `cols`, the `file` label), `Mode = Normal | Visual { anchor } |
  Search { query, anchor, origin, hit } |
  Panel { selected, scroll } | Input { draft, discard_asked, back, target } |
  Overlay { kind, back, scroll }` (`kind` is `Overlay::Help | Review`;
  `target` is what saving the box writes: `Target::NewThread | Reply(id) |
  HandOff | Close { verdict, confirm_asked }`)
  (`draft` is a `tui::draft::Draft`;
  `back` is the `Back` the box returns to on `esc`: `Normal | Panel {
  selected, scroll } | Visual { anchor }`), and
  `update(&mut App, Event) -> Option<Effect>`. In `Visual` the cursor is the
  selection's moving end and `anchor` its fixed one (0-based file lines), so
  Normal and Visual share one movement helper and the view clamp keeps the
  moving end on screen; `selection()` is the ordered `(first, last)` of the
  two in `Visual` and in a box opened from it (the range stays fixed while
  typing), else the cursor line alone. `enter` in `Visual` always opens a
  box for a new thread on the selection, never the panel; a save from it
  goes to Normal. In `Input`, `enter` inserts a newline and `ctrl+s`
  (or `ctrl+S`) saves the draft as typed less trailing whitespace and leading blank lines (one that
  is only whitespace and newlines saves nothing). `esc` or `ctrl+c` returns
  to `back` at once on a blank draft; otherwise the first sets
  `discard_asked` and only a second discards, any other key (or a paste)
  clearing it. A paste in `Input` goes in through `Draft::paste` and clears
  `status` as a key does; in every other mode it is ignored, `status`
  included. The box is as tall as
  its draft's rows at `draft_width()` (terminal columns less the border and
  one cell for a cursor after a full row), at least 1, capped at
  `min(10, rows / 2 - 2)` (at least 1), plus its 2 border rows; the view
  shrinks to stay above it. A thread covers its range as
  `view` maps it to the file read at open (or after a save), whatever its
  `state`, so an outdated thread sits at its best-guess lines; `Marker = Waiting | Open | Resolved` is derived
  per line (waiting > open > resolved). Waiting means open and last comment
  not by `author`. The panel's thread set is recomputed from the threads and
  the cursor, never cached. `screen_rows()` maps the view to
  `ScreenRow { line, first, text }`, filling the view height from `top` with
  each line's rows at `text_width()` (terminal columns minus the gutter, at
  least 1): its `tables` rows when it has them, in the line's whole-line
  style only, else the `tui::wrap` rows of the raw line with its styling
  applied. `text` is a styled ratatui `Line`; styling never changes a row's
  text, so layout, the clamp and every key behave the same highlighted or
  not. `render` draws from it, and the view clamp in `update` counts the
  same rows; `lines()` stays the raw file.
  `Event` is `Key | Resize | Saved { id, done, threads, review } |
  ReviewSaved { message, threads, review } | SaveFailed | Paste(text)` (the
  terminal's bracketed paste), where
  `done` (`Added | Replied | Resolved | Reopened`) picks the footer message and `id` is
  always the thread id; `Effect` is `Add { start, end, body } | Reply { id,
  body } | Resolve { id } | Reopen { id } | HandOff { note } | Close {
  verdict, note } | ReopenReview | Quit`, where `start..=end` is
  1-based (`start == end` for one line). The target thread id is captured when `r`, `x` or `o` is pressed, never
  an index; `x` on a resolved thread and `o` on an open one are refused in
  `update` with a footer message. `update` does no I/O and reads no clock.
  `H` opens a box with `Target::HandOff` and `C` one with `Target::Close`;
  both save on `ctrl+s` even when blank (no note), unlike a comment. In a
  close box `tab` cycles the verdict (`approved`, `dropped`, `approved`);
  `ctrl+s` with `approved` and open threads first sets `confirm_asked` and
  returns nothing, a second `ctrl+s` sends `Close`, and any other key clears
  it. A close with no verdict is still sent, as `verdict: None`, so the
  core's `MissingVerdict` is the one rule. `O` sends `ReopenReview` at once.
  The TUI never checks the turn itself: its state can be stale until live
  refresh (#36), so the core decides and a refusal shows like any failed
  write. `ReviewSaved` stores the re-read threads and state, returns to
  Normal and shows `message` in the footer.
- `tui::draft`: `Draft { text, cursor, goal, scroll }`, pure, no widgets:
  the comment being typed. `text` holds printable characters, `\t` and
  `\n` only, so `wrap` drops none of it and rows are byte ranges of it; a
  zero-width cluster has a range but no cell. `insert` refuses every
  control character, so a tab is never typed; `paste(text)` inserts at the
  cursor in one step, making `\r\n`, `\r` and `\n` each a `\n`, keeping
  `\t` and dropping every other control character, then leaves the cursor
  after it (on a boundary). A tab is one cluster four cells wide, drawn as
  four spaces.
  `cursor` is a byte offset that always sits on a grapheme cluster boundary
  (`wrap::clusters`, plus each `\n` as its own cluster): `←`/`→`,
  `backspace` and `delete` move over or remove one whole cluster, and an
  edit that leaves the cursor inside a cluster (a typed joiner, or a removal
  that lets two halves of a flag pair up) moves it to that cluster's end. `rows(width)` is each line (split on `\n`) wrapped by
  `wrap::row_ranges`, as byte ranges of `text`, an empty line or trailing
  `\n` giving one empty row; `cursor_at(width)` is the cursor's (row,
  column in cells). An offset at a soft break shows at the start of the next
  row, and one on hanging whitespace in cell `width`. The width is always
  passed in, never cached, so `Draft` stays plain data (and `Mode` `Eq`).
  `scroll` is the first row the box shows; `App` calls
  `clamp_scroll(width, height)` after every event with the box's width and
  text rows, so the draft holds where it is scrolled, not how it is laid
  out.
  `up(width)`/`down(width)` move by screen row (wrapped rows and lines
  alike) to the last boundary at or before a sticky goal column (`goal`,
  in cells, kept across consecutive `↑`/`↓` and cleared by any other edit or
  move); on a soft-wrapped row that is at most the start of its last
  cluster. `home`/`end` go to the ends of the cursor's whole line, not its
  screen row.
- `tui::panel`: `layout(threads, selected, width) -> Layout { lines,
  thread_starts }`, pure. Per thread: header (`c3 open`, `c3 open · outdated`,
  `c3 resolved by claude`), `> ` quote lines, then each comment's
  `author · YYYY-MM-DD HH:MMZ` and body. Every row is wrapped by
  `tui::wrap::plain_rows`, one source line at a time, so newlines and blank
  lines are kept and whitespace shows as written. Text fits the panel by
  cells; only hanging whitespace at a break is clipped, and a single cluster
  wider than the room after its indent overflows. `update` clamps scroll
  from `lines.len()`; `render` draws a slice.
- `tui::wrap`: `rows(&Line, width) -> Vec<Line>`, pure, the only place the
  TUI wraps text; `plain_rows(text, width) -> Vec<String>` and
  `row_count(text, width)` run the same algorithm on unstyled text (table
  cells, thread panel rows, and the view clamp, which counts rows without
  building them); `row_ranges(text, width)` gives each row's byte range and
  `clusters(text)` each drawn cluster's, for the comment draft, whose text
  has no control characters but tabs: each range, its tabs expanded, is the
  `plain_rows` row.
  Breaks after the last whitespace that fits, hard-breaks a word wider than
  a row, and leaves whitespace at a break hanging on the end of its row, so
  the rows join back to the displayed line. A tab is one cluster four cells
  wide (ratatui would drop it), kept in place while segmenting and expanded
  to four spaces only in the returned rows, so the draft (which keeps its
  tabs) wraps exactly as its saved text does; other control characters are
  dropped. Widths are measured as ratatui draws
  (`styled_graphemes`, `CellWidth`), segmenting the whole line at once, so a
  grapheme cluster is never split, even across two spans. Every cluster
  keeps its style (a straddling one takes its first byte's), so a styled
  line wraps into exactly the rows of its plain text. `width(text)` is the
  same measure on one unbounded row. `cut(text, room)` and `tail(text,
  room)` are the only places one-row text is cut to fit: `cut` keeps the
  start, `tail` the end. `cut` returns the text as drawn (tabs as four
  spaces, control characters dropped): all of it if its `width` fits, else
  the longest prefix of whole clusters that leaves a cell for `…`, then `…`,
  so a wide cluster that would straddle the cut is dropped whole and the
  result is never wider than `room`. One cell is `…` alone; zero cells is
  empty. The result is borrowed exactly when it is the text unchanged, so a
  caller holding an owned copy can keep it. `tail(text, room)` is the same
  from the other end — all of it if it fits, else `…` and the longest
  suffix of whole clusters, owned either way — for the search prompt, which
  keeps the query's end visible.
- `tui::table`: `align(lines, width) -> Vec<Option<Vec<String>>>`, pure, the
  only place tables are parsed: one entry per file line, that line's screen
  rows if it belongs to a well-formed table that lays out in `width`, else
  `None`. A GFM subset: a header and a `|---|:-:|`
  delimiter with equal cell counts, a body up to the first blank, pipeless
  or fence line; `\|` is a pipe in a cell; lines in ```` ``` ```` / `~~~`
  fences or indented 4+ columns are never tables; a ragged body row leaves
  the whole block raw. A table that fits keeps its natural column widths;
  one too wide shares `width` max-min fairly (narrow columns keep their
  width, the rest split what is left), each column at least
  `min(natural, MIN_COLUMN)` cells or the whole table is `None`. Cells are
  wrapped by `wrap::rows` inside their column, padded by `wrap::width`
  (left, right or center, each piece on its own) and joined by ` │ `; a line
  takes as many rows as its tallest cell, shorter cells top-aligned. The
  delimiter is one row of `─` runs joined by `┼`. Every row of a table is
  equally wide, so the `App` shows a table laid out or raw as a whole.
- `tui::keys`: the help listing. `HELP: &[Section]` is a `const` table of every
  key bound in Normal mode and the thread panel (`Entry { keys, what }`,
  with aliases in one label such as `j/k, ↓/↑`), and `rows(width)` lays it out
  (keys beside the description, or stacked in a narrow window; nothing is
  cut). `Mode::Overlay { kind: Help, back, scroll }` shows it full-screen,
  opened by `?` from Normal or the panel (not from Visual, Search or the
  box, where `?` is text or unbound); `back` is `Normal` or `Panel` and
  closing returns there untouched; `?`/`esc`/`q`/`ctrl+c` close, `j`/`k`,
  `ctrl+d`/`ctrl+u`, `PgDn`/`PgUp`, `g`/`G`, `Home`/`End` scroll, clamped in
  `update`. `i` in Normal opens the other overlay, `kind: Review`: the state
  as a header, what the latest log entry was, by whom (and which side), when
  and on which revision, then its whole note, wrapped; it scrolls and
  closes the same way, with `i` in place of `?`. The
  handlers are not table-driven; a probe test in `tui::app` presses every
  candidate key in each section's fixture and fails when a key does
  something unlisted or a listed key does nothing.
- `tui::search`: `find(line, query) -> Vec<Range<usize>>` and
  `emphasize(Line, ranges) -> Line`, pure. Matching is smart-case and folds
  one `char` at a time, so ranges are valid byte ranges of the line (`ß`/`SS`
  does not match). `Mode::Search` is the `/` prompt, opened from Normal
  (`anchor: None`) or Visual (`anchor: Some`, so the live jumps extend the
  selection and `enter`/`esc` return to it). `origin` is the `(cursor, top)`
  at `/`; every edit to the query re-searches from `origin` (never from the
  live cursor) for the first matching line strictly after it, wrapping, and
  moves the cursor there like `]`; `esc` restores `origin`, `enter` keeps the
  position and sets `App.search` (even if nothing matched). `n`/`N` live in
  the shared `movement` helper, so Visual gets them; they search the raw file
  lines (never gutter or comments) and a table line on its laid-out rows,
  so every jump lands on something highlighted; one stop per line. The file view applies `emphasize` (reverse video, patched over the
  existing style) to the styled line before wrapping, so wrapped rows carry
  it; a match split across two wrapped table rows is neither found nor
  marked. The prompt replaces the footer row: `/query` (its tail, after `…`,
  when too wide) plus ` [no match]` or ` [wrapped]` only when that fits whole. Backspace on an empty query
  leaves the prompt. `n`/`N` ignore ctrl/alt chords; the prompt treats
  ctrl+alt (AltGr) as text.
- `tui::render`: `render(&App, &mut Frame)`, pure over state. One file line
  is one or more screen rows (word-wrapped by `tui::wrap`, tabs shown as four
  spaces). A well-formed markdown table shows aligned, in place; one too
  wide for the view wraps its cells inside their columns. Each table line is
  still one number, glyph and highlight, over all of its rows. Gutter: `●` open, `◆` waiting on you (yellow), `○` resolved (dim),
  then the line number, then `│`; the glyph and number show on a line's first
  row only, continuation rows keep just the `│`. The cursor highlight covers
  every row of every selected line (`App::selection()`: the cursor line, or
  the whole `Visual` range, also while its comment box is open), wrapped and
  table rows included; the moving end gets no separate mark. A highlighted
  line's text is drawn with its styles patched over the row's, so the cursor
  `bg` shows under markdown styling; styling sets no backgrounds and never
  touches the gutter, and `DIM` is dropped on highlighted rows, where faint
  text on the cursor background is hard to read.
  Banner: the top row in every mode but an overlay, so the file view
  starts one row down and its height is the rows left after the banner and
  the chrome below. It reads `your turn`, `agent's turn` or
  `closed: <verdict>`, then ` · <author>: <note>` when the latest entry has
  a note (its first line, with `…` when there are more lines), bold and
  `wrap::cut` to leave room for the key hint flush right (`H:hand off
  C:close` on your turn, `C:close` on the agent's, `O:reopen` when closed);
  a row too narrow for both drops the hint.
  Footer: path (or the last save message, on one row as on the comment
  box's border),
  `L<line>/<total>`, `visual <a>-<b>` in `Visual` (1-based, ascending),
  `N outdated` (open threads only), key hints, the whole row `wrap::cut` to
  the terminal width, so a footer too wide ends in `…`. The input box is titled
  `Comment on line N as <author>`, `Comment on lines A-B as <author>` for a
  range, `Reply to <id> as <author>`, `Hand off to the agent (note
  optional)`, or `Close as <verdict or ?> (tab: approved / dropped)`; its
  bottom border's hint names what saving does (`ctrl+s:hand off`,
  `tab:verdict ctrl+s:close`), and a close waiting to confirm shows
  `N open threads stay open: ctrl+s again to approve` there (after a
  pending error or discard prompt). In `Panel`
  a bordered box of half the terminal (at least 4 rows), titled
  `Threads on line N`, sits above the footer and the view shrinks to stay
  above it. A title too wide for its box is `wrap::cut` to end in `…`
  before the corner. In `Input` the bordered box
  replaces the footer (and hides the panel it came from), as tall as
  `App` says; it draws the draft's `rows(draft_width())` from its `scroll`
  (tabs expanded to four spaces, as the cursor counts them),
  so text wraps inside the border exactly as `update` counts it, the cursor
  is a reversed cell at `cursor_at`, and the bottom border reads
  `ctrl+s:save enter:newline esc:cancel` (`ctrl+s:save esc:cancel` when
  that does not fit), or, while `discard_asked`, `esc:discard the draft  any
  other key:keep it` (`esc:discard other:keep`, then `esc:discard?`, when
  that does not fit; widths in cells), or, after a failed save, the error
  message (`status`, red and bold, each run of whitespace and control
  characters drawn as one space; one that is left with no text keeps the
  hint) until the next key clears it. `status` wins over the discard
  prompt, which wins over the hint. Border text (error or hint) that does
  not fit between the corners is `wrap::cut`, so it ends in `…` and a wide
  character that would cross the corner is left out whole.
- `tui` (module root): the driver. `load` takes the lines, all threads and
  the review's state from one `Reviews::view`, so they come from the same
  read of the file; `Loaded::into_app` highlights the file only when it is
  markdown (`highlight::is_markdown`) and `color` is wanted (`cli` passes
  `color_wanted(NO_COLOR)`, read once at startup); `step(app, event, reviews, file, author)` is the only
  place effects are performed (`Add` → `Reviews::add_thread`, `Reply` →
  `Reviews::reply`, `Resolve`/`Reopen` → `Reviews::resolve`/`reopen` with no
  note, then `Reviews::view(file, true)` → `Event::Saved`; `HandOff` →
  `Reviews::hand_off` as `Side::Human`, `Close` → `Reviews::close`,
  `ReopenReview` → `Reviews::reopen_review` as `Side::Human`, then the same
  re-read → `Event::ReviewSaved`, whose message for a hand-off lists the
  `waiting` threads); `run` is the blocking loop (read event →
  `step` → draw) on a `TerminalSession`. Setup turns on raw mode, the
  alternate screen and bracketed paste (best-effort: a terminal that refuses
  it still runs, its pastes arriving as keys), so a paste arrives as one
  `Event::Paste` rather than as keys (a `q` or `ctrl+s` in it is text);
  the escape sequences are written by `enter_screen`/`leave_screen` over any
  `Write`.

**Invariants**
- All keys are handled in `update`; `render` holds no logic. The state
  machine is tested without a terminal.
- The TUI owns no persistent state. Every write goes through `review`; after
  a write it re-reads `view` rather than patching local state. If that re-read
  fails, the threads are dropped from the screen (a retry would duplicate a
  comment) but the review state is kept: the one the screen had for a
  thread write, the write's own for a hand-off, close or reopen.
- After every event `top <= cursor` and lines `top..=cursor` fit in `height`
  screen rows (wrapped rows counted), where height is the rows left after the
  banner and the footer, panel or input box, so the whole cursor line is on
  screen. The one
  exception: a cursor line taller than the view sets `top == cursor` and its
  tail is cut off (`top` is a file line; there is no sub-line scroll). `top`
  never leaves blank rows below the last line while earlier lines could fill
  them. Each clamp walks back at most `height` lines. The panel never moves
  the cursor, so the listed threads are always those of the line shown
  selected.
- In `Input`, after every event (resizes included) the draft's `scroll`
  shows its cursor row among the box's rows and leaves no blank rows below
  the text, so the box follows the cursor as it grows, scrolls and rewraps.
- A failed write keeps the draft and mode and shows the error (in `Input`,
  on the comment box's bottom border in place of the key hint; elsewhere in
  the footer); nothing is lost silently.
- The terminal is restored on every exit path: raw mode, the alternate
  screen and bracketed paste are all undone (paste off before leaving the
  alternate screen, the reverse of setup, each step tried even if one
  fails). `TerminalSession` restores on drop (normal return, `?`,
  unwinding), and a chained panic hook restores before the panic message
  prints. `SIGTERM`/`SIGHUP` skip both and are not
  handled.
- Display only; never mutates the reviewed file (it is read once on open).

*Deferred within `tui`:* character or column selection; sub-line scrolling of a
line taller than the view; a hanging indent for wrapped list items; real tab
stops; inline markup in laid-out table cells (a laid-out table gets only its
header line's bold); `NO_COLOR` for the TUI's chrome as well;
GFM padding of ragged table rows; pipes inside inline code in a cell; tables
inside lists or blockquotes; polling the file and `review.json` mtimes and re-syncing; draft
survival across a re-sync; mapping the cursor through `anchor`;
editing threads; notes on resolve/reopen from the TUI. Until live refresh exists,
the screen shows the file as read at open, while `add_thread` quotes the file
as it is at save time.

### `highlight`
Lives at `tui::highlight`. Depends on nothing else in the crate (only
`pulldown-cmark` and ratatui's `Style`/`Line`).

**Role:** make a markdown file's structure visible in the file view.
`highlight(lines) -> Vec<Styling>` parses the lines, joined by `\n`, as one
CommonMark document (strikethrough and tables on, nothing else) and returns
one `Styling { line: Style, spans: Vec<(Range<usize>, Style)> }` per line: a
whole-line style plus inline spans in byte offsets of the raw line, outermost
first, cut at line boundaries. `Styling::apply(line) -> Line` builds the
styled line. `is_markdown(path)` is true for a `md`, `markdown` or `mdx`
extension in any case; only those files are highlighted.

Styles: heading lines bold magenta; strong bold, emphasis italic,
strikethrough crossed out; inline code cyan, backticks included; fenced and
indented code blocks with dim fence lines and a cyan body, no markup parsed
inside; list markers bold green, the marker only; block quotes with dim `>`
markers and italic text; links with underlined text and a dim `(target)`;
a table's header line bold. The style constants sit together at the top of
the module.

**Invariants**
- Highlighting never changes text: delimiters stay visible, `apply` yields
  exactly the raw line, and spans are non-empty and on `char` boundaries.
- Only modifiers and the 16 named ANSI colors, which the terminal theme sets
  for light or dark; no RGB, no 256-color indexes, no backgrounds (the
  cursor owns `bg`).
- Off when `NO_COLOR` is set and non-empty, decided once in `cli`; the TUI's
  own chrome (cursor, gutter glyphs) keeps its colors.
- Styling is computed once per session (the file is read once); a resize
  only re-wraps.

*Deferred:* highlighting code inside fenced blocks by language (`syntect` or
tree-sitter, language from the info string), highlighting non-markdown files
by extension, front-matter detection (a leading `---` block may show as a
setext heading), a toggle key, and themes (#49).

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

- `model`: round-trip and a golden-JSON test pinning the wire shape (the
  log's entries included); a table test of `Review::state()` over logs.
- `store`: data-dir and review-dir naming, stale-temp recovery, schema
  refusal, lock contention; snapshots hold exact bytes, are not written on
  `Err` or `Locked`, are never rewritten, and leave the reviewed file's bytes
  and mtime alone.
- `review`: one test per rule, with an injected clock; editing the file between
  calls to exercise revisions and mapping (insert above, edit, delete, revert,
  threads on different revisions, legacy anchors, a missing snapshot); turns
  by rule too: hand-off by either side, refusal out of turn or when closed
  (with `review.json` byte-identical), `waiting`, close during either turn,
  reopen by either side, error precedence, out-of-turn thread writes, and
  each log entry's revision and snapshot.
- `tests/core.rs`: concurrent writers, cross-handle reads, a full review
  round, and a whole turn round (hand-off, hand-back, close, reopen) through
  separate handles, its log and state read back from disk.
- `anchor`: one example per diff shape plus properties checked exhaustively
  over every pair of texts of 0–4 lines over `{a, b, c}` and every range
  (identity, in-bounds, `Current` ⇒ identical text).
- `cli`: in-process tests (golden JSON, TTY rendering, error table, help) in
  `src/cli.rs`; process tests in `tests/cli.rs` for streams, exit status, and
  env, and turns passing between the library (as the TUI writes them) and
  separate `ptal` processes. Schema conformance is deferred with `ptal schema`.
- `tui`: scripted key events through `update` assert on state (including the search prompt, `n`/`N` and Visual search, and the help probe that ties `tui::keys::HELP` to the key handlers); `TestBackend`
  snapshots (inline expected strings) cover Normal, Visual, Panel and Input
  rendering, gutter glyphs and styles (including the selection highlight on
  wrapped and table rows, before and after a resize, and the comment box
  wrapping, growing and scrolling its draft); `tui::draft` has examples of
  its rows and cursor cell by width, of cluster and row moves and of paste
  normalization (tabs as 4-cell clusters), plus a
  property over a fixed multi-line draft (one line tab-indented) that every cluster boundary is
  reachable by `↓` then `→` and editable there; the driver's
  `enter_screen`/`leave_screen` are checked for the bracketed-paste escape
  sequences (`ESC[?2004h`/`ESC[?2004l`, off before leaving the alternate
  screen) written to a `Vec<u8>`; `tui::wrap` has examples plus a property test
  over a fixed corpus (ASCII, CJK, emoji, tabs, long words) and widths
  1..=12: rows join back to the line and each fits the width; `tui::panel`
  has examples plus a property over a fixed corpus (ASCII, CJK, emoji ZWJ,
  combining marks, tabs, newlines) as quotes, authors and bodies, and widths
  5..=20: every row, trailing whitespace trimmed, fits the panel by cells
  unless it holds a single cluster wider than the room after its indent;
  `tui::table` has examples (GFM subset, fences, indentation, ragged blocks,
  alignment, wide characters, wrapped cells, column-width allocation) plus
  properties: allocated widths fit, respect the minimums and use all the
  space when short; over a fixed corpus and view widths 1..=60, one entry per
  line, every row of a table equally wide and within the view with `│` in
  the same columns, and no cell text lost;
  `tui::highlight` has examples per element (nesting, emphasis across a line
  break, unclosed fences, quoted lists) plus a property over a fixed corpus:
  one styling per line, spans in bounds and on `char` boundaries, and the
  styled line's text is the raw line; `tui::app` checks that screen rows,
  the clamp and key effects are identical with highlighting on and off;
  `tests/tui.rs` runs scripted rounds through `step` against a real
  `Reviews` (single-line and Visual range comments checked through the real
  `ptal comments`; a thread moving with lines inserted above it, then
  outdated once its line is edited; a hand-off, a hand-back seen after the
  next write, closing with and without a verdict, the approve confirmation,
  reopening, refusals keeping the draft, and the state after a reload); the
  banner and the review overlay are covered in `tui::render` and `tui::app`;
  the terminal guard and panic hook are tested with injected
  `restore` functions (`tests/panic_hook.rs` is its own process because the
  panic hook is global). A real-terminal check (pty) is manual.

## Distribution

The crate is publishable; the procedure is [docs/releasing.md](docs/releasing.md).
The `include` allowlist in `Cargo.toml` is the source of truth for what ships
(`src/`, `tests/`, `Cargo.toml`, `Cargo.lock`, `README.md`, `LICENSE`); `docs/`,
`ARCHITECTURE.md`, `CHANGELOG.md`, `scripts/` and agent/editor directories are
deliberately excluded; README links to them are relative, which GitHub and
crates.io (via `repository`) resolve against the repo. The allowlist is
reviewed against the previous tag's package list before every publish.
`rust-version` is the floor declared by the dependency tree, checked by hand and
not enforced in CI. CI runs `cargo package --locked`, fails if a forbidden path
enters the tarball, and builds the docs from the unpacked tarball.

A release is: bump `Cargo.toml` and `CHANGELOG.md`, `cargo publish` by hand
(crates.io first, as it is irreversible), then push tag `vX.Y.Z`.
`.github/workflows/release.yml` verifies the tag against `Cargo.toml` and
`CHANGELOG.md`, builds `.tar.gz` archives for `aarch64`/`x86_64` Apple Darwin
and `aarch64`/`x86_64` Linux (glibc), writes `SHA256SUMS`, and creates the
GitHub Release with the CHANGELOG section as notes. Windows binaries, musl
builds, signing, and installers are deferred.

## Deferred

Moving or renaming a reviewed file; MCP server (a thin wrapper over `review`,
should the CLI prove insufficient); inline and side-by-side diff views;
severity or categories; multi-file reviews.
