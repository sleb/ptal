# Review interactions: a catalog of common variants

> **Research material, not a spec.** This page collects the back-and-forth
> patterns between a human and an agent in a ptal review, as concrete
> transcripts. It exists to be analyzed later, to extract a rough protocol
> and the core abstractions (review, revision, turn, thread, comment,
> verdict). It describes interactions people have, not what ptal does:
> each scenario says which steps ptal supports today and which happen out of
> band or need planned work (v0.3.0: #35 finish/hand-off, #36 wait and live
> refresh, #45 revisions, #46 changes since last look).

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
  visible to the agent at all? (Deferred in v0.3.0: draft comments hidden
  until submit.)

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
A  resolve c1..cN note "Superseded by the CDN rewrite."      (or leaves them for the human to judge)
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
H  add L7 c3 "Mention the Retry-After header here too."
── H → A ──
A  edit ...; resolve c1, c2, c3
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
  critty's plan versions), or only implied by revisions and hand-offs?

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
| Agent edits the file | A1–A2, A4b, A5, A6b, B1–B4, C1–C2, C5, D1–D4 |
| Human edits the file | C4 |
| Agent resolves with a note | A1, A2, A4b, A5, A6b, B1–B4, C1–C2, C5, D1, D4 |
| Human resolves | A3, A4, A6a, A7 |
| Human reopens | A2, D1 (by reply) |
| Thread goes outdated | B3, C1, C5 |
| Review-level verdict or note | C1, C2, D2 |
| Turn passes without a file change | A5, A6, A7, D4 |
| Writes overlap a turn | E1, E2 |

## Recurring questions

Collected from the scenarios above, unanswered here on purpose:

- **Turn:** is it per review or per thread? Is it enforced or a signal? Is
  a hand-off a snapshot? (A5, B1, E1, E2) *Proposed: see [Turn](#turn).*
- **Resolution:** who may resolve, and are fixed / won't fix / deferred /
  superseded / withdrawn kinds of resolution or just prose? (A3, A6, A7, C1, D4)
- **Acknowledgement:** does the human's acceptance of an agent's resolve
  need recording? (A1, C2)
- **Revision:** which revision is a thread, a comment, a resolve anchored
  to? Who authored a revision? Is a rewrite still the same review? (B2, B3,
  C1, C4, D3)
- **Round:** first-class (numbered) or implied by hand-offs? (D2)
- **Comment intent:** question, change request, nit; explicit or prose? (A4, C2)
- **Verdict:** does approval with open threads block, and does a later edit
  undo it? (C2)

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
   and the agent's (#35's "hand-off") are the same event. The human's may
   carry a verdict (see Verdict, still to be defined); front ends may
   still name the two commands differently.
9. **Hand-offs are kept as a log.** Append-only. The current turn is the
   recipient of the last entry, or the human if there is none. The log is
   what Round will build on.

**The scenarios under this definition.**

| Scenario | How the turn plays out |
|---|---|
| A1–A6, D1–D4 | One hand-off each way per round. Each `── H → A ──` / `── A → H ──` line is a hand-off record. |
| A5 | The agent's turn ends with only a question and no edit. That is a valid hand-off: nothing requires a revision. The agent can't park just `c1`, but it doesn't need to; it finishes the other threads, then hands off. |
| B1 | The agent hands back with `c2` waiting on the human and nothing waiting on itself: a clean hand-off. "Answered", not "closed", is what a turn needs. |
| C2 | The human hands off with `approved`. The agent's nit fix happens in its own turn and needs no hand-back. Whether approval ends the review is a Verdict question. |
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
  different shapes). They differ only in the verdict. One event keeps the
  log uniform for Round.
- **Sides by author name.** A changed `--as` name, or a human using the CLI
  under their own name, would invent a third side. A turn needs exactly two
  parties, so it uses roles; names stay for attribution and "waiting on".
- **Either side may pass the turn** (so the agent can "hand off" before the
  human's first look, C3). Makes the turn meaningless as a signal: both
  sides could claim to be waiting. The default-human start covers C3.
- **Hand-off refuses while threads wait on the sender.** Too strict for E1
  (the agent may reasonably leave a late thread for the next round) and
  for "I'll come back to that" replies. Reporting them (rule 7) keeps the
  information without the friction.

**What it means for open stories.**

- **#35:** both commands write the same hand-off record (rule 8). Only the
  holder may hand off (`not_your_turn`). The output lists threads still
  waiting on the sender. Whose turn it is shows in the TUI and in
  `ptal comments` (or a `status` command).
- **#36:** the agent's wait returns when the turn becomes the agent's and
  returns the hand-off record (author, note, verdict, mark). The TUI's
  live refresh shows out-of-turn writes without moving the turn.
- **#46:** "last look" could be the human's last hand-off rather than a
  separate marker. Left to that story.
- **#45:** a hand-off records the revision once revisions exist.
- **Not decided here:** what a verdict is and whether `approved` ends the
  turn-taking (Verdict); whether rounds are numbered from the log (Round).
