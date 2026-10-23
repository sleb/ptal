//! Persistence: where reviews live on disk and how they are read and written.
//!
//! Layout under the data dir:
//!
//! ```text
//! <data>/<slug>-<hash8>/
//!   review.json   the [`Review`]; replaced atomically, never edited in place
//!   review.lock   advisory lock guarding every read-modify-write
//!   revisions/
//!     <sha256>    raw bytes of each revision written against; never rewritten
//! ```
//!
//! `hash8` is derived from the canonical absolute path of the reviewed file,
//! so the same file always maps to the same review.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::model::{Review, SCHEMA_VERSION};

const REVIEW_FILE: &str = "review.json";
const TEMP_FILE: &str = "review.json.tmp";
const LOCK_FILE: &str = "review.lock";
const REVISIONS_DIR: &str = "revisions";
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Resolve the data dir from an environment lookup.
///
/// `PTAL_DATA_DIR`, then `XDG_DATA_HOME/ptal`, then `$HOME/.local/share/ptal`.
/// Empty variables count as unset. Deliberately not `dirs::data_dir()`, which
/// would pick `~/Library/Application Support` on macOS.
pub fn resolve_data_dir(get: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    let get = |key: &str| get(key).filter(|v| !v.is_empty());
    if let Some(dir) = get("PTAL_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = get("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg).join("ptal"));
    }
    if let Some(home) = get("HOME") {
        return Ok(PathBuf::from(home).join(".local/share/ptal"));
    }
    Err(Error::NoDataDir)
}

/// Handle on the data dir. Cheap to create; holds no open files.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    lock_timeout: Duration,
}

impl Store {
    /// A store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// A store rooted at the data dir from the process environment.
    pub fn from_env() -> Result<Self> {
        resolve_data_dir(|key| std::env::var(key).ok()).map(Self::new)
    }

    /// Change how long [`Store::with_review`] waits for a contended lock.
    #[must_use]
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// The directory holding the review of `file`.
    ///
    /// # Errors
    /// [`Error::FileUnreadable`] if `file` cannot be canonicalized.
    pub fn review_dir(&self, file: &Path) -> Result<PathBuf> {
        Ok(self.dir_for(&canonical(file)?))
    }

    fn dir_for(&self, canonical: &Path) -> PathBuf {
        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if slug.is_empty() {
            slug.push_str("file");
        }
        let hash = content_hash(canonical.as_os_str().as_encoded_bytes());
        self.root.join(format!("{slug}-{}", &hash[..8]))
    }

    /// Read the review of `file`; empty if none has been saved yet.
    ///
    /// Takes no lock: writes replace `review.json` by rename, so a reader sees
    /// either the old or the new file, never a partial one.
    ///
    /// # Errors
    /// [`Error::FileUnreadable`], [`Error::Corrupt`], [`Error::UnsupportedSchema`]
    /// or [`Error::Io`].
    pub fn load(&self, file: &Path) -> Result<Review> {
        self.reader(file)?.load()
    }

    /// A reader of the review of `file`, its location resolved now, once.
    ///
    /// Each [`ReviewReader::load`] then reads only `review.json`, so a
    /// reader that polls keeps working while the reviewed file is briefly
    /// gone (an editor saving by rename).
    ///
    /// # Errors
    /// [`Error::FileUnreadable`] if `file` cannot be canonicalized.
    pub fn reader(&self, file: &Path) -> Result<ReviewReader> {
        let canonical = canonical(file)?;
        Ok(ReviewReader {
            review_path: self.dir_for(&canonical).join(REVIEW_FILE),
            canonical,
        })
    }

    /// The snapshot of the revision whose content hashes to `sha256`, or
    /// `None` if there is none.
    ///
    /// Takes no lock: snapshots are written by rename before the
    /// `review.json` that names them, and never removed.
    ///
    /// # Errors
    /// [`Error::FileUnreadable`] if `file` cannot be canonicalized, or
    /// [`Error::Io`].
    pub fn load_revision(&self, file: &Path, sha256: &str) -> Result<Option<Vec<u8>>> {
        let path = self
            .dir_for(&canonical(file)?)
            .join(REVISIONS_DIR)
            .join(sha256);
        read_optional(&path)
    }

    /// Run `f` as one read-modify-write transaction on the review of `file`.
    ///
    /// Holds an exclusive lock for the duration. `f` may [`Snapshots::keep`]
    /// file contents. If `f` returns `Err`, nothing is written, and an
    /// unchanged review is not rewritten. Otherwise each kept snapshot is
    /// written, then the new review, each atomically and all under the lock,
    /// so any revision a reader finds in `review.json` has its snapshot.
    ///
    /// # Errors
    /// Anything [`Store::load`] returns, [`Error::Locked`] if the lock could
    /// not be taken within the timeout, or whatever `f` returns.
    pub fn with_review<R>(
        &self,
        file: &Path,
        f: impl FnOnce(&mut Review, &mut Snapshots) -> Result<R>,
    ) -> Result<R> {
        let canonical = canonical(file)?;
        let dir = self.dir_for(&canonical);
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let _lock = Lock::acquire(&dir.join(LOCK_FILE), self.lock_timeout)?;

        let review_path = dir.join(REVIEW_FILE);
        let before = read_review(&review_path, &canonical)?;
        let mut review = before.clone();
        let mut snapshots = Snapshots::default();
        let out = f(&mut review, &mut snapshots)?;
        snapshots.write(&dir.join(REVISIONS_DIR))?;
        if review != before {
            write_atomic(&dir, &review)?;
        }
        Ok(out)
    }
}

/// Reads one review again and again without resolving the reviewed file's
/// path each time. Made by [`Store::reader`].
#[derive(Debug, Clone)]
pub struct ReviewReader {
    review_path: PathBuf,
    canonical: PathBuf,
}

impl ReviewReader {
    /// Read the review; empty if none has been saved yet. Takes no lock,
    /// like [`Store::load`].
    ///
    /// # Errors
    /// [`Error::Corrupt`], [`Error::UnsupportedSchema`] or [`Error::Io`].
    pub fn load(&self) -> Result<Review> {
        read_review(&self.review_path, &self.canonical)
    }

    /// The raw bytes of `review.json`, or `None` if none has been saved.
    ///
    /// # Errors
    /// [`Error::Io`].
    pub fn bytes(&self) -> Result<Option<Vec<u8>>> {
        read_optional(&self.review_path)
    }
}

/// File contents staged for snapshotting by a [`Store::with_review`]
/// transaction. Nothing is written unless the transaction succeeds.
#[derive(Debug, Default)]
pub struct Snapshots {
    staged: Vec<(String, Vec<u8>)>,
}

impl Snapshots {
    /// Stage `bytes` as a snapshot and return their [`content_hash`].
    pub fn keep(&mut self, bytes: &[u8]) -> String {
        let sha = content_hash(bytes);
        if !self.staged.iter().any(|(s, _)| *s == sha) {
            self.staged.push((sha.clone(), bytes.to_vec()));
        }
        sha
    }

    /// Write each staged snapshot that is not on disk yet: temp file, fsync,
    /// rename. An existing snapshot is never rewritten.
    fn write(self, dir: &Path) -> Result<()> {
        if self.staged.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(dir).map_err(io_err(dir))?;
        for (sha, bytes) in self.staged {
            let target = dir.join(&sha);
            if target.exists() {
                continue;
            }
            let temp = dir.join(format!("{sha}.tmp"));
            write_atomically(&temp, &target, &bytes)?;
        }
        sync_dir(dir);
        Ok(())
    }
}

/// Lowercase hex SHA-256 of `bytes`: a revision's identity.
///
/// # Examples
/// ```
/// # use ptal::store::content_hash;
/// assert_eq!(content_hash(b"").len(), 64);
/// assert_ne!(content_hash(b"a\n"), content_hash(b"a\r\n"));
/// ```
pub fn content_hash(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        hex.push(char::from(DIGITS[usize::from(b >> 4)]));
        hex.push(char::from(DIGITS[usize::from(b & 0xf)]));
    }
    hex
}

/// Map an I/O failure on `path` to [`Error::Io`]; for `map_err`.
///
/// Borrows `path`; it is copied only when an error is built.
fn io_err(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Run a read, turning `NotFound` into `None` and any other failure into
/// [`Error::Io`] on `path`.
fn optional<T>(path: &Path, read: impl FnOnce(&Path) -> io::Result<T>) -> Result<Option<T>> {
    match read(path) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(path)(e)),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    optional(path, fs::read)
}

fn read_optional_string(path: &Path) -> Result<Option<String>> {
    optional(path, fs::read_to_string)
}

/// Write `bytes` to `temp` (truncating a stale one), fsync, and rename it to
/// `target`. Does not sync the directory; the caller does that once.
fn write_atomically(temp: &Path, target: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(temp).map_err(io_err(temp))?;
    file.write_all(bytes).map_err(io_err(temp))?;
    file.sync_all().map_err(io_err(temp))?;
    drop(file);
    fs::rename(temp, target).map_err(io_err(target))
}

/// Persist renames in `dir`; best effort, not every platform allows it.
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

fn canonical(file: &Path) -> Result<PathBuf> {
    fs::canonicalize(file).map_err(|source| Error::FileUnreadable {
        path: file.to_path_buf(),
        source,
    })
}

fn read_review(review_path: &Path, canonical: &Path) -> Result<Review> {
    let Some(text) = read_optional_string(review_path)? else {
        return Ok(Review::new(canonical.to_string_lossy()));
    };
    let corrupt = |source| Error::Corrupt {
        path: review_path.to_path_buf(),
        source,
    };
    // Check the version before the full parse: a newer file may not fit
    // today's shape.
    #[derive(serde::Deserialize)]
    struct Version {
        schema_version: u32,
    }
    let Version { schema_version } = serde_json::from_str(&text).map_err(corrupt)?;
    if schema_version > SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    serde_json::from_str(&text).map_err(corrupt)
}

fn write_atomic(dir: &Path, review: &Review) -> Result<()> {
    let mut json = serde_json::to_vec_pretty(review).expect("review serializes");
    json.push(b'\n');

    write_atomically(&dir.join(TEMP_FILE), &dir.join(REVIEW_FILE), &json)?;
    sync_dir(dir);
    Ok(())
}

/// Exclusive advisory lock, released when dropped.
struct Lock(File);

impl Lock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(io_err(path))?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                Err(TryLockError::WouldBlock) => return Err(Error::Locked),
                Err(TryLockError::Error(e)) => return Err(io_err(path)(e)),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Closing the file releases the lock regardless; unlock is explicit.
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::model::{Review, SCHEMA_VERSION};
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        data: std::path::PathBuf,
        file: std::path::PathBuf,
        store: Store,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let file = tmp.path().join("plan.md");
        fs::write(&file, "one\ntwo\n").unwrap();
        let store = Store::new(&data);
        Fixture {
            _tmp: tmp,
            data,
            file,
            store,
        }
    }

    // --- io helpers ---

    #[test]
    fn io_err_carries_the_path_and_the_source() {
        let err = io_err(Path::new("/some/file"))(io::Error::from(io::ErrorKind::PermissionDenied));

        match err {
            Error::Io { path, source } => {
                assert_eq!(path, Path::new("/some/file"));
                assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            }
            other => panic!("expected Error::Io, got {other:?}"),
        }
    }

    #[test]
    fn read_optional_is_none_for_a_missing_path_and_bytes_for_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");

        assert!(read_optional(&file).unwrap().is_none());
        fs::write(&file, b"hi").unwrap();
        assert_eq!(read_optional(&file).unwrap().as_deref(), Some(&b"hi"[..]));
    }

    #[test]
    fn read_optional_of_a_directory_is_an_io_error_with_that_path() {
        let tmp = tempfile::tempdir().unwrap();

        match read_optional(tmp.path()) {
            Err(Error::Io { path, .. }) => assert_eq!(path, tmp.path()),
            other => panic!("expected Error::Io, got {other:?}"),
        }
    }

    #[test]
    fn read_optional_string_of_non_utf8_is_an_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        fs::write(&file, [0xff, 0xfe]).unwrap();

        assert!(matches!(
            read_optional_string(&file),
            Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::InvalidData
        ));
        assert!(
            read_optional_string(&tmp.path().join("missing"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn write_atomically_replaces_target_and_a_stale_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let (temp, target) = (tmp.path().join("t.tmp"), tmp.path().join("t"));
        fs::write(&temp, "stale stale stale").unwrap();
        fs::write(&target, "old").unwrap();

        write_atomically(&temp, &target, b"new").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!temp.exists());
    }

    #[test]
    fn write_atomically_into_a_missing_dir_fails_on_the_temp_path() {
        let tmp = tempfile::tempdir().unwrap();
        let temp = tmp.path().join("nope/t.tmp");
        let target = tmp.path().join("nope/t");

        match write_atomically(&temp, &target, b"x") {
            Err(Error::Io { path, .. }) => assert_eq!(path, temp),
            other => panic!("expected Error::Io, got {other:?}"),
        }
        assert!(!target.exists());
    }

    // --- data dir resolution ---

    #[test]
    fn data_dir_prefers_ptal_data_dir() {
        let get = env(&[
            ("PTAL_DATA_DIR", "/p"),
            ("XDG_DATA_HOME", "/x"),
            ("HOME", "/h"),
        ]);
        assert_eq!(resolve_data_dir(get).unwrap(), Path::new("/p"));
    }

    #[test]
    fn data_dir_falls_back_to_xdg_data_home() {
        let get = env(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(resolve_data_dir(get).unwrap(), Path::new("/x/ptal"));
    }

    #[test]
    fn data_dir_falls_back_to_home_local_share() {
        let get = env(&[("HOME", "/h")]);
        assert_eq!(
            resolve_data_dir(get).unwrap(),
            Path::new("/h/.local/share/ptal")
        );
    }

    #[test]
    fn data_dir_treats_empty_vars_as_unset() {
        let get = env(&[("PTAL_DATA_DIR", ""), ("XDG_DATA_HOME", ""), ("HOME", "/h")]);
        assert_eq!(
            resolve_data_dir(get).unwrap(),
            Path::new("/h/.local/share/ptal")
        );
    }

    #[test]
    fn data_dir_errors_when_nothing_is_set() {
        assert!(matches!(resolve_data_dir(env(&[])), Err(Error::NoDataDir)));
    }

    // --- review dir naming ---

    #[test]
    fn review_dir_is_slug_dash_eight_hex() {
        let fx = fixture();
        let dir = fx.store.review_dir(&fx.file).unwrap();

        let name = dir.file_name().unwrap().to_str().unwrap();
        let (slug, hash) = name.rsplit_once('-').unwrap();
        assert_eq!(slug, "plan.md");
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(dir.parent().unwrap(), fx.data);
    }

    #[test]
    fn review_dir_slug_is_sanitized() {
        let fx = fixture();
        let odd = fx.file.with_file_name("my plan (v2)é.md");
        fs::write(&odd, "x").unwrap();

        let dir = fx.store.review_dir(&odd).unwrap();

        let name = dir.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("my_plan__v2__.md-"), "{name}");
    }

    #[test]
    fn same_file_via_symlink_or_dot_path_maps_to_one_dir() {
        let fx = fixture();
        let link = fx.file.with_file_name("link.md");
        std::os::unix::fs::symlink(&fx.file, &link).unwrap();
        let dotted = fx.file.parent().unwrap().join(".").join("plan.md");

        let direct = fx.store.review_dir(&fx.file).unwrap();

        assert_eq!(fx.store.review_dir(&link).unwrap(), direct);
        assert_eq!(fx.store.review_dir(&dotted).unwrap(), direct);
    }

    #[test]
    fn same_name_in_different_directories_maps_to_different_dirs() {
        let fx = fixture();
        let other_dir = fx.file.parent().unwrap().join("sub");
        fs::create_dir(&other_dir).unwrap();
        let other = other_dir.join("plan.md");
        fs::write(&other, "x").unwrap();

        assert_ne!(
            fx.store.review_dir(&fx.file).unwrap(),
            fx.store.review_dir(&other).unwrap()
        );
    }

    #[test]
    fn review_dir_for_missing_file_is_file_unreadable() {
        let fx = fixture();
        let missing = fx.file.with_file_name("nope.md");

        assert!(matches!(
            fx.store.review_dir(&missing),
            Err(Error::FileUnreadable { .. })
        ));
    }

    // --- load / save ---

    #[test]
    fn load_of_missing_review_is_empty_and_writes_nothing() {
        let fx = fixture();

        let review = fx.store.load(&fx.file).unwrap();

        assert!(review.threads.is_empty());
        assert_eq!(review.next_id, 1);
        assert_eq!(review.schema_version, SCHEMA_VERSION);
        assert!(!fx.data.exists());
    }

    #[test]
    fn saved_review_loads_back_identical() {
        let fx = fixture();
        let id = fx
            .store
            .with_review(&fx.file, |r, _| Ok(r.next_comment_id()))
            .unwrap();

        let review = fx.store.load(&fx.file).unwrap();

        assert_eq!(id, "c1");
        assert_eq!(review.next_id, 2);
        let canonical = fs::canonicalize(&fx.file).unwrap();
        assert_eq!(review.path, canonical.to_string_lossy());
    }

    // --- atomic writes ---

    #[test]
    fn stale_temp_file_is_ignored_and_replaced() {
        let fx = fixture();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("review.json.tmp"), "{ half a wri").unwrap();

        fx.store
            .with_review(&fx.file, |r, _| Ok(r.next_comment_id()))
            .unwrap();

        let text = fs::read_to_string(dir.join("review.json")).unwrap();
        serde_json::from_str::<Review>(&text).unwrap();
        assert!(!dir.join("review.json.tmp").exists());
    }

    #[test]
    fn corrupt_review_file_is_reported() {
        let fx = fixture();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("review.json"), "not json").unwrap();

        assert!(matches!(
            fx.store.load(&fx.file),
            Err(Error::Corrupt { .. })
        ));
    }

    // --- schema version ---

    #[test]
    fn newer_schema_version_is_refused_and_file_untouched() {
        let fx = fixture();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        fs::create_dir_all(&dir).unwrap();
        let future = format!(
            r#"{{"schema_version":{},"path":"x","next_id":1,"threads":[],"new_field":true}}"#,
            SCHEMA_VERSION + 1
        );
        fs::write(dir.join("review.json"), &future).unwrap();

        let loaded = fx.store.load(&fx.file);
        let written = fx
            .store
            .with_review(&fx.file, |r, _| Ok(r.next_comment_id()));

        for result in [loaded.map(|_| ()), written.map(|_| ())] {
            assert!(matches!(
                result,
                Err(Error::UnsupportedSchema {
                    found: 2,
                    supported: 1
                })
            ));
        }
        assert_eq!(fs::read_to_string(dir.join("review.json")).unwrap(), future);
    }

    // --- with_review semantics ---

    #[test]
    fn closure_error_writes_nothing() {
        let fx = fixture();

        let result: crate::error::Result<()> = fx.store.with_review(&fx.file, |r, _| {
            r.next_comment_id();
            Err(Error::EmptyBody)
        });

        assert!(matches!(result, Err(Error::EmptyBody)));
        let dir = fx.store.review_dir(&fx.file).unwrap();
        assert!(!dir.join("review.json").exists());
    }

    #[test]
    fn closure_error_leaves_existing_review_unchanged() {
        let fx = fixture();
        fx.store
            .with_review(&fx.file, |r, _| Ok(r.next_comment_id()))
            .unwrap();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        let before = fs::read(dir.join("review.json")).unwrap();

        let _ = fx
            .store
            .with_review(&fx.file, |r, _| -> crate::error::Result<()> {
                r.next_comment_id();
                Err(Error::EmptyBody)
            });

        assert_eq!(fs::read(dir.join("review.json")).unwrap(), before);
    }

    // --- locking ---

    #[test]
    fn contended_lock_times_out_with_locked_then_is_released() {
        let fx = fixture();
        let holder = Store::new(&fx.data);
        let waiter = Store::new(&fx.data).with_lock_timeout(Duration::from_millis(100));
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        std::thread::scope(|s| {
            let file = &fx.file;
            let handle = s.spawn(move || {
                holder
                    .with_review(file, |_, _| {
                        locked_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap();
            });
            locked_rx.recv().unwrap();

            let blocked = waiter.with_review(&fx.file, |_, _| Ok(()));
            assert!(matches!(blocked, Err(Error::Locked)));

            release_tx.send(()).unwrap();
            handle.join().unwrap();
        });

        waiter.with_review(&fx.file, |_, _| Ok(())).unwrap();
    }

    #[test]
    fn unchanged_review_is_not_written() {
        let fx = fixture();

        fx.store.with_review(&fx.file, |_, _| Ok(())).unwrap();

        let dir = fx.store.review_dir(&fx.file).unwrap();
        assert!(!dir.join("review.json").exists());
    }

    // --- snapshots ---

    fn snapshot_path(fx: &Fixture, bytes: &[u8]) -> std::path::PathBuf {
        let dir = fx.store.review_dir(&fx.file).unwrap();
        dir.join("revisions").join(content_hash(bytes))
    }

    #[test]
    fn content_hash_is_lowercase_hex_sha256() {
        assert_eq!(
            content_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn content_hash_pads_each_byte_to_two_digits() {
        // Search for an input whose digest has a byte below 0x10, so the
        // zero padding is exercised.
        let input = (0u32..)
            .map(|n| n.to_string().into_bytes())
            .find(|b| Sha256::digest(b).iter().any(|&x| x < 0x10))
            .unwrap();
        let hex = content_hash(&input);

        assert_eq!(hex.len(), 64);
        for (i, b) in Sha256::digest(&input).iter().enumerate() {
            assert_eq!(&hex[2 * i..2 * i + 2], format!("{b:02x}"));
        }
    }

    #[test]
    fn kept_content_is_snapshotted_with_exactly_those_bytes() {
        let fx = fixture();

        let sha = fx
            .store
            .with_review(&fx.file, |r, snapshots| {
                r.next_comment_id();
                Ok(snapshots.keep(b"one\r\ntwo\n"))
            })
            .unwrap();

        assert_eq!(sha, content_hash(b"one\r\ntwo\n"));
        let path = snapshot_path(&fx, b"one\r\ntwo\n");
        assert_eq!(fs::read(&path).unwrap(), b"one\r\ntwo\n");
        assert_eq!(
            fx.store.load_revision(&fx.file, &sha).unwrap().as_deref(),
            Some(&b"one\r\ntwo\n"[..])
        );
    }

    #[test]
    fn closure_error_after_keep_writes_no_snapshot_and_no_review() {
        let fx = fixture();

        let result: crate::error::Result<()> = fx.store.with_review(&fx.file, |r, snapshots| {
            snapshots.keep(b"one\n");
            r.next_comment_id();
            Err(Error::EmptyBody)
        });

        assert!(matches!(result, Err(Error::EmptyBody)));
        let dir = fx.store.review_dir(&fx.file).unwrap();
        assert!(!dir.join("revisions").exists());
        assert!(!dir.join("review.json").exists());
    }

    #[test]
    fn keeping_the_same_content_twice_leaves_one_unrewritten_file() {
        let fx = fixture();
        let keep = |fx: &Fixture| {
            fx.store
                .with_review(&fx.file, |r, snapshots| {
                    r.next_comment_id();
                    Ok(snapshots.keep(b"same"))
                })
                .unwrap()
        };
        keep(&fx);
        let path = snapshot_path(&fx, b"same");
        let first = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));

        keep(&fx);

        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), first);
        let entries = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
    }

    #[test]
    fn locked_review_writes_no_snapshot() {
        let fx = fixture();
        let holder = Store::new(&fx.data);
        let waiter = Store::new(&fx.data).with_lock_timeout(Duration::from_millis(50));
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        std::thread::scope(|s| {
            let file = &fx.file;
            let handle = s.spawn(move || {
                holder
                    .with_review(file, |_, _| {
                        locked_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap();
            });
            locked_rx.recv().unwrap();

            let blocked = waiter.with_review(&fx.file, |r, snapshots| {
                r.next_comment_id();
                Ok(snapshots.keep(b"never"))
            });
            assert!(matches!(blocked, Err(Error::Locked)));
            assert!(!snapshot_path(&fx, b"never").exists());

            release_tx.send(()).unwrap();
            handle.join().unwrap();
        });
    }

    #[test]
    fn stale_snapshot_temp_file_is_overwritten() {
        let fx = fixture();
        let path = snapshot_path(&fx, b"fresh");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let temp = path.with_extension("tmp");
        fs::write(&temp, "half a wri").unwrap();

        fx.store
            .with_review(&fx.file, |r, snapshots| {
                r.next_comment_id();
                Ok(snapshots.keep(b"fresh"))
            })
            .unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"fresh");
        assert!(!temp.exists());
    }

    #[test]
    fn load_revision_of_an_unknown_hash_is_none() {
        let fx = fixture();

        assert_eq!(
            fx.store
                .load_revision(&fx.file, &content_hash(b"x"))
                .unwrap(),
            None
        );
        assert!(!fx.data.exists());
    }

    #[test]
    fn snapshotting_never_touches_the_reviewed_file() {
        let fx = fixture();
        let bytes = fs::read(&fx.file).unwrap();
        let mtime = fs::metadata(&fx.file).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));

        fx.store
            .with_review(&fx.file, |r, snapshots| {
                r.next_comment_id();
                Ok(snapshots.keep(&bytes))
            })
            .unwrap();

        assert_eq!(fs::read(&fx.file).unwrap(), bytes);
        assert_eq!(fs::metadata(&fx.file).unwrap().modified().unwrap(), mtime);
    }
}
