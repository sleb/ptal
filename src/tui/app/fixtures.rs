//! Test helpers shared by more than one of `tui::app`'s submodules.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::anchor::{Change, ChangeKind, LineRange};
use crate::model::{Anchor, Comment, ReviewState, Side, State, Status, Thread};
use crate::review::{ReviewStatus, ThreadView, View};

use super::*;

pub(super) fn press(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub(super) fn ch(c: char) -> Event {
    press(KeyCode::Char(c))
}

pub(super) fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// `n` lines on a terminal of `rows` rows (one is the footer).
pub(super) fn app(n: usize, rows: u16) -> App {
    let lines = (1..=n).map(|i| format!("line {i}")).collect();
    App::open(showing(lines, &[]), "scott", (80, rows))
}

pub(super) fn send(app: &mut App, events: impl IntoIterator<Item = Event>) {
    for e in events {
        update(app, e);
    }
}

pub(super) fn at(app: &App) -> (usize, usize) {
    (app.cursor, app.top)
}

/// Whether every row of the cursor line is on screen.
pub(super) fn cursor_line_shown(a: &App) -> bool {
    let shown = a
        .screen_rows()
        .iter()
        .filter(|r| r.line == a.cursor)
        .count();
    shown == a.rows_of(a.cursor)
}

pub(super) fn typed(app: &mut App, text: &str) {
    send(app, text.chars().map(ch));
}

/// The draft's text and its cursor as a `char` index.
pub(super) fn draft(app: &App) -> (&str, usize) {
    match &app.mode {
        Mode::Input { draft, .. } => {
            let text = draft.text();
            (text, text[..draft.cursor()].chars().count())
        }
        _ => panic!("not in Input mode"),
    }
}

pub(super) fn input_app() -> App {
    let mut a = app(5, 10);
    send(&mut a, [ch('j'), ch('j'), press(KeyCode::Enter)]);
    a
}

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

/// `view` with a comment by each of `authors`, in order.
pub(super) fn said_by(mut v: ThreadView, authors: &[&str]) -> ThreadView {
    v.thread.comments = authors
        .iter()
        .enumerate()
        .map(|(i, author)| Comment {
            id: format!("{}-{i}", v.thread.id),
            author: (*author).into(),
            body: format!("body {i}"),
            created_at: "2026-10-12T10:00:00Z".into(),
            revision: None,
        })
        .collect();
    v
}

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

pub(super) fn panel_app(threads: &[ThreadView], rows: u16) -> App {
    let lines = (1..=10).map(|i| format!("line {i}")).collect();
    App::open(showing(lines, threads), "scott", (40, rows))
}

pub(super) fn listed(a: &App) -> usize {
    a.panel_layout().unwrap().thread_starts.len()
}

/// The panel on line 2 holding `c1` (by claude) and `c2` (by claude),
/// with `c2` selected.
pub(super) fn two_threads_panel() -> App {
    let threads = [chatty("c1", (2, 2), 1), chatty("c2", (2, 2), 1)];
    let mut a = panel_app(&threads, 20);
    send(
        &mut a,
        [ch('j'), press(KeyCode::Enter), press(KeyCode::Tab)],
    );
    a
}

/// `n` lines with the cursor on 0-based `line`, then `v`.
pub(super) fn visual_at(n: usize, rows: u16, line: usize) -> App {
    let mut a = app(n, rows);
    send(&mut a, (0..line).map(|_| ch('j')));
    send(&mut a, [ch('v')]);
    a
}

pub(super) fn alt_graph(c: char) -> Event {
    Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ))
}

pub(super) fn snapshot(a: &App) -> (Mode, usize, usize, Option<String>) {
    (a.mode.clone(), a.cursor, a.top, a.search.clone())
}

pub(super) fn handed_off(note: &str) -> ReviewStatus {
    ReviewStatus {
        state: ReviewState::Turn(Side::Agent),
        latest: Some(Entry {
            event: crate::model::Event::HandOff,
            side: Side::Human,
            author: "scott".into(),
            created_at: "2026-10-12T10:00:00Z".into(),
            revision: "r2".into(),
            next_id: 4,
            note: Some(note.into()),
        }),
    }
}

pub(super) fn change(start: u32, end: u32, kind: ChangeKind) -> Change {
    Change {
        range: LineRange { start, end },
        kind,
    }
}

/// A re-read whose text is what `a` shows.
pub(super) fn same_text(a: &App) -> View {
    View {
        sha256: a.seen.clone(),
        ..showing(a.lines.clone(), &[])
    }
}

/// A view of `lines` and `threads`, with no review or changes.
pub(super) fn showing(lines: Vec<String>, threads: &[ThreadView]) -> View {
    View {
        lines,
        threads: threads.to_vec(),
        ..View::default()
    }
}

pub(super) fn numbered(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("line {i}")).collect()
}
