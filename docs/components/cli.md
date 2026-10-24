# `cli`

**Role:** the agent-facing front end (`src/cli.rs`, clap derive). It parses args, calls [`review`](review.md), and prints. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the layering and [docs/interfaces.md](../interfaces.md) for the full command and key map.

## Structure
- The whole flow is `run(args, &Env, out, err) -> u8`.
- The process facts are injected via `Env`: store, clock, ticker, `PTAL_AUTHOR` and TTY flags. Every path is therefore testable in-process, and `main` only fills `Env` from the real process.
- The ticker comes from `Env.ticker`, the `T` of `Env<C, T>` (`SystemTicker` in `main`), so in-process tests never sleep.
- `run` splits into `run_command` and `launch_tui`.
- `launch_tui` refuses everything it can before the terminal is touched: no TTY on stdin and stdout (`not_a_terminal`), blank `$USER` (`missing_author`), unreadable file.
- The TUI's author is `$USER` (via `Env.user`), never `PTAL_AUTHOR`.

## Commands
- `comments`, `add`, `resolve`, `reply`, `reopen`, `handoff`, `wait`, `skill`.
- Bare `ptal <file>` launches the TUI (`args_conflicts_with_subcommands`). A file named like a subcommand is written `./name`.
- `--lines` is `N` or `A-B`. Syntax errors are `usage`; range validity against the file is the core's `InvalidRange`.
- There is no CLI close; only the human closes, in the TUI.

## Author
- Writes require `--as <name>` or `PTAL_AUTHOR` (blank counts as unset).
- The CLI never invents an author and never reads `$USER`.
- Threads carry no `waiting_on` field. A thread is waiting on an agent when `Thread::is_waiting_on` its `--as` name: open, last comment by someone else, exact match.
- The help says so and tells agents to use one stable name. A hand-off's `waiting` asks the same of the author handing off.

## Output shapes

### Streams
- **stdout** is data only: JSON when not a TTY (or `--json`), a readable rendering when a TTY.
- The one exception is `skill`, which always prints the markdown of `skills/ptal-review/SKILL.md` (embedded with `include_str!`) and needs no data dir.
- **stderr** is diagnostics. The two are never mixed; on failure stdout is empty.
- `--help` and `--version` go to stdout with exit `0`.
- A failed stdout write is `Failure::Stdout` (`io`, exit `1`). A closed pipe (`BrokenPipe`) ends quietly with the command's own status.
- `file` in every JSON output is the canonical path. A non-UTF-8 byte in it prints as U+FFFD: the command still works, and the printed path isn't usable.
- The readable rendering hangs a multi-line comment or note's later lines under its body.

### `comments`
- Prints `{"file", "revision", "threads": [...]}`. `revision` is the file's current revision id, or `null` if nothing has been written against this content.
- Each thread is self-contained: id, `status`, `state`, `anchor`, and its flat list of `comments`.
- The `anchor` has its `revision`, its lines mapped to the file now, and the original quote. Each comment has its `revision`.
- Resolved threads add `resolved_by`, `resolved_at` and `resolved_revision`.
- Default is open threads; `--all` includes resolved. Threads are in id order.
- Replies are flat: there is no reply tree.
- A top-level `review` is the `ReviewStatus`: `turn`, `verdict`, and `latest`, the last log entry in its on-disk shape. Only the latest entry is printed; the full log stays on disk.

### `add`, `reply`, `resolve`, `reopen`
- Print `{"file", "thread"}`, with no `state`: it is a read-time concept.
- The printed anchor's lines are mapped to the file as it is now, like `comments`, so the CLI never shows unmapped lines. `anchor.revision` stays the revision the thread was created on.
- Resolving a resolved thread fails with `already_resolved` and reopening an open thread with `already_open`. Both exit `2`.

### `handoff` and review `reopen`
- `handoff <file> [--side human|agent] [--note]` prints `{"file", "review", "waiting"}`.
- `reopen <file>` with no thread id reopens the review and prints `{"file", "review"}`.
- The CLI acts as the agent. `--side human` overrides it on these two. It is a flag, not an environment variable, so an agent can't inherit it.
- `--side` on a thread reopen is `usage` (a clap conflict): threads have no side.

### `wait`
- `wait <file> [--timeout SECS]` prints `{"file", "reason", "review"}`.
- `reason` is the core's `Woke`: `turn`, `closed`, `timeout`. All three exit `0`: a timeout is an outcome the agent loops on, not a failure.
- A wait that never read the review fails with `io` (exit `1`).
- It takes no `--as` (a read) and no `--side` (only the agent waits).
- It is one blocking call per turn, not a stream: an agent's command tool reads a command's output when it exits.

## Errors and exit codes
- Errors are one line on stderr, `{"error": {"code", "message"}}`. On a TTY stderr they are readable text unless `--json` appears before `--`.
- `Failure` is a `std::error::Error` (`thiserror`); its `Display` is the message.
- The code-to-exit mapping is the `Failure::classify` table, pinned by a unit test and documented in [docs/interfaces.md](../interfaces.md#exit-codes).
- `2` means the caller can fix the invocation; `1` is a runtime failure.

## Help
- `--help` is part of the product, split so each fact lives in one place.
- The top-level help is the one page an agent must read. It has the turn loop, the concepts, the author, output and error conventions, the full exit-code table and env vars, and a pointer to each command's help.
- The turn loop covers opening `ptal <file>` for the human in a multiplexer pane (or asking them to run it), then `wait`, what each `reason` and verdict means, and backgrounding or `--timeout`.
- Each command's `after_long_help` (one `*_HELP` const) has its output shape, the errors it can return, and examples.
- The help states facts about the tool (what a command does, what a field means), not policy, and names no particular multiplexer.
- When to resolve, what to ask and how to hand over to the human are policy. They belong to the user's skill or prompt.
- ptal ships one as an example, `skills/ptal-review/SKILL.md` (`ptal skill`), whose `Your choices` section the user edits. The help stays policy-free.

## Invariants
- Output shapes are the `model` types, so changing them is a `schema_version` bump.
- No business rule lives here; the CLI maps arguments to `review` calls and errors to exit codes.

## Contract
The commands, output shapes and exit codes above, with [docs/interfaces.md](../interfaces.md) as the table form.

## Tests
- In-process tests in `src/cli.rs`: golden JSON, TTY rendering, the error table, help.
- Help tests pin the split: every `Failure::classify` code is in the top-level table, every subcommand is named there, and every subcommand's help says what it prints and has an example.
- Process tests in `tests/cli.rs` cover streams, exit status and env; turns passing between the library (as the TUI writes them) and separate `ptal` processes; and a spawned `ptal wait` woken by another process's hand-off.
- `tests/docs.rs` runs the built binary against `skills/ptal-review/SKILL.md`: the commands, flags and JSON fields the skill names must exist in the help, and `ptal skill` must print the file byte for byte.
- `tests/docs.rs` also keeps the contributor skills `internal`, so `npx skills` lists only `ptal-review`.
