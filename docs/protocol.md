# The review protocol

How a ptal review runs between you and an agent: who acts when, what each
side can do, and how a review ends. The command and key for each operation
are in [interfaces.md](interfaces.md). The scenarios and reasoning these
definitions were worked out from are in `docs/review-interactions.md` at
commit `0365930` (removed since).

> **Status.** Threads, comments, replies, resolving and reopening are built,
> and so are revisions and threads that follow edits (#45), and turns,
> hand-offs and closing a review (#35), and changes since your last look
> (#46, TUI only), and waiting for a turn and live refresh (#36). Each section below says
> which it is. Planned behavior has no command names yet; those are settled
> in its story.

## The shape of a review

A review is one file, reviewed by you (the **human**, in the TUI) and
revised by an **agent** (through the CLI). It goes in rounds: you comment,
hand off; the agent revises and answers, hands back; until you close it.

```
 start ──► human's turn ──hand off──► agent's turn
               ▲                           │
               └──────── hand back ────────┘

 either turn ──close (approved | dropped)──► closed
 closed ──reopen──► human's turn
```

## Threads and comments

*Built.*

- A **thread** is a conversation anchored to a line or a range of lines.
  Either side starts one; its first comment says what it is about.
- A **comment** is one message in a thread, with an author and a time.
  What a comment is for (a question, a change request, a nit) is said in
  its text; there are no tags.
- A thread is **open** or **resolved**. Either side may resolve it, with an
  optional note saying why: fixed, won't fix, deferred, no longer relevant.
  Whether the agent resolves its own fixes or leaves them for you to
  confirm is up to how you prompt it.
- Reopening a resolved thread, or replying to it, makes it open again.
- A thread is **waiting on you** when it is open and its last comment isn't
  yours (`◆` in the gutter).
- **Acknowledgement is implicit.** A resolve stands unless someone reopens
  it. If you disagree with a resolution, reopen it with a comment saying
  why; the agent does the same.

## Revisions

*Built (#45, #35).*

- A **revision** is one version of the file's content. ptal never edits
  the file; it notices a new revision when someone writes something (a
  comment, a resolve, a reopen, a hand-off, a close) against content it hasn't
  kept yet, and keeps a copy. Reading never keeps one, so edits made between
  two writes are one revision, and content nobody has written against yet
  has no number. Revisions are numbered `r1`, `r2`, … in the order they
  are kept. Going back to earlier content returns to that revision.
- A thread stays anchored to the lines it was created on, in the revision
  it was created on. When the file changes, ptal maps those lines to the
  current file:
  - every anchored line unchanged: the thread moves with them and is
    **current**;
  - any anchored line edited or removed: the thread is **outdated**, shown
    near where its text was, with the original text quoted.
- Every comment and every resolve records the revision it was written
  against (a thread reopen through its note, if it has one), and so does
  every hand-off, close and review reopen, so you can see which version of
  the file a fix or a hand-off referred to.
- Revisions have no author. What the agent changed is what changed between
  your hand-off and its hand-back.
- A review belongs to the file's path. However much the file is rewritten,
  it is the same review.

## Turns

*Built (#35, #36).*

- One side at a time has the **turn**: the side expected to act next. A
  review starts on your turn.
- The turn is a signal, not a lock. Either side can comment, reply, resolve
  or edit at any time; doing so doesn't move the turn.
- The turn moves only by a **hand-off** from the side that holds it. A
  hand-off can carry a note ("addressed everything but c4"). It lists any
  threads still waiting on the side handing off, without refusing.
- Your hand-off means "your move": what you want is in the threads and the
  note. The agent's hand-back means its revision is ready for you to read.
- Every hand-off, close and reopen is kept, in order, with the review, and
  whose turn it is (or the verdict) is read from them, so it survives
  restarts of ptal on either side.
- An agent can **wait** for its turn instead of being told in chat
  (`ptal wait`). The wait returns when you hand off (with your hand-off and
  note), or when you close the review (with the verdict); a wait on a
  review already on the agent's turn, or closed, returns at once, so a
  hand-off between two waits is never missed. A wait can also give up after
  a timeout, still on your turn. Writes out of turn don't end it.
- You don't wait: the TUI shows the agent's writes as they land (live
  refresh), and the banner flips to `your turn` when it hands back. A
  hand-off or close is made against the file as the screen shows it; if
  the agent changed the file since, it is refused until you've seen the
  change, so your last look is always what you were shown.

## Closing and the verdict

*Built (#35).*

- Only you **close** a review, at any time, even during the agent's turn.
  A closed review has no turn: nothing more is expected in it.
- A close always carries a **verdict**, which tells the agent what to do
  next, outside the review:
  - **approved**: take the next step with the plan as it stands (publish
    it, commit it, implement it);
  - **dropped**: don't. Why (abandoned, covered elsewhere, not now) goes in
    the close note.
- Approving with threads still open accepts them as they stand. They stay
  open in the record, and the agent owes nothing on them. ptal asks you to
  confirm first.
- A closed review still accepts comments and edits; they don't reopen it.
- Either side can **reopen** a closed review, which starts a new round on
  your turn. The agent does this when you've asked it, outside the review,
  to revise the plan again.

## Rounds

*Planned, derived from the hand-offs; nothing extra is stored.*

- A **round** is one of your turns plus the agent turn that answers it.
  Round 1 starts with the review; each hand-back or reopen starts the next.
  A close ends the current round.
- Each round reviews one revision and produces the next. "What changed
  since my last look" is what changed since this round started: the diff
  from the revision on your latest hand-off or close to the file now.
  Opening the TUI is not a look (reads never write), and a reopen is not
  one either. *(Built: the TUI marks these changes, #46.)*
- Every comment belongs to the round and turn it was written in, so a
  late comment, written after its side handed off, shows as late.

## Glossary

| Term | Meaning |
|---|---|
| review | One file's comments, revisions and turn history |
| thread | A conversation anchored to lines of the file |
| comment | One message in a thread |
| open / resolved | A thread's status |
| waiting on you | An open thread whose last comment isn't yours |
| revision | One version of the file's content |
| current / outdated | Whether a thread's anchored lines are unchanged in the current revision |
| turn | The side expected to act next |
| hand-off | Passing the turn to the other side |
| close | Ending the review, with a verdict |
| verdict | `approved` (agent takes the next step) or `dropped` (it doesn't) |
| reopen | Bringing back a resolved thread, or a closed review |
| round | Your turn plus the agent's answer to it |
