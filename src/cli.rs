//! The agent-facing command line: parse, dispatch, render, map errors.
//!
//! A thin layer over [`crate::review`]. All rules live in the core; this
//! module only decides what bytes go to which stream and which exit code.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::error::Error;
use crate::model::{Anchor, State, Status, Thread};
use crate::review::{Clock, Reviews, SystemClock, ThreadView};
use crate::store::Store;
use crate::tui;

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
    /// `ptal <file>` needs an interactive terminal on stdin and stdout.
    NotATerminal,
    /// The terminal failed while the TUI was running.
    Terminal(io::Error),
    /// The core refused or failed.
    Core(Error),
}

impl Failure {
    /// Stable machine-readable code and process exit status.
    pub fn classify(&self) -> (&'static str, u8) {
        match self {
            Failure::Usage(_) => ("usage", EXIT_CALLER),
            Failure::MissingAuthor => ("missing_author", EXIT_CALLER),
            Failure::NotATerminal => ("not_a_terminal", EXIT_RUNTIME),
            Failure::Terminal(_) => ("terminal", EXIT_RUNTIME),
            Failure::Core(error) => match error {
                Error::EmptyAuthor => ("missing_author", EXIT_CALLER),
                Error::NotFound(_) => ("not_found", EXIT_CALLER),
                Error::AlreadyOpen(_) => ("already_open", EXIT_CALLER),
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
            Failure::NotATerminal => {
                "the interactive view needs a terminal; agents should use `ptal comments`"
                    .to_string()
            }
            Failure::Terminal(error) => format!("terminal error: {error}"),
            Failure::Core(error) => error.to_string(),
        }
    }
}

/// Everything `run` needs from the process, injected so tests can drive every
/// path in-process.
pub struct Env<C: Clock> {
    /// Review operations (store and clock). `None` when no data directory
    /// could be resolved; only commands that need the store then fail, with
    /// `no_data_dir`, so `--help`, `--version` and usage errors still work.
    pub reviews: Option<Reviews<C>>,
    /// Value of `PTAL_AUTHOR`, if set.
    pub author_env: Option<String>,
    /// Value of `USER`, if set: who the TUI writes comments as.
    pub user: Option<String>,
    /// Whether stdin is a terminal (the TUI reads keys from it).
    pub stdin_is_tty: bool,
    /// Whether stdout is a terminal (selects readable output).
    pub stdout_is_tty: bool,
    /// Whether stderr is a terminal (selects readable errors).
    pub stderr_is_tty: bool,
}

// Help text. Examples live here once; `--help` and the docs quote these.

const COMMENTS_EXAMPLE: &str = "\
Examples:
  ptal comments plan.md           # open threads
  ptal comments plan.md --all     # include resolved threads";

const ADD_EXAMPLE: &str = "\
Examples:
  ptal add plan.md --lines 12 \"Why not X?\" --as claude
  ptal add plan.md --lines 12-14 \"Why not X?\" --as claude";

const RESOLVE_EXAMPLE: &str = "\
Examples:
  ptal resolve plan.md c3 --as claude
  ptal resolve plan.md c3 --as claude --note \"Done, see step 2.\"";

const REPLY_EXAMPLE: &str = "\
Examples:
  ptal reply plan.md c3 \"Which of the two do you mean?\" --as claude";

const REOPEN_EXAMPLE: &str = "\
Examples:
  ptal reopen plan.md c3 --as scott
  ptal reopen plan.md c3 --as scott --note \"Not fixed: still fails on empty input.\"";

const WORKFLOW: &str = "\
Review a throwaway text file. A human comments on lines in the TUI, opened
with `ptal <file>` (interactive terminals only; without one it fails with
not_a_terminal). You read and answer those comments here. Every command reads
from disk, so each call sees the other side's latest writes.

Workflow:
  1. ptal comments <file>             list open threads (--all adds resolved)
  2. edit the file to address them    ptal never modifies the reviewed file
  3. ptal comments <file>             re-check: your edit moves or changes lines
  4. ptal reply <file> <id> \"<body>\" --as <name>
     ptal resolve <file> <id> --as <name> [--note \"what changed\"]
     ptal reopen <file> <id> --as <name> [--note \"why\"]

Threads are conversations. `ptal reply` asks, answers or discusses;
`ptal resolve` closes a thread you addressed; `ptal reopen` takes a resolved
thread back. A reply to a resolved thread reopens it, so use `reopen` only
when you have nothing to say; reopening an open thread is already_open.

A thread is waiting on you when its last comment is not yours: its author
differs from your --as name (exact match), so use the same --as name every
time. Only open threads are listed by default.

Use `ptal add <file> --lines <N|A-B> \"<body>\" --as <name>` to raise a
question of your own on a line or range. That starts a new thread; it is not
for answering one, use `ptal reply`.

Resolve a thread only when you addressed it. If a comment is unclear or you
disagree, do not guess: leave the thread open, ask with `ptal reply`, and tell
the human. When you resolve on a reading of an ambiguous comment, state your
interpretation in --note so the human can correct it.

Output is JSON when stdout is not a terminal, or with --json. `comments`
prints one document:

  {\"file\": \"/abs/path/plan.md\",
   \"threads\": [{
     \"id\": \"c3\", \"status\": \"open\" | \"resolved\",
     \"state\": \"current\" | \"outdated\",
     \"anchor\": {\"start_line\": 12, \"end_line\": 14, \"quote\": \"...\"},
     \"comments\": [{\"id\", \"author\", \"body\", \"created_at\"}],
     \"resolved_by\", \"resolved_at\"   (resolved threads only)
   }]}

`state` is \"outdated\" when the quoted lines are no longer at the stored range,
including after lines were inserted above them. Your own edits cause this:
threads you just addressed turn outdated, which is expected, so resolve them.
For one you did not address, re-read the file and, if the comment still
applies, re-anchor it with `ptal add` rather than trusting the stale range.

`add`, `reply`, `resolve` and `reopen` print {\"file\", \"thread\"} (no `state`).

Author: write commands need --as <name> or the PTAL_AUTHOR environment
variable. ptal never guesses one.

Errors: nothing on stdout; stderr gets one line,
  {\"error\": {\"code\": \"not_found\", \"message\": \"...\"}}

Exit codes:
  0  success
  2  fix the invocation: usage, missing_author, not_found, invalid_range,
     empty_body, already_open
  1  runtime failure: file_unreadable, locked, unsupported_schema, io,
     corrupt, no_data_dir, not_a_terminal, terminal

Data lives under PTAL_DATA_DIR (default ~/.local/share/ptal).";

#[derive(Debug, Parser)]
#[command(
    name = "ptal",
    version,
    about = "Review a throwaway text file",
    long_about = WORKFLOW,
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true,
    override_usage = "ptal <FILE>\n       ptal [--json] <COMMAND>",
)]
struct Cli {
    /// Always print JSON, even on a terminal.
    #[arg(long, global = true)]
    json: bool,

    /// Open FILE in the interactive view (a file named like a subcommand
    /// needs a path: `./comments`).
    file: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the comment threads on a file.
    #[command(after_long_help = COMMENTS_EXAMPLE)]
    Comments {
        file: PathBuf,
        /// Include resolved threads.
        #[arg(long)]
        all: bool,
    },
    /// Start a thread on a line or a range of lines.
    #[command(after_long_help = ADD_EXAMPLE)]
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
    #[command(after_long_help = RESOLVE_EXAMPLE)]
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
    /// Add a comment to a thread.
    #[command(after_long_help = REPLY_EXAMPLE)]
    Reply {
        file: PathBuf,
        /// Thread id, e.g. `c3`.
        id: String,
        /// The comment text.
        body: String,
        /// Who is replying; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
    },
    /// Reopen a resolved thread.
    #[command(after_long_help = REOPEN_EXAMPLE)]
    Reopen {
        file: PathBuf,
        /// Thread id, e.g. `c3`.
        id: String,
        /// Who is reopening; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
        /// Leave a comment saying why.
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
    /// A write's result: the updated thread and what was done to it.
    Thread(Thread, Verb),
}

/// What a write command did, for the readable rendering.
#[derive(Debug, Clone, Copy)]
enum Verb {
    Added,
    Resolved,
    Replied,
    Reopened,
}

impl Output {
    fn to_json(&self, file: &Path) -> String {
        let value = match self {
            Output::Threads(threads) => json!({ "file": file, "threads": threads }),
            Output::Thread(thread, _) => {
                json!({ "file": file, "thread": thread })
            }
        };
        format!("{value}\n")
    }

    /// Presentation only, not a contract.
    fn to_text(&self) -> String {
        match self {
            Output::Threads(threads) => render_text(threads),
            Output::Thread(thread, Verb::Added) => {
                let Anchor {
                    start_line,
                    end_line,
                    ..
                } = thread.anchor;
                format!("{} added (lines {start_line}-{end_line})\n", thread.id)
            }
            Output::Thread(thread, Verb::Resolved) => format!("{} resolved\n", thread.id),
            Output::Thread(thread, Verb::Replied) => {
                let reply = thread.comments.last().map_or("", |c| c.id.as_str());
                format!("{} replied ({reply})\n", thread.id)
            }
            Output::Thread(thread, Verb::Reopened) => format!("{} reopened\n", thread.id),
        }
    }
}

/// Readable listing: a header per thread, the quote, then the comments.
fn render_text(threads: &[ThreadView]) -> String {
    if threads.is_empty() {
        return "no threads\n".to_string();
    }
    let blocks: Vec<String> = threads
        .iter()
        .map(|view| {
            let Thread {
                id,
                anchor,
                status,
                comments,
            } = &view.thread;
            let status = match status {
                Status::Open => "open".to_string(),
                Status::Resolved { resolved_by, .. } => format!("resolved by {resolved_by}"),
            };
            let state = match view.state {
                State::Current => "current",
                State::Outdated => "outdated",
            };
            let mut block = format!(
                "{id}  {status}  {state}  lines {}-{}\n",
                anchor.start_line, anchor.end_line
            );
            for line in anchor.quote.lines() {
                block.push_str(&format!("  > {line}\n"));
            }
            for comment in comments {
                block.push_str(&format!("  {}: {}\n", comment.author, comment.body));
            }
            block
        })
        .collect();
    blocks.join("\n")
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

    let json = cli.json;
    let result = match (cli.command, cli.file) {
        (Some(command), _) => run_command(&command, env, out, json),
        (None, Some(file)) => launch_tui(&file, env),
        (None, None) => Err(Failure::Usage(
            "a FILE or a COMMAND is required; see --help".to_string(),
        )),
    };
    match result {
        Ok(()) => 0,
        Err(failure) => {
            let (_, code) = failure.classify();
            let text = if env.stderr_is_tty && !json {
                format!("error: {}\n", failure.message())
            } else {
                error_line(&failure)
            };
            finish(err, &text, code)
        }
    }
}

/// Execute an agent command and print its result to `out`.
fn run_command<C: Clock>(
    command: &Command,
    env: &Env<C>,
    out: &mut dyn Write,
    json: bool,
) -> Result<(), Failure> {
    let file = match command {
        Command::Comments { file, .. }
        | Command::Add { file, .. }
        | Command::Resolve { file, .. }
        | Command::Reply { file, .. }
        | Command::Reopen { file, .. } => file,
    };
    let output = dispatch(command, env)?;
    let canonical = std::fs::canonicalize(file).map_err(|source| {
        Failure::Core(Error::FileUnreadable {
            path: file.clone(),
            source,
        })
    })?;
    let text = if json || !env.stdout_is_tty {
        output.to_json(&canonical)
    } else {
        output.to_text()
    };
    match finish(out, &text, 0) {
        0 => Ok(()),
        _ => Err(Failure::Terminal(io::Error::other(
            "cannot write to stdout",
        ))),
    }
}

/// Open the interactive view. Everything that can be refused is refused
/// here, before the terminal is touched.
fn launch_tui<C: Clock>(file: &Path, env: &Env<C>) -> Result<(), Failure> {
    if !(env.stdin_is_tty && env.stdout_is_tty) {
        return Err(Failure::NotATerminal);
    }
    let author = env
        .user
        .as_deref()
        .map(str::trim)
        .filter(|user| !user.is_empty())
        .ok_or(Failure::MissingAuthor)?;
    let reviews = env
        .reviews
        .as_ref()
        .ok_or(Failure::Core(Error::NoDataDir))?;
    let rows = tui::terminal_rows().map_err(Failure::Terminal)?;
    let app = tui::open(reviews, file, author, rows)?;
    tui::run(app, reviews, file, author).map_err(Failure::Terminal)
}

fn dispatch<C: Clock>(command: &Command, env: &Env<C>) -> Result<Output, Failure> {
    let reviews = env
        .reviews
        .as_ref()
        .ok_or(Failure::Core(Error::NoDataDir))?;
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
            let thread = reviews.add_thread(file, *start, *end, body, &author)?;
            Ok(Output::Thread(thread, Verb::Added))
        }
        Command::Resolve {
            file,
            id,
            author: flag,
            note,
        } => {
            let author = author(flag)?;
            let thread = reviews.resolve(file, id, &author, note.as_deref())?;
            Ok(Output::Thread(thread, Verb::Resolved))
        }
        Command::Reply {
            file,
            id,
            body,
            author: flag,
        } => {
            let author = author(flag)?;
            let thread = reviews.reply(file, id, body, &author)?;
            Ok(Output::Thread(thread, Verb::Replied))
        }
        Command::Reopen {
            file,
            id,
            author: flag,
            note,
        } => {
            let author = author(flag)?;
            let thread = reviews.reopen(file, id, &author, note.as_deref())?;
            Ok(Output::Thread(thread, Verb::Reopened))
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
    let env = Env {
        reviews: Store::from_env()
            .ok()
            .map(|store| Reviews::with_clock(store, SystemClock)),
        author_env: std::env::var("PTAL_AUTHOR").ok(),
        user: std::env::var("USER").ok(),
        stdin_is_tty: io::stdin().is_terminal(),
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
            (Failure::NotATerminal, "not_a_terminal", 1),
            (Failure::Terminal(io_err()), "terminal", 1),
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
                Failure::Core(Error::AlreadyOpen("c1".into())),
                "already_open",
                2,
            ),
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
                reviews: Some(Reviews::with_clock(Store::new(&self.data), clock)),
                author_env: author.map(String::from),
                user: Some("scott".into()),
                stdin_is_tty: tty,
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

        fn env_without_store(&self, tty: bool) -> Env<fn() -> String> {
            Env {
                reviews: None,
                ..self.env(None, tty)
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
    fn without_a_store_help_and_usage_errors_still_work() {
        let fx = fixture("one\n");
        let env = fx.env_without_store(false);

        let help = fx.run_with(&["--help"], &env);
        let usage = fx.run_with(&["comments"], &env);

        assert_eq!((help.code, help.stderr.as_str()), (0, ""));
        assert_eq!(usage.code, 2);
        assert_eq!(usage.error_code(), "usage");
    }

    #[test]
    fn without_a_store_commands_fail_with_no_data_dir() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["comments", "FILE"], &fx.env_without_store(false));

        assert_eq!(out.code, 1);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "no_data_dir");
    }

    #[test]
    fn json_flag_forces_json_errors_on_a_terminal() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["comments", "FILE", "--json"], &fx.env_without_store(true));

        assert_eq!(out.error_code(), "no_data_dir");
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
    fn reply_appends_a_comment_and_prints_the_thread() {
        let fx = fixture_with_two_threads();

        let out = fx.run(&[
            "reply",
            "FILE",
            "c1",
            "Typo fixed",
            "--as",
            "claude",
            "--json",
        ]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.stderr, "");
        let json = out.json();
        assert_eq!(json["file"], fx.canonical());
        let thread = &json["thread"];
        assert_eq!(thread["id"], "c1");
        assert!(thread.get("state").is_none());
        let comments = thread["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[1]["id"], "c3");
        assert_eq!(comments[1]["author"], "claude");
        assert_eq!(comments[1]["body"], "Typo fixed");
    }

    #[test]
    fn comments_after_a_reply_shows_it_last_under_the_same_thread_id() {
        let fx = fixture_with_two_threads();
        fx.run(&["reply", "FILE", "c1", "one", "--as", "claude"]);
        fx.run(&["reply", "FILE", "c1", "two", "--as", "scott"]);

        let listed = fx.run(&["comments", "FILE"]).json();

        let thread = &listed["threads"][0];
        assert_eq!(thread["id"], "c1");
        let bodies: Vec<&str> = thread["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["body"].as_str().unwrap())
            .collect();
        assert_eq!(bodies, ["Typo?", "one", "two"]);
    }

    #[test]
    fn replying_to_a_resolved_thread_brings_it_back_to_the_default_view() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);
        assert_eq!(
            fx.run(&["comments", "FILE"]).json()["threads"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        fx.run(&["reply", "FILE", "c1", "Still wrong", "--as", "scott"]);

        let listed = fx.run(&["comments", "FILE"]).json();
        let threads = listed["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0]["id"], "c1");
        assert_eq!(threads[0]["status"], "open");
        assert!(threads[0].get("resolved_by").is_none());
        assert!(threads[0].get("resolved_at").is_none());
    }

    #[test]
    fn reply_failures_exit_2_with_their_codes_and_leave_the_review_untouched() {
        let fx = fixture_with_two_threads();
        let before = fx.review_bytes();

        for (args, code) in [
            (
                &["reply", "FILE", "c99", "hi", "--as", "claude"][..],
                "not_found",
            ),
            (
                &["reply", "FILE", "c1", "  ", "--as", "claude"],
                "empty_body",
            ),
            (&["reply", "FILE", "c1", "hi"], "missing_author"),
        ] {
            let out = fx.run(args);
            assert_eq!(out.code, 2, "{args:?}");
            assert_eq!(out.stdout, "", "{args:?}");
            assert_eq!(out.error_code(), code, "{args:?}");
        }
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn reply_falls_back_to_ptal_author() {
        let fx = fixture_with_two_threads();

        let out = fx.run_with(
            &["reply", "FILE", "c1", "hi"],
            &fx.env(Some("codex"), false),
        );

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.json()["thread"]["comments"][1]["author"], "codex");
    }

    #[test]
    fn reply_on_a_terminal_prints_the_thread_and_the_new_comment_id() {
        let fx = fixture_with_two_threads();

        let out = fx.run_with(
            &["reply", "FILE", "c1", "hi", "--as", "claude"],
            &fx.env(None, true),
        );

        assert_eq!(out.stdout, "c1 replied (c3)\n");
    }

    #[test]
    fn reopen_brings_a_resolved_thread_back_to_the_default_view() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);

        let out = fx.run(&["reopen", "FILE", "c1", "--as", "scott"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        let thread = &out.json()["thread"];
        assert_eq!(thread["status"], "open");
        assert!(thread.get("resolved_by").is_none());
        assert!(thread.get("state").is_none());
        let listed = fx.run(&["comments", "FILE"]).json();
        assert_eq!(listed["threads"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn reopen_note_is_appended_as_a_comment_by_the_reopener() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);

        let out = fx.run(&[
            "reopen",
            "FILE",
            "c1",
            "--as",
            "scott",
            "--note",
            "Wrong guess",
        ]);

        let comments = out.json()["thread"]["comments"].as_array().unwrap().clone();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[1]["author"], "scott");
        assert_eq!(comments[1]["body"], "Wrong guess");
    }

    #[test]
    fn reopening_an_open_thread_is_already_open_and_changes_nothing() {
        let fx = fixture_with_two_threads();
        let before = fx.review_bytes();

        let out = fx.run(&["reopen", "FILE", "c1", "--as", "scott", "--note", "Hmm"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "already_open");
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn reopen_unknown_id_and_missing_author_are_caller_errors() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);
        let before = fx.review_bytes();

        for (args, code) in [
            (&["reopen", "FILE", "c99", "--as", "scott"][..], "not_found"),
            (&["reopen", "FILE", "c1"], "missing_author"),
        ] {
            let out = fx.run(args);
            assert_eq!(out.code, 2, "{args:?}");
            assert_eq!(out.stdout, "", "{args:?}");
            assert_eq!(out.error_code(), code, "{args:?}");
        }
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn reopen_on_a_terminal_says_reopened() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);

        let out = fx.run_with(
            &["reopen", "FILE", "c1", "--as", "scott"],
            &fx.env(None, true),
        );

        assert_eq!(out.stdout, "c1 reopened\n");
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
            &["--frobnicate"][..],
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

        let out = fx.run_with(&["--frobnicate"], &fx.env(None, true));

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

    #[test]
    fn comments_on_a_tty_is_a_readable_listing() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Fixed"]);
        fs::write(&fx.file, "one\nTWO\nthree\n").unwrap();

        let out = fx.run_with(&["comments", "FILE", "--all"], &fx.env(None, true));

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(
            out.stdout,
            "\
c1  resolved by claude  current  lines 1-1
  > one
  scott: Typo?
  claude: Fixed

c2  open  outdated  lines 2-3
  > two
  > three
  scott: Why?
"
        );
    }

    #[test]
    fn comments_on_a_tty_with_no_threads_says_so() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["comments", "FILE"], &fx.env(None, true));

        assert_eq!(out.stdout, "no threads\n");
    }

    #[test]
    fn json_flag_forces_json_on_a_tty() {
        let fx = fixture_with_two_threads();

        let out = fx.run_with(&["comments", "FILE", "--json"], &fx.env(None, true));

        assert_eq!(out.json()["threads"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn writes_on_a_tty_print_a_one_line_confirmation() {
        let fx = fixture("one\ntwo\nthree\n");
        let tty = fx.env(None, true);

        let added = fx.run_with(
            &["add", "FILE", "--lines", "2-3", "Why?", "--as", "scott"],
            &tty,
        );
        let resolved = fx.run_with(&["resolve", "FILE", "c1", "--as", "claude"], &tty);

        assert_eq!(added.stdout, "c1 added (lines 2-3)\n");
        assert_eq!(resolved.stdout, "c1 resolved\n");
    }

    #[test]
    fn json_errors_are_forced_on_a_tty_by_the_json_flag() {
        let fx = fixture("one\n");

        let out = fx.run_with(
            &["resolve", "FILE", "c9", "--as", "x", "--json"],
            &fx.env(None, true),
        );

        assert_eq!(out.error_code(), "not_found");
    }

    #[test]
    fn top_level_help_teaches_the_whole_agent_workflow() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let at = |needle: &str| {
            help.find(needle)
                .unwrap_or_else(|| panic!("help is missing {needle:?}:\n{help}"))
        };
        assert!(at("ptal comments") < at("edit the file"));
        assert!(at("edit the file") < at("ptal resolve"));
        for needle in [
            "add",
            "--as",
            "PTAL_AUTHOR",
            "--all",
            "--json",
            "Exit codes",
            "outdated",
            "threads",
            "state",
            "anchor",
            "start_line",
            "quote",
            "comments",
        ] {
            at(needle);
        }
    }

    #[test]
    fn top_level_help_says_to_recheck_after_editing() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let edit = help.find("edit the file").expect("workflow edit step");
        let resolve = help.find("ptal resolve").expect("workflow resolve step");
        assert!(
            help[edit..resolve].contains("ptal comments"),
            "no re-check between editing and resolving:\n{help}"
        );
    }

    #[test]
    fn top_level_help_shows_how_to_call_add() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let line = help
            .lines()
            .find(|line| line.contains("ptal add"))
            .expect("a line that introduces `ptal add`");
        assert!(line.contains("--lines"), "{line}");
        assert!(line.contains("--as"), "{line}");
    }

    #[test]
    fn top_level_help_says_unclear_comments_stay_open() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = help
            .split("\n\n")
            .find(|paragraph| paragraph.contains("unclear"))
            .unwrap_or_else(|| panic!("help never covers unclear comments:\n{help}"));
        let paragraph = paragraph.to_lowercase();
        assert!(paragraph.contains("resolve"), "{paragraph}");
        assert!(paragraph.contains("open"), "{paragraph}");
        assert!(paragraph.contains("ptal reply"), "{paragraph}");
    }

    fn paragraph_with(help: &str, needles: &[&str]) -> String {
        help.split("\n\n")
            .find(|p| needles.iter().all(|n| p.contains(n)))
            .unwrap_or_else(|| panic!("no paragraph contains {needles:?}:\n{help}"))
            .to_string()
    }

    #[test]
    fn top_level_help_lists_already_open_as_a_caller_error() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let exit_2 = help.find("  2  ").expect("exit 2 line");
        let exit_1 = help.find("  1  ").expect("exit 1 line");
        assert!(help[exit_2..exit_1].contains("already_open"), "{help}");
    }

    #[test]
    fn top_level_help_shows_how_to_call_reply_and_reopen() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        for command in ["ptal reply", "ptal reopen"] {
            let line = help
                .lines()
                .find(|line| line.contains(command))
                .unwrap_or_else(|| panic!("no line introduces `{command}`"));
            assert!(line.contains("--as"), "{line}");
        }
    }

    #[test]
    fn top_level_help_says_when_to_reply_resolve_and_reopen() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        paragraph_with(
            &help,
            &[
                "ptal reply",
                "ptal resolve",
                "ptal reopen",
                "ask",
                "addressed",
                "back",
            ],
        );
    }

    #[test]
    fn top_level_help_defines_waiting_on_you_as_the_last_comment_not_yours() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = paragraph_with(&help, &["last comment", "not yours"]);
        assert!(paragraph.contains("--as"), "{paragraph}");
    }

    #[test]
    fn top_level_help_says_to_state_the_interpretation_on_resolve() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        paragraph_with(&help, &["interpretation", "--note"]);
    }

    #[test]
    fn top_level_help_says_add_is_for_new_threads_not_answers() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = paragraph_with(&help, &["new thread", "not"]);
        assert!(paragraph.contains("answer"), "{paragraph}");
        assert!(paragraph.contains("ptal add"), "{paragraph}");
    }

    #[test]
    fn top_level_help_says_which_commands_print_the_thread() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let line = help
            .lines()
            .find(|l| l.contains(r#"{"file", "thread"}"#))
            .unwrap_or_else(|| panic!("no line describes the thread output:\n{help}"));
        for command in ["add", "reply", "resolve", "reopen"] {
            assert!(line.contains(command), "{command}: {line}");
        }
    }

    #[test]
    fn every_subcommand_help_has_an_example() {
        let fx = fixture("one\n");

        for command in ["comments", "add", "resolve", "reply", "reopen"] {
            let help = fx.run(&[command, "--help"]).stdout;
            assert!(help.contains("Example"), "{command}:\n{help}");
            assert!(
                help.contains(&format!("ptal {command}")),
                "{command}:\n{help}"
            );
        }
    }

    // ---- bare `ptal <file>` (the TUI) --------------------------------------

    #[test]
    fn a_file_without_a_terminal_is_not_a_terminal() {
        let fx = fixture("one\n");
        let piped_out = Env {
            stdout_is_tty: false,
            stderr_is_tty: false,
            ..fx.env(None, true)
        };
        let piped_in = Env {
            stdin_is_tty: false,
            stderr_is_tty: false,
            ..fx.env(None, true)
        };

        for env in [fx.env(None, false), piped_out, piped_in] {
            let out = fx.run_with(&["FILE"], &env);
            assert_eq!(out.code, 1);
            assert_eq!(out.stdout, "");
            assert_eq!(out.error_code(), "not_a_terminal");
        }
        assert!(fx.review_bytes().is_empty());
    }

    #[test]
    fn the_tui_needs_a_user_before_it_touches_the_terminal() {
        let fx = fixture("one\n");
        for user in [None, Some("  ".to_string())] {
            let env = Env {
                user,
                stderr_is_tty: false,
                ..fx.env(None, true)
            };

            let out = fx.run_with(&["FILE"], &env);

            assert_eq!(out.code, 2);
            assert_eq!(out.stdout, "");
            assert_eq!(out.error_code(), "missing_author");
        }
    }

    #[test]
    fn the_tui_reports_a_missing_file_before_touching_the_terminal() {
        let fx = fixture("one\n");
        fs::remove_file(&fx.file).unwrap();

        let out = fx.run_with(&["FILE", "--json"], &fx.env(None, true));

        assert_eq!(out.code, 1);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "file_unreadable");
    }

    #[test]
    fn a_path_named_like_a_subcommand_is_a_file_when_written_with_dot_slash() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["./comments", "--json"], &fx.env(None, true));

        assert_eq!(out.error_code(), "file_unreadable");
        assert!(out.stderr.contains("./comments"), "{}", out.stderr);
    }

    #[test]
    fn subcommands_still_parse_with_the_bare_file_form_present() {
        let fx = fixture("one\n");

        let out = fx.run(&["comments", "FILE"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.json()["threads"], serde_json::json!([]));
    }

    #[test]
    fn no_arguments_print_help_and_exit_2() {
        let fx = fixture("one\n");

        let out = fx.run_with(&[], &fx.env(None, true));

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert!(out.stderr.contains("Usage"), "{}", out.stderr);
    }

    #[test]
    fn json_flag_alone_is_a_usage_error_not_a_hang() {
        let fx = fixture("one\n");

        let out = fx.run(&["--json"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.error_code(), "usage");
    }

    #[test]
    fn top_level_help_introduces_the_human_entry_point() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        assert!(help.contains("ptal <file>"), "{help}");
        assert!(help.contains("not_a_terminal"), "{help}");
    }
}
