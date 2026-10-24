# Releasing ptal

The procedure for cutting a release. The maintainer runs it by hand from a local
checkout (an agent can drive the commands when asked). `cargo publish` is manual
and local; the GitHub Release and its binaries are built by
`.github/workflows/release.yml` when the tag is pushed.

Replace `X.Y.Z` with the new version and `PREV` with the previous one.

## 0.x semver policy

While the version is `0.y.z`, a minor bump (`0.y`) may break anything: CLI
JSON, exit codes, the review store format, TUI keys. A patch bump is fixes
only. A `schema_version` bump is called out in the notes. 1.0 is cut when the
CLI JSON surface is intended to be stable.

## 1. Preconditions

```sh
git switch main && git pull --ff-only
git status --porcelain                      # expect: empty
gh run list --branch main --limit 1         # expect: completed  success
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo package --locked
knap lint .                                 # expect: 0 problem(s)
```

Every story issue in the milestone is closed with its acceptance criteria
ticked. The milestone issue's own `Done means` boxes are checked in step 9,
since most of them only become true as the release completes.

The crates.io token needs the `publish-update` scope and no crate-name
restriction that excludes `ptal`. A token without them fails at upload with
`403 Forbidden`, after the dry run passed. Save a replacement with
`cargo login` in a normal terminal tab: it needs an interactive prompt, so
`! cargo login` from an agent session saves nothing.

## 2. Bump the version

Edit `version` in `Cargo.toml`, then refresh the lockfile and re-check:

```sh
cargo update --workspace --offline          # expect: Updating ptal vPREV -> vX.Y.Z
git diff --stat                             # expect: Cargo.toml, Cargo.lock only
cargo package --locked
```

## 3. Release notes

GitHub generates a draft; you edit it; `CHANGELOG.md` keeps it, and the
workflow publishes that same section as the Release body, so the two cannot
diverge.

```sh
gh api repos/sleb/ptal/releases/generate-notes \
  -f tag_name=vX.Y.Z -f previous_tag_name=vPREV --jq .body
```

Edit the text, then replace the `## Unreleased` heading in `CHANGELOG.md` with
`## X.Y.Z - YYYY-MM-DD` followed by the notes, and add a fresh empty
`## Unreleased` above it. Commit the bump and notes together and merge through a
PR; the notes are reviewed there.

## 4. Allowlist review

The `include` list in `Cargo.toml` decides what ships. Compare the tarball file
list against the previous release:

```sh
git switch --detach vPREV && cargo package --list --locked > /tmp/prev.txt
git switch - && cargo package --list --locked > /tmp/new.txt
diff /tmp/prev.txt /tmp/new.txt             # accept every added/removed path, or edit `include`
```

The `include` policy is in [ARCHITECTURE.md](../ARCHITECTURE.md) "Distribution".

## 5. Publish to crates.io

crates.io is irreversible (yank only), so it goes before the tag.

```sh
cargo publish --dry-run --locked
cargo publish --locked                      # needs a crates.io token, local only
```

## 6. Tag and push

Run from the merged release commit on `main`.

```sh
git tag -a vX.Y.Z -m "ptal X.Y.Z"
git push origin vX.Y.Z
```

The tag triggers `release.yml`:

| Job | Does |
|---|---|
| `verify tag` | Fails unless the tag is `v<Cargo.toml version>`, `CHANGELOG.md` has a `## X.Y.Z - ` section, and the commit is on `main`. |
| `build <target>` | `cargo build --release --locked` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`; archives `ptal-X.Y.Z-<target>.tar.gz` (`ptal`, `LICENSE`, `README.md` in one directory). |
| `checksums and release` | Writes `SHA256SUMS`, then `gh release create` with the CHANGELOG section as notes and the archives attached. |

Linux binaries link the runner's glibc and may not run on old distros; those
users run `cargo install ptal`.

## 7. Watch the workflow

```sh
gh run watch "$(gh run list --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')"
gh release view vX.Y.Z --json assets -q '.assets[].name'
# expect: four ptal-X.Y.Z-<target>.tar.gz files and SHA256SUMS
```

Dry run (build and archive, no Release; only once the workflow is on `main`):

```sh
gh workflow run release.yml -f tag=vX.Y.Z -f dry_run=true
```

## 8. Verify

crates.io's index lags by a few minutes: retry before treating a failure as real.

```sh
root=$(mktemp -d)
cargo install ptal --version X.Y.Z --locked --root "$root"
"$root/bin/ptal" --version                  # expect: ptal X.Y.Z
"$root/bin/ptal" --help

d=$(mktemp -d) && cd "$d"
gh release download vX.Y.Z --repo sleb/ptal
shasum -a 256 -c SHA256SUMS --ignore-missing   # expect: OK for each downloaded archive
```

Also open https://crates.io/crates/ptal and https://docs.rs/ptal and confirm
the version and a successful docs build. `https://docs.rs/ptal/X.Y.Z/ptal/`
returning `200` means the build succeeded. `crate/ptal/X.Y.Z/builds.json`
returned HTML, not JSON, when tried, so don't script against it.

## 9. Close out the milestone

The release is done only when its tracking is too. Every story issue in the
milestone must be closed with its acceptance criteria ticked, and every box in
the milestone issue's `Done means` list must be ticked, including the green
gate on the release commit and this procedure having been followed.

```sh
milestone=$(gh api repos/sleb/ptal/milestones \
  --jq '.[] | select(.title=="vX.Y.Z") | .number')
gh issue list --milestone vX.Y.Z --state all
for n in $(gh issue list --milestone vX.Y.Z --state all --json number -q '.[].number'); do
  unticked=$(gh issue view "$n" --json body -q .body | grep -c '^[[:space:]]*- \[ \]' || true)
  echo "#$n: $unticked unticked"
done
```

Tick each box that is genuinely done (`gh issue edit <n> --body-file body.md`
with the edited body), and open an issue for anything that is not. Then close
the milestone issue and the milestone itself:

```sh
gh issue close <milestone-issue>
gh api -X PATCH "repos/sleb/ptal/milestones/$milestone" -f state=closed
```

## 10. Failure handling

| Situation | Action |
|---|---|
| Publish failed | Fix and rerun step 5; nothing is tagged yet. |
| Published, tag not pushed | Tag the same commit (step 6). |
| Tag pushed, workflow failed | Re-run the failed jobs: `gh run rerun <id> --failed`. Delete and re-push the tag only if the commit was wrong: `git push origin :refs/tags/vX.Y.Z`, then retag. |
| Workflow cannot be fixed | `gh release create vX.Y.Z --verify-tag --notes-file notes.md <archives> SHA256SUMS`, with `notes.md` the CHANGELOG section and archives built locally with `cargo build --release --locked`. |

### Yank policy

Yank only for a broken build or install, data loss or corruption in the review
store, or a security issue. Never for a cosmetic or doc bug: fix forward with a
patch release.

```sh
cargo yank --version X.Y.Z          # undo: cargo yank --version X.Y.Z --undo
```

Yanking does not remove the tag or Release: edit the Release notes to say it
is yanked and why. A published version can never be overwritten or deleted.
