//! Test helpers shared by more than one of `tui::app`'s submodules and by
//! no other `tui` module; those shared across `tui` are in
//! `tui::test_support`. Some read `App` methods private to `tui::app`, so
//! they cannot move up.

use ratatui::crossterm::event::KeyCode;

use crate::review::View;
use crate::tui::test_support::*;

use super::*;

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

pub(super) fn snapshot(a: &App) -> (Mode, usize, usize, Option<String>) {
    (a.mode.clone(), a.cursor, a.top, a.search.clone())
}

/// A re-read whose text is what `a` shows.
pub(super) fn same_text(a: &App) -> View {
    View {
        sha256: a.seen.clone(),
        ..showing(a.lines.clone(), &[])
    }
}
