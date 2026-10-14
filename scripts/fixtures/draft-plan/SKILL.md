---
name: draft-plan
description: Draft an execution plan for a task, review it with the user, then publish it. Use when the user asks for a plan.
---

# draft-plan

Produce an execution plan for one task, written to `plan.md` in the working directory.

## Process

1. **Gather context (read-only).** Read the files the task names and anything they reference.
2. **Draft the plan** to `plan.md`: goal, decisions made up front, numbered steps, risks.
3. **Show it to the user** in the reply. Lead with the decisions they may want to veto, and ask for changes. Iterate on the file until they approve.
4. **Publish only on explicit approval**: copy `plan.md` to `published/plan.md`. Do not start implementing.

## Rules

- Keep it scannable: tables for decisions, short bullets for steps.
- Make edge cases explicit decisions rather than leaving them to be discovered later.
- Stay inside the task's scope; flag adjacent work instead of absorbing it.
