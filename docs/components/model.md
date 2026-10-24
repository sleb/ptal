# `model`

**Role:** the data types that are persisted and exposed to agents. Depends on nothing else in the crate. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the layering.

## Owns

### Review and revisions
- `Review` — one per reviewed file: `schema_version`, `path`, `next_id`, `revisions`, `threads`, `log`.
- `Revision` — `{ id, sha256 }`: one kept version of the file, `r1`, `r2`, … in the order first written against.
- `Review::revision_for(sha)` returns the known id or appends the next one. There is no separate counter, because revisions are never pruned.

### Threads and comments
- `Thread` — `{ id, anchor, status, comments }`. Its id is its first comment's id.
- `Thread::is_waiting_on(author)` — the one "waiting on you" rule: the thread is open and its last comment isn't by `author` (exact match). A thread with no comments waits on no one. `review`'s hand-off and the TUI's markers both ask it.
- `Status` — `open` or `resolved`. `resolved_by`, `resolved_at` and `resolved_revision` appear on the wire only when resolved. Reopening, or replying to a resolved thread, makes the thread `open` and drops them.
- `Comment` — `{ id, author, body, created_at, revision }`. Timestamps are RFC 3339 UTC.
- `Anchor` — `{ revision, start_line, end_line, quote }` (1-based, inclusive). `quote` is the anchored lines joined by `\n`, and the lines refer to `revision`.
- Anchors are immutable; `review` maps them to the current file when viewing.
- `State` — `current | outdated`. Computed by `review` when viewing, never persisted.

### The review log
- `Entry` — one hand-off, close or reopen in `Review.log`: `{ event, side, author, created_at, revision, next_id, note }`.
- On the wire `event` is `"hand_off" | "close" | "reopen"`. A close also carries `verdict` (`Verdict`: `approved | dropped`), and `note` is skipped when `None`.
- `side` (`Side`: `human | agent`) is who wrote the entry.
- `next_id` is the review's counter at that moment, so every comment id below it came first.
- `log` defaults to empty, so a `review.json` from before turns still loads.
- `ReviewState` — `Turn(Side) | Closed(Verdict)`, from `Review::state()`. It is never persisted.
- An empty log is the human's turn; a hand-off by one side gives the turn to the other; a close sets the verdict; a reopen gives the turn to the human. Each entry sets the state outright, so it is the latest entry's.
- `Review::last_look()` — the latest log entry written by the human side that is a hand-off or a close. Its revision is what the human last saw.
- A look is matched by side, not author. A reopen is not a look, and neither is opening the TUI (reads never write). It is `None` before the first one.

### Older data
- Every `revision` field (`anchor.revision`, `comments[].revision`, `resolved_revision`) is skipped when `None`, which only data written before revisions existed has.
- `revisions` defaults to empty, so such a `review.json` still loads.

## Invariants
- Comment ids are short, human-typeable, and unique within a review (`c1`, `c2`, …). They are assigned from `next_id`, a monotonic counter that is never reused.
- Threads, replies and notes draw from the same counter.
- `start_line <= end_line`, both within the file when the thread is created; `quote` is exactly those lines.
- Status lives on the thread.
- The log is append-only: entries are only ever pushed, and the review's state is derived from it, never stored beside it.

## Contract
- serde (de)serialization is the on-disk and on-wire format.
- Every persisted file carries `schema_version`; unknown future versions are refused, not guessed at.

## Tests
- Round-trip and a golden-JSON test pin the wire shape, the log's entries included.
- A table test checks `Review::state()` over logs.
