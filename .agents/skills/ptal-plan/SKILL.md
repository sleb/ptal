---
name: ptal-plan
description: Draft an execution plan for a ptal GitHub issue (story), review it with the user, then publish it to the issue as a comment. Use when the user says "/ptal-plan <issue#>" or asks to plan the next story.
---

# ptal-plan

Produce an execution plan for one GitHub issue in this repo, in the same shape as the plan on #3. Input: an issue number (`/ptal-plan 4`). If none is given, list open issues with `gh issue list` and ask which one.

## Process

1. **Gather context (read-only).**
   - `gh issue view <n> --json title,body,comments` — scope, acceptance criteria, out of scope.
   - The parent issue it references ("Part of #N") — the MVP design decisions table and out-of-scope list bind this story.
   - Dependency issues ("Depends on #M") and their plans/comments — reuse their decisions; don't re-decide.
   - `ARCHITECTURE.md` and `README.md` — note where the MVP deliberately narrows the architecture.
   - The code that exists now (`src/`, `tests/`, `Cargo.toml`) — plans must build on what is actually there, not what the docs say should be.
   - Project instruction files if present (`AGENTS.md`, `CLAUDE.md`, or equivalent for the running agent).
2. **Draft the plan** to the scratchpad as `plan-<n>.md` using the template below.
3. **Review it with ptal.** `ptal --help` has the details of every command below; read it once. Use your own agent name for every write (`--as claude`, `pi`, `codex`, or set `PTAL_AUTHOR`).
   - **Check ptal.** `ptal wait <file> --timeout 0` must exit 0 (it prints `"reason": "timeout"` on a fresh review, which starts on the human's turn). If `ptal` is missing or the command is a usage error, the installed ptal is too old: stop and tell the user to run `cargo install --path .`.
   - **Open the review for the user.** Inside herdr (`HERDR_ENV=1`), open it in a pane beside you:
     `herdr pane split --current --direction right --cwd "$PWD" --focus`, read the new pane's id from `.result.pane.pane_id`, then `herdr pane run <pane_id> "exec ptal <absolute path to plan-<n>.md>"` (`exec`: the pane closes when the user quits ptal). Outside herdr, or if a herdr call fails, ask the user to run `ptal <absolute path to plan-<n>.md>` in a terminal (it needs an interactive one).
   - **Hand over.** In your reply, lead with the decisions the user may want to veto, and say that when they're done they hand the turn back with `H` in ptal (or close with `C`: `approved` or `dropped`).
   - **Wait for your turn:** `ptal wait <file>`. It blocks until the user hands off or closes, so run it in the background if your harness can (in Claude Code: `run_in_background`, and you are re-invoked when it exits); otherwise loop on `ptal wait <file> --timeout 300`. Act on its `reason`:
     - `timeout`: wait again.
     - `turn`: read the hand-off note (`review.latest.note`) and the threads (`ptal comments <file>`). Edit the plan for each thread, then `ptal reply <file> <id> "..."`, or `ptal resolve <file> <id> --note "<what changed>"` once it is addressed. Re-run `ptal comments`, since edits can leave threads `outdated`. Leave a thread open and reply when a comment is unclear or you disagree. Then `ptal handoff <file> --note "<what changed>"`, tell the user in a line or two what changed, and wait again. The review pane refreshes on its own; don't reopen it while it is still open.
     - `closed` with verdict `approved`: that is the user's approval; go to step 4. Mention any threads still open.
     - `closed` with verdict `dropped`: don't publish; tell the user and stop.
   - If the user answers in the conversation instead (approval, or "done" without handing off), follow that: it overrides the wait. Stop any background `ptal wait` before ending.
4. **Publish only on explicit approval** (an `approved` close, or the user saying so): `gh issue comment <n> --body-file <file>`. Post as a comment (keeps the original scope and acceptance criteria intact) unless the user asks to edit the body. Return the comment URL.
5. Do not start implementing unless asked.

## Non-negotiable working agreements

Every plan states these at the top, verbatim in spirit:

1. **Strict TDD.** Every behavior starts as a failing test, seen failing for the right reason, then minimum code to pass, then refactor green. One red→green→refactor per commit-sized step; no production code without a test that demanded it.
2. **Fearless refactoring.** Single user, no compatibility burden. If something is wrong or the decision changes, delete it and rewrite it. No shims, deprecated aliases, or migration code for formats nobody has.
3. **Docs stay true.** Each step lands with the docs that describe it (rustdoc, `ARCHITECTURE.md`, `README.md`). Docs describe what exists; planned-but-unbuilt design is labeled as such. A step is not done while a doc contradicts the code.

Definition of done for every step: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` green; docs touched where behavior changed.

## Plan template

```markdown
## Execution plan

Working agreements: <the three above, plus the definition of done>

### Decisions made up front
| Question | Decision |
<one row per choice not settled by the issue/parent: crates, file layout, error types, wire shapes, edge-case semantics. Include "no X crate" decisions with the reason.>

### Steps
**0. <setup, only if needed>**
**1. <module or slice>**
- Red: <the failing test, stated as behavior>
- Green: <the minimum implementation>
- Refactor: <what to tidy while green>
- Docs: <exactly which doc sections change>
... each step is a list of small red/green cycles, ordered so each builds on the last.
**N. Docs pass and close-out** — ARCHITECTURE status, README status, tick acceptance criteria with proving tests, PR (`Closes #<n>`), one commit per step.

### Acceptance criteria → proving tests
| Criterion (from the issue, verbatim) | Proved by (step) |

### Risks / watch-outs
```

## Rules for good plans

- **Every acceptance criterion maps to a named test/step.** If one can't be mapped, the criterion is under-specified: say so and propose a precise reading in the decisions table.
- **Stay inside the issue's scope and the parent's out-of-scope list.** Flag, don't absorb, anything adjacent.
- **Prefer seams for testability:** pure functions over env/clock/randomness injected, no printing in core, front ends thin.
- **Make edge cases explicit decisions** (idempotency, empty input, encoding, ordering, error precedence) rather than leaving them to be discovered mid-implementation.
- **Resolve doc drift in the plan.** If ARCHITECTURE/README describe something the MVP doesn't build, the plan includes rewriting that section (and moving the future design to a labeled deferred section), not annotating around it.
- **Verify before asserting.** Check that a std API or crate behavior you rely on exists in the installed toolchain (`cargo --version`, docs) and list unverified assumptions under Risks.
- Pronouns: use they/them for anyone not stated.
- Keep it scannable: tables for decisions and criteria, short bullets for steps. No prose essays.
