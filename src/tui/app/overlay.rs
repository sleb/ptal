//! The full-screen overlays, [`Mode::Overlay`]: help and the review state,
//! their listings and their keys.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::model::{Event as LogEvent, ReviewState, Side};
use crate::review::ReviewStatus;
use crate::tui::{keys, wrap};

use super::{App, Effect, Mode, Mods, Overlay, OverlayBack, Scroll};

impl App {
    /// The help listing laid out for the window.
    pub fn help_rows(&self) -> &[keys::Row] {
        &self.help
    }

    /// The listing of overlay `kind`, laid out for the window.
    pub fn overlay_rows(&self, kind: Overlay) -> &[keys::Row] {
        match kind {
            Overlay::Help => self.help_rows(),
            Overlay::Review => &self.review_rows,
        }
    }

    /// Keep an overlay's scroll inside its listing.
    pub(super) fn clamp_overlay(&mut self) {
        let Mode::Overlay { kind, .. } = self.mode else {
            return;
        };
        let max = self
            .overlay_rows(kind)
            .len()
            .saturating_sub(self.overlay_body_rows());
        if let Mode::Overlay { scroll, .. } = &mut self.mode {
            *scroll = (*scroll).min(max);
        }
    }

    /// Open overlay `kind` from `back`.
    pub(super) fn open_overlay(&mut self, kind: Overlay, back: OverlayBack) {
        self.mode = Mode::Overlay {
            kind,
            back,
            scroll: 0,
        };
    }
}

/// `your turn`, `agent's turn` or `closed: <verdict>`, as the human reads it.
pub(super) fn state_name(state: ReviewState) -> String {
    match state {
        ReviewState::Turn(Side::Human) => "your turn".into(),
        ReviewState::Turn(Side::Agent) => "agent's turn".into(),
        ReviewState::Closed(verdict) => format!("closed: {}", verdict.as_str()),
    }
}

/// The review overlay's listing for `width` cells: the state as a header,
/// what the latest entry was, by whom, when and on which revision, then its
/// whole note, each line wrapped.
pub(super) fn review_rows(review: &ReviewStatus, width: usize) -> Vec<keys::Row> {
    let width = width.max(1);
    let mut out = vec![
        keys::Row {
            text: state_name(review.state),
            heading: true,
        },
        blank_row(),
    ];
    let Some(entry) = &review.latest else {
        push_wrapped(&mut out, "no hand-off yet", width);
        return out;
    };
    let what = match entry.event {
        LogEvent::HandOff => "hand-off".to_string(),
        LogEvent::Close { verdict } => format!("closed as {}", verdict.as_str()),
        LogEvent::Reopen => "reopened".to_string(),
    };
    let by = format!("{what} by {} ({})", entry.author, entry.side.as_str());
    push_wrapped(&mut out, &by, width);
    let at = format!("at {} on {}", entry.created_at, entry.revision);
    push_wrapped(&mut out, &at, width);
    if let Some(note) = &entry.note {
        out.push(blank_row());
        for line in note.lines() {
            push_wrapped(&mut out, line, width);
        }
    }
    out
}

/// An empty row between the parts of the listing.
fn blank_row() -> keys::Row {
    keys::Row {
        text: String::new(),
        heading: false,
    }
}

/// Push `line` wrapped to `width`, a row for each wrapped row.
fn push_wrapped(out: &mut Vec<keys::Row>, line: &str, width: usize) {
    out.extend(
        wrap::plain_rows(line, width)
            .into_iter()
            .map(|text| keys::Row {
                text,
                heading: false,
            }),
    );
}

/// Leave an overlay for `back`.
fn close(app: &mut App, back: OverlayBack) -> Option<Effect> {
    app.mode = back.mode();
    None
}

/// Keys in [`Mode::Overlay`]: its own key (`?` for help), `esc`, `q` and
/// `ctrl+c` close it (never quit), the movement keys scroll the listing.
pub(super) fn overlay_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Overlay { kind, back, scroll } = app.mode else {
        return None;
    };
    let ctrl = Mods::of(key).ctrl();
    let half = (app.overlay_body_rows() / 2).max(1);
    let to = match key.code {
        KeyCode::Esc | KeyCode::Char('q') if !ctrl => return close(app, back),
        KeyCode::Char(c) if !ctrl && c == kind.toggle() => return close(app, back),
        KeyCode::Char('c') if ctrl => return close(app, back),
        _ => match Scroll::of(key)? {
            Scroll::Up => scroll.saturating_sub(1),
            Scroll::Down => scroll + 1,
            Scroll::HalfUp => scroll.saturating_sub(half),
            Scroll::HalfDown => scroll + half,
            Scroll::Top => 0,
            // Clamped to the listing by `clamp_overlay`.
            Scroll::Bottom => usize::MAX,
        },
    };
    app.mode = Mode::Overlay {
        kind,
        back,
        scroll: to,
    };
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::View;
    use crate::tui::app::fixtures::*;
    use crate::tui::app::{Event, update};
    use crate::tui::test_support::*;

    // --- help ---

    fn help_scroll(a: &App) -> usize {
        match a.mode {
            Mode::Overlay { scroll, .. } => scroll,
            _ => panic!("not in an overlay: {:?}", a.mode),
        }
    }

    #[test]
    fn question_mark_opens_help_and_the_closing_keys_return_to_normal() {
        for close in [ch('?'), press(KeyCode::Esc), ch('q'), ctrl('c')] {
            let mut a = app(20, 10);
            send(
                &mut a,
                [ch('j'), ch('j'), ch('/'), ch('5'), press(KeyCode::Enter)],
            );
            let before = snapshot(&a);

            send(&mut a, [ch('?')]);
            assert!(matches!(
                a.mode,
                Mode::Overlay {
                    kind: Overlay::Help,
                    back: OverlayBack::Normal,
                    ..
                }
            ));
            let effect = update(&mut a, close);

            assert_eq!(effect, None);
            assert_eq!(snapshot(&a), before);
        }
    }

    #[test]
    fn help_from_the_panel_returns_to_the_same_selection_and_scroll() {
        let threads = [chatty("c1", (2, 2), 6), chatty("c2", (2, 2), 1)];
        for close in [ch('?'), press(KeyCode::Esc), ch('q'), ctrl('c')] {
            let mut a = panel_app(&threads, 12);
            send(
                &mut a,
                [
                    ch('j'),
                    press(KeyCode::Enter),
                    ch('j'),
                    ch('j'),
                    press(KeyCode::Tab),
                ],
            );
            let before = a.mode.clone();

            send(&mut a, [ch('?')]);
            assert!(matches!(
                a.mode,
                Mode::Overlay {
                    kind: Overlay::Help,
                    back: OverlayBack::Panel { .. },
                    ..
                }
            ));
            let effect = update(&mut a, close);

            assert_eq!(effect, None);
            assert_eq!(a.mode, before);
        }
    }

    #[test]
    fn question_mark_does_not_open_help_in_visual_and_is_text_in_search_and_the_box() {
        let mut a = app(5, 10);
        typed(&mut a, "v?");
        assert!(matches!(a.mode, Mode::Visual { .. }));

        let mut a = app(5, 10);
        typed(&mut a, "/?");
        assert!(matches!(&a.mode, Mode::Search { query, .. } if query == "?"));

        let mut a = input_app();
        typed(&mut a, "?");
        assert_eq!(draft(&a), ("?", 1));
    }

    #[test]
    fn help_scrolls_and_clamps_to_the_listing() {
        let mut a = app(5, 10);
        let max = a.help_rows().len() - a.overlay_body_rows();
        assert!(max > 3, "the listing must not fit in 10 rows");

        typed(&mut a, "?k");
        assert_eq!(help_scroll(&a), 0);
        send(&mut a, [ch('j'), press(KeyCode::Down)]);
        assert_eq!(help_scroll(&a), 2);
        send(&mut a, [ch('k')]);
        assert_eq!(help_scroll(&a), 1);
        send(&mut a, [ch('G')]);
        assert_eq!(help_scroll(&a), max);
        send(&mut a, [ch('j')]);
        assert_eq!(help_scroll(&a), max);
        send(&mut a, [ch('g')]);
        assert_eq!(help_scroll(&a), 0);
        send(&mut a, [ctrl('d')]);
        assert_eq!(help_scroll(&a), a.overlay_body_rows() / 2);
        send(&mut a, [ctrl('u')]);
        assert_eq!(help_scroll(&a), 0);
        send(&mut a, [press(KeyCode::End)]);
        assert_eq!(help_scroll(&a), max);

        send(&mut a, [Event::Resize(80, 40)]);
        assert_eq!(help_scroll(&a), 0, "a taller window fits it all");
    }

    fn overlay_text(a: &App) -> Vec<String> {
        let Mode::Overlay { kind, .. } = a.mode else {
            panic!("not in an overlay: {:?}", a.mode);
        };
        a.overlay_rows(kind)
            .iter()
            .map(|r| r.text.clone())
            .collect()
    }

    #[test]
    fn i_opens_the_review_with_its_whole_note_and_its_closing_keys_go_back() {
        let note = "Over to you.\nSee c1 and c3.\nThanks";
        for close in [ch('i'), press(KeyCode::Esc), ch('q'), ctrl('c')] {
            let mut a = App::open(
                View {
                    review: handed_off(note),
                    ..showing(numbered(20), &[])
                },
                "scott",
                (80, 10),
            );
            send(&mut a, [ch('j')]);
            let before = snapshot(&a);

            send(&mut a, [ch('i')]);
            assert!(matches!(
                a.mode,
                Mode::Overlay {
                    kind: Overlay::Review,
                    back: OverlayBack::Normal,
                    ..
                }
            ));
            let text = overlay_text(&a).join("\n");
            for needle in [
                "agent's turn",
                "scott",
                "2026-10-12T10:00:00Z",
                "r2",
                "Over to you.",
                "See c1 and c3.",
                "Thanks",
            ] {
                assert!(text.contains(needle), "{needle}: {text}");
            }
            assert_eq!(update(&mut a, close), None);
            assert_eq!(snapshot(&a), before);
        }
    }

    #[test]
    fn the_review_overlay_scrolls_and_clamps_like_help() {
        let note = (1..=20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = App::open(
            View {
                review: handed_off(&note),
                ..showing(numbered(5), &[])
            },
            "scott",
            (80, 8),
        );
        send(&mut a, [ch('i')]);
        let max = overlay_text(&a).len() - a.overlay_body_rows();
        assert!(max > 3, "the note must not fit");

        typed(&mut a, "jj");
        assert_eq!(help_scroll(&a), 2);
        send(&mut a, [ch('k')]);
        assert_eq!(help_scroll(&a), 1);
        send(&mut a, [ch('G')]);
        assert_eq!(help_scroll(&a), max);
        send(&mut a, [ch('g')]);
        assert_eq!(help_scroll(&a), 0);
    }

    #[test]
    fn a_narrow_review_listing_wraps_each_line_of_the_entry() {
        let rows = review_rows(&handed_off("one two three\nfour"), 12);
        let shown: Vec<(&str, bool)> = rows
            .iter()
            .map(|row| (row.text.as_str(), row.heading))
            .collect();

        assert_eq!(
            shown,
            vec![
                ("agent's turn", true),
                ("", false),
                ("hand-off by ", false),
                ("scott ", false),
                ("(human)", false),
                ("at ", false),
                ("2026-10-12T1", false),
                ("0:00:00Z on ", false),
                ("r2", false),
                ("", false),
                ("one two ", false),
                ("three", false),
                ("four", false),
            ]
        );
    }

    #[test]
    fn a_fresh_review_overlay_says_nothing_was_handed_off() {
        let mut a = app(5, 10);

        send(&mut a, [ch('i')]);

        let text = overlay_text(&a).join("\n");
        assert!(text.contains("your turn"), "{text}");
        assert!(text.contains("no hand-off yet"), "{text}");
    }
}
