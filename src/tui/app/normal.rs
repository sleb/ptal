//! The file view's keys: [`Mode::Normal`], [`Mode::Visual`] and the
//! movement they share.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Back, Effect, Mode, Mods, Overlay, OverlayBack, Scroll, Target};

impl App {
    pub(super) fn move_to(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.clamp_view();
    }

    /// Move to the nearest covered line strictly after (or before) the
    /// cursor; stay put when there is none.
    fn jump_to_covered(&mut self, forward: bool) {
        // Only the side being searched: a marker scans every thread.
        let side = if forward {
            self.cursor + 1..self.lines.len()
        } else {
            0..self.cursor
        };
        let covered = side.filter(|&line| self.marker(line).is_some());
        if let Some(line) = nearest(covered, self.cursor, forward) {
            self.move_to(line);
        }
    }

    /// Move to the start of the nearest change strictly after (or before)
    /// the cursor; stay put when there is none.
    fn jump_to_change(&mut self, forward: bool) {
        // Sorted already: `changes` is one hunk each, in file order.
        let starts = self
            .changes
            .iter()
            .map(|change| (change.range.start as usize).saturating_sub(1));
        if let Some(line) = nearest(starts, self.cursor, forward) {
            self.move_to(line);
        }
    }

    /// Move `top` and the cursor by the whole lines, from `top` down (or
    /// above `top` up), that fit in half the view's rows; at least one line.
    /// Past either end of the file a line counts as one row, so the cursor
    /// still moves half a page when the view cannot.
    fn scroll_half_page(&mut self, down: bool) {
        let half = (self.height() / 2).max(1);
        let rows_at = |step: usize| {
            let line = if down {
                self.top.checked_add(step)
            } else {
                self.top.checked_sub(step + 1)
            };
            line.filter(|&line| line < self.lines.len())
                .map_or(1, |line| self.rows_of(line))
        };
        let (mut used, mut step) = (0, 0);
        loop {
            used += rows_at(step);
            if used > half {
                break;
            }
            step += 1;
        }
        let step = step.max(1);
        if down {
            self.top += step;
            self.cursor += step;
        } else {
            self.top = self.top.saturating_sub(step);
            self.cursor = self.cursor.saturating_sub(step);
        }
        self.clamp_view();
    }
}

/// The first of `sorted` strictly after `from` (`forward`), or the last
/// strictly before it; `None` when there is none that way.
///
/// `]`/`[` and `}`/`{` both jump with it, over the covered lines and the
/// change starts.
fn nearest(
    mut sorted: impl DoubleEndedIterator<Item = usize>,
    from: usize,
    forward: bool,
) -> Option<usize> {
    if forward {
        sorted.find(|&line| line > from)
    } else {
        sorted.rev().find(|&line| line < from)
    }
}

/// Apply a key that moves the cursor; `false` if `key` is not one.
///
/// Normal and Visual share it, so the two modes move alike.
fn movement(app: &mut App, key: KeyEvent) -> bool {
    if let Some(scroll) = Scroll::of(key) {
        match scroll {
            Scroll::Up => app.move_to(app.cursor.saturating_sub(1)),
            Scroll::Down => app.move_to(app.cursor + 1),
            Scroll::HalfUp => app.scroll_half_page(false),
            Scroll::HalfDown => app.scroll_half_page(true),
            Scroll::Top => app.move_to(0),
            Scroll::Bottom => app.move_to(app.last_line()),
        }
        return true;
    }
    let mods = Mods::of(key);
    let (chord, ctrl_only) = (mods.chord(), mods.ctrl_only());
    match key.code {
        KeyCode::Char(']') if !ctrl_only => app.jump_to_covered(true),
        KeyCode::Char('[') if !ctrl_only => app.jump_to_covered(false),
        KeyCode::Char('}') if !ctrl_only => app.jump_to_change(true),
        KeyCode::Char('{') if !ctrl_only => app.jump_to_change(false),
        KeyCode::Char('n') if !chord => app.repeat_search(true),
        KeyCode::Char('N') if !chord => app.repeat_search(false),
        _ => return false,
    }
    true
}

/// `q` or `ctrl+c`: quit in Normal mode. Every sub-mode that is not typing
/// leaves on these or `esc` instead, so one stray `q` cannot quit.
fn is_quit(key: KeyEvent) -> bool {
    let ctrl = Mods::of(key).ctrl();
    matches!(key.code, KeyCode::Char('c') if ctrl)
        || matches!(key.code, KeyCode::Char('q') if !ctrl)
}

/// [`is_quit`] or `esc`: back to Normal from Visual or the panel.
pub(super) fn is_leave(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || is_quit(key)
}

pub(super) fn normal_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    if is_quit(key) {
        return Some(Effect::Quit);
    }
    if movement(app, key) {
        return None;
    }
    let ctrl = Mods::of(key).ctrl();
    match key.code {
        KeyCode::Char('?') if !ctrl => app.open_overlay(Overlay::Help, OverlayBack::Normal),
        KeyCode::Char('i') if !ctrl => app.open_overlay(Overlay::Review, OverlayBack::Normal),
        KeyCode::Char('H') if !ctrl => app.mode = Mode::input(Back::Normal, Target::HandOff),
        KeyCode::Char('C') if !ctrl => {
            let close = Target::Close {
                verdict: None,
                confirm_asked: false,
            };
            app.mode = Mode::input(Back::Normal, close);
        }
        KeyCode::Char('O') if !ctrl => return Some(Effect::ReopenReview),
        KeyCode::Char('v' | 'V') if !ctrl && !app.lines.is_empty() => {
            app.mode = Mode::Visual { anchor: app.cursor };
        }
        KeyCode::Char('/') if !ctrl => app.start_search(None),
        KeyCode::Esc => app.search = None,
        KeyCode::Enter if !app.threads_on(app.cursor).is_empty() => app.open_panel(),
        KeyCode::Enter if !app.lines.is_empty() => {
            app.mode = Mode::input(Back::Normal, Target::NewThread);
        }
        _ => {}
    }
    None
}

/// Keys in [`Mode::Visual`]: movement extends the selection and `enter`
/// opens a comment box for it; nothing here quits or writes.
pub(super) fn visual_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Visual { anchor } = app.mode else {
        return None;
    };
    if is_leave(key) {
        app.mode = Mode::Normal;
        return None;
    }
    if movement(app, key) {
        return None;
    }
    let ctrl = Mods::of(key).ctrl();
    match key.code {
        KeyCode::Char('v' | 'V') if !ctrl => app.mode = Mode::Normal,
        KeyCode::Char('/') if !ctrl => app.start_search(Some(anchor)),
        KeyCode::Enter => {
            app.mode = Mode::input(Back::Visual { anchor }, Target::NewThread);
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::ChangeKind;
    use crate::model::{State, Status};
    use crate::review::{ThreadWrite, View};
    use crate::tui::app::fixtures::*;
    use crate::tui::app::{Event, update};
    use crate::tui::test_support::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyModifiers};

    #[test]
    fn j_and_k_move_the_cursor_and_clamp_at_both_ends() {
        let mut a = app(3, 10);

        send(&mut a, [ch('k')]);
        assert_eq!(a.cursor, 0);
        typed(&mut a, "jjjj");
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('k')]);
        assert_eq!(a.cursor, 1);
    }

    #[test]
    fn g_and_capital_g_jump_to_first_and_last_line() {
        let mut a = app(20, 7); // view height 5

        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (19, 15));
        send(&mut a, [ch('g')]);
        assert_eq!(at(&a), (0, 0));
    }

    #[test]
    fn ctrl_d_and_ctrl_u_move_half_a_page_and_the_view_follows() {
        let mut a = app(20, 12); // view height 10, half page 5

        send(&mut a, [ctrl('d')]);
        assert_eq!(at(&a), (5, 5));
        send(&mut a, [ctrl('d'), ctrl('d'), ctrl('d')]);
        assert_eq!(at(&a), (19, 10), "clamped at the last line and last page");
        send(&mut a, [ctrl('u')]);
        assert_eq!(at(&a), (14, 5));
        send(&mut a, [ctrl('u'), ctrl('u'), ctrl('u')]);
        assert_eq!(at(&a), (0, 0));
    }

    #[test]
    fn half_page_is_at_least_one_line() {
        let mut a = app(5, 3); // view height 1

        send(&mut a, [ctrl('d')]);

        assert_eq!(at(&a), (1, 1));
    }

    #[test]
    fn cursor_stays_inside_the_view_through_a_scripted_walk() {
        let mut a = app(30, 9); // view height 7
        let walk = "jjjjjjjjjjGkkkkkgjjjjjjjjjj";
        let mut events: Vec<Event> = walk.chars().map(ch).collect();
        events.extend([ctrl('d'), ctrl('u'), ctrl('d'), ctrl('d')]);

        for e in events {
            update(&mut a, e);
            assert!(
                a.top <= a.cursor && a.cursor < a.top + 7,
                "cursor {} outside top {}..+7",
                a.cursor,
                a.top
            );
        }
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        for event in [ch('q'), ctrl('c')] {
            let mut a = app(3, 10);
            assert_eq!(update(&mut a, event), Some(Effect::Quit));
        }
    }

    #[test]
    fn unbound_keys_and_key_releases_do_nothing() {
        let mut a = app(3, 10);
        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));

        assert_eq!(update(&mut a, ch('z')), None);
        assert_eq!(update(&mut a, press(KeyCode::F(5))), None);
        assert_eq!(update(&mut a, release), None);
        assert_eq!(at(&a), (0, 0));
    }

    #[test]
    fn arrow_page_home_end_keys_match_their_letter_aliases() {
        let pairs = [
            (KeyCode::Down, ch('j')),
            (KeyCode::Up, ch('k')),
            (KeyCode::PageDown, ctrl('d')),
            (KeyCode::PageUp, ctrl('u')),
            (KeyCode::End, ch('G')),
            (KeyCode::Home, ch('g')),
        ];
        // Start mid-file so Up/PageUp/Home have something to undo.
        let prelude = [
            ch('j'),
            ch('j'),
            ch('j'),
            ch('j'),
            ch('j'),
            ch('j'),
            ch('j'),
        ];

        for (code, letter) in pairs {
            let (mut a, mut b) = (app(30, 8), app(30, 8));
            send(&mut a, prelude.clone());
            send(&mut b, prelude.clone());

            send(&mut a, [press(code)]);
            send(&mut b, [letter.clone()]);

            assert_eq!(at(&a), at(&b), "{code:?} vs {letter:?}");
        }
    }

    #[test]
    fn an_empty_file_ignores_moves_without_panicking() {
        let mut a = App::open(showing(Vec::new(), &[]), "scott", (80, 10));

        send(
            &mut a,
            [ch('j'), ch('k'), ch('G'), ch('g'), ctrl('d'), ctrl('u')],
        );

        assert_eq!(at(&a), (0, 0));
    }

    #[test]
    fn nearest_is_strictly_after_or_before_and_none_past_the_ends() {
        let set = || [2, 5, 9].into_iter();

        assert_eq!(nearest(set(), 5, true), Some(9), "itself is skipped");
        assert_eq!(nearest(set(), 5, false), Some(2));
        assert_eq!(nearest(set(), 6, true), Some(9), "from between two");
        assert_eq!(nearest(set(), 6, false), Some(5));
        assert_eq!(nearest(set(), 0, true), Some(2), "from before the first");
        assert_eq!(nearest(set(), 0, false), None);
        assert_eq!(nearest(set(), 12, false), Some(9), "from after the last");
        assert_eq!(nearest(set(), 12, true), None);
        assert_eq!(nearest(std::iter::empty(), 3, true), None, "an empty set");
        assert_eq!(nearest(std::iter::empty(), 3, false), None);
    }

    // --- ] and [ ---

    fn jump_app() -> App {
        let threads = [
            view("c1", (3, 3), Status::Open, State::Current),
            view("c2", (6, 7), resolved(), State::Current),
            view("c3", (10, 10), Status::Open, State::Outdated),
        ];
        let lines = (1..=12).map(|i| i.to_string()).collect();
        App::open(showing(lines, &threads), "scott", (80, 6))
    }

    #[test]
    fn brackets_jump_between_covered_lines_and_stop_at_the_ends() {
        let mut a = jump_app();
        let mut visited = vec![];

        for _ in 0..5 {
            send(&mut a, [ch(']')]);
            visited.push(a.cursor);
        }
        assert_eq!(visited, [2, 5, 6, 9, 9], "no wrap past the last");

        let mut back = vec![];
        for _ in 0..5 {
            send(&mut a, [ch('[')]);
            back.push(a.cursor);
        }
        assert_eq!(back, [6, 5, 2, 2, 2], "no wrap before the first");
    }

    #[test]
    fn jumping_scrolls_the_view_to_the_cursor() {
        let mut a = jump_app();

        typed(&mut a, "]]]]");

        assert_eq!(a.cursor, 9);
        assert!(a.top <= a.cursor && a.cursor < a.top + a.height());
    }

    #[test]
    fn brackets_do_nothing_without_threads_and_are_text_in_input() {
        let mut none = app(5, 10);
        typed(&mut none, "][");
        assert_eq!(none.cursor, 0);

        let mut a = input_app();
        typed(&mut a, "][");
        assert_eq!(draft(&a), ("][", 2));
        assert_eq!(a.cursor, 2);
    }

    // --- visual selection ---

    #[test]
    fn v_starts_a_selection_of_the_cursor_line() {
        let a = visual_at(10, 12, 3);

        assert_eq!(a.mode, Mode::Visual { anchor: 3 });
        assert_eq!(a.selection(), (3, 3));
    }

    #[test]
    fn outside_visual_the_selection_is_the_cursor_line() {
        let mut a = app(10, 12);
        typed(&mut a, "jj");

        assert_eq!(a.selection(), (2, 2));
    }

    #[test]
    fn j_and_k_extend_the_selection_either_way_and_across_the_anchor() {
        let mut down = visual_at(10, 12, 3);
        typed(&mut down, "jj");
        assert_eq!(down.selection(), (3, 5));
        assert_eq!(down.cursor, 5, "the cursor is the moving end");

        let mut up = visual_at(10, 12, 3);
        typed(&mut up, "kk");
        assert_eq!(up.selection(), (1, 3));

        send(&mut up, (0..4).map(|_| press(KeyCode::Down)));
        assert_eq!(up.selection(), (3, 5), "crossed the anchor");
        send(&mut up, [press(KeyCode::Up)]);
        assert_eq!(up.selection(), (3, 4));
        assert_eq!(up.mode, Mode::Visual { anchor: 3 });
    }

    #[test]
    fn every_movement_key_moves_the_end_and_the_view_follows_it() {
        let mut a = visual_at(20, 7, 2); // view height 5

        send(&mut a, [ch('G')]);
        assert_eq!((a.selection(), at(&a)), ((2, 19), (19, 15)));
        send(&mut a, [ch('g')]);
        assert_eq!((a.selection(), at(&a)), ((0, 2), (0, 0)));
        send(&mut a, [press(KeyCode::End), press(KeyCode::Home)]);
        assert_eq!(a.selection(), (0, 2));
        send(&mut a, [ctrl('d')]);
        assert_eq!(a.selection(), (2, 2));
        send(&mut a, [press(KeyCode::PageDown), press(KeyCode::PageUp)]);
        assert_eq!(a.selection(), (2, 2));
        send(&mut a, [ctrl('d'), ctrl('d'), ctrl('u')]);
        assert_eq!(a.selection(), (2, 4));
        assert_eq!(a.mode, Mode::Visual { anchor: 2 });
        assert!(a.top <= a.cursor && a.cursor < a.top + 5);
    }

    #[test]
    fn brackets_move_the_end_to_the_next_and_previous_threaded_line() {
        let lines = (1..=10).map(|i| format!("line {i}")).collect();
        let threads = [view("c1", (7, 8), Status::Open, State::Current)];
        let mut a = App::open(showing(lines, &threads), "scott", (40, 12));
        typed(&mut a, "jv");

        send(&mut a, [ch(']')]);
        assert_eq!(a.selection(), (1, 6));
        typed(&mut a, "G[");
        assert_eq!(a.selection(), (1, 7));
    }

    #[test]
    fn capital_v_selects_exactly_as_v_does() {
        let mut a = app(10, 12);
        typed(&mut a, "jVj");

        assert_eq!(a.mode, Mode::Visual { anchor: 1 });
        assert_eq!(a.selection(), (1, 2));
    }

    #[test]
    fn leaving_visual_keeps_the_cursor_and_never_quits() {
        for leave in [press(KeyCode::Esc), ch('q'), ctrl('c'), ch('v'), ch('V')] {
            let mut a = visual_at(10, 12, 3);
            send(&mut a, [ch('j')]);

            assert_eq!(update(&mut a, leave.clone()), None, "{leave:?}");

            assert_eq!(a.mode, Mode::Normal, "{leave:?}");
            assert_eq!(a.cursor, 4);
        }
    }

    #[test]
    fn other_keys_do_nothing_in_visual() {
        let mut a = visual_at(10, 12, 3);

        for key in [ch('a'), ch('x'), ch('r'), press(KeyCode::Tab)] {
            assert_eq!(update(&mut a, key), None);
        }

        assert_eq!(
            (a.mode.clone(), a.selection()),
            (Mode::Visual { anchor: 3 }, (3, 3))
        );
    }

    #[test]
    fn v_on_an_empty_file_stays_normal() {
        let mut a = App::open(showing(vec![], &[]), "scott", (40, 10));

        send(&mut a, [ch('v')]);

        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn enter_in_visual_opens_the_box_for_the_range_and_saves_it_either_way() {
        for (moves, lines, (start, end)) in [("jj", (3, 5), (4, 6)), ("kk", (1, 3), (2, 4))] {
            let mut a = visual_at(10, 12, 3);
            typed(&mut a, moves);

            send(&mut a, [press(KeyCode::Enter)]);
            assert_eq!(draft(&a), ("", 0));
            assert_eq!(a.selection(), lines, "kept in Input");
            typed(&mut a, "why?");

            assert_eq!(
                update(&mut a, ctrl('s')),
                Some(Effect::Add {
                    start,
                    end,
                    body: "why?".into()
                })
            );
        }
    }

    #[test]
    fn enter_in_visual_on_a_threaded_line_comments_and_never_opens_the_panel() {
        let mut a = panel_app(&[chatty("c1", (2, 2), 1)], 20);
        send(&mut a, [ch('j'), ch('v'), press(KeyCode::Enter)]);

        assert!(matches!(
            a.mode,
            Mode::Input {
                back: Back::Visual { anchor: 1 },
                target: Target::NewThread,
                ..
            }
        ));
    }

    #[test]
    fn esc_in_the_box_returns_to_the_same_selection_and_again_to_normal() {
        for cancel in [press(KeyCode::Esc), ctrl('c')] {
            let mut a = visual_at(10, 12, 3);
            send(&mut a, [ch('j'), press(KeyCode::Enter)]);
            typed(&mut a, "draft");

            assert_eq!(update(&mut a, cancel.clone()), None);
            assert_eq!(update(&mut a, cancel.clone()), None);
            assert_eq!(a.mode, Mode::Visual { anchor: 3 });
            assert_eq!(a.selection(), (3, 4));

            assert_eq!(update(&mut a, cancel), None);
            assert_eq!((a.mode.clone(), a.cursor), (Mode::Normal, 4));
        }
    }

    #[test]
    fn a_saved_range_returns_to_normal_and_says_what_was_added() {
        let mut a = visual_at(10, 12, 3);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "x");
        update(&mut a, ctrl('s'));
        let threads = vec![chatty("c4", (4, 5), 1)];

        let text = same_text(&a);
        update(
            &mut a,
            Event::Saved {
                id: "c4".into(),
                done: ThreadWrite::Added,
                view: View { threads, ..text },
            },
        );

        assert_eq!((a.mode.clone(), a.cursor), (Mode::Normal, 4));
        assert_eq!(a.status.as_deref(), Some("added c4"));
    }

    #[test]
    fn a_failed_range_save_keeps_the_draft_and_the_selection() {
        let mut a = visual_at(10, 12, 3);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "x");
        update(&mut a, ctrl('s'));

        update(&mut a, Event::SaveFailed("locked".into()));

        assert_eq!(draft(&a), ("x", 1));
        assert_eq!(a.selection(), (3, 4));
        assert!(matches!(
            a.mode,
            Mode::Input {
                back: Back::Visual { anchor: 3 },
                ..
            }
        ));
    }

    // --- } and { ---

    fn change_app() -> App {
        let lines = (1..=12).map(|i| i.to_string()).collect();
        App::open(
            View {
                changes: vec![
                    change(3, 4, ChangeKind::Changed),
                    change(7, 7, ChangeKind::Removed),
                    change(10, 11, ChangeKind::Changed),
                ],
                ..showing(lines, &[])
            },
            "scott",
            (80, 6),
        )
    }

    #[test]
    fn braces_jump_between_change_starts_and_stop_at_the_ends() {
        let mut a = change_app();
        let mut visited = vec![];
        for _ in 0..4 {
            send(&mut a, [ch('}')]);
            visited.push(a.cursor);
        }
        assert_eq!(visited, [2, 6, 9, 9], "no wrap past the last");

        let mut back = vec![];
        for _ in 0..4 {
            send(&mut a, [ch('{')]);
            back.push(a.cursor);
        }
        assert_eq!(back, [6, 2, 2, 2], "no wrap before the first");
    }

    #[test]
    fn open_brace_inside_a_change_goes_to_the_previous_change() {
        let mut a = change_app();
        send(&mut a, std::iter::repeat_with(|| ch('j')).take(10));
        assert_eq!(a.cursor, 10, "inside the last change");

        send(&mut a, [ch('{')]);

        assert_eq!(a.cursor, 9, "its start is strictly before the cursor");
    }

    #[test]
    fn braces_do_nothing_without_changes() {
        let mut a = app(5, 10);
        typed(&mut a, "j}{");
        assert_eq!(a.cursor, 1);
    }

    #[test]
    fn jumping_to_a_change_past_wrapped_lines_keeps_the_cursor_on_screen() {
        let long = "word ".repeat(30);
        let lines = (1..=12)
            .map(|i| if i < 10 { long.clone() } else { i.to_string() })
            .collect();
        let mut a = App::open(
            View {
                changes: vec![change(11, 11, ChangeKind::Changed)],
                ..showing(lines, &[])
            },
            "scott",
            (20, 8),
        );

        send(&mut a, [ch('}')]);

        assert_eq!(a.cursor, 10);
        assert!(a.top <= a.cursor && a.cursor < a.top + a.height());
    }

    #[test]
    fn a_brace_in_visual_extends_the_selection() {
        let mut a = change_app();

        typed(&mut a, "v}");

        assert_eq!(a.selection(), (0, 2));
    }
}
