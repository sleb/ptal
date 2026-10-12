# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project state

ptal is a Rust terminal tool for reviewing a single throwaway text file (usually a `.md` plan): a human comments in a TUI, an agent reads and resolves comments through a JSON CLI. **No code exists yet.** The repo holds design docs only; implementation is tracked in GitHub issues:

- #2 MVP umbrella (its "MVP design decisions" table narrows ARCHITECTURE.md)
- #3 core: `model`, `store`, `review` → #4 agent CLI → #5 TUI → #6 agent dry run

`README.md` and `ARCHITECTURE.md` describe the full design. **Where they disagree with #2, the issue wins for the MVP** (no revisions/snapshots, no `anchor` mapping, no replies/edit/delete, one crate rather than a workspace). Fix the docs when code lands rather than annotating around them.

## Working agreements

1. **Strict TDD.** Failing test first (seen failing for the right reason), minimum code to pass, then refactor green. No production code without a test that demanded it.
2. **Fearless refactoring.** Single user, no compatibility burden. If something is wrong, delete and rewrite it. No shims, deprecated aliases, or migration code for formats nobody has; `schema_version` stays `1` until real data exists.
3. **Docs stay true.** Each change lands with the docs that describe it (rustdoc, `ARCHITECTURE.md`, `README.md`). Docs describe what exists; unbuilt design is labeled deferred.

Green gate for every step: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

Plan stories with `/ptal-plan <issue#>`; the approved plan is posted as a comment on the issue and is the execution script.

## Commands

Once the crate exists (step 0 of #3):

```sh
cargo build
cargo test
cargo test <name>                 # single test by substring
cargo test --test <file>          # one integration test file in tests/
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Architecture (the parts that span files)

Layering is strict and downward-only: `tui`/`cli` → `review` → `store` → `model`. Front ends hold no business rules and never touch `store` directly.

- **`review` is the only write path.** Every mutation is a transaction: `store::with_review` takes an exclusive lock, reads, applies, re-checks invariants, atomically writes. This is what lets the TUI and an agent driving the CLI run concurrently. Never hold state across a lock release.
- **Core has no UI knowledge.** `review` returns typed results and typed errors (`NotFound`, `InvalidRange`, `FileUnreadable`, `Locked`, …); no printing. Exit-code mapping (0 ok / 1 runtime / 2 usage-or-not-found) belongs to `cli`.
- **Time and id counter are injected** so core logic is deterministic in tests.
- **The reviewed file is read-only to ptal.** Data lives under `$PTAL_DATA_DIR` → `$XDG_DATA_HOME/ptal` → `~/.local/share/ptal` (also on macOS; do not use the `dirs` crate), in `<slug>-<hash8>/review.json` keyed by the canonical path. Never write next to the reviewed file.
- **Wire format is the model types.** stdout is data only (JSON when not a TTY or `--json`); diagnostics and errors go to stderr. Lines are 1-based and inclusive everywhere. Comment ids are `c1`, `c2`, … from a monotonic, never-reused counter shared by threads and resolve notes.
- **Staleness (MVP):** `state: current | outdated` is computed by checking the stored quote at the stored lines; there is no range mapping. `state` is in the schema so real mapping can replace this later without an output change.
- **CLI help is part of the product.** `ptal --help` alone must teach an agent the list → address → resolve workflow; every command carries an example. Writes require `--as` or `PTAL_AUTHOR`; the CLI never invents an author.
- **TUI (when built):** all keys handled in a pure `update(state, event)`; `render` is pure over state; the TUI persists nothing itself and re-reads `view` after each write.
