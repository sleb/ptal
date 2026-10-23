---
name: ptal-review
description: Run a ptal review of a plan or other text file with the human. Use when the user asks you to put a file up for review, or says they left comments on one.
---

# ptal-review

ptal lets a human comment on lines of a file in a terminal view while you read
and answer those comments through the `ptal` CLI. Read `ptal --help` once
before you start: it has the turn loop, the output shapes and how commands fail.
`ptal <command> --help` has each command's output and errors. The full
reference is in [interfaces](https://github.com/sleb/ptal/blob/main/docs/interfaces.md)
and [protocol](https://github.com/sleb/ptal/blob/main/docs/protocol.md).

Use the absolute path of the file and the same `--as` name for every write.

## The loop

1. Once per review, open it for the human. In a terminal multiplexer such as
   herdr, open a pane beside you running `exec ptal '<absolute file>'`.
   Otherwise ask the human to run `ptal <file>` in a terminal.
2. Run `ptal wait <file>`. If your harness can, run it in the background;
   if not, pass `--timeout` below your command time limit and run it again
   on a timeout. Read `reason` in the output:
   - `"turn"`: your move. The human's note is `review.latest.note`.
   - `"closed"`: stop looping and read `review.verdict`. `"approved"` means
     take the next step (see your choices); `"dropped"` means stop.
   - `"timeout"`: still the human's turn, so wait again.
3. Run `ptal comments <file>` and read the open threads.
4. Edit the file to address them, then run `ptal comments <file>` again,
   because your edits can leave threads outdated.
5. For each thread, `ptal reply` to ask or explain, and `ptal resolve` when
   you fixed it, with a `--note` saying what changed. `ptal reopen` takes a
   resolved thread back.
6. Run `ptal handoff <file> --as <name> --note "what you did"`, then go back
   to step 2.

Only the human closes a review. If they ask you to revise a closed one, run
`ptal reopen <file> --as <name>` with no thread id, then wait again.

## Your choices

Edit these to fit how you work.

- Who resolves: by default you resolve a thread you fixed, with a `--note`.
  To leave that to the human, reply instead and let them resolve it in the
  view.
- What `approved` triggers: name the next step here, such as publish the
  plan, commit the file, or start implementing it.
- Your `--as` name: pick one stable name for yourself and use it every time
  (or set `PTAL_AUTHOR`), so ptal can tell which threads wait on you.
