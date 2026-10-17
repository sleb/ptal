# AGENTS.md

This file provides guidance to coding agents (Claude Code, pi, Codex, etc.) when working with code in this repository. `CLAUDE.md` is a symlink to it.

## Project state

ptal is a Rust terminal tool for reviewing a single throwaway text file (usually a `.md` plan): a human comments in a TUI, an agent reads and resolves comments through a JSON CLI.

Work is tracked in GitHub: an **MVP/milestone issue** holds the scope and design decisions, and **story issues** are sub-issues of it, each with scope, acceptance criteria, and out-of-scope. Find current work with `gh issue list`; don't rely on issue numbers written in docs.

`README.md` and `ARCHITECTURE.md` describe the design. The MVP deliberately narrows it, and the milestone issue lists the deviations. Where they disagree, the milestone issue wins for the MVP, and the docs are corrected as code lands rather than annotated around.

## Working agreements

1. **Strict TDD.** Failing test first (seen failing for the right reason), minimum code to pass, then refactor green. No production code without a test that demanded it.
2. **Fearless refactoring.** Single user, no compatibility burden. If something is wrong, delete and rewrite it. No shims, deprecated aliases, or migration code for formats nobody has; `schema_version` stays `1` until real data exists.
3. **Docs stay true.** Each change lands with the docs that describe it (rustdoc, `ARCHITECTURE.md`, `README.md`). Docs describe what exists; unbuilt design is labeled deferred.
4. **One interface map.** `docs/interfaces.md` lists every CLI command and TUI key side by side. Any change to the user-facing surface (command, flag, key, mode) updates that table in the same change.

5. **Rust skills.** Any step that writes, reviews, or refactors Rust code uses `/rust-skills`.

6. **Releases.** Cutting a release follows `docs/releasing.md`.
7. **PR review comments.** CI reviews a PR once, when it is opened; pushes do not re-review. After addressing a review comment, reply on its thread saying what changed (naming the commit) or why not (won't fix, deferred to an issue), and resolve the thread, before merging. The review follows up on each reply: it accepts a won't-fix or deferral, answers questions, and checks a claimed fix, reopening the thread if the fix falls short; wait for those follow-ups before merging.
8. **Watch the PR.** Right after opening a PR, subscribe to it for the rest of the session (in Claude Code, a `Monitor` that polls `gh` about once a minute and re-arms when it expires). Report new reviews, review comments, threads that become unresolved, check results, and the PR being merged or closed. Track items by ID, not by their text (a push moves comments to new lines), and skip your own replies. The watch only reports what is new, so read what is already on the PR when you start it. Stop the watch as soon as the PR is merged or closed: have the polling script exit on that state, and if it is still running after the merge, stop it (`TaskStop`) before ending the turn. Don't leave a monitor running past the merge.

Green gate for every step: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`, `cargo package --locked`.

Plan stories with `/ptal-plan <issue#>`; the approved plan is posted as a comment on the issue and is the execution script.

## Commands

Once the crate exists:

```sh
cargo build
cargo test
cargo test <name>                 # single test by substring
cargo test --test <file>          # one integration test file in tests/
cargo clippy --all-targets -- -D warnings
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo package --locked
```

## Architecture

See `ARCHITECTURE.md` (layering and dependency rules, per-component invariants and contracts, "Cross-cutting rules", "Testing strategy"). It is the single source of truth; don't restate it here. If a rule matters and is missing from it, add it there.
