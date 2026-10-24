# `store`

**Role:** persistence. Reads and writes `Review`s. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the layering.

## Owns

### Layout
The data dir is `$PTAL_DATA_DIR`, else `$XDG_DATA_HOME/ptal`, else `~/.local/share/ptal`. Empty variables count as unset.

```
<data>/<slug>-<hash8>/        slug = file name sanitized to [A-Za-z0-9._-];
                              hash8 = of the canonical absolute path
  review.json                 Review (schema_version, path, next_id, revisions, threads, log)
  review.lock                 advisory lock file; never renamed or replaced
  revisions/<sha256>          raw bytes of each revision written against
```

### Identity
- Review identity is the canonicalized absolute path of the file, so a symlink or `./` path reaches the same review.
- Moving the file starts a new review.

## Atomicity and locking
- Writes are atomic: write `review.json.tmp`, fsync, rename. A crash leaves the old or the new `review.json`, never a partial one. A stale temp file is overwritten by the next write.
- Every mutation is read-modify-write under an exclusive advisory lock on `review.lock`.
- The lock is a separate file because `review.json` is replaced by rename, which would orphan a lock held on it.
- Acquisition retries for ~5s, then fails with `Locked`.
- A `review.json` with a newer `schema_version` is refused (`UnsupportedSchema`) and left untouched.
- An unchanged review is not rewritten.

## Snapshots
- A snapshot is named by the SHA-256 of the file's raw bytes, so a change of line endings alone is a new revision.
- It is written as `<sha>.tmp`, fsynced, then renamed into place.
- An existing snapshot is never rewritten, and none is ever removed.
- The reviewed file itself is only ever read.

## Invariants
- Snapshots kept by a transaction are written after its closure returns `Ok` and **before** `review.json` is renamed, all under the lock. A reader that sees `rN` in `review.json` always finds its snapshot.
- An `Err` from the closure writes nothing, snapshots included.

## Contract
- `with_review(file, |&mut Review, &mut Snapshots| -> Result<R>) -> Result<R>` is the only mutation primitive.
- `Snapshots::keep(bytes)` stages a snapshot and returns its hash.
- `load(file)` and `load_revision(file, sha) -> Option<bytes>` take no lock: rename makes each read a consistent snapshot.
- A review with no `review.json` loads as empty without touching disk.
- `reader(file) -> ReviewReader` resolves the review's location once. `ReviewReader::load()` then reads only `review.json` (`load` is `reader(file)?.load()`).
- `ReviewReader::bytes()` returns the raw bytes of `review.json` (`None` before the first write). A poller therefore survives the reviewed file briefly missing while an editor saves it by rename.
- `content_hash(bytes)` is the one hashing rule, shared with `review`.

## Tests
- Data-dir and review-dir naming, stale-temp recovery, schema refusal and lock contention.
- Snapshots hold exact bytes, are not written on `Err` or `Locked`, are never rewritten, and leave the reviewed file's bytes and mtime alone.
