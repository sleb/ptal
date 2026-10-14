//! The crate-wide error type. Mapping to exit codes is the CLI's job.

use std::io;
use std::path::PathBuf;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No thread with this id exists in the review.
    #[error("thread {0} not found")]
    NotFound(String),

    /// The thread is already open, so there is nothing to reopen.
    #[error("thread {0} is already open")]
    AlreadyOpen(String),

    /// The thread is already resolved, so there is nothing to resolve.
    #[error("thread {0} is already resolved")]
    AlreadyResolved(String),

    /// The requested line range is not valid for the file.
    #[error("invalid line range {start}-{end}: {reason}")]
    InvalidRange {
        start: u32,
        end: u32,
        reason: &'static str,
    },

    /// The reviewed file is missing, unreadable, or not valid UTF-8.
    #[error("cannot read {}: {source}", path.display())]
    FileUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Another process held the review lock for longer than the timeout.
    #[error("review is locked by another process")]
    Locked,

    /// `review.json` was written by a newer ptal.
    #[error("review schema version {found} is newer than the supported {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },

    /// Every comment needs a non-empty author.
    #[error("author must not be empty")]
    EmptyAuthor,

    /// Every comment needs a non-empty body.
    #[error("comment body must not be empty")]
    EmptyBody,

    /// Reading or writing the data directory failed.
    #[error("storage error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// `review.json` exists but is not a valid review.
    #[error("corrupt review file {}: {source}", path.display())]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Neither `PTAL_DATA_DIR`, `XDG_DATA_HOME` nor `HOME` is usable.
    #[error("cannot determine the data directory: set PTAL_DATA_DIR")]
    NoDataDir,
}
