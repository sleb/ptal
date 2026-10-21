//! The agent-facing command line: parse, dispatch, render, map errors.
//!
//! A thin layer over [`crate::review`]. All rules live in the core; this
//! module only decides what bytes go to which stream and which exit code.

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use crate::error::Error;
use crate::model::{Anchor, ReviewState, Side, State, Status, Thread};
use crate::review::{
    Clock, HandedOff, ReviewStatus, Reviews, SystemClock, SystemTicker, ThreadView, Ticker, View,
    Waited, Woke,
};
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
                Error::AlreadyResolved(_) => ("already_resolved", EXIT_CALLER),
                Error::InvalidRange { .. } => ("invalid_range", EXIT_CALLER),
                Error::EmptyBody => ("empty_body", EXIT_CALLER),
                Error::NotYourTurn => ("not_your_turn", EXIT_CALLER),
                Error::MissingVerdict => ("missing_verdict", EXIT_CALLER),
                Error::NotClosed => ("not_closed", EXIT_CALLER),
                Error::AlreadyClosed => ("already_closed", EXIT_CALLER),
                Error::FileChanged => ("file_changed", EXIT_CALLER),
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
    /// Whether the TUI may highlight text: [`color_wanted`] of `NO_COLOR`.
    pub color: bool,
    /// The clock `wait` measures its timeout on and sleeps with.
    pub ticker: Box<dyn Ticker>,
}

// Help text. The top level is the loop and the concepts; each command's
// `after_long_help` owns its output, its errors and its examples.

const COMMENTS_HELP: &str = "\
Prints one document (JSON when stdout is not a terminal, or with --json):

  {\"file\": \"/abs/path/plan.md\",
   \"revision\": \"r3\" | null,
   \"threads\": [{
     \"id\": \"c3\", \"status\": \"open\" | \"resolved\",
     \"state\": \"current\" | \"outdated\",
     \"anchor\": {\"revision\": \"r1\", \"start_line\": 12, \"end_line\": 14,
                \"quote\": \"...\"},
     \"comments\": [{\"id\", \"author\", \"body\", \"created_at\", \"revision\"}],
     \"resolved_by\", \"resolved_at\", \"resolved_revision\"   (resolved only)
   }],
   \"review\": {
     \"turn\": \"human\" | \"agent\" | null,          (null when closed)
     \"verdict\": \"approved\" | \"dropped\" | null,  (set when closed)
     \"latest\": null | {
       \"event\": \"hand_off\" | \"close\" | \"reopen\", \"verdict\" (close only),
       \"side\": \"human\" | \"agent\", \"author\", \"created_at\", \"revision\",
       \"next_id\": 7,   (comment ids below c7 came before it)
       \"note\"          (if any)
     }}}

Threads are in id order; only open ones unless --all. Only the latest
hand-off, close or reopen is printed. On a terminal: the turn (or verdict)
and the latest note, then each thread with its quote and comments.

Examples:
  ptal comments plan.md           # open threads
  ptal comments plan.md --all     # include resolved threads";

const ADD_HELP: &str = "\
Prints {\"file\", \"thread\"}: the new thread, shaped as in
`ptal comments --help` but without `state`.

To answer an existing thread, use `ptal reply`; `add` starts a new one.

Errors: missing_author, empty_body, invalid_range (line 0, start after end,
or past the end of the file). Nothing is written.

Examples:
  ptal add plan.md --lines 12 \"Why not X?\" --as claude
  ptal add plan.md --lines 12-14 \"Why not X?\" --as claude";

const RESOLVE_HELP: &str = "\
Prints {\"file\", \"thread\"}: the resolved thread, shaped as in
`ptal comments --help` but without `state`. A --note is added to the thread
as a comment by you.

Errors: missing_author, not_found, already_resolved (nothing is written, so
the note is dropped).

Examples:
  ptal resolve plan.md c3 --as claude
  ptal resolve plan.md c3 --as claude --note \"Done, see step 2.\"";

const REPLY_HELP: &str = "\
Prints {\"file\", \"thread\"}: the thread with your comment last, shaped as
in `ptal comments --help` but without `state`. A reply to a resolved thread
reopens it.

Errors: missing_author, empty_body, not_found.

Examples:
  ptal reply plan.md c3 \"Which of the two do you mean?\" --as claude";

const REOPEN_HELP: &str = "\
With a thread id: prints {\"file\", \"thread\"}, the reopened thread shaped
as in `ptal comments --help` but without `state`. A --note is added as a
comment by you. Errors: missing_author, not_found, already_open (nothing is
written); --side is usage, since threads have no side.

With no id: reopens a closed review and prints {\"file\", \"review\"}; it
returns to the human's turn. It acts as the agent; --side human acts as the
human. Errors: missing_author, not_closed (the review is active).

Examples:
  ptal reopen plan.md c3 --as scott
  ptal reopen plan.md c3 --as scott --note \"Not fixed: still fails on empty input.\"
  ptal reopen plan.md --as claude --note \"Revising per chat.\"   # the review";

const HANDOFF_HELP: &str = "\
Prints {\"file\", \"review\", \"waiting\": [\"c3\"]}: `review` is the state
after the hand-off (as in `ptal comments --help`), and `waiting` lists the
open threads whose last comment isn't yours.

It acts as the agent; --side human acts as the human.

Errors: missing_author, not_your_turn (your side doesn't hold the turn, or
the review is closed). Nothing is written.

Examples:
  ptal handoff plan.md --as claude
  ptal handoff plan.md --as claude --note \"Addressed everything but c4.\"";

const WAIT_HELP: &str = "\
Prints {\"file\", \"reason\", \"review\"}; every reason is exit 0:
  \"turn\"     it is your turn; review.latest is the human's hand-off and note
  \"closed\"   the review is closed; review.verdict is \"approved\" or \"dropped\"
  \"timeout\"  --timeout passed and it is still the human's turn
`review` is shaped as in `ptal comments --help`. It returns at once if it is
already your turn or the review is closed, re-reads every 250 ms otherwise,
and never writes. On a terminal: the turn or verdict and the note, or
`timeout: still the human's turn`.

Errors: file_unreadable if the file is missing when the wait starts.

Examples:
  ptal wait plan.md                  # block until your turn or a close
  ptal wait plan.md --timeout 300    # give up after 5 minutes: reason \"timeout\"";

const WORKFLOW: &str = "\
Review a throwaway text file. A human comments on lines in the TUI,
`ptal <file>`; you read and answer those comments here. Every command reads
from disk, so each call sees the other side's latest writes.

The turn loop:
  0. open the review for the human. `ptal <file>` needs an interactive
     terminal (without one it fails with not_a_terminal), and its working
     directory may differ from yours, so give it an absolute path.
     In a terminal multiplexer, open a pane beside you running
     `exec ptal <absolute file>`; the pane closes when the human quits.
     In herdr:
       herdr pane split --current --direction right --cwd \"$PWD\" --focus
       herdr pane run <pane_id> \"exec ptal <absolute file>\"
     with <pane_id> from the split's .result.pane.pane_id.
     Outside a multiplexer, ask the human to run `ptal <file>` in a terminal.
  1. ptal wait <file>                 block until your turn or a close
     \"turn\":    your move; the human's note is in review.latest.note
     \"closed\":  act on review.verdict, then stop (see below)
     \"timeout\": still the human's turn; run ptal wait again
  2. ptal comments <file>             the threads and the review's state
                                      (document: ptal comments --help)
  3. edit the file to address them    ptal never modifies the reviewed file
  4. ptal comments <file>             re-check: edits can leave threads outdated
  5. ptal reply <file> <id> \"<body>\" --as <name>
     ptal resolve <file> <id> --as <name> [--note \"what changed\"]
     ptal reopen <file> <id> --as <name> [--note \"why\"]
  6. ptal handoff <file> --as <name> [--note \"what you did\"]
                                      hand the turn back; its `waiting` lists
                                      threads still waiting on you
  7. back to 1: wait again for the human's turn to end.

If your command tool can run a command in the background, run `ptal wait`
that way; otherwise pass --timeout SECS below the tool's time limit and run
it again on \"timeout\".

Only the human closes a review, with a verdict: \"approved\" means take the
next step with the file as it stands (publish, commit, implement), even with
threads still open; \"dropped\" means stop. Either way the loop ends. A
closed review has no turn. When the human asks you to revise one, reopen it
with `ptal reopen <file> --as <name>` (no thread id): it returns to the
human's turn, and you wait again.

Threads are conversations. `ptal reply` adds a comment to a thread;
`ptal resolve` closes it, and its --note is recorded as a comment;
`ptal reopen` takes a resolved thread back. A reply to a resolved thread
reopens it. Start a new thread on a line or range with:
  ptal add <file> --lines <N|A-B> \"<body>\" --as <name>

A thread is waiting on you when its last comment is not yours: its author
differs from your --as name (exact match), so use the same --as name every
time. Only open threads are listed by default.

Turns: a review is on the human's turn or yours, or closed; `turn` in the
`review` object says whose move it is. Only the side holding the turn can
hand off; otherwise it fails with not_your_turn. The turn is a signal, not a
lock: comment, reply, resolve and reopen at any time.

Lines are where the anchored text is in the file now: edits above a thread
move it. `state` is \"outdated\" when an anchored line was edited or removed;
the thread is then shown at a best guess, with its original `quote`, so your
own edits can cause it.

A revision (r1, r2, ...) is a version of the file that a write was made
against. The `revision` fields say which version a thread, comment, resolve
or hand-off referred to; the top-level `revision` is the file's current one,
or null if nothing has been written against it yet.

Author: write commands need --as <name> or the PTAL_AUTHOR environment
variable. ptal never guesses one.

Output is JSON when stdout is not a terminal, or with --json. Each command's
help, `ptal <command> --help`, has its output shape, the errors it can
return and examples: comments, add, reply, resolve, reopen, handoff, wait.

Errors: nothing on stdout; stderr gets one line,
  {\"error\": {\"code\": \"not_found\", \"message\": \"...\"}}

Exit codes:
  0  success
  2  fix the invocation: usage, missing_author, not_found, invalid_range,
     empty_body, already_open, already_resolved, not_your_turn,
     missing_verdict, not_closed, already_closed, file_changed
  1  runtime failure: file_unreadable, locked, unsupported_schema, io,
     corrupt, no_data_dir, not_a_terminal, terminal

Environment: PTAL_AUTHOR is the default --as name. Reviews are kept under
PTAL_DATA_DIR, else $XDG_DATA_HOME/ptal, else ~/.local/share/ptal.";

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
    #[command(after_long_help = COMMENTS_HELP)]
    Comments {
        /// The reviewed file.
        file: PathBuf,
        /// Include resolved threads.
        #[arg(long)]
        all: bool,
    },
    /// Start a thread on a line or a range of lines.
    #[command(after_long_help = ADD_HELP)]
    Add {
        /// The reviewed file.
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
    #[command(after_long_help = RESOLVE_HELP)]
    Resolve {
        /// The reviewed file.
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
    #[command(after_long_help = REPLY_HELP)]
    Reply {
        /// The reviewed file.
        file: PathBuf,
        /// Thread id, e.g. `c3`.
        id: String,
        /// The comment text.
        body: String,
        /// Who is replying; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
    },
    /// Reopen a resolved thread, or with no id a closed review.
    #[command(after_long_help = REOPEN_HELP)]
    Reopen {
        /// The reviewed file.
        file: PathBuf,
        /// Thread id, e.g. `c3`; leave it out to reopen the review.
        id: Option<String>,
        /// Who is reopening; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
        /// Say why: a comment on the thread, or the review reopen's note.
        #[arg(long)]
        note: Option<String>,
        /// Which side reopens the review (default agent); threads have none.
        #[arg(long, value_enum)]
        side: Option<SideArg>,
    },
    /// Hand the turn to the other side.
    #[command(after_long_help = HANDOFF_HELP)]
    Handoff {
        /// The reviewed file.
        file: PathBuf,
        /// Who is handing off; falls back to `PTAL_AUTHOR`.
        #[arg(long = "as", value_name = "NAME")]
        author: Option<String>,
        /// Which side is handing off: the agent unless `human`.
        #[arg(long, value_enum, default_value_t = SideArg::Agent)]
        side: SideArg,
        /// A note for the other side.
        #[arg(long)]
        note: Option<String>,
    },
    /// Block until it is your turn or the review is closed.
    #[command(after_long_help = WAIT_HELP)]
    Wait {
        /// The reviewed file.
        file: PathBuf,
        /// Give up after SECS seconds (0 checks once); default: no limit.
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,
    },
}

/// `--side`: which side the CLI acts as. The agent unless told otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SideArg {
    Human,
    Agent,
}

impl From<SideArg> for Side {
    fn from(side: SideArg) -> Self {
        match side {
            SideArg::Human => Side::Human,
            SideArg::Agent => Side::Agent,
        }
    }
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

/// Whether the TUI may highlight text, given the value of `NO_COLOR`: not
/// when it is set and non-empty (<https://no-color.org>).
pub fn color_wanted(no_color: Option<&OsStr>) -> bool {
    no_color.is_none_or(OsStr::is_empty)
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
    Threads(View),
    /// A write's result: the updated thread and what was done to it.
    Thread(Thread, Verb),
    HandedOff(HandedOff),
    /// A reopened review.
    Reopened(ReviewStatus),
    /// Why a wait returned.
    Waited(Waited),
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
            Output::Threads(view) => json!({
                "file": file,
                "revision": view.revision,
                "threads": view.threads,
                "review": view.review,
            }),
            Output::Thread(thread, _) => {
                json!({ "file": file, "thread": thread })
            }
            Output::HandedOff(HandedOff { review, waiting }) => {
                json!({ "file": file, "review": review, "waiting": waiting })
            }
            Output::Reopened(review) => json!({ "file": file, "review": review }),
            Output::Waited(Waited { reason, review }) => {
                json!({ "file": file, "reason": reason, "review": review })
            }
        };
        format!("{value}\n")
    }

    /// Presentation only, not a contract.
    fn to_text(&self) -> String {
        match self {
            Output::Threads(view) => {
                format!(
                    "{}\n{}",
                    render_status(&view.review),
                    render_text(&view.threads)
                )
            }
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
            Output::HandedOff(HandedOff { review, waiting }) => {
                let mut line = format!("handed off: {}", state_line(review));
                if !waiting.is_empty() {
                    line.push_str(&format!(" (waiting on you: {})", waiting.join(", ")));
                }
                line + "\n"
            }
            Output::Reopened(review) => format!("reopened: {}\n", state_line(review)),
            Output::Waited(Waited {
                reason: Woke::Timeout,
                ..
            }) => "timeout: still the human's turn\n".to_string(),
            Output::Waited(Waited { review, .. }) => render_status(review),
        }
    }
}

/// `human's turn`, `agent's turn` or `closed: <verdict>`.
fn state_line(review: &ReviewStatus) -> String {
    match review.state {
        ReviewState::Turn(side) => format!("{}'s turn", side.as_str()),
        ReviewState::Closed(verdict) => format!("closed: {}", verdict.as_str()),
    }
}

/// Readable state header: `turn: <side>` or `closed: <verdict>`, then each
/// line of the latest note under its author.
fn render_status(review: &ReviewStatus) -> String {
    let mut text = match review.state {
        ReviewState::Turn(side) => format!("turn: {}\n", side.as_str()),
        ReviewState::Closed(_) => format!("{}\n", state_line(review)),
    };
    if let Some(entry) = &review.latest {
        for line in entry.note.iter().flat_map(|note| note.lines()) {
            text.push_str(&format!("  {}: {line}\n", entry.author));
        }
    }
    text
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
        | Command::Reopen { file, .. }
        | Command::Handoff { file, .. }
        | Command::Wait { file, .. } => file,
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
    let loaded = tui::load(reviews, file)?;
    let size = tui::terminal_size().map_err(Failure::Terminal)?;
    let app = loaded.into_app(file, author, size, env.color);
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
            id: Some(id),
            author: flag,
            note,
            side,
        } => {
            if side.is_some() {
                return Err(Failure::Usage(
                    "--side applies only to reopening the review: threads have no side".to_string(),
                ));
            }
            let author = author(flag)?;
            let thread = reviews.reopen(file, id, &author, note.as_deref())?;
            Ok(Output::Thread(thread, Verb::Reopened))
        }
        Command::Reopen {
            file,
            id: None,
            author: flag,
            note,
            side,
        } => {
            let author = author(flag)?;
            let side = side.unwrap_or(SideArg::Agent).into();
            let review = reviews.reopen_review(file, side, &author, note.as_deref())?;
            Ok(Output::Reopened(review))
        }
        Command::Handoff {
            file,
            author: flag,
            side,
            note,
        } => {
            let author = author(flag)?;
            let handed = reviews.hand_off(file, (*side).into(), &author, note.as_deref())?;
            Ok(Output::HandedOff(handed))
        }
        Command::Wait { file, timeout } => {
            let timeout = timeout.map(std::time::Duration::from_secs);
            Ok(Output::Waited(reviews.wait(
                file,
                timeout,
                env.ticker.as_ref(),
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
    let env = Env {
        reviews: Store::from_env()
            .ok()
            .map(|store| Reviews::with_clock(store, SystemClock)),
        author_env: std::env::var("PTAL_AUTHOR").ok(),
        user: std::env::var("USER").ok(),
        stdin_is_tty: io::stdin().is_terminal(),
        stdout_is_tty,
        stderr_is_tty,
        color: color_wanted(std::env::var_os("NO_COLOR").as_deref()),
        ticker: Box::new(SystemTicker),
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
    use crate::model::{Side, Verdict};
    use crate::store::Store;
    use std::io;
    use std::path::PathBuf;

    fn io_err() -> io::Error {
        io::Error::other("boom")
    }

    /// Every failure the CLI reports, with its documented code and exit.
    fn all_failures() -> Vec<(Failure, &'static str, u8)> {
        let path = PathBuf::from("/x");
        let corrupt = serde_json::from_str::<u32>("x").unwrap_err();
        vec![
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
                Failure::Core(Error::AlreadyResolved("c1".into())),
                "already_resolved",
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
            (Failure::Core(Error::NotYourTurn), "not_your_turn", 2),
            (Failure::Core(Error::MissingVerdict), "missing_verdict", 2),
            (Failure::Core(Error::NotClosed), "not_closed", 2),
            (Failure::Core(Error::AlreadyClosed), "already_closed", 2),
            (Failure::Core(Error::FileChanged), "file_changed", 2),
        ]
    }

    #[test]
    fn every_failure_maps_to_its_documented_code_and_exit() {
        for (failure, code, exit) in all_failures() {
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
                color: true,
                ticker: Box::new(SystemTicker),
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

    impl Fixture {
        /// The core, as the TUI drives it, on the fixture's store.
        fn reviews(&self) -> Reviews<fn() -> String> {
            Reviews::with_clock(
                Store::new(&self.data),
                (|| NOW.to_string()) as fn() -> String,
            )
        }

        /// A human hand-off through the core, as the TUI writes it.
        fn human_hands_off(&self, note: Option<&str>) {
            self.reviews()
                .hand_off(&self.file, Side::Human, "scott", note)
                .unwrap();
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
                "revision": "r1",
                "threads": [{
                    "id": "c2",
                    "anchor": {"revision": "r1", "start_line": 2, "end_line": 3, "quote": "two\nthree"},
                    "status": "open",
                    "comments": [{"id": "c2", "author": "scott", "body": "Why?", "created_at": NOW, "revision": "r1"}],
                    "state": "current",
                }],
                "review": {"turn": "human", "verdict": null, "latest": null},
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
    fn comments_follows_lines_inserted_above_and_keeps_the_thread_current() {
        let fx = fixture_with_two_threads();
        fs::write(&fx.file, "zero\none\ntwo\nthree\n").unwrap();

        let json = fx.run(&["comments", "FILE"]).json();

        let c2 = &json["threads"][1];
        assert_eq!(c2["anchor"]["start_line"], 3);
        assert_eq!(c2["anchor"]["end_line"], 4);
        assert_eq!(c2["anchor"]["revision"], "r1");
        assert_eq!(c2["state"], "current");
        assert_eq!(json["revision"], serde_json::Value::Null);
    }

    #[test]
    fn comments_shows_an_edited_thread_outdated_with_its_original_quote() {
        let fx = fixture_with_two_threads();
        fs::write(&fx.file, "zero\none\nTWO\nthree\n").unwrap();

        let json = fx.run(&["comments", "FILE"]).json();

        let c2 = &json["threads"][1];
        assert_eq!(c2["state"], "outdated");
        assert_eq!(c2["anchor"]["start_line"], 3);
        assert_eq!(c2["anchor"]["end_line"], 4);
        assert_eq!(c2["anchor"]["quote"], "two\nthree");
    }

    #[test]
    fn writes_print_the_thread_at_its_mapped_lines() {
        let fx = fixture_with_two_threads();
        fs::write(&fx.file, "zero\none\ntwo\nthree\n").unwrap();

        let out = fx.run(&["reply", "FILE", "c2", "ok", "--as", "claude"]);

        let anchor = &out.json()["thread"]["anchor"];
        assert_eq!(
            (&anchor["start_line"], &anchor["end_line"]),
            (&3.into(), &4.into())
        );
        let revision = &out.json()["thread"]["comments"][1]["revision"];
        assert_eq!(revision, "r2");
    }

    #[test]
    fn comments_with_no_threads_is_an_empty_list() {
        let fx = fixture("one\n");

        let out = fx.run(&["comments", "FILE"]);

        assert_eq!(
            out.json(),
            serde_json::json!({
                "file": fx.canonical(),
                "revision": null,
                "threads": [],
                "review": {"turn": "human", "verdict": null, "latest": null},
            })
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
    fn color_is_wanted_unless_no_color_is_set_and_non_empty() {
        assert!(color_wanted(None));
        assert!(color_wanted(Some(OsStr::new(""))));
        assert!(!color_wanted(Some(OsStr::new("1"))));
        assert!(!color_wanted(Some(OsStr::new("0"))), "any non-empty value");
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
                    "anchor": {"revision": "r1", "start_line": 2, "end_line": 3, "quote": "two\nthree"},
                    "status": "open",
                    "comments": [{"id": "c1", "author": "scott", "body": "Why?", "created_at": NOW, "revision": "r1"}],
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
    fn resolving_a_resolved_thread_is_already_resolved_and_changes_nothing() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Fixed"]);
        let before = fx.review_bytes();

        let out = fx.run(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Again"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "already_resolved");
        assert_eq!(fx.review_bytes(), before);
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
        fs::write(&fx.file, "zero\none\nTWO\nthree\n").unwrap();

        let out = fx.run_with(&["comments", "FILE", "--all"], &fx.env(None, true));

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(
            out.stdout,
            "\
turn: human

c1  resolved by claude  current  lines 2-2
  > one
  scott: Typo?
  claude: Fixed

c2  open  outdated  lines 3-4
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

        assert_eq!(out.stdout, "turn: human\n\nno threads\n");
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

    /// Every subcommand, from clap, so a new one can't be missed.
    fn subcommands() -> Vec<String> {
        use clap::CommandFactory;
        Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect()
    }

    fn flat(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
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
            "--json",
            "Exit codes",
            "outdated",
        ] {
            at(needle);
        }
    }

    #[test]
    fn top_level_help_walks_the_turn_loop() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let at = |needle: &str| {
            help.find(needle)
                .unwrap_or_else(|| panic!("help is missing {needle:?}:\n{help}"))
        };
        for open in [
            "herdr pane split --current --direction right --cwd \"$PWD\" --focus",
            "pane_id",
            "exec ptal <absolute file>",
            "ask the human to run `ptal <file>`",
        ] {
            assert!(at(open) < at("ptal wait"), "{open} comes after ptal wait");
        }
        assert!(at("ptal wait") < at("ptal comments"));
        assert!(at("ptal comments") < at("edit the file"));
        assert!(at("edit the file") < at("ptal resolve"));
        assert!(at("ptal resolve") < at("ptal handoff"));
        let loop_text = flat(&help[at("ptal wait")..]);
        for reason in ["\"turn\"", "\"closed\"", "\"timeout\""] {
            assert!(loop_text.contains(reason), "{reason}: {loop_text}");
        }
        let verdicts = paragraph_with(&help, &["\"approved\"", "\"dropped\""]);
        let verdicts = flat(&verdicts);
        assert!(verdicts.contains("take the next step"), "{verdicts}");
        assert!(verdicts.contains("stop"), "{verdicts}");
        let advice = flat(&paragraph_with(&help, &["background", "--timeout"]));
        assert!(advice.contains("ptal wait"), "{advice}");
        let after = flat(&help[at("ptal handoff")..]);
        assert!(after.contains("wait again"), "{after}");
    }

    #[test]
    fn top_level_help_says_lines_follow_edits_and_when_a_thread_is_outdated() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = flat(&paragraph_with(&help, &["`state`", "outdated"]));
        for needle in [
            "edits above a thread move it",
            "an anchored line was edited or removed",
            "original `quote`",
        ] {
            assert!(paragraph.contains(needle), "{needle}: {paragraph}");
        }
        paragraph_with(&help, &["revision", "null"]);
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
    fn top_level_help_states_what_each_thread_command_does() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = paragraph_with(&help, &["Threads are conversations"]);
        for needle in ["ptal reply", "ptal resolve", "ptal reopen", "--note"] {
            assert!(paragraph.contains(needle), "{needle}: {paragraph}");
        }
        assert!(
            flat(&paragraph).contains("A reply to a resolved thread reopens it"),
            "{paragraph}"
        );
    }

    #[test]
    fn top_level_help_says_to_give_the_human_an_absolute_path() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        paragraph_with(&help, &["not_a_terminal", "absolute path"]);
    }

    fn paragraph_with(help: &str, needles: &[&str]) -> String {
        help.split("\n\n")
            .find(|p| needles.iter().all(|n| p.contains(n)))
            .unwrap_or_else(|| panic!("no paragraph contains {needles:?}:\n{help}"))
            .to_string()
    }

    #[test]
    fn top_level_help_lists_the_state_errors_as_caller_errors() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let exit_2 = help.find("  2  ").expect("exit 2 line");
        let exit_1 = help.find("  1  ").expect("exit 1 line");
        for code in [
            "already_open",
            "already_resolved",
            "not_your_turn",
            "missing_verdict",
            "not_closed",
            "already_closed",
        ] {
            assert!(help[exit_2..exit_1].contains(code), "{code}: {help}");
        }
    }

    #[test]
    fn top_level_help_lists_every_error_code_under_its_exit() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let exit_2 = help.find("  2  ").expect("exit 2 line");
        let exit_1 = help.find("  1  ").expect("exit 1 line");
        let table_end = help[exit_1..]
            .find("\n\n")
            .map_or(help.len(), |n| exit_1 + n);
        for (failure, code, exit) in all_failures() {
            let rows = if exit == 2 {
                &help[exit_2..exit_1]
            } else {
                &help[exit_1..table_end]
            };
            assert!(
                rows.contains(code),
                "{failure:?} ({code}) not under {exit}:\n{help}"
            );
        }
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
    fn top_level_help_defines_waiting_on_you_as_the_last_comment_not_yours() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = paragraph_with(&help, &["last comment", "not yours"]);
        assert!(paragraph.contains("--as"), "{paragraph}");
    }

    #[test]
    fn top_level_help_names_every_subcommand_and_points_to_its_help() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let pointer = flat(&paragraph_with(&help, &["ptal <command> --help"]));
        for needle in ["output", "errors", "examples"] {
            assert!(pointer.contains(needle), "{needle}: {pointer}");
        }
        for command in subcommands() {
            assert!(
                help.contains(&format!("ptal {command}")),
                "{command}:\n{help}"
            );
            assert!(pointer.contains(&command), "{command}: {pointer}");
        }
    }

    #[test]
    fn top_level_help_names_the_environment_variables() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let paragraph = paragraph_with(&help, &["PTAL_DATA_DIR"]);
        for needle in ["PTAL_AUTHOR", "XDG_DATA_HOME", "~/.local/share/ptal"] {
            assert!(paragraph.contains(needle), "{needle}: {paragraph}");
        }
    }

    #[test]
    fn comments_help_shows_the_output_document() {
        let fx = fixture("one\n");

        let help = fx.run(&["comments", "--help"]).stdout;

        let shape = paragraph_with(&help, &["\"threads\""]);
        for field in [
            "\"revision\"",
            "\"state\"",
            "\"anchor\"",
            "\"start_line\"",
            "\"quote\"",
            "\"comments\"",
            "resolved_revision",
            "\"review\"",
            "\"turn\"",
            "\"verdict\"",
            "\"latest\"",
            "hand_off",
            "next_id",
        ] {
            assert!(shape.contains(field), "{field}: {shape}");
        }
        assert!(help.contains("--all"), "{help}");
    }

    #[test]
    fn add_help_says_it_is_for_new_threads_not_answers() {
        let fx = fixture("one\n");

        let help = fx.run(&["add", "--help"]).stdout;

        let paragraph = paragraph_with(&help, &["ptal reply"]);
        assert!(paragraph.contains("answer"), "{paragraph}");
        for code in ["invalid_range", "empty_body", "missing_author"] {
            assert!(help.contains(code), "{code}: {help}");
        }
    }

    #[test]
    fn thread_writes_help_says_they_print_the_thread() {
        let fx = fixture("one\n");

        for (command, codes) in [
            ("add", &["invalid_range"][..]),
            ("reply", &["not_found", "empty_body"]),
            ("resolve", &["not_found", "already_resolved"]),
            ("reopen", &["not_found", "already_open"]),
        ] {
            let help = fx.run(&[command, "--help"]).stdout;
            assert!(help.contains(r#"{"file", "thread"}"#), "{command}:\n{help}");
            for code in codes {
                assert!(help.contains(code), "{command} {code}:\n{help}");
            }
        }
    }

    #[test]
    fn handoff_help_says_what_waiting_lists_and_when_it_is_refused() {
        let fx = fixture("one\n");

        let help = fx.run(&["handoff", "--help"]).stdout;

        let output = flat(&paragraph_with(&help, &["\"waiting\""]));
        assert!(output.contains("last comment isn't yours"), "{output}");
        paragraph_with(&help, &["not_your_turn", "closed"]);
        paragraph_with(&help, &["--side human", "as the human"]);
    }

    #[test]
    fn reopen_help_covers_reopening_the_review() {
        let fx = fixture("one\n");

        let help = fx.run(&["reopen", "--help"]).stdout;

        let review = flat(&paragraph_with(&help, &["no id", "not_closed"]));
        for needle in [r#"{"file", "review"}"#, "human's turn", "--side human"] {
            assert!(review.contains(needle), "{needle}: {review}");
        }
    }

    #[test]
    fn wait_help_explains_each_reason_and_the_exit_status() {
        let fx = fixture("one\n");

        let help = fx.run(&["wait", "--help"]).stdout;

        let output = flat(&paragraph_with(&help, &[r#"{"file", "reason", "review"}"#]));
        for needle in [
            "\"turn\"",
            "\"closed\"",
            "\"timeout\"",
            "exit 0",
            "never writes",
        ] {
            assert!(output.contains(needle), "{needle}: {output}");
        }
    }

    #[test]
    fn every_subcommand_help_has_an_output_shape_and_an_example() {
        let fx = fixture("one\n");

        for command in subcommands() {
            let help = fx.run(&[&command, "--help"]).stdout;
            assert!(help.to_lowercase().contains("prints"), "{command}:\n{help}");
            assert!(help.contains("Example"), "{command}:\n{help}");
            assert!(
                help.contains(&format!("ptal {command}")),
                "{command}:\n{help}"
            );
            assert!(
                !help.contains("<FILE>\n          \n"),
                "{command}: FILE undocumented:\n{help}"
            );
        }
    }

    // ---- turns, closing, reopening the review -----------------------------

    #[test]
    fn comments_reports_the_latest_hand_off_in_the_review_object() {
        let fx = fixture_with_two_threads();
        fx.human_hands_off(Some("Over to you"));

        let review = &fx.run(&["comments", "FILE"]).json()["review"];

        assert_eq!(
            *review,
            serde_json::json!({
                "turn": "agent",
                "verdict": null,
                "latest": {
                    "event": "hand_off",
                    "side": "human",
                    "author": "scott",
                    "created_at": NOW,
                    "revision": "r1",
                    "next_id": 3,
                    "note": "Over to you",
                },
            })
        );
    }

    #[test]
    fn comments_has_no_changes_since_the_last_look_for_the_agent() {
        let fx = fixture_with_two_threads();
        fx.human_hands_off(None);
        fs::write(&fx.file, "zero\none\nTWO\nthree\n").unwrap();

        let json = fx.run(&["comments", "FILE"]).json();

        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["file", "review", "revision", "threads"]);
    }

    #[test]
    fn handoff_during_the_agents_turn_hands_back_and_lists_what_is_waiting() {
        let fx = fixture_with_two_threads();
        fx.run(&["reply", "FILE", "c2", "Because", "--as", "claude"]);
        fx.human_hands_off(None);

        let out = fx.run(&["handoff", "FILE", "--as", "claude", "--note", "Revised"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        let json = out.json();
        assert_eq!(json["file"], fx.canonical());
        assert_eq!(json["waiting"], serde_json::json!(["c1"]));
        assert_eq!(json["review"]["turn"], "human");
        assert_eq!(json["review"]["latest"]["side"], "agent");
        assert_eq!(json["review"]["latest"]["note"], "Revised");
        let listed = fx.run(&["comments", "FILE"]).json();
        assert_eq!(listed["review"], json["review"]);
    }

    #[test]
    fn handoff_out_of_turn_or_without_an_author_fails_and_writes_nothing() {
        let fx = fixture_with_two_threads();
        let before = fx.review_bytes();

        for (args, code) in [
            (&["handoff", "FILE", "--as", "claude"][..], "not_your_turn"),
            (&["handoff", "FILE"], "missing_author"),
        ] {
            let out = fx.run(args);
            assert_eq!(out.code, 2, "{args:?}");
            assert_eq!(out.stdout, "", "{args:?}");
            assert_eq!(out.error_code(), code, "{args:?}");
        }
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn handoff_side_human_acts_as_the_human() {
        let fx = fixture("one\n");

        let out = fx.run(&["handoff", "FILE", "--as", "scott", "--side", "human"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        let review = &out.json()["review"];
        assert_eq!(review["turn"], "agent");
        assert_eq!(review["latest"]["side"], "human");
        let back = fx.run(&["handoff", "FILE", "--as", "claude", "--side", "agent"]);
        assert_eq!(back.json()["review"]["latest"]["side"], "agent");
    }

    #[test]
    fn a_bad_side_or_a_side_on_a_thread_reopen_is_usage() {
        let fx = fixture_with_two_threads();
        fx.run(&["resolve", "FILE", "c1", "--as", "claude"]);
        let before = fx.review_bytes();

        for args in [
            &["handoff", "FILE", "--as", "x", "--side", "robot"][..],
            &["reopen", "FILE", "c1", "--as", "x", "--side", "agent"],
        ] {
            let out = fx.run(args);
            assert_eq!(out.code, 2, "{args:?}");
            assert_eq!(out.error_code(), "usage", "{args:?}");
        }
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn reopen_without_an_id_reopens_a_closed_review() {
        let fx = fixture_with_two_threads();
        fx.reviews()
            .close(&fx.file, "scott", Some(Verdict::Approved), Some("Ship it"))
            .unwrap();
        let closed = fx.run(&["comments", "FILE"]).json();
        assert_eq!(closed["review"]["turn"], serde_json::Value::Null);
        assert_eq!(closed["review"]["verdict"], "approved");
        assert_eq!(closed["review"]["latest"]["note"], "Ship it");

        let out = fx.run(&["reopen", "FILE", "--as", "claude", "--note", "Revising"]);

        assert_eq!(out.code, 0, "{}", out.stderr);
        let json = out.json();
        assert_eq!(json["file"], fx.canonical());
        assert_eq!(json["review"]["turn"], "human");
        assert_eq!(json["review"]["verdict"], serde_json::Value::Null);
        assert_eq!(json["review"]["latest"]["event"], "reopen");
        assert_eq!(json["review"]["latest"]["side"], "agent");
        assert!(json.get("thread").is_none());
    }

    #[test]
    fn reopening_an_active_review_is_not_closed() {
        let fx = fixture_with_two_threads();
        let before = fx.review_bytes();

        let out = fx.run(&["reopen", "FILE", "--as", "claude"]);

        assert_eq!(out.code, 2);
        assert_eq!(out.stdout, "");
        assert_eq!(out.error_code(), "not_closed");
        assert_eq!(fx.review_bytes(), before);
    }

    #[test]
    fn comments_on_a_tty_leads_with_the_turn_and_the_latest_note() {
        let fx = fixture("one\n");
        fx.human_hands_off(Some("Look at c1\nand c2"));
        let tty = fx.env(None, true);

        let agent = fx.run_with(&["comments", "FILE"], &tty);
        fx.reviews()
            .close(&fx.file, "scott", Some(Verdict::Dropped), None)
            .unwrap();
        let closed = fx.run_with(&["comments", "FILE"], &tty);

        assert_eq!(
            agent.stdout,
            "turn: agent\n  scott: Look at c1\n  scott: and c2\n\nno threads\n"
        );
        assert_eq!(closed.stdout, "closed: dropped\n\nno threads\n");
    }

    #[test]
    fn handoff_and_review_reopen_on_a_tty_print_one_line() {
        let fx = fixture_with_two_threads();
        let tty = fx.env(None, true);
        fx.human_hands_off(None);

        let waiting = fx.run_with(&["handoff", "FILE", "--as", "claude"], &tty);
        fx.run(&["handoff", "FILE", "--as", "scott", "--side", "human"]);
        fx.run(&["reply", "FILE", "c1", "ok", "--as", "claude"]);
        fx.run(&["reply", "FILE", "c2", "ok", "--as", "claude"]);
        let none = fx.run_with(&["handoff", "FILE", "--as", "claude"], &tty);
        fx.reviews()
            .close(&fx.file, "scott", Some(Verdict::Approved), None)
            .unwrap();
        let reopened = fx.run_with(&["reopen", "FILE", "--as", "claude"], &tty);

        assert_eq!(
            waiting.stdout,
            "handed off: human's turn (waiting on you: c1, c2)\n"
        );
        assert_eq!(none.stdout, "handed off: human's turn\n");
        assert_eq!(reopened.stdout, "reopened: human's turn\n");
    }

    #[test]
    fn top_level_help_teaches_turns_hand_offs_and_closing() {
        let fx = fixture("one\n");

        let help = fx.run(&["--help"]).stdout;

        let resolve = help.find("ptal resolve").expect("workflow resolve step");
        let handoff = help.find("ptal handoff").expect("workflow hand-off step");
        assert!(resolve < handoff, "hand off after answering:\n{help}");
        let step = help[handoff..].lines().next().unwrap();
        assert!(step.contains("--as"), "{step}");
        let turns = flat(&paragraph_with(&help, &["`turn`", "not_your_turn"]));
        assert!(turns.contains("signal, not a lock"), "{turns}");
        paragraph_with(&help, &["closes", "ptal reopen <file>"]);
    }

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

    // ---- waiting for the agent's turn -------------------------------------

    /// A ticker that never really sleeps: each sleep moves its clock on.
    struct InstantTicker(std::cell::Cell<std::time::Instant>);

    impl crate::review::Ticker for InstantTicker {
        fn now(&self) -> std::time::Instant {
            self.0.get()
        }

        fn sleep(&self, duration: std::time::Duration) {
            self.0.set(self.0.get() + duration);
        }
    }

    impl Fixture {
        fn wait_env(&self, tty: bool) -> Env<fn() -> String> {
            Env {
                ticker: Box::new(InstantTicker(std::cell::Cell::new(
                    std::time::Instant::now(),
                ))),
                ..self.env(None, tty)
            }
        }
    }

    #[test]
    fn wait_on_the_agents_turn_prints_the_hand_off() {
        let fx = fixture("one\n");
        fx.human_hands_off(Some("Over to you"));

        let out = fx.run_with(&["wait", "FILE"], &fx.wait_env(false));

        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(
            out.json(),
            serde_json::json!({
                "file": fx.canonical(),
                "reason": "turn",
                "review": {
                    "turn": "agent",
                    "verdict": null,
                    "latest": {
                        "event": "hand_off", "side": "human", "author": "scott",
                        "created_at": NOW, "revision": "r1", "next_id": 1,
                        "note": "Over to you",
                    },
                },
            })
        );
    }

    #[test]
    fn wait_on_a_closed_review_prints_the_verdict() {
        let fx = fixture("one\n");
        fx.reviews()
            .close(&fx.file, "scott", Some(Verdict::Approved), None)
            .unwrap();

        let out = fx.run_with(&["wait", "FILE"], &fx.wait_env(false));

        assert_eq!(out.code, 0, "{}", out.stderr);
        let json = out.json();
        assert_eq!(json["reason"], "closed");
        assert_eq!(json["review"]["verdict"], "approved");
        assert_eq!(json["review"]["turn"], serde_json::Value::Null);
    }

    #[test]
    fn wait_with_a_timeout_on_the_humans_turn_is_timeout_and_exit_0() {
        let fx = fixture("one\n");

        for timeout in ["0", "3"] {
            let out = fx.run_with(&["wait", "FILE", "--timeout", timeout], &fx.wait_env(false));

            assert_eq!(out.code, 0, "{}", out.stderr);
            let json = out.json();
            assert_eq!(json["reason"], "timeout");
            assert_eq!(json["review"]["turn"], "human");
            assert_eq!(json["review"]["latest"], serde_json::Value::Null);
        }
        assert_eq!(fx.review_bytes(), b"", "a wait writes nothing");
    }

    #[test]
    fn wait_on_a_tty_prints_the_state_header_or_the_timeout() {
        let fx = fixture("one\n");
        let tty = fx.wait_env(true);

        let timeout = fx.run_with(&["wait", "FILE", "--timeout", "0"], &tty);
        fx.human_hands_off(Some("Look at c1\nand c2"));
        let turn = fx.run_with(&["wait", "FILE"], &tty);
        fx.reviews()
            .close(&fx.file, "scott", Some(Verdict::Dropped), None)
            .unwrap();
        let closed = fx.run_with(&["wait", "FILE"], &tty);

        assert_eq!(timeout.stdout, "timeout: still the human's turn\n");
        assert_eq!(
            turn.stdout,
            "turn: agent\n  scott: Look at c1\n  scott: and c2\n"
        );
        assert_eq!(closed.stdout, "closed: dropped\n");
    }

    #[test]
    fn wait_rejects_a_bad_timeout_a_side_and_an_author_as_usage() {
        let fx = fixture("one\n");

        for args in [
            &["wait", "FILE", "--timeout", "soon"][..],
            &["wait", "FILE", "--timeout", "-1"],
            &["wait", "FILE", "--side", "human"],
            &["wait", "FILE", "--as", "claude"],
        ] {
            let out = fx.run_with(args, &fx.wait_env(false));
            assert_eq!(out.code, 2, "{args:?}");
            assert_eq!(out.error_code(), "usage", "{args:?}");
            assert_eq!(out.stdout, "", "{args:?}");
        }
    }

    #[test]
    fn wait_needs_no_author() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["wait", "FILE", "--timeout", "0"], &fx.wait_env(false));

        assert_eq!(out.code, 0, "{}", out.stderr);
    }

    #[test]
    fn wait_on_a_missing_file_is_file_unreadable_with_empty_stdout() {
        let fx = fixture("one\n");
        fs::remove_file(&fx.file).unwrap();

        let out = fx.run_with(&["wait", "FILE", "--timeout", "0"], &fx.wait_env(false));

        assert_eq!(out.code, 1);
        assert_eq!(out.error_code(), "file_unreadable");
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn a_file_named_wait_opens_as_dot_slash_wait() {
        let fx = fixture("one\n");

        let out = fx.run_with(&["./wait", "--json"], &fx.env(None, true));

        assert_eq!(out.error_code(), "file_unreadable");
        assert!(out.stderr.contains("./wait"), "{}", out.stderr);
    }
}
