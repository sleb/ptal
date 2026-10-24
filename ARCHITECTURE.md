# Architecture

ptal is a terminal tool for reviewing a throwaway text file, usually a markdown plan. Today a review covers one file, keyed by the file's canonical path. A human uses the TUI; an agent uses the CLI. Both are thin front ends over one core that owns all rules and all persistence.

This document is the end-to-end view: the layering, how comments follow edits, the cross-cutting rules and distribution. Each component's invariants and contract are in its own doc under [docs/components/](docs/components/model.md).

```
        ┌─────────┐      ┌─────────┐
        │   tui   │      │   cli   │        front ends (no business rules)
        └────┬────┘      └────┬────┘
             └───────┬────────┘
               ┌─────▼─────┐
               │  review   │   the only write path; enforces invariants
               └──┬─────┬──┘
          ┌───────▼┐   ┌▼────────┐
          │ anchor │   │  store  │          pure logic │ disk I/O
          └───┬────┘   └────┬────┘
              └──────┬──────┘
               ┌─────▼─────┐
               │   model   │   plain data types, serde
               └───────────┘
        (highlight lives under tui)
```

- Dependencies point downward only. `model` depends on nothing in the crate.
- `tui` and `cli` never use `store` directly. They use `anchor` only for read-only mapping (the TUI keeps its cursor in place across a re-read) and never to change data.
- `cli` only constructs the `Store` (in `main` and in tests) to build `Reviews`.

## Key idea: revisions and anchoring

A plan file is mutable and untracked, so there are no commits to anchor to. ptal supplies its own: every distinct content of the file is a **revision**, snapshotted and content-addressed (SHA-256) in the data dir.

Snapshots are taken only on writes (see [docs/protocol.md § Revisions](docs/protocol.md#revisions)). Edits made between two writes collapse into one revision, and viewing never writes.

A thread is anchored to a line range **in the revision it was created on**. Showing it against a newer revision means mapping that range forward by diffing the two snapshots, the same approach GitHub uses between PR commits:

- All anchored lines are unchanged → the thread maps to the new line numbers (`current`).
- Any anchored line was edited or removed → the thread is `outdated`. It is still listed, shown at the best-guess position, with its original quoted text.

There is no fuzzy text search. Mapping is deterministic and explainable, and outdated threads are never silently dropped or silently re-pointed at the wrong text.

### Change markers
There is no separate diff-view component. The one presentation, gutter markers for the changes since the human's last look, is split across the layers:

- `anchor::Diff::changes` finds the hunks.
- `review::view` picks the base revision (`Review::last_look`) and returns `View.changes`.
- `tui::app` stores them and answers `change(line)` and the `}`/`{` jumps.
- `tui::render` draws the glyphs and the footer count.

No trait is built for other presentations while there is one.

## Components

- [`model`](docs/components/model.md) — the data types that are persisted and exposed to agents: `Review`, `Thread`, `Anchor`, the review log and its derived state.
- [`anchor`](docs/components/anchor.md) — pure functions mapping line ranges between revisions, and listing what changed.
- [`store`](docs/components/store.md) — persistence: the data dir, atomic `review.json`, the lock and the `revisions/` snapshots.
- [`review`](docs/components/review.md) — the application core and the only write path: threads, turns, closing, `wait`, and the typed errors.
- [`cli`](docs/components/cli.md) — the agent-facing front end: commands, JSON output, exit codes and `--help`.
- [`tui`](docs/components/tui.md) — the human front end: the interactive review, live refresh, and rendering.
- [`highlight`](docs/components/highlight.md) — markdown structure styling for the file view, under `tui`.

[docs/interfaces.md](docs/interfaces.md) pairs each CLI command with its TUI key; [docs/protocol.md](docs/protocol.md) describes the human–agent protocol.

## Cross-cutting rules

- **Single write path.** If it changes data, it goes through `review`.
- **The file is read-only to us.** ptal never modifies the reviewed file.
- **Core has no UI knowledge; front ends have no rules.**
- **The front ends split the work.** The CLI is what an agent drives; the TUI is what the human drives. Both reach the same `review` operations, so any rule is enforced once.
- **One interface map.** [docs/interfaces.md](docs/interfaces.md) pairs each CLI command with its TUI key. Any change to either front end's surface updates it in the same change.
- **Lines are 1-based and inclusive** everywhere in the model and on the wire.
- **Time is injected** (the clock, and `wait`'s ticker) so core logic is testable deterministically. Ids come from `Review::next_comment_id`, plain state, and nothing is random.
- **Lints live in `Cargo.toml`'s `[lints]`:** a short curated list of clippy lints that caught real issues, not the `pedantic` group.
- **Forward compatibility.** `schema_version` on every persisted file; newer versions are refused. Older `review.json`s load through serde defaults, and legacy anchors are adopted on the next write. There is no migration step.

## Testing strategy

Each component doc has a `Tests` section with its claims. The layers:

- **Unit tests** beside the code pin each rule: one test per rule in `review`, golden JSON in `model` and `cli`, exhaustive properties in `anchor`, snapshots and properties in `tui`.
- **Injected clock and ticker** make time deterministic; tests hand off from inside a fake `sleep`.
- **Integration tests** in `tests/` exercise the layers together: `core.rs` (concurrent writers, a whole turn round), `cli.rs` (the built binary), `tui.rs` (scripted rounds through `step`), `panic_hook.rs` (the global hook, in its own process).
- **Docs tests** in `tests/docs.rs` tie the docs to the code:
  - the help listing against README's key tables and the interface map, with short interface-map cells;
  - README prose: no status or roadmap words, paragraphs of at most 120 words, and a `For agents` section linking the reference docs and the skill;
  - the built binary against `skills/ptal-review/SKILL.md`, and the contributor skills staying `internal`;
  - `ARCHITECTURE.md` and the component docs: no status words or local deferrals, future work only in the final issue-linked list, prose units of at most 120 words, each component doc's shape, and links to every component doc.
- A real-terminal check (pty) is manual.

## Distribution

The crate is publishable; the procedure is [docs/releasing.md](docs/releasing.md).

### What ships
- The `include` allowlist in `Cargo.toml` is the source of truth for what ships: `src/`, the integration tests, `Cargo.toml`, `Cargo.lock`, `README.md`, `LICENSE`, and `skills/`, which `ptal skill` embeds.
- The test files are listed one by one, so a new one is added by hand or it does not ship.
- `tests/docs.rs` is left out on purpose: it reads `docs/`, `AGENTS.md` and `.agents/skills/`, which the package does not hold.
- `docs/`, `ARCHITECTURE.md`, `CHANGELOG.md` and agent/editor directories are deliberately excluded. README links to them are relative, which GitHub and crates.io (via `repository`) resolve against the repo.
- The allowlist is reviewed against the previous tag's package list before every publish.
- `rust-version` is the floor declared by the dependency tree, checked by hand and not enforced in CI.

### CI
- CI runs `cargo package --locked`, fails if a forbidden path enters the tarball, and builds the docs from the unpacked tarball.
- A separate step runs `knap lint .`, which checks that every markdown link and anchor in the repo resolves.

### A release
- Bump `Cargo.toml` and `CHANGELOG.md`, `cargo publish` by hand (crates.io first, as it is irreversible), then push tag `vX.Y.Z`.
- `.github/workflows/release.yml` verifies the tag against `Cargo.toml` and `CHANGELOG.md`.
- It builds `.tar.gz` archives for `aarch64`/`x86_64` Apple Darwin and `aarch64`/`x86_64` Linux (glibc), writes `SHA256SUMS`, and creates the GitHub Release with the CHANGELOG section as notes.

## Deferred

Each item links the issue that holds its detail.

- Multi-file reviews (#137).
- Code review: git-diff-aware reviews of a diff or commits, not only a mutable untracked file (#138).
- Moving or renaming a reviewed file (#139).
- MCP server, a thin wrapper over `review` should the CLI prove insufficient (#140).
- Severity or categories, and `sync` (#141).
- Data model: nested replies, edit, delete, `ptal schema`, the review log's history (#133).
- Views: inline and side-by-side diffs, older revisions, mark as seen, `as_of` (#135).
- TUI text layout: sub-line scroll, hanging indent, real tab stops, ragged table rows, pipes in code cells, tables in lists, character selection, `NO_COLOR` for the TUI chrome (#131).
- TUI table cells: inline markup (#107).
- TUI: thread editing, resolve/reopen notes, sidebar and resolved toggles, turn-back notification (#132).
- Highlight: code blocks by language, non-markdown files, front matter, a toggle key (#134); themes (#49).
- Distribution: Windows, musl, signing, installers (#136).
