//! The agent-facing command line: parse, dispatch, render, map errors.
//!
//! A thin layer over [`crate::review`]. All rules live in the core; this
//! module only decides what bytes go to which stream and which exit code.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::error::Error;
use crate::model::Thread;
use crate::review::{Clock, Reviews, SystemClock, ThreadView};
use crate::store::Store;

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

    /// Human-readable description.
    pub fn message(&self) -> String {
        match self {
            Failure::Usage(message) => message.clone(),
            Failure::MissingAuthor => "no author: pass --as <name> or set PTAL_AUTHOR".to_string(),
            Failure::Core(error) => error.to_string(),
        }
    }
}

/// Everything `run` needs from the process, injected so tests can drive every
/// path in-process.
pub struct Env<C: Clock> {
    /// Review operations (store and clock).
    pub reviews: Reviews<C>,
    /// Value of `PTAL_AUTHOR`, if set.
    pub author_env: Option<String>,
    /// Whether stdout is a terminal (selects readable output).
    pub stdout_is_tty: bool,
    /// Whether stderr is a terminal (selects readable errors).
    pub stderr_is_tty: bool,
}

#[derive(Debug, Parser)]
#[command(name = "ptal", version, about = "Review a throwaway text file")]
struct Cli {
    /// Always print JSON, even on a terminal.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the comment threads on a file.
    Comments {
        file: PathBuf,
        /// Include resolved threads.
        #[arg(long)]
        all: bool,
    },
    /// Start a thread on a line or a range of lines.
    Add {
        file: PathBuf,
        /// Line `N` or inclusive range `A-B` (1-based).
        #[arg(long, value_name = "N|A-B", value_parser = parse_lines)]
        lines: (u32, u32),
        /// The comment text.
        body: String,
        /// Who is commenting; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
    },
    /// Mark a thread resolved.
    Resolve {
        file: PathBuf,
        /// Thread id, e.g. `c3`.
        id: String,
        /// Who is resolving; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
        /// Leave a closing comment on the thread.
        #[arg(long)]
        note: Option<String>,
    },
}

/// Parse `A-B` or a bare `N` (sugar for `N-N`). Range validity against the
/// file is the core's call.
fn parse_lines(spec: &str) -> Result<(u32, u32), String> {
    let bad = || format!("invalid line spec {spec:?}: expected N or A-B");
    let number = |s: &str| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        s.parse::<u32>().map_err(|_| bad())
    };
    match spec.split_once('-') {
        None => number(spec).map(|n| (n, n)),
        Some((a, b)) => Ok((number(a)?, number(b)?)),
    }
}

/// The author for a write: `--as`, else `PTAL_AUTHOR`; blank counts as unset.
///
/// # Errors
/// [`Failure::MissingAuthor`] when neither is usable.
pub fn resolve_author(flag: Option<&str>, env: Option<&str>) -> Result<String, Failure> {
    [flag, env]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|a| !a.is_empty())
        .map(String::from)
        .ok_or(Failure::MissingAuthor)
}

/// What a successful command has to say.
enum Output {
    Threads(Vec<ThreadView>),
    Thread(Thread),
}

/// Parse `args` (including the program name), execute, and return the exit
/// status. Data goes to `out`, errors to `err`.
pub fn run<C, I, T>(args: I, env: &Env<C>, out: &mut dyn Write, err: &mut dyn Write) -> u8
where
    C: Clock,
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let json_flag = args.iter().any(|a| a == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) if e.use_stderr() => {
            let text = if env.stderr_is_tty && !json_flag {
                e.to_string()
            } else {
                error_line(&Failure::Usage(e.to_string().trim().to_string()))
            };
            return finish(err, &text, EXIT_CALLER);
        }
        // `--help` and `--version`.
        Err(e) => return finish(out, &e.to_string(), 0),
    };

    let file = match &cli.command {
        Command::Comments { file, .. }
        | Command::Add { file, .. }
        | Command::Resolve { file, .. } => file,
    };
    let result = dispatch(&cli.command, env).and_then(|output| {
        let canonical = std::fs::canonicalize(file).map_err(|source| {
            Failure::Core(Error::FileUnreadable {
                path: file.clone(),
                source,
            })
        })?;
        Ok((canonical, output))
    });
    match result {
        Ok((canonical, output)) => {
            let value = match output {
                Output::Threads(threads) => json!({ "file": canonical, "threads": threads }),
                Output::Thread(thread) => json!({ "file": canonical, "thread": thread }),
            };
            finish(out, &format!("{value}\n"), 0)
        }
        Err(failure) => {
            let (_, code) = failure.classify();
            let text = if env.stderr_is_tty && !cli.json {
                format!("error: {}\n", failure.message())
            } else {
                error_line(&failure)
            };
            finish(err, &text, code)
        }
    }
}

fn dispatch<C: Clock>(command: &Command, env: &Env<C>) -> Result<Output, Failure> {
    let reviews = &env.reviews;
    let author = |flag: &Option<String>| resolve_author(flag.as_deref(), env.author_env.as_deref());
    match command {
        Command::Comments { file, all } => Ok(Output::Threads(reviews.view(file, *all)?)),
        Command::Add {
            file,
            lines: (start, end),
            body,
            author: flag,
        } => {
            let author = author(flag)?;
            Ok(Output::Thread(
                reviews.add_thread(file, *start, *end, body, &author)?,
            ))
        }
        Command::Resolve {
            file,
            id,
            author: flag,
            note,
        } => {
            let author = author(flag)?;
            Ok(Output::Thread(reviews.resolve(
                file,
                id,
                &author,
                note.as_deref(),
            )?))
        }
    }
}

/// Write `text` to `w` and return `code`. A closed pipe ends quietly; any
/// other write failure is a runtime error.
fn finish(w: &mut dyn Write, text: &str, code: u8) -> u8 {
    match w.write_all(text.as_bytes()).and_then(|()| w.flush()) {
        Ok(()) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => code,
        Err(_) => EXIT_RUNTIME,
    }
}

fn error_line(failure: &Failure) -> String {
    format!("{}\n", error_json(failure))
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Failure::Core(error)
    }
}

/// The one-line JSON error envelope written to stderr.
pub fn error_json(failure: &Failure) -> String {
    let (code, _) = failure.classify();
    serde_json::json!({ "error": { "code": code, "message": failure.message() } }).to_string()
}

/// Entry point for the `ptal` binary: read the process environment, run, and
/// return the exit status.
pub fn main() -> ExitCode {
    let stdout_is_tty = io::stdout().is_terminal();
    let stderr_is_tty = io::stderr().is_terminal();
    let store = match Store::from_env() {
        Ok(store) => store,
        Err(error) => {
            let failure = Failure::from(error);
            let text = if stderr_is_tty {
                format!("error: {}\n", failure.message())
            } else {
                error_line(&failure)
            };
            let (_, code) = failure.classify();
            return ExitCode::from(finish(&mut io::stderr(), &text, code));
        }
    };
    let env = Env {
        reviews: Reviews::with_clock(store, SystemClock),
        author_env: std::env::var("PTAL_AUTHOR").ok(),
        stdout_is_tty,
        stderr_is_tty,
    };
    let code = run(
        std::env::args_os(),
        &env,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
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

    // ---- end-to-end, in-process -------------------------------------------

    use std::fs;

    const NOW: &str = "2026-10-12T10:00:00Z";

    struct Fixture {
        _tmp: tempfile::TempDir,
        data: PathBuf,
        file: PathBuf,
    }

    struct Out {
        code: u8,
        stdout: String,
        stderr: String,
    }

    impl Out {
        fn json(&self) -> serde_json::Value {
            assert!(self.stdout.ends_with('\n'), "{:?}", self.stdout);
            assert_eq!(self.stdout.lines().count(), 1, "{:?}", self.stdout);
            serde_json::from_str(&self.stdout).unwrap()
        }

        fn error_code(&self) -> String {
            assert_eq!(self.stderr.lines().count(), 1, "{:?}", self.stderr);
            let v: serde_json::Value = serde_json::from_str(&self.stderr).unwrap();
            v["error"]["code"].as_str().unwrap().to_string()
        }
    }

    fn fixture(content: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("plan.md");
        fs::write(&file, content).unwrap();
        let data = tmp.path().join("data");
        Fixture {
            _tmp: tmp,
            data,
            file,
        }
    }

    impl Fixture {
        fn env(&self, author: Option<&str>, tty: bool) -> Env<fn() -> String> {
            let clock: fn() -> String = || NOW.to_string();
            Env {
                reviews: Reviews::with_clock(Store::new(&self.data), clock),
                author_env: author.map(String::from),
                stdout_is_tty: tty,
                stderr_is_tty: tty,
            }
        }

        /// Run with piped streams and no `PTAL_AUTHOR`; `FILE` is replaced
        /// by the fixture's path.
        fn run(&self, args: &[&str]) -> Out {
            self.run_with(args, &self.env(None, false))
        }

        fn run_with(&self, args: &[&str], env: &Env<fn() -> String>) -> Out {
            let path = self.file.to_str().unwrap();
            let argv = std::iter::once("ptal")
                .chain(args.iter().copied())
                .map(|a| if a == "FILE" { path } else { a });
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(argv, env, &mut out, &mut err);
            Out {
                code,
                stdout: String::from_utf8(out).unwrap(),
                stderr: String::from_utf8(err).unwrap(),
            }
        }

        fn canonical(&self) -> String {
            fs::canonicalize(&self.file)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        }

        fn review_bytes(&self) -> Vec<u8> {
            let dir = Store::new(&self.data).review_dir(&self.file).unwrap();
            fs::read(dir.join("review.json")).unwrap_or_default()
        }
    }

    fn fixture_with_two_threads() -> Fixture {
        let fx = fixture("one\ntwo\nthree\n");
        let reviews =
            Reviews::with_clock(Store::new(&fx.data), (|| NOW.to_string()) as fn() -> String);
        reviews
            .add_thread(&fx.file, 1, 1, "Typo?", "scott")
            .unwrap();
        reviews.add_thread(&fx.file, 2, 3, "Why?", "scott").unwrap();
        fx
    }

    #[test]
    fn comments_lists_open_threads_only_as_one_json_document() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);

        let out = fx.run(&["comments", "FILE"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.stderr, "");
        assert_eq!(
            out.json(),
            serde_json::json!({
                "file": fx.canonical(),
                "threads": [{
                    "id": "c2",
                    "anchor": {"start_line": 2, "end_line": 3, "quote": "two\nthree"},
                    "status": "open",
                    "comments": [{"id": "c2", "author": "scott", "body": "Why?", "created_at": NOW}],
                    "state": "current",
                }],
            })
        );
    }

    #[test]
    fn comments_all_includes_resolved_with_who_and_when() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);

        let out = fx.run(&["comments", "FILE", "--all"]);

        let json = out.json();
        let threads = json["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0]["status"], "resolved");
        assert_eq!(threads[0]["resolved_by"], "claude");
        assert_eq!(threads[0]["resolved_at"], NOW);
        assert_eq!(threads[1]["status"], "open");
    }

    #[test]
    fn comments_shows_outdated_next_to_current() {
        let fx = fixture_with_two_threads();
        fs::write(&fx.file, "ONE\ntwo\nthree\n").unwrap();

        let out = fx.run(&["comments", "FILE"]);

        let json = out.json();
        assert_eq!(json["threads"][0]["state"], "outdated");
        assert_eq!(json["threads"][1]["state"], "current");
    }

    #[test]
    fn comments_with_no_threads_is_an_empty_list() {
        let fx = fixture("one\n");

        let out = fx.run(&["comments", "FILE"]);

        assert_eq!(
            out.json(),
            serde_json::json!({"file": fx.canonical(), "threads": []})
        );
    }

    #[test]
    fn comments_on_a_missing_file_fails_with_empty_stdout() {
        let fx = fixture("one\n");
        fs::remove_file(&fx.file).unwrap();

        let out = fx.run(&["comments", "FILE"]);

        assert_eq!(out.code, 1);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "file_unreadable");
    }

    #[test]
    fn author_flag_wins_then_env_then_missing() {
        assert_eq!(resolve_author(Some("flag"), Some("env")).unwrap(), "flag");
        assert_eq!(resolve_author(None, Some("env")).unwrap(), "env");
        assert!(matches!(
            resolve_author(None, None),
            Err(Failure::MissingAuthor)
        ));
        assert!(matches!(
            resolve_author(Some("  "), Some("")),
            Err(Failure::MissingAuthor)
        ));
    }

    #[test]
    fn add_prints_the_new_thread_and_comments_lists_it() {
        let fx = fixture("one\ntwo\nthree\n");

        let out = fx.run(&["add", "FILE", "--lines", "2-3", "Why?", "--as", "scott"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(
            out.json(),
            serde_json::json!({
                "file": fx.canonical(),
                "thread": {
                    "id": "c1",
                    "anchor": {"start_line": 2, "end_line": 3, "quote": "two\nthree"},
                    "status": "open",
                    "comments": [{"id": "c1", "author": "scott", "body": "Why?", "created_at": NOW}],
                },
            })
        );
        let listed = fx.run(&["comments", "FILE"]).json();
        assert_eq!(listed["threads"][0]["id"], "c1");
    }

    #[test]
    fn add_without_an_author_exits_2_and_writes_nothing() {
        let fx = fixture("one\n");

        let out = fx.run(&["add", "FILE", "--lines", "1", "Hi"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "missing_author");
        assert!(fx.review_bytes().is_empty());
    }

    #[test]
    fn add_takes_the_author_from_the_environment() {
        let fx = fixture("one\n");

        let out = fx.run_with(
            &["add", "FILE", "--lines", "1", "Hi"],
            &fx.env(Some("claude"), false),
        );

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.json()["thread"]["comments"][0]["author"], "claude");
    }

    #[test]
    fn add_past_end_of_file_is_invalid_range_and_writes_nothing() {
        let fx = fixture("one\ntwo\n");

        let out = fx.run(&["add", "FILE", "--lines", "5-9", "Hi", "--as", "scott"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "invalid_range");
        assert!(fx.review_bytes().is_empty());
    }

    #[test]
    fn add_a_bare_line_number_is_a_one_line_range() {
        let fx = fixture("one\ntwo\nthree\n");

        let out = fx.run(&["add", "FILE", "--lines", "3", "Hi", "--as", "scott"]);

        let anchor = &out.json()["thread"]["anchor"];
        assert_eq!(anchor["start_line"], 3);
        assert_eq!(anchor["end_line"], 3);
    }

    #[test]
    fn add_rejects_malformed_line_specs_as_usage() {
        let fx = fixture("one\ntwo\nthree\n");

        for spec in ["3-", "b-a", "1-2-3", "", "a"] {
            let arg = format!("--lines={spec}");
            let out = fx.run(&["add", "FILE", &arg, "Hi", "--as", "scott"]);
            assert_eq!(out.code, 2, "{spec:?}");
            assert_eq!(out.error_code(), "usage", "{spec:?}");
        }
    }

    #[test]
    fn add_with_an_empty_body_is_empty_body() {
        let fx = fixture("one\n");

        let out = fx.run(&["add", "FILE", "--lines", "1", "  ", "--as", "scott"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.error_code(), "empty_body");
    }

    #[test]
    fn resolve_marks_the_thread_and_hides_it_from_the_default_view() {
        let fx = fixture_with_two_threads();

        let out = fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        let thread = &out.json()["thread"];
        assert_eq!(thread["status"], "resolved");
        assert_eq!(thread["resolved_by"], "claude");
        assert!(thread.get("state").is_none());
        let open = fx.run(&["comments", "FILE"]).json();
        assert_eq!(open["threads"].as_array().unwrap().len(), 1);
        let all = fx.run(&["comments", "FILE", "--all"]).json();
        assert_eq!(all["threads"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn resolve_note_appears_as_a_second_comment() {
        let fx = fixture_with_two_threads();

        fx.run(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Fixed"]);

        let all = fx.run(&["comments", "FILE", "--all"]).json();
        let comments = all["threads"][0]["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[1]["author"], "claude");
        assert_eq!(comments[1]["body"], "Fixed");
    }

    #[test]
    fn resolve_unknown_id_is_not_found_and_leaves_the_review_untouched() {
        let fx = fixture_with_two_threads();
        let before = fx.review_bytes();

        let out = fx.run(&["resolve", "FILE", "c99", "--as", "claude"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "not_found");
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn resolving_twice_succeeds_without_a_duplicate_note() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Fixed"]);

        let again = fx.run(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Again"]);

        assert_eq!(again.code, 0, "{}", again.stderr);
        assert_eq!(
            again.json()["thread"]["comments"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn usage_errors_are_json_on_a_piped_stderr_with_exit_2() {
        let fx = fixture("one\n");

        for args in [
            &["frobnicate"][..],
            &["comments"],
            &["comments", "FILE", "--nope"],
            &[],
        ] {
            let out = fx.run(args);
            assert_eq!(out.code, 2, "{args:?}");
            assert_eq!(out.stdout, "", "{args:?}");
            assert_eq!(out.error_code(), "usage", "{args:?}");
        }
    }

    #[test]
    fn usage_errors_are_clap_text_on_a_tty_stderr() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["frobnicate"], &fx.env(None, true));

        assert_eq!(out.code, 2);
        assert!(out.stderr.contains("frobnicate"));
        assert!(!out.stderr.starts_with('{'));
    }

    #[test]
    fn help_and_version_go_to_stdout_with_exit_0() {
        let fx = fixture("one\n");

        for flag in ["--help", "--version"] {
            let out = fx.run(&[flag]);
            assert_eq!(out.code, 0, "{flag}");
            assert_eq!(out.stderr, "", "{flag}");
            assert!(!out.stdout.is_empty(), "{flag}");
        }
    }
}
