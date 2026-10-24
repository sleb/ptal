# `review`

**Role:** the application core and the **only write path**. Each operation is a transaction through `store`, and each validates its inputs before committing. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the layering.

## Owns

`Reviews` holds a `Store` and an injected `Clock`. Both front ends call it; it prints nothing and knows no terminal.

## Operations

### Reading
- `view(file, include_resolved) -> View { revision, sha256, lines, threads, review, changes }`. One read of the file serves all of its fields.
- `threads` are in id order, open only by default, each with its anchor's lines mapped to the file as it is now and a computed `state`.
- `revision` is the current content's id, or `None` if nothing has been written against it yet.
- `review` is the `ReviewStatus`.
- `changes` are the `anchor::Change` hunks since the human's last look (`Review::last_look`).
- `sha256` is the content hash of that read.
- `stamp(file) -> Stamp` is the SHA-256 of the file's bytes and of `review.json`'s bytes (`ReviewReader::bytes`), so a poller can tell that either changed without a full `view`.
- `stamp` uses hashes, not mtimes: mtime granularity can miss a same-second, same-length write. It never writes.

### Thread writes
- `add_thread(file, start, end, body, author)` anchors a new open thread to lines `start..=end` of the file as it is now, storing the quote.
- `resolve(file, id, author, note?)` marks the thread resolved. A note is appended as a second comment by the resolver.
- `reply(file, id, body, author)` appends a comment. A reply to a resolved thread sets it `open` in the same write; no extra comment is synthesised.
- `reopen(file, id, author, note?)` makes a resolved thread `open`. A note is appended as a comment by the reopener.
- `resolve` on a resolved thread is `AlreadyResolved` and `reopen` on an open thread is `AlreadyOpen`. Both write nothing, so a note is dropped.
- `resolve`, `reply` and `reopen` return the thread with its lines mapped to the content they read; the stored anchor is unchanged. `add_thread`'s range is as given, since it anchors to the current content.
- `ThreadWrite` is `Added | Replied | Resolved | Reopened`: the four writes a front end can make, and the one kind both the CLI and the TUI report. `as_str()` gives `added`, `replied`, … It is never persisted.

### Turns and closing
- `hand_off(file, side, author, note?) -> HandedOff { review, waiting }` appends a `hand_off` entry by `side` to the log and passes the turn to the other side.
- `waiting` lists the threads that `Thread::is_waiting_on(author)`, the rule the TUI's gutter uses too.
- A hand-off by the side without the turn, or on a closed review, is `NotYourTurn`.
- `close(file, author, verdict?, note?) -> ReviewStatus`: the human ends the review with a `close` entry, during either side's turn. Open threads stay open.
- `verdict` is an `Option` so a front end can pass on an unchosen one; `None` is `MissingVerdict`.
- Closing a closed review is `AlreadyClosed`, so a verdict never changes under an agent acting on it.
- `reopen_review(file, side, author, note?) -> ReviewStatus`: either side brings a closed review back, on the human's turn. On an active review it is `NotClosed`.

### Writing against what was shown
- `against(sha256) -> Against` covers the writes that depend on what a front end showed: `add_thread`, `hand_off` and `close`.
- Each is refused with `FileChanged`, writing nothing, when the file now hashes differently from `sha256` (a `View.sha256`).
- The TUI makes these three through it, so a hand-off or close records the content shown as the last look and a new thread lands on the lines shown.
- `reply`, `resolve` and `reopen` are by id and stay unguarded.
- The plain methods take no hash: the CLI's agent reads the file itself.

### Waiting
- `wait(file, timeout?, &impl Ticker) -> Waited { reason, review }` blocks until it is not the human's turn.
- `Woke::Turn` is the agent's turn, `Woke::Closed` a closed review, and `Woke::Timeout` is once `timeout` passes on the ticker's clock.
- It is level-triggered on the state, checked before the first sleep. A wait on a closed review or on the agent's turn returns at once, and a hand-off between two waits can't be missed.
- It polls `review.json` every `POLL_INTERVAL` through a `store::ReviewReader` resolved once, after checking the file is readable. It never writes.
- An `Io` error reading `review.json` is retried on the next poll, up to `WAIT_READ_RETRIES` in a row. Any other error (a corrupt or newer review) ends the wait at once.
- A wait that times out without ever reading `review.json` fails with the last read's `Io` error rather than reporting a made-up state.
- `Ticker { now, sleep }` is injected like the clock, as a generic (`SystemTicker` for real), so tests hand off from inside a fake `sleep`.
- It polls rather than watching files: `review.json` and editors' saves replace files by rename, which breaks per-file watches, and the files are a few KB.

## Status and view shapes
- `ReviewStatus { state, latest }` is the review's state as front ends see it: the `ReviewState` and the last log entry, if any.
- It serializes as `{"turn", "verdict", "latest"}`. `turn` is `null` exactly when the review is closed, and `verdict` is set exactly then.
- `View.lines` is the file's lines from the same read the threads were mapped against. A front end that displays them shows exactly what `add_thread` quotes and where each thread is.

## Invariants

### Reading the file
- Lines are read with `str::lines()` (`\n` or `\r\n`, final newline ignored); the file must be valid UTF-8.
- One rule supplies the quote at `add_thread`, the mapping in `view`, and `View.lines` for the TUI's display.

### Mapping
- Anchors are never rewritten on disk. `view` maps each one from its own revision's snapshot to the current file with an `anchor::Diff`, so an edit that is later reverted maps back to `current`.
- Each revision is loaded and diffed against the current file at most once per call.
- `state` is `current` iff the mapping is `Current` **and** the lines at the mapped range read as `quote`. The second check keeps an adopted legacy anchor whose quote no longer matched from turning `current`. Otherwise it is `outdated`, at the best-guess range, with the original `quote`.
- An anchor with no revision (from before revisions, not yet adopted) is checked in place against the current file.
- An anchor whose snapshot is missing or unreadable (deleted or damaged by hand) is `outdated` at its stored range clamped to the file. It is still listed, and mapping never fails a read or a write.
- A thread anchored before revisions existed (`anchor.revision` absent) is anchored to the revision of the first write after the upgrade, "the file as first seen". Its old comments keep no revision.
- `View.changes` diff the last look's snapshot against the same read as `lines`, reusing the mapper's diff for that revision.
- They are derived from the log, never stored: empty with no last look, or when its snapshot is missing or unreadable (no error).
- Viewing is not a look, since reads never write. Changes are for the TUI only; `comments` JSON does not carry them.

### Writes
- A mutation that fails (unknown id, bad range, empty author or body) changes nothing, snapshots included.
- Every write reads the file once, outside the lock, and records the revision it was made against, inside the lock: it keeps the snapshot and assigns the next ordinal if the content is new. Reads hash the file but never write.
- `add_thread` sets `anchor.revision` and the comment's `revision`; `reply` sets the comment's; `resolve` sets `resolved_revision` and the note's.
- `reopen` sets only its note's. A reopen itself is not recorded, so one without a note keeps no snapshot and assigns no ordinal.
- `hand_off`, `close` and `reopen_review` set their log entry's revision.
- A log entry records the revision of the file as it is now, like any other write, and the review's `next_id`. A blank note is no note.
- Every message has a non-empty `author` and `body`.
- An id is allocated only after the thread is found, so a failed write never burns one.

### Turns
- The turn is a signal, not a lock: no thread write reads it or moves it.
- Comments, replies, resolves and reopens succeed during either side's turn, or on a closed review, and leave the log alone. They never reopen a closed review.

## Error precedence
- `reply` checks author, body, file, then thread id.
- `resolve` and `reopen` check author, file, thread id, then status.
- `hand_off` checks author, file, then the turn under the lock.
- `close` checks author, verdict, file, then whether it is closed.
- `reopen_review` checks author, file, then whether it is closed.
- A file that is missing or not UTF-8 fails every write with `FileUnreadable`.
- Through `against`, `FileChanged` is checked right after the file is read, before the range, the turn or whether the review is closed.

## Contract
- Returns typed results and typed errors: `NotFound`, `AlreadyOpen`, `AlreadyResolved`, `NotYourTurn`, `MissingVerdict`, `NotClosed`, `AlreadyClosed`, `FileChanged`, `InvalidRange`, `FileUnreadable`, `Locked`, `UnsupportedSchema`, `EmptyAuthor`, `EmptyBody`, `Io`, `Corrupt`.
- No printing, no terminal awareness. This is the API both front ends share.
- Mapping errors to exit codes belongs to [`cli`](cli.md).

## Tests
- One test per rule, with an injected clock.
- The file is edited between calls to exercise revisions and mapping: insert above, edit, delete, revert, threads on different revisions, legacy anchors, a missing snapshot.
- Turns by rule: hand-off by either side, refusal out of turn or when closed (with `review.json` byte-identical), `waiting`, close during either turn, reopen by either side, error precedence, out-of-turn thread writes, and each log entry's revision and snapshot.
- `wait` with a fake ticker: returns at once when closed or on the agent's turn; a hand-off or close from another handle inside `sleep`; timeouts on the ticker's clock; a timeout before any read failing with the read's error; the file removed mid-wait; never writes.
- `against` refuses stale content with `review.json` byte-identical, behaves as the plain methods on the current content, and has its error precedence and a guarded hand-off's last look tested.
- `stamp` changes with the file or `review.json` and never writes.
- `tests/core.rs`: concurrent writers, cross-handle reads, a full review round, and a whole turn round (hand-off, hand-back, close, reopen) through separate handles, its log and state read back from disk.
