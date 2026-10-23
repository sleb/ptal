//! Test helpers shared by more than one `tui` module: key events and
//! typing, apps of numbered lines, views, threads, comments and review
//! statuses.
//!
//! Helpers only `tui::app`'s submodules share stay in `app::fixtures`, since
//! some read `App` methods that are private to `tui::app`. `tests/tui.rs`
//! cannot see this module (it is `cfg(test)`), so it keeps its own helpers
//! under the same names.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::anchor::{Change, ChangeKind, LineRange};
use crate::model::{Anchor, Comment, Entry, ReviewState, Side, State, Status, Thread};
use crate::review::{ReviewStatus, ThreadView, View};
use crate::tui::app::{App, Event, update};

pub(super) fn press(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub(super) fn ch(c: char) -> Event {
    press(KeyCode::Char(c))
}

pub(super) fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

pub(super) fn shift(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT))
}

pub(super) fn alt_graph(c: char) -> Event {
    Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ))
}

pub(super) fn paste(text: &str) -> Event {
    Event::Paste(text.into())
}

pub(super) fn send(app: &mut App, events: impl IntoIterator<Item = Event>) {
    for e in events {
        update(app, e);
    }
}

pub(super) fn typed(app: &mut App, text: &str) {
    send(app, text.chars().map(ch));
}

/// `n` lines on a terminal of `rows` rows (one is the footer).
pub(super) fn app(n: usize, rows: u16) -> App {
    App::open(showing(numbered(n), &[]), "scott", (80, rows))
}

/// Ten lines on a 40-column terminal of `rows` rows, with `threads`.
pub(super) fn panel_app(threads: &[ThreadView], rows: u16) -> App {
    App::open(showing(numbered(10), threads), "scott", (40, rows))
}

/// A view of `lines` and `threads`, with no review or changes.
pub(super) fn showing(lines: Vec<String>, threads: &[ThreadView]) -> View {
    View {
        lines,
        threads: threads.to_vec(),
        ..View::default()
    }
}

/// `line 1` to `line n`.
pub(super) fn numbered(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("line {i}")).collect()
}

/// A thread on lines `start..=end` with no quote and no comments.
pub(super) fn view(id: &str, (start, end): (u32, u32), status: Status, state: State) -> ThreadView {
    ThreadView {
        thread: Thread {
            id: id.into(),
            anchor: Anchor {
                revision: None,
                start_line: start,
                end_line: end,
                quote: String::new(),
            },
            status,
            comments: vec![],
        },
        state,
    }
}

/// `view` with its anchor quoting `quote`.
pub(super) fn quoted(mut view: ThreadView, quote: &str) -> ThreadView {
    view.thread.anchor.quote = quote.into();
    view
}

/// `view` with one more comment, by `author` at `at`; its id is the
/// thread's and the comment's index (`c1-0`).
pub(super) fn comment(mut view: ThreadView, author: &str, at: &str, body: &str) -> ThreadView {
    let id = format!("{}-{}", view.thread.id, view.thread.comments.len());
    view.thread.comments.push(Comment {
        id,
        author: author.into(),
        body: body.into(),
        created_at: at.into(),
        revision: None,
    });
    view
}

/// `view` with a comment by each of `authors`, in order, each body
/// `body {i}`.
pub(super) fn said_by(view: ThreadView, authors: &[&str]) -> ThreadView {
    authors.iter().enumerate().fold(view, |v, (i, author)| {
        comment(v, author, "2026-10-12T10:00:00Z", &format!("body {i}"))
    })
}

/// A thread quoting `quote` with a comment for each (author, body) of
/// `said`, a minute apart from 09:30:30.
pub(super) fn talk(
    id: &str,
    range: (u32, u32),
    quote: &str,
    status: Status,
    state: State,
    said: &[(&str, &str)],
) -> ThreadView {
    let view = quoted(view(id, range, status, state), quote);
    said.iter()
        .enumerate()
        .fold(view, |v, (i, (author, body))| {
            comment(v, author, &format!("2026-10-12T09:{:02}:30Z", 30 + i), body)
        })
}

/// Resolved by `claude`.
pub(super) fn resolved() -> Status {
    Status::Resolved {
        resolved_by: "claude".into(),
        resolved_at: "2026-10-12T10:00:00Z".into(),
        resolved_revision: None,
    }
}

/// A thread with `n` single-line comments by `claude`.
pub(super) fn chatty(id: &str, range: (u32, u32), n: usize) -> ThreadView {
    let authors = vec!["claude"; n];
    said_by(view(id, range, Status::Open, State::Current), &authors)
}

pub(super) fn change(start: u32, end: u32, kind: ChangeKind) -> Change {
    Change {
        range: LineRange { start, end },
        kind,
    }
}

/// A review in `state`, its latest entry the human's hand-off with `note`
/// at revision `r2`, or no entry without one.
pub(super) fn review_status(state: ReviewState, note: Option<&str>) -> ReviewStatus {
    let latest = note.map(|note| Entry {
        event: crate::model::Event::HandOff,
        side: Side::Human,
        author: "scott".into(),
        created_at: "2026-10-12T10:00:00Z".into(),
        revision: "r2".into(),
        next_id: 4,
        note: Some(note.into()),
    });
    ReviewStatus { state, latest }
}

/// The agent's turn, after the human handed off with `note`.
pub(super) fn handed_off(note: &str) -> ReviewStatus {
    review_status(ReviewState::Turn(Side::Agent), Some(note))
}
