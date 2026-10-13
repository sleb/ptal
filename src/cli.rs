//! The agent-facing command line: parse, dispatch, render, map errors.
//!
//! A thin layer over [`crate::review`]. All rules live in the core; this
//! module only decides what bytes go to which stream and which exit code.

use crate::error::Error;

/// The caller can fix the invocation: usage, author, unknown id, bad range.
const EXIT_CALLER: u8 = 2;
/// Something failed at runtime: unreadable file, lock, storage.
const EXIT_RUNTIME: u8 = 1;

/// A failure the CLI reports: a core error, or one only the CLI can detect.
#[derive(Debug)]
pub enum Failure {
    /// The invocation itself is malformed (clap, `--lines` syntax).
    Usage(String),
    /// A write command had neither `--as` nor `PTAL_AUTHOR`.
    MissingAuthor,
    /// The core refused or failed.
    Core(Error),
}

impl Failure {
    /// Stable machine-readable code and process exit status.
    pub fn classify(&self) -> (&'static str, u8) {
        match self {
            Failure::Usage(_) => ("usage", EXIT_CALLER),
            Failure::MissingAuthor => ("missing_author", EXIT_CALLER),
            Failure::Core(error) => match error {
                Error::EmptyAuthor => ("missing_author", EXIT_CALLER),
                Error::NotFound(_) => ("not_found", EXIT_CALLER),
                Error::InvalidRange { .. } => ("invalid_range", EXIT_CALLER),
                Error::EmptyBody => ("empty_body", EXIT_CALLER),
                Error::FileUnreadable { .. } => ("file_unreadable", EXIT_RUNTIME),
                Error::Locked => ("locked", EXIT_RUNTIME),
                Error::UnsupportedSchema { .. } => ("unsupported_schema", EXIT_RUNTIME),
                Error::Io { .. } => ("io", EXIT_RUNTIME),
                Error::Corrupt { .. } => ("corrupt", EXIT_RUNTIME),
                Error::NoDataDir => ("no_data_dir", EXIT_RUNTIME),
            },
        }
    }

    fn message(&self) -> String {
        match self {
            Failure::Usage(message) => message.clone(),
            Failure::MissingAuthor => "no author: pass --as <name> or set PTAL_AUTHOR".to_string(),
            Failure::Core(error) => error.to_string(),
        }
    }
}

/// The one-line JSON error envelope written to stderr.
pub fn error_json(failure: &Failure) -> String {
    let (code, _) = failure.classify();
    serde_json::json!({ "error": { "code": code, "message": failure.message() } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::path::PathBuf;

    fn io_err() -> io::Error {
        io::Error::other("boom")
    }

    #[test]
    fn every_failure_maps_to_its_documented_code_and_exit() {
        let path = PathBuf::from("/x");
        let corrupt = serde_json::from_str::<u32>("x").unwrap_err();
        let table: Vec<(Failure, &str, u8)> = vec![
            (Failure::Usage("bad".into()), "usage", 2),
            (Failure::MissingAuthor, "missing_author", 2),
            (Failure::Core(Error::EmptyAuthor), "missing_author", 2),
            (Failure::Core(Error::NotFound("c9".into())), "not_found", 2),
            (
                Failure::Core(Error::InvalidRange {
                    start: 1,
                    end: 2,
                    reason: "r",
                }),
                "invalid_range",
                2,
            ),
            (Failure::Core(Error::EmptyBody), "empty_body", 2),
            (
                Failure::Core(Error::FileUnreadable {
                    path: path.clone(),
                    source: io_err(),
                }),
                "file_unreadable",
                1,
            ),
            (Failure::Core(Error::Locked), "locked", 1),
            (
                Failure::Core(Error::UnsupportedSchema {
                    found: 2,
                    supported: 1,
                }),
                "unsupported_schema",
                1,
            ),
            (
                Failure::Core(Error::Io {
                    path: path.clone(),
                    source: io_err(),
                }),
                "io",
                1,
            ),
            (
                Failure::Core(Error::Corrupt {
                    path,
                    source: corrupt,
                }),
                "corrupt",
                1,
            ),
            (Failure::Core(Error::NoDataDir), "no_data_dir", 1),
        ];

        for (failure, code, exit) in table {
            assert_eq!(failure.classify(), (code, exit), "{failure:?}");
        }
    }

    #[test]
    fn error_envelope_is_one_line_of_json() {
        let json = error_json(&Failure::Core(Error::NotFound("c9".into())));

        assert_eq!(
            json,
            r#"{"error":{"code":"not_found","message":"thread c9 not found"}}"#
        );
    }
}
