//! Persistence: where reviews live on disk and how they are read and written.
//!
//! Layout under the data dir:
//!
//! ```text
//! <data>/<slug>-<hash8>/
//!   review.json   the [`Review`]; replaced atomically, never edited in place
//!   review.lock   advisory lock guarding every read-modify-write
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
        let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
        let hash8: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        self.root.join(format!("{slug}-{hash8}"))
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
        let canonical = canonical(file)?;
        read_review(&self.dir_for(&canonical).join(REVIEW_FILE), &canonical)
    }

    /// Run `f` as one read-modify-write transaction on the review of `file`.
    ///
    /// Holds an exclusive lock for the duration. If `f` returns `Err`, nothing
    /// is written, and an unchanged review is not rewritten. Otherwise the new
    /// review is written atomically.
    ///
    /// # Errors
    /// Anything [`Store::load`] returns, [`Error::Locked`] if the lock could
    /// not be taken within the timeout, or whatever `f` returns.
    pub fn with_review<R>(
        &self,
        file: &Path,
        f: impl FnOnce(&mut Review) -> Result<R>,
    ) -> Result<R> {
        let canonical = canonical(file)?;
        let dir = self.dir_for(&canonical);
        fs::create_dir_all(&dir).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        let _lock = Lock::acquire(&dir.join(LOCK_FILE), self.lock_timeout)?;

        let review_path = dir.join(REVIEW_FILE);
        let before = read_review(&review_path, &canonical)?;
        let mut review = before.clone();
        let out = f(&mut review)?;
        if review != before {
            write_atomic(&dir, &review)?;
        }
        Ok(out)
    }
}

fn canonical(file: &Path) -> Result<PathBuf> {
    fs::canonicalize(file).map_err(|source| Error::FileUnreadable {
        path: file.to_path_buf(),
        source,
    })
}

fn read_review(review_path: &Path, canonical: &Path) -> Result<Review> {
    let text = match fs::read_to_string(review_path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Review::new(canonical.to_string_lossy()));
        }
        Err(source) => {
            return Err(Error::Io {
                path: review_path.to_path_buf(),
                source,
            });
        }
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
    let io_err = |path: &Path| {
        let path = path.to_path_buf();
        move |source| Error::Io { path, source }
    };
    let temp = dir.join(TEMP_FILE);
    let target = dir.join(REVIEW_FILE);
    let mut json = serde_json::to_vec_pretty(review).expect("review serializes");
    json.push(b'\n');

    let mut file = File::create(&temp).map_err(io_err(&temp))?;
    file.write_all(&json).map_err(io_err(&temp))?;
    file.sync_all().map_err(io_err(&temp))?;
    drop(file);
    fs::rename(&temp, &target).map_err(io_err(&target))?;
    // Persist the rename itself; best effort, not every platform allows it.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Exclusive advisory lock, released when dropped.
struct Lock(File);

impl Lock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self> {
        let io_err = |source| Error::Io {
            path: path.to_path_buf(),
            source,
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(io_err)?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(LOCK_RETRY_INTERVAL);
                }
                Err(TryLockError::WouldBlock) => return Err(Error::Locked),
                Err(TryLockError::Error(e)) => return Err(io_err(e)),
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
            .with_review(&fx.file, |r| Ok(r.next_comment_id()))
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
            .with_review(&fx.file, |r| Ok(r.next_comment_id()))
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
        let written = fx.store.with_review(&fx.file, |r| Ok(r.next_comment_id()));

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

        let result: crate::error::Result<()> = fx.store.with_review(&fx.file, |r| {
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
            .with_review(&fx.file, |r| Ok(r.next_comment_id()))
            .unwrap();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        let before = fs::read(dir.join("review.json")).unwrap();

        let _ = fx
            .store
            .with_review(&fx.file, |r| -> crate::error::Result<()> {
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
                    .with_review(file, |_| {
                        locked_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap();
            });
            locked_rx.recv().unwrap();

            let blocked = waiter.with_review(&fx.file, |_| Ok(()));
            assert!(matches!(blocked, Err(Error::Locked)));

            release_tx.send(()).unwrap();
            handle.join().unwrap();
        });

        waiter.with_review(&fx.file, |_| Ok(())).unwrap();
    }

    #[test]
    fn unchanged_review_is_not_written() {
        let fx = fixture();

        fx.store.with_review(&fx.file, |_| Ok(())).unwrap();

        let dir = fx.store.review_dir(&fx.file).unwrap();
        assert!(!dir.join("review.json").exists());
    }
}
