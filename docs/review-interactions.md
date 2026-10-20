# Review interactions: a catalog of common variants

> **Research material, not a spec.** This page collects the back-and-forth
> patterns between a human and an agent in a ptal review, as concrete
> transcripts. It exists to be analyzed later, to extract a rough protocol
> and the core abstractions (review, revision, turn, thread, comment,
> verdict). It describes interactions people have, not what ptal does:
> each scenario says which steps ptal supports today and which happen out of
> band or need planned work (v0.3.0: #35 finish/hand-off, #36 wait and live
> refresh, #45 revisions, #46 changes since last look).
>
> The definitions worked out below are summarized for users in
> [protocol.md](protocol.md).

## Notation

Every scenario runs against the same sample file, `plan.md`:

```
 1  # Plan: rate-limit the public API
 2
 3  ## Goal
 4  Stop a single client from exhausting the API.
 5
 6  ## Approach
 7  1. Add a token-bucket limiter in the gateway middleware.
 8  2. Key buckets by API key; anonymous requests by IP.
 9  3. Store buckets in process memory.
10  4. Return 429 with a Retry-After header when empty.
11
12  ## Limits
13  - 100 requests/minute per key
14  - 20 requests/minute per IP
15
16  ## Rollout
17  1. Ship behind a flag, log-only for a week.
18  2. Enforce for anonymous traffic.
19  3. Enforce for all keys.
20
21  ## Open questions
22  - Do internal services bypass the limiter?
```

Transcript lines:

| Line | Meaning | Today |
|---|---|---|
| `H  add L9 c1 "..."` | Human starts thread `c1` on line 9 (or `L13-14`, a range) | TUI `enter` / `v` |
| `A  add L22 c1 "..."` | Agent starts a thread | `ptal add` |
| `X  reply c1 "..."` | Human (`H`) or agent (`A`) appends a comment to `c1` | TUI `r` / `ptal reply` |
| `X  resolve c1 [note "..."]` | Resolve, optionally with a note (a comment) | TUI `x` (no note) / `ptal resolve` |
| `X  reopen c1 [note "..."]` | Reopen a resolved thread | TUI `o` (no note) / `ptal reopen` |
| `A  edit L9: "..."` | Agent edits the file (shown as the new text) | agent's own tools |
| `H  edit ...` | Human edits the file outside ptal | `$EDITOR` |
| `A  read` | Agent lists threads (`ptal comments`) | `ptal comments` |
| `── H → A ──` | The turn passes to the agent | out of band today (chat: "done, take a look"); planned: finish (#35) + wait (#36) |
| `── A → H ──` | The turn passes back to the human | out of band today; planned: hand-off (#35), live refresh (#36) |
| `H  finish approved / changes_requested ["..."]` | Review-level verdict and note | planned (#35) |
| `H  close <verdict> ["..."]`, `X  reopen review` | End the review; bring it back | planned (#35's finish and reopen) |
| `[outdated]`, `[current]` | A thread's state after an edit | computed today, mapped after #45 |

Ids follow ptal's rule: one counter per review, so replies and notes use up
ids too (`c1` thread, `c2` a reply in it, `c3` the next thread).

---

## A. One thread, one round

### A1. Request, fix, resolve

The baseline. The human asks for a change; the agent makes it and closes the
thread, saying what changed.

```
H  add L9 c1 "In-memory buckets reset on every deploy and don't share across replicas."
── H → A ──
A  read
A  edit L9: "3. Store buckets in Redis, shared by all gateway replicas."
A  resolve c1 note "Moved buckets to Redis (step 3)."
── A → H ──
H  (reads the note, checks line 9, moves on)
```

- **Ends:** thread resolved by the agent; the human accepts silently.
- **Questions:** Is a resolve by the agent final, or a proposal the human
  confirms? Does the human's silent acceptance need to be recorded (an
  acknowledgement), or is "not reopened by the next finish" enough?

### A2. Fix rejected: human reopens

The agent's fix misses the point.

```
H  add L10 c1 "Clients need to know their remaining budget, not just when they're cut off."
── H → A ──
A  edit L10: "4. Return 429 with Retry-After and X-RateLimit-Remaining when empty."
A  resolve c1 note "Added X-RateLimit-Remaining."
── A → H ──
H  reopen c1 note "Remaining has to be on every response, not only the 429."
── H → A ──
A  edit L10: "4. Send X-RateLimit-Remaining on every response; 429 with Retry-After when empty."
A  resolve c1 note "Header now on every response."
── A → H ──
```

- **Ends:** resolved after two agent attempts.
- **Questions:** The thread now holds two resolutions; is "resolved" a state
  or an event in the thread's history? Should the human be able to see which
  revision each resolve referred to?

### A3. Fix verified by the human (agent leaves the thread open)

Some humans want to close threads themselves. The agent replies instead of
resolving.

```
H  add L14 c1 "20/min per IP will hit offices behind one NAT."
── H → A ──
A  edit L14: "- 60 requests/minute per IP"
A  reply c1 "Raised to 60/min. Leaving this open for you to confirm."
── A → H ──
H  resolve c1
```

- **Ends:** resolved by the human.
- **Questions:** Who owns resolution: the thread's opener, the person who
  made the change, either? Is this a per-review convention (the agent's
  skill/prompt decides, as the README says today) or something ptal should
  model? After the reply the thread is waiting on the human (`◆`); is
  "waiting on" the right primitive for "needs verification"?

### A4. Human asks a question, no change wanted

Not every comment is a change request.

```
H  add L7 c1 "Why token bucket rather than a sliding window?"
── H → A ──
A  reply c1 "Token bucket allows short bursts, which our batch clients rely on; a sliding window would reject them."
── A → H ──
H  resolve c1
```

Variant: the answer is worth keeping in the plan, so the agent edits too.

```
A  edit L7: "1. Add a token-bucket limiter in the gateway middleware (allows bursts for batch clients)."
A  resolve c1 note "Answered and added the reason to step 1."
```

- **Ends:** resolved with no file change (or a documentation-only change).
- **Questions:** Comment intent (question / change request / nit / praise)
  is implicit in prose. Does the protocol need it, or does "the answer is a
  reply vs. an edit" carry enough?

### A5. Agent asks for clarification before acting

The comment is ambiguous; the agent asks instead of guessing.

```
H  add L18-19 c1 "Order seems wrong."
── H → A ──
A  reply c1 "Do you mean enforce for all keys before anonymous traffic, or put the log-only week after enforcement starts?"
── A → H ──
H  reply c1 "The first. Paying customers are the ones we can contact if it misfires."
── H → A ──
A  edit L18-19: "2. Enforce for all keys.\n3. Enforce for anonymous traffic."
A  resolve c1 note "Swapped steps 2 and 3."
── A → H ──
```

- **Ends:** resolved after one extra round trip on the thread.
- **Questions:** The agent's turn ended with a question and no file change.
  Is that a full turn (hand-off with nothing revised), or should one thread
  be able to wait on the human while the agent keeps working on others?

### A6. Agent pushes back

The agent disagrees and gives a reason. Two endings.

```
H  add L17 c1 "Skip the log-only week, just enforce."
── H → A ──
A  reply c1 "I'd keep it: without a week of logs we can't tell whether 100/min breaks any existing integration. Keep, or drop it anyway?"
── A → H ──
```

Ending (a), the human is convinced:

```
H  resolve c1
```

Ending (b), the human insists:

```
H  reply c1 "Drop it; we already have a week of access logs to check against."
── H → A ──
A  edit L17: "1. Ship behind a flag; check limits against last week's access logs first."
A  resolve c1 note "Dropped the log-only week; added the access-log check."
```

- **Ends:** resolved with no change (a), or changed after a second exchange (b).
- **Questions:** Is "won't fix" a distinct resolution from "fixed"? Today
  both are `resolved` with a note; the reason is prose only.

### A7. Human withdraws a comment

The human answers their own question, usually by reading further.

```
H  add L8 c1 "What about requests with an invalid key?"
H  (reads on, finds nothing, but realises an invalid key is rejected before the limiter)
H  resolve c1
```

Variant: the human withdraws after the agent has already read it.

```
── H → A ──
A  reply c1 "Invalid keys are rejected by auth before the limiter runs. Want that stated in step 2?"
── A → H ──
H  resolve c1
```

- **Ends:** resolved by its opener, with no agent work (or wasted agent work).
- **Questions:** Should a thread the opener resolved before the hand-off be
  visible to the agent at all? (Draft comments hidden until submit were
  considered for v0.3.0 and cut; see "Batch submit" under [Turn](#turn).)

---

## B. Many threads, one round

### B1. Batch review, batch reply

The common shape: the human leaves several comments, then hands over once.

```
H  add L9  c1 "In-memory won't survive deploys."
H  add L13 c2 "Is 100/min per key enough for the dashboard's polling?"
H  add L22 c3 "Yes they bypass; say so and say how they authenticate."
── H → A ──
A  read
A  edit L9:  "3. Store buckets in Redis, shared by all gateway replicas."
A  resolve c1 note "Redis."
A  reply c2 "The dashboard polls 6 endpoints every 5s, so 72/min per key. 100 leaves headroom; raise it?"
A  edit L22: "- Internal services bypass the limiter; they authenticate with mTLS."
A  resolve c3 note "Stated the bypass and mTLS."
── A → H ──
```

- **Ends:** two resolved, one waiting on the human.
- **Questions:** The hand-off back is "all threads answered", not "all
  threads closed". What is the unit of a turn: the review, or each thread?

### B2. One edit addresses several threads

```
H  add L9  c1 "Buckets must be shared across replicas."
H  add L10 c2 "What happens when the store is down?"
── H → A ──
A  edit L9: "3. Store buckets in Redis; if Redis is unreachable, fail open and log."
A  resolve c1 note "Redis, shared (step 3)."
A  resolve c2 note "Fail open, see step 3."
```

- **Ends:** both resolved, the second pointing at a line its own anchor
  doesn't cover.
- **Questions:** A resolution can refer to text outside the thread's anchor.
  Should a resolve be able to cite where the fix landed (a line range in the
  new revision)?

### B3. Fixing one thread makes others outdated

The agent's edit shifts or rewrites lines anchored by other threads.

```
H  add L9  c1 "Explain the eviction policy for idle buckets."
H  add L14 c2 "Per-IP limit too low."
── H → A ──
A  edit L9: "3. Store buckets in Redis.\n   Idle buckets expire after 10 minutes (Redis TTL)."    (inserts line 10)
A  resolve c1 note "Added the TTL line."
A  read
     c2 [outdated] today: its quote now sits at L15, not L14
         [current, L15] after #45: mapped across the insert
A  edit L15: "- 60 requests/minute per IP"
A  resolve c2 note "60/min."
```

- **Ends:** both resolved; in between, c2 was spuriously outdated today.
- **Questions:** Which revision is a thread anchored to after the agent
  edits around it? Does it re-anchor to each new revision it maps
  `current` to?

### B4. Partial fix

The agent does what it can and says what is left.

```
H  add L16-19 c1 "Rollout needs a rollback plan and an owner for each step."
── H → A ──
A  edit L20: "4. Rollback: turn the flag off; buckets expire on their own."
A  reply c1 "Added the rollback step. Who owns each step? I don't know the team."
── A → H ──
H  reply c1 "Platform team owns 1 and 2, API team owns 3."
── H → A ──
A  edit L17-19: (adds owners)
A  resolve c1 note "Owners added; rollback is step 4."
```

- **Ends:** resolved over two rounds, with a change in each.
- **Questions:** Should "partly addressed" be visible without reading the
  prose (a thread state between open and resolved)?

---

## C. Review-level and file-level

### C1. Approach-level feedback (not on any line)

The human objects to the plan as a whole.

```
H  finish changes_requested "Rate limiting belongs in the CDN, not the gateway. Rewrite around Cloudflare rules."
     today: no review-level note; the human puts it on line 1, or in the agent chat
── H → A ──
A  edit L1-22: (rewrites the plan)
     every open thread → [outdated]; quotes no longer exist
A  read
A  resolve c1 note "Superseded by the CDN rewrite."   (and so on, one call per open thread;
                                                       or leaves them for the human to judge)
── A → H ──
```

- **Ends:** a new plan; old threads closed as superseded.
- **Questions:** Is a rewrite a new revision of the same review, or a new
  review? What happens to threads whose text is gone: resolved, kept
  outdated, or carried as "context" for the new plan?

### C2. Approved with nits

The human approves but leaves small threads that should not need another
round.

```
H  add L4 c1 "nit: 'a single client' → 'any single client'"
H  finish approved "Go ahead and implement; fix the nit on the way."
── H → A ──
A  edit L4: "Stop any single client from exhausting the API."
A  resolve c1 note "Fixed."
     (agent proceeds to implement; no hand-off back)
```

- **Ends:** approved; the last fix is never reviewed.
- **Questions:** Does approval with open threads mean "fix and don't ask", or
  is it blocked until they are resolved (#35 asks this)? Does the agent's
  later edit un-approve the review?

### C3. Agent seeds questions before the human reviews

The agent writes the plan and marks where it needs input.

```
A  (writes plan.md)
A  add L13-14 c1 "I picked these limits from the dashboard's polling rate; please confirm or give real numbers."
A  add L22    c2 "Do internal services bypass? I couldn't find a config for it."
── A → H ──    (agent asks the human to run `ptal plan.md`; planned: opens a pane, #37)
H  reply c1 "100 per key is fine; IP limit should be 60."
H  reply c2 "They bypass; they use mTLS."
H  add L7 c5 "Mention the Retry-After header here too."
── H → A ──
A  edit ...; resolve c1, c2, c5
```

- **Ends:** the agent's threads answered and resolved by the agent.
- **Questions:** The first turn is the agent's. Is "the review starts with
  the agent's questions" the same protocol with the turn order flipped?
  Agent-opened threads are waiting on the human until answered (`◆`).

### C4. The human edits the file directly

Faster to fix a typo than to describe it.

```
H  edit L13 in $EDITOR: "- 120 requests/minute per key"
H  add L13 c1 "I bumped this to 120; propagate to the dashboard section if there is one."
── H → A ──
A  read     (sees a file it did not write)
A  reply c1 "There's no dashboard section; nothing else references the limit."
A  resolve c1
```

- **Ends:** resolved; the file changed without the agent.
- **Questions:** A revision authored by the human. Do revisions carry an
  author, and does "changes since your last look" (#46) apply to the agent too?

### C5. Anchored text removed

The agent deletes the section a thread is on.

```
H  add L21-22 c1 "This question is answered elsewhere; drop or link it."
── H → A ──
A  edit: deletes lines 21-22
     c1 [outdated], best guess clamped to the new last line
A  resolve c1 note "Removed the Open questions section."
```

- **Ends:** resolved; the thread's quote now exists only in ptal.
- **Questions:** How does the TUI place a thread whose lines are gone, and
  is a thread with no surviving text still openable from the gutter?

---

### C6. The human ends the review

The human is done reviewing; nothing more is expected on this review.
Two outcomes:

```
(a) accepted: the agent takes the next step (publish the plan to an issue, implement)
H  close approved "Good to go."
(b) not going ahead, for any reason (abandoned, covered elsewhere, not now)
H  close dropped "Merged into api-quotas.md."
```

Variant: the human stops with threads still open, and must still pick one
(usually `approved`, leaving minor threads as they are).

Later, either side brings it back:

```
(the human asks the agent, in chat, to fold in a new requirement)
A  edit L13-14: (adds a burst allowance)
A  reopen review note "Added burst limits; please take another look."
── A → H ──
H  (reviews as in any round)
```

- **Ends:** no one is expected to act; reopening starts a new round.
- **Questions:** Which outcomes does an agent need to tell apart? May the
  agent end a review? Does a write to a
  closed review bring it back, or only an explicit reopen? What does an
  agent waiting on its turn (#36) see when the review closes?

## D. Across rounds

### D1. Resolved thread gets a new follow-up

The human returns to a resolved thread with a related point.

```
(round 1) H add L9 c1 "..."; A resolve c1 note "Redis."
(round 2)
H  reply c1 "Which Redis? The shared cache cluster is already near capacity."
     reply to a resolved thread reopens it
── H → A ──
A  edit L9: "3. Store buckets in a dedicated Redis instance."
A  resolve c1 note "Dedicated instance."
```

- **Ends:** one thread, two resolve cycles, topics drifting.
- **Questions:** When is a follow-up a reply and when a new thread? Should
  a thread's history be shown per round?

### D2. Multi-round convergence

The common full arc: comments, fixes, a second pass on the fixes, approval.

```
round 1  H: c1, c2, c3            ── H → A ──   A: fixes c1, c3; asks on c2   ── A → H ──
round 2  H: answers c2; reopens c3; adds c7 on new text   ── H → A ──
         A: fixes c2, c3, c7       ── A → H ──
round 3  H: (reviews changes since last look, #46) finish approved
```

- **Ends:** approved after three human turns.
- **Questions:** What does each round see: all threads, or what changed? A
  "round" is a pair of turns; is it a first-class thing (numbered, like
  critty's plan versions; critty is a plan-review tool cited in the v0.3.0
  issues), or only implied by revisions and hand-offs?

### D3. Comment on text the agent just wrote

Round 2 comments land on lines that didn't exist in round 1.

```
(round 1) A edit L10-11: adds "   Idle buckets expire after 10 minutes."
(round 2)
H  add L11 c4 "10 minutes is longer than the window; 2 is enough."
```

- **Ends:** ordinary thread, anchored to a later revision than the others.
- **Questions:** Threads in one review are anchored to different revisions;
  is "the revision a thread was created on" enough, or does each comment
  need the revision it was written against?

### D4. Stalled thread, deferred out of the review

Agent and human agree it's not for this plan.

```
H  add L22 c1 "We also need per-endpoint limits eventually."
── H → A ──
A  reply c1 "Out of scope for this plan? I can note it as a follow-up."
── A → H ──
H  reply c1 "Yes, note it."
── H → A ──
A  edit L23: "- Follow-up: per-endpoint limits."
A  resolve c1 note "Deferred; listed as a follow-up."
```

- **Ends:** resolved as deferred.
- **Questions:** Another resolution kind alongside fixed and won't-fix
  (see A6).

---

## E. Timing

### E1. Human comments while the agent is working

Today nothing stops the human from adding threads after handing over.

```
── H → A ──
A  read                          sees c1, c2
H  add L19 c3 "One more thing ..."   (TUI still open)
A  edit, resolve c1, c2
── A → H ──                      c3 never read
```

- **Ends:** c3 is waiting on the agent, but the agent thinks its turn is
  done.
- **Questions:** Is a hand-off a snapshot of the threads at that moment?
  Should the agent re-read before handing back (today's `--help` step 3
  does, for outdated state), or should adding a thread during the agent's
  turn be refused, queued, or reported by #36's wait?

### E2. Agent writes while the human is reviewing

The agent answers early (or another agent run overlaps the human's turn).

```
── A → H ──
H  (TUI open, reading c2)
A  reply c2 "Actually, one more detail: ..."
     today: the human sees it only after reopening ptal
     after #36: the TUI refreshes, keeping the cursor and any draft
```

- **Ends:** the human may answer a thread that has changed under them.
- **Questions:** Whose turn is it when both can write at any time? Is
  turn-taking advisory (a signal) or enforced?

---

## Moves by scenario

Which basic moves each scenario uses; a starting point for the analysis.

| Move | Used in |
|---|---|
| Human starts a thread | all but C3's first turn |
| Agent starts a thread | C3 |
| Agent asks a question in a thread | A5, A6, B1, B4, D4, A7 |
| Human answers in a thread | A5, A6b, B4, C3, D4 |
| Agent edits the file | A1–A3, A4b, A5, A6b, B1–B4, C1–C3, C5, D1–D4 |
| Human edits the file | C4 |
| Agent resolves with a note | A1, A2, A4b, A5, A6b, B1–B4, C1–C2, C5, D1, D2, D4 |
| Agent resolves without a note | C3, C4 |
| Human resolves | A3, A4, A6a, A7 |
| Human reopens | A2, D1 (by reply) |
| Thread goes outdated | B3, C1, C5 |
| Review-level verdict or note | C1, C2, C6, D2 |
| Review ends or comes back | C6 |
| Turn passes without a file change | A4, A5, A6, A7, D4 |
| Writes overlap a turn | E1, E2 |

## Recurring questions

Collected from the scenarios above, unanswered here on purpose:

- **Turn:** is it per review or per thread? Is it enforced or a signal? Is
  a hand-off a snapshot? (A5, B1, E1, E2) *Proposed: see [Turn](#turn).*
- **Resolution:** who may resolve, and are fixed / won't fix / deferred /
  superseded / withdrawn kinds of resolution or just prose? (A3, A6, A7, C1, D4)
  *Proposed: see [Resolution](#resolution).*
- **Acknowledgement:** does the human's acceptance of an agent's resolve
  need recording? (A1, C2)
  *Answered: see [Acknowledgement](#acknowledgement).*
- **Revision:** which revision is a thread, a comment, a resolve anchored
  to? Who authored a revision? Is a rewrite still the same review? (B2, B3,
  C1, C4, D3) *Proposed: see [Revision](#revision).*
- **Round:** first-class (numbered) or implied by hand-offs? (D2)
  *Proposed: see [Round](#round).*
- **Comment intent:** question, change request, nit; explicit or prose? (A4, C2)
  *Answered: see [Comment intent](#comment-intent).*
- **Verdict:** does approval with open threads block, and does a later edit
  undo it? Which outcomes are there? (C2, C6) *Answered in [Turn](#turn):
  closing carries the verdict, `approved` or `dropped` (rule 12); open
  threads don't block `approved` (rule 12); a later edit doesn't undo an
  approval, only an explicit reopen does (rules 13, 14).*

---

## Definitions

Proposed definitions, one concept at a time, each checked against the
scenarios above. Each gives the recommendation, how the relevant scenarios
play out under it, the alternatives considered and dropped, and what it
means for open stories. Nothing here is built.

### Turn

**Definition.** A *turn* says which side of a review, the human or the
agent, is expected to act next. There is one turn per review, held by
exactly one side at a time. It passes only through a *hand-off*: an
explicit, recorded act by the side holding it. The turn is a signal, not a
lock: either side may write at any time.

A review is either *active*, and someone holds the turn, or *closed*, and
no one does. Closing is how the human ends a review, and the close carries
the review's *verdict*: the one answer the agent needs to know what to do
next. Rules 1–9 describe an active review; rules
10–14 cover closing and reopening.

**Rules.**

1. **Per review.** The turn belongs to the review, not to a thread.
   Per-thread "whose move" already exists as *waiting on* (an open thread
   whose last comment is not yours) and stays a separate, derived notion.
2. **Two sides, by role.** The sides are `human` (the TUI) and `agent` (the
   CLI), not author names. Names stay on the hand-off record as its
   author.
3. **Starts with the human.** A review with no hand-offs is the human's
   turn: a review exists so a human can review the file.
4. **Only the holder hands off.** A hand-off by the side that doesn't hold
   the turn fails (`not_your_turn`, exit `2`) and writes nothing, like
   `already_resolved`.
5. **Writes are never refused for being out of turn.** Comments, replies,
   resolves, reopens and file edits are valid from either side at any time.
   They don't move the turn.
6. **A hand-off is a marker, not a copy.** It records who handed off, when,
   an optional note, the comment-id high-water mark (`next_id` at that
   moment) and, once revisions exist (#45), the file's revision. It does
   not copy or freeze threads. "Everything up to `cN` and revision `R`" is
   what was handed over; later writes are visibly later.
7. **A hand-off reports what it leaves behind.** It never refuses because
   threads are still waiting on the side handing off, but its output lists
   them, so the hand-off is a decision rather than an accident.
8. **One event for both directions.** The human's hand-off (#35's "finish")
   and the agent's (#35's "hand-off") are the same event, with the same
   shape; front ends may still name the two commands differently. A
   hand-off carries no verdict: a human hand-off always means "your move",
   and what is wanted is in the threads and the note.
9. **Hand-offs are kept as a log.** Append-only. The current turn is the
   recipient of the last entry, or the human if there is none. The log is
   what Round will build on.
10. **Closing removes the turn.** A *close* is a log entry like a hand-off
    but with no recipient: who, when, a verdict, an optional note, the id
    mark and revision. While closed, no one holds the turn, so neither side
    is waiting on the other.
11. **Only the human closes, and at any time.** The review exists for the
    human (rule 3), so ending it is the human's call, during either side's
    turn; closing isn't passing the turn, so rule 4 doesn't apply. The
    agent that thinks the work is done says so in a hand-off note and lets
    the human close.
12. **Closing is the verdict.** A close carries one of two verdicts. Both
    end the review: no more action is expected in it. They differ in what
    the agent does next, outside the review:
    - `approved`: take the next step with the plan as it stands (publish
      it to an issue, commit, implement).
    - `dropped`: don't take the next step. Why (abandoned, covered by
      another plan, not now) goes in the note.

    There is no close without a verdict (`missing_verdict`, exit `2`,
    nothing written), and a verdict is set only by closing.

    Open threads don't block `approved`. Approving with threads open
    accepts them as they stand: the agent owes nothing on them. The TUI
    may ask for confirmation when threads are open (`approve with 2 open
    threads?`).
13. **A closed review still takes writes, and they don't reopen it.** A
    reply to a last question or a note about what shipped is valid (rule 5
    again), though none is expected. Threads left open at close stay open; the
    close output lists them, as a hand-off does (rule 7).
14. **Reopening is explicit, by either side, and gives the turn to the
    human.** Either side may reopen a closed review (the agent, when it has
    revised the plan at the human's request outside the review). A reopen
    is a log entry and returns the review to its start state, the human's
    turn (rule 3), with all history kept. Reopening an active review is
    `already_open`-style (`not_closed`, exit `2`), writing nothing.

**The scenarios under this definition.**

| Scenario | How the turn plays out |
|---|---|
| A1–A6, D1–D4 | One hand-off each way per round. Each `── H → A ──` / `── A → H ──` line is a hand-off record. |
| A5 | The agent's turn ends with only a question and no edit. That is a valid hand-off: nothing requires a revision. The agent can't park just `c1`, but it doesn't need to; it finishes the other threads, then hands off. |
| B1 | The agent hands back with `c2` waiting on the human and nothing waiting on itself: a clean hand-off. "Answered", not "closed", is what a turn needs. |
| C2 | Approval ends the review, so nothing in it is expected afterwards (rule 12). If the nit matters, the human hands off for it and approves on the next turn; otherwise they approve, and the open nit is accepted as it stands. The agent's next step uses the plan as approved. |
| C6 | Each ending is a close (rule 10) by the human (rule 11) with its verdict (rule 12). In the variant, the open threads are listed and stay open (rule 13), but the agent still learns whether to proceed. The agent's reopen is a log entry and makes it the human's turn (rule 14). |
| D2 | Round 3 ends with a close carrying `approved`, not a hand-off. |
| C3 | The review starts as the human's turn. The agent seeds `c1`, `c2` during it (allowed by rule 5), then asks the human to open ptal. No hand-off is needed, or possible, until the human finishes. |
| C4 | The human edits the file during their own turn; the revision on their hand-off record captures it. |
| E1 | The human adds `c3` after handing off. When the agent hands back, the output lists `c3` as still waiting on the agent (rule 7). The agent answers it or hands back knowingly. The record's high-water mark shows `c3` arrived mid-turn. |
| E2 | The agent replies during the human's turn. Allowed; the TUI refreshes (#36); the turn doesn't change. |
| A7 | The human resolves their own thread before handing off; the agent sees it was resolved before the hand-off and owes nothing. |

**Considered and dropped.**

- **Per-thread turns** (each thread's "waiting on" *is* the turn). Can't say
  "my whole revision is done". Edits aren't per thread (B2, C1); the
  human needs one signal that the file is ready to reread, not a count of
  answered threads. Kept as the derived "waiting on" instead.
- **Enforced turns** (writes out of turn are refused). E1, E2, C3 and A7
  are all useful writes out of turn. Refusing them pushes the content back
  into the agent chat, which v0.3.0 sets out to remove. ptal can't lock the
  reviewed file anyway (C4).
- **Implicit turns** (inferred from quitting the TUI, the last writer, or
  the file changing). Quitting isn't finishing; the human may come back.
  The agent writes incrementally, and nothing in its writes says "done".
  #35 already chose an explicit hand-off.
- **Batch submit** (comments stay pending, hidden from the other side until
  the hand-off, like a GitHub pending review). Cut from v0.3.0 already. The
  marker in rule 6 gives the useful part (what is new since the hand-off)
  without a second copy of threads. Live refresh (#36) would show pending
  writes anyway unless they were filtered.
- **Hand-off as a full snapshot** (copy threads and file into the record).
  Duplicates data the review and revisions already hold; the id mark and
  revision point at it instead.
- **Two separate events** (a human "finish" and an agent "hand-off" with
  different shapes). Once verdicts live on the close, the two have nothing
  to tell them apart but direction. One event keeps the log uniform for
  Round.
- **Sides by author name.** A changed `--as` name, or a human using the CLI
  under their own name, would invent a third side. A turn needs exactly two
  parties, so it uses roles; names stay for attribution and "waiting on".
- **Either side may pass the turn** (so the agent can "hand off" before the
  human's first look, C3). Makes the turn meaningless as a signal: both
  sides could claim to be waiting. The default-human start covers C3.
- **Closing without a verdict** (closing is separate from judging; a close
  may carry `approved` or nothing). A bare close leaves the agent unable
  to tell "go implement" from "stop": the one thing it needs to know when
  its wait returns. Ending a review is the verdict.
- **Verdicts on hand-offs** (#35's `changes_requested`, or an `approved`
  hand-off for C2). Every human hand-off requests something, so
  `changes_requested` says nothing the hand-off doesn't. An approval that
  still expects a fix is a hand-off; approve once it's done.
- **More verdicts** (`duplicate`, on hold, "accepted but not yours to carry
  out"). Each tells the agent to stop, as `dropped` does; the
  differences are for whoever reads the review later, and the note
  carries them. The verdict answers one question: take the next step, or
  not.
- **Free-text outcome** (the close note alone says what happened). The
  agent would have to parse prose to decide whether to proceed. The note
  stays for detail; the verdict is an enum.
- **Closing is a hand-off to the agent with a "done" flag.** The agent
  would hold a turn in which it is expected to do nothing, and its wait
  (#36) would return as if there were work. A closed review has no holder.
- **The agent may close.** Lets the agent declare its own work accepted.
  It can say so in a hand-off note; the human closes.
- **Close only by the holder** (rule 4 applied to closing). The human
  couldn't stop a review while the agent holds it, which is when stopping
  matters most (the agent is off track, or the plan is dropped).
- **A closed review refuses writes.** Breaks the last reply to a
  question and the "shipped" note; pushes them back into chat.
- **Any write reopens a closed review.** A late reply or a "shipped"
  note would wake the human for nothing, and a close would never stick.
  Reopen is explicit.
- **Closing resolves the open threads.** Silently rewrites their outcome:
  C6c's threads were never answered. They stay open and are listed.
- **Reopen keeps the turn with whoever reopens.** An agent reopening would
  hold the turn and have to hand off immediately; a human reopening is
  about to review anyway. Both end at the human's turn, so reopening
  always does.
- **Hand-off refuses while threads wait on the sender.** Too strict for E1
  (the agent may reasonably leave a late thread for the next round) and
  for "I'll come back to that" replies. Reporting them (rule 7) keeps the
  information without the friction.

**What it means for open stories.**

- **#35:** both commands write the same hand-off record (rule 8). Only the
  holder may hand off (`not_your_turn`). The output lists threads still
  waiting on the sender. Whose turn it is shows in the TUI and in
  `ptal comments` (or a `status` command). #35's statuses (open,
  `approved`, `changes_requested`) become: active (with a turn) or closed
  with a verdict (`approved`, `dropped`). `changes_requested`
  is just the human's hand-off. The TUI needs a close action alongside finish, and the
  existing "reopen a finished review" becomes the reopen of rule 14,
  available to the agent's CLI too.
- **#36:** the agent's wait returns when the turn becomes the agent's and
  returns the hand-off record (author, note, verdict, mark), or when the
  review closes, returning the close record. The close's verdict is how an
  agent's loop knows whether to proceed or stop; a wait on an already closed review returns at once. The TUI's
  live refresh shows out-of-turn writes without moving the turn.
- **#46:** "last look" could be the human's last hand-off rather than a
  separate marker. Left to that story.
- **#45:** a hand-off records the revision once revisions exist.
- **Not decided here:** whether rounds are numbered from the log (Round).

### Revision

**Definition.** A *revision* is one distinct content of the reviewed file,
identified by the SHA-256 of its bytes and kept as a snapshot in the
review's data dir. ptal never writes the file, so it doesn't create
revisions; it *observes* them when it reads the file, and keeps the ones
something was written against. A review is a
sequence of observed revisions plus the threads and turn log written
against them.

**Rules.**

1. **Identity is content.** Same bytes, same revision. A file edited and
   then reverted (A → B → A) is back on revision A, not a third one.
   Snapshots are content-addressed, so a revisited content costs nothing.
2. **Kept when written against.** Every write (a thread, comment,
   resolve, reopen, hand-off or close) hashes the file and, if that
   content isn't kept yet, snapshots it in the same locked transaction.
   Reads hash the file but never write, so viewing stays lock-free. Edits
   made between two writes collapse into one revision: the review records
   what someone acted on, not every save.
3. **Numbered for people, hashed for identity.** Each kept revision gets a
   per-review ordinal (`r1`, `r2`, ...), for display and for `ptal`
   output. A revisited content keeps its first ordinal. A view of content
   nobody has written against yet reports it as new (no ordinal).
4. **Every write records the revision it saw.** A thread's anchor is
   `{revision, start_line, end_line, quote}` on the revision current when
   it was created. Each comment, resolve and reopen also records the
   revision current when written, and so does each hand-off and close
   ([Turn](#turn) rule 6). It is one short field per entry.
5. **Anchors are immutable; positions are computed.** A thread's anchor
   never changes after creation. Where it sits now is computed when
   viewing, by mapping the anchor's range directly from its revision to
   the current one (`anchor::map_range`, #45): `current` when every
   anchored line is unchanged, otherwise `outdated` at a best-guess,
   in-bounds position. Viewing never writes.
6. **Revisions have no author.** ptal can't see who edited the file (C4:
   the human in `$EDITOR`). Attribution comes from the turn log instead:
   the revision on a hand-off is what its sender handed over, so "what
   changed in the agent's turn" is the diff between the revision on the
   human's hand-off and the one on the agent's hand-back.
7. **Same path, same review.** Review identity stays the canonical path
   (`store`). A rewrite, however large, is a new revision of the same
   review (C1); its threads go `outdated`. Starting again on a closed
   review is a reopen ([Turn](#turn) rule 14).
8. **Snapshots live as long as the review.** Plan files are small and
   throwaway; every observed revision is kept, so any anchor can always be
   mapped and any two revisions diffed. No pruning.

**The scenarios under this definition.**

| Scenario | How revisions play out |
|---|---|
| A2 | Each of the agent's two resolves records its revision (rule 4), so the human can see that the first referred to `r2` and the second to `r3`, and what each changed. |
| B2 | `c2`'s resolve records `r2`; "see step 3" is checkable as the diff from `c2`'s anchor revision to `r2`, even though the change is outside `c2`'s lines. |
| B3 | The agent's insert makes `r2`. `c2`'s anchor (`r1`, L14) maps to L15 in `r2` and stays `current` (rule 5). The agent sees the mapped line in `ptal comments`. |
| C1 | The rewrite is `r2` of the same review (rule 7); every thread maps `outdated` with its original quote, and the agent resolves or leaves them. |
| C4 | The human's edit becomes a new revision when the human comments on it (rule 2). It carries no author (rule 6); it falls inside the human's turn, between the agent's hand-back and the human's hand-off. |
| C5 | The anchored lines are gone in `r2`; `c1` maps `outdated`, clamped in bounds, quote kept (rule 5). |
| D3 | `c4` is anchored to `r2`, the others to `r1`; each maps from its own revision (rules 4, 5). |
| E1, E2 | Out-of-turn writes record their revision like any other, so it is visible which version of the file a late comment was written against. |

**Considered and dropped.**

- **Sequential identity** (every observation, or every write, is a new
  revision). A → B → A would show as three revisions with a spurious
  diff, and re-reading an unchanged file would mint revisions.
- **Hash only, no ordinal.** Hashes are unreadable in the TUI and in an
  agent's notes ("resolved in `9f2c...`"). The ordinal costs one counter.
- **Snapshot on every read.** Makes `ptal comments` and every TUI refresh
  a write under the lock, for content nobody acted on.
- **Snapshot only at hand-offs.** Threads created mid-turn would anchor to
  content ptal never kept, so they couldn't be mapped.
- **Watch the file to catch every save** (#36 may watch for refresh
  anyway). Intermediate saves aren't something anyone reviewed; snapshots
  of them add revisions without adding meaning. Rule 2 records what each
  side acted on.
- **Revision only on the thread, not on comments and resolves.** Leaves
  A2's and B2's question ("which version did this resolve refer to?")
  unanswerable, for one field per entry.
- **Re-anchor a thread to each newer revision it maps `current` to.**
  Turns viewing into a write, so `ptal comments` would need the lock and
  could fail with `locked`. Mapping straight from the creation revision
  gives the same answer for `current` lines, and an edit that is later
  reverted maps back to `current`, which chained re-anchoring would lose.
- **Map step by step through every intermediate revision.** Same
  objection: once a step goes `outdated` there is no way back, though the
  text may be identical again. The direct diff is simpler and pure.
- **Author on revisions** (by who first observed them, or by turn holder).
  Wrong for C4, where the human edits and the agent's CLI may be first to
  read it; the turn log already says which side's turn a change fell in.
- **A large rewrite starts a new review.** Any threshold is arbitrary, and
  the old threads' history would be cut off from the plan they shaped.
- **Pruning old snapshots.** Breaks mapping for threads anchored to them;
  the files are too small for the space to matter.

**What it means for open stories.**

- **#45:** snapshots on writes, in the write's transaction, never on
  reads (rule 2); ordinals
  (rule 3); `revision` on anchors, comments, resolves and reopens (rule
  4); direct mapping from the anchor's revision with no stored re-anchor
  (rule 5); no pruning (rule 8). Existing reviews have no revisions: their
  threads' anchors get the revision of the file as first seen after
  upgrading, which is no worse than today. `schema_version` stays `1`.
  `ptal comments` keeps its shape and gains the revision fields.
- **#46:** "changes since my last look" is the diff from the revision on
  the human's last hand-off (or close) to the current one, which needs
  no separate marker. Whether the TUI's own last open also counts is left
  to that story.
- **#35:** each hand-off and close records its revision ([Turn](#turn)
  rule 6).
- **Not decided here:** whether rounds are numbered from hand-offs or
  revisions (Round).

### Round

**Definition.** A *round* is one human turn followed by the agent turn that
answers it: the human reviews, the agent revises. Rounds are derived from
the turn log, never stored. They are vocabulary for people reading a
review ("round 3", "comments from round 2"), not part of the protocol: no
command takes a round, and nothing a side must do depends on one.

**Rules.**

1. **A round starts whenever the human's turn starts.** That is the start
   of the review ([Turn](#turn) rule 3), an agent's hand-off, or a reopen
   (rule 14). It ends when the next one starts, or at a close.
2. **Numbered by counting.** Round *n* is the *n*th time the human's turn
   started, counted along the log. The log is append-only, so a number
   never changes once given. No counter is stored.
3. **A round has up to two turns.** The human's turn always; the agent's
   only if the human handed off. A close ends the round inside the human's
   turn (D2's round 3), or, when the human closes during the agent's turn,
   cuts that turn short.
4. **A round reviews one revision.** The revision on the entry that
   started it (the agent's hand-back, or the reopen) is what the human
   reviews in that round; the one on the agent's hand-back that ends it is
   what the round produced. Round 1 reviews the revision the human first
   writes against ([Revision](#revision) rule 2). A round with no file
   change (A5) reviews and produces the same revision.
5. **Every write belongs to one round and one turn.** The hand-off marks
   ([Turn](#turn) rule 6) bound each turn in the log, so a comment's id
   alone places it: after the mark that started the round, before or
   after the human's hand-off. Out-of-turn writes (E1, E2) land in the
   round and turn they were written during, and can be shown as such.
6. **Threads span rounds.** A thread belongs to the round it was created
   in; its later comments belong to theirs. A thread's history can be read
   round by round (D1).

**The scenarios under this definition.**

| Scenario | How rounds play out |
|---|---|
| A1, A3–A6 | One round: the human's comment and hand-off, the agent's answer and hand-back. A5's question-only turn is still a full round (rule 4). |
| A2, A5, B4, D1, D4 | Two rounds. In D1, `c1` was created in round 1 and gets comments in round 2; the panel can group them (rule 6). |
| C3 | The agent's seeded threads are written in round 1, during the human's turn (rule 5). |
| C6 | The close ends the round; the agent's reopen starts the next one (rule 1). |
| D2 | Three rounds; round 3 is only a human turn, ended by the `approved` close (rule 3). |
| D3 | `c4` is created in round 2, anchored to the revision round 2 reviews (rule 4). |
| E1 | The human's late `c3` belongs to round *n*, written during the agent's turn (rule 5); it shows as late rather than as part of the human's hand-off. |

**Considered and dropped.**

- **A stored round counter, or an explicit "new round" command.** Says
  nothing the turn log doesn't, and two sources of truth can disagree.
- **Round = revision** (each plan version is a round, as critty numbers
  plan versions). A5's rounds change nothing, C4's human edit makes a
  revision inside a turn, and A → B → A revisits one. Revisions are what
  a round reviews and produces (rule 4), not what it is.
- **Round = one turn.** Then "round" is a second name for "turn"; the
  useful unit for people is review-plus-revision.
- **Agent turn first** (the agent revises, then the human reviews). The
  review starts with the human, and C3's seeded questions fit in the
  human's turn of round 1. Starting rounds at the agent would make the
  first one half-empty.
- **Rounds per thread** (each reply-and-answer in a thread is its own
  round). That is the thread's comment history; review-level rounds
  already slice it (rule 6).
- **Rounds in the protocol** (a command or wait keyed by round number).
  The turn log and revisions already carry everything a side needs; a
  round number would only restate them.

**What it means for open stories.**

- **#35, #36:** nothing to store. Status output and the TUI can show the
  current round number next to whose turn it is, computed from the log.
- **#46:** "since my last look" can be stated as "since this round
  started": the revision on the hand-back that began it ([Revision](#revision)).
- **TUI (later):** the thread panel can group a thread's comments by
  round, and mark out-of-turn ones. Not tied to any v0.3.0 story.

### Resolution

**Definition.** A thread's *resolution* is its status: `open` or
`resolved`, as built today. Why a thread was resolved (fixed, won't fix,
deferred, withdrawn, superseded) is said in the comment that goes with it,
the resolve note.

**Rules.**

1. **Two states.** `open` and `resolved`, nothing else. Reopening, or
   replying to a resolved thread, makes it `open` again.
2. **The reason is prose.** A resolve may carry a note, recorded as a
   comment by the resolver. It is where "fixed in step 3", "won't fix,
   because ..." and "deferred to a follow-up" go.
3. **Either side may resolve or reopen.** Whether the agent closes its own
   fixes (A1) or leaves them for the human to confirm (A3) is a
   convention for the agent's skill or prompt, not a ptal rule.
4. **Thread status and the verdict are separate.** An `approved` close
   accepts open threads as they stand ([Turn](#turn) rule 12) but doesn't
   resolve them; they stay `open` in the record.

**Considered and dropped.**

- **Resolution kinds** (`fixed`, `wont_fix`, `deferred`, `withdrawn`,
  `superseded`). The verdict is an enum because the agent does something
  different for each value; no one acts differently on a thread resolved
  as "fixed" versus "won't fix". The human reads the note either way.
- **Only the opener may resolve, or only the human.** A1 and A3 are both
  common; which one a review follows is the agent's convention.
- **An `approved` close resolves its open threads.** Rewrites what
  happened: the threads were accepted, not answered.

### Acknowledgement

**Definition.** Acknowledgement is implicit. A resolve stands unless
someone reopens it; nothing records that the other side has seen or
accepted it.

**Rules.**

1. **Disagreement is a reopen with a comment.** If the human has a concern
   with a resolution, they reopen the thread with a note saying why (A2);
   the same goes for the agent with a resolve by the human.
2. **Silence accepts.** A resolve that is never reopened is accepted. An
   `approved` close accepts whatever is left, resolved or open
   ([Turn](#turn) rule 12).

**Considered and dropped.**

- **An explicit acknowledgement** (a per-thread "seen" or "confirmed"
  mark; ARCHITECTURE.md listed it as a deferred `acknowledge` operation
  until this decision removed it). It adds
  a step to every thread for the rare case of a disputed fix, which a
  reopen already covers.

### Comment intent

**Definition.** A comment has no declared intent. Whether it asks a
question, requests a change, points out a nit or just agrees is said in its
text.

**Considered and dropped.**

- **An intent tag** (`question`, `change`, `nit`, chosen when writing). No
  one acts differently on the tag: the agent reads the comment either way,
  and answers with a reply (A4) or an edit (A1) as the text asks. Same
  reasoning as [Resolution](#resolution) kinds.
