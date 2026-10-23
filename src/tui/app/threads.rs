//! The thread panel, [`Mode::Panel`]: reading, selecting, replying to,
//! resolving and reopening the threads on the cursor line.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::model::Status;

use super::{
    App, Back, Effect, Marker, Mode, Mods, Overlay, OverlayBack, Scroll, Target, is_leave,
};

impl App {
    /// Whether the panel's selected thread is resolved.
    pub fn selected_is_resolved(&self) -> bool {
        let Mode::Panel { selected, .. } = self.mode else {
            return false;
        };
        self.threads_on(self.cursor)
            .get(selected)
            .is_some_and(|view| view.thread.status != Status::Open)
    }

    /// Open the panel on the cursor line's threads, selecting the first one
    /// waiting on the author, else the first.
    pub(super) fn open_panel(&mut self) {
        let selected = self
            .threads_on(self.cursor)
            .iter()
            .position(|view| self.marker_of(view) == Marker::Waiting)
            .unwrap_or(0);
        self.mode = Mode::Panel {
            selected,
            scroll: 0,
        };
    }

    /// Select thread `index` and scroll its header into view.
    fn select_thread(&mut self, index: usize) {
        let Some(layout) = self.panel_layout() else {
            return;
        };
        let start = layout.thread_starts.get(index).copied().unwrap_or(0);
        let rows = self.panel_inner_rows();
        if let Mode::Panel { selected, scroll } = &mut self.mode {
            *selected = index;
            if start < *scroll || start >= *scroll + rows {
                *scroll = start;
            }
        }
    }

    /// Ask to resolve (`resolve`) or reopen the `selected` thread of the
    /// cursor line, or say why there is nothing to do. The thread's id is
    /// captured here, so a re-read before the write cannot retarget it.
    fn change_status(&mut self, selected: usize, resolve: bool) -> Option<Effect> {
        let view = *self.threads_on(self.cursor).get(selected)?;
        let id = view.thread.id.clone();
        let is_open = view.thread.status == Status::Open;
        if is_open != resolve {
            let now = if is_open { "open" } else { "resolved" };
            self.status = Some(format!("{id} is already {now}"));
            return None;
        }
        Some(if resolve {
            Effect::Resolve { id }
        } else {
            Effect::Reopen { id }
        })
    }

    /// Open the panel on the cursor line with thread `id` selected (the
    /// first thread if the re-read no longer lists it).
    pub(super) fn back_to_panel(&mut self, id: &str) {
        self.mode = Mode::Panel {
            selected: 0,
            scroll: 0,
        };
        let index = self
            .threads_on(self.cursor)
            .iter()
            .position(|t| t.thread.id == id);
        self.select_thread(index.unwrap_or(0));
    }
}

pub(super) fn panel_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Panel { selected, scroll } = app.mode else {
        return None;
    };
    if is_leave(key) {
        app.mode = Mode::Normal;
        return None;
    }
    let mods = Mods::of(key);
    let (ctrl, shift) = (mods.ctrl(), mods.shift());
    let count = app.threads_on(app.cursor).len().max(1);
    match key.code {
        KeyCode::Char('?') if !ctrl => {
            app.open_overlay(Overlay::Help, OverlayBack::Panel { selected, scroll });
        }
        KeyCode::Char('a') if !ctrl => {
            app.mode = Mode::input(Back::Panel { selected, scroll }, Target::NewThread);
        }
        KeyCode::Char('r') if !ctrl => {
            let id = app
                .threads_on(app.cursor)
                .get(selected)
                .map(|view| view.thread.id.clone());
            if let Some(id) = id {
                app.mode = Mode::input(Back::Panel { selected, scroll }, Target::Reply(id));
            }
        }
        KeyCode::Char('x') if !ctrl => return app.change_status(selected, true),
        KeyCode::Char('o') if !ctrl => return app.change_status(selected, false),
        KeyCode::Tab if !shift => app.select_thread((selected + 1) % count),
        KeyCode::Tab | KeyCode::BackTab => app.select_thread((selected + count - 1) % count),
        // The panel scrolls a row at a time; the other intents are unbound.
        _ => match Scroll::of(key) {
            Some(Scroll::Up) => {
                app.mode = Mode::Panel {
                    selected,
                    scroll: scroll.saturating_sub(1),
                };
            }
            Some(Scroll::Down) => {
                app.mode = Mode::Panel {
                    selected,
                    scroll: scroll + 1,
                };
            }
            Some(Scroll::HalfUp | Scroll::HalfDown | Scroll::Top | Scroll::Bottom) | None => {}
        },
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Comment, State};
    use crate::review::{ThreadWrite, View};
    use crate::tui::app::fixtures::*;
    use crate::tui::app::{Event, update};
    use crate::tui::test_support::*;
    use ratatui::crossterm::event::KeyModifiers;

    // --- thread panel ---

    fn panel(a: &App) -> (usize, usize) {
        match a.mode {
            Mode::Panel { selected, scroll } => (selected, scroll),
            _ => panic!("not in Panel mode: {:?}", a.mode),
        }
    }

    fn shift_tab() -> Event {
        Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
    }

    #[test]
    fn enter_on_a_threaded_line_opens_the_panel_and_elsewhere_the_input_box() {
        let mut a = panel_app(&[chatty("c1", (2, 2), 1)], 20);

        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        assert_eq!(panel(&a), (0, 0));

        send(
            &mut a,
            [press(KeyCode::Esc), ch('j'), press(KeyCode::Enter)],
        );
        assert!(matches!(a.mode, Mode::Input { .. }));
    }

    #[test]
    fn the_panel_lists_every_thread_on_the_line_and_only_those() {
        let threads = [
            chatty("c1", (2, 4), 1),
            chatty("c2", (3, 3), 1),
            chatty("c3", (5, 5), 1),
        ];
        let mut a = panel_app(&threads, 20);

        send(&mut a, [ch('j'), ch('j'), press(KeyCode::Enter)]);
        assert_eq!(listed(&a), 2, "c1 and c2 overlap line 3");

        send(
            &mut a,
            [press(KeyCode::Esc), ch('j'), press(KeyCode::Enter)],
        );
        assert_eq!(listed(&a), 1, "only c1 reaches line 4");
    }

    #[test]
    fn the_first_waiting_thread_is_selected_else_the_first() {
        let mine = said_by(view("c1", (1, 1), Status::Open, State::Current), &["scott"]);
        let theirs = chatty("c2", (1, 1), 1);
        let mut a = panel_app(&[mine.clone(), theirs], 20);
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(panel(&a).0, 1);

        let mut b = panel_app(&[mine], 20);
        send(&mut b, [press(KeyCode::Enter)]);
        assert_eq!(panel(&b).0, 0);

        let done = said_by(view("c3", (1, 1), resolved(), State::Current), &["claude"]);
        let mut c = panel_app(&[done], 20);
        send(&mut c, [press(KeyCode::Enter)]);
        assert_eq!(panel(&c).0, 0, "a resolved-only line opens too");
    }

    #[test]
    fn tab_and_shift_tab_cycle_the_selection_and_wrap() {
        let threads = [
            chatty("c1", (1, 1), 1),
            chatty("c2", (1, 1), 1),
            chatty("c3", (1, 1), 1),
        ];
        let mut a = panel_app(&threads, 20);
        send(&mut a, [press(KeyCode::Enter)]);

        send(&mut a, [press(KeyCode::Tab), press(KeyCode::Tab)]);
        assert_eq!(panel(&a).0, 2);
        send(&mut a, [press(KeyCode::Tab)]);
        assert_eq!(panel(&a).0, 0, "wraps forward");
        send(&mut a, [shift_tab()]);
        assert_eq!(panel(&a).0, 2, "wraps backward");
        let shifted_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        send(&mut a, [Event::Key(shifted_tab)]);
        assert_eq!(panel(&a).0, 1, "SHIFT+Tab counts as BackTab");
    }

    #[test]
    fn selecting_a_thread_scrolls_its_header_into_view() {
        // 12 rows: panel 6 (4 inside), each thread is 3 + 2n rows.
        let threads = [chatty("c1", (1, 1), 5), chatty("c2", (1, 1), 5)];
        let mut a = panel_app(&threads, 12);
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(panel(&a), (0, 0));

        send(&mut a, [press(KeyCode::Tab)]);
        let start = a.panel_layout().unwrap().thread_starts[1];
        assert_eq!(panel(&a), (1, start));

        send(&mut a, [press(KeyCode::Tab)]);
        assert_eq!(panel(&a), (0, 0));
    }

    #[test]
    fn j_and_k_scroll_the_panel_clamped_without_moving_the_cursor() {
        let mut a = panel_app(&[chatty("c1", (1, 1), 5)], 12);
        send(&mut a, [press(KeyCode::Enter)]);
        let rows = a.panel_layout().unwrap().lines.len();
        let max = rows - a.panel_inner_rows();

        send(&mut a, [ch('k'), press(KeyCode::Up)]);
        assert_eq!(panel(&a).1, 0);
        for _ in 0..rows + 3 {
            send(&mut a, [ch('j')]);
        }
        assert_eq!(panel(&a).1, max);
        send(&mut a, [press(KeyCode::Up)]);
        assert_eq!(panel(&a).1, max - 1);
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn panel_scroll_clamps_to_rows_wrapped_by_cell_width() {
        // 40 cols: 34 cells after the body indent, so 17 wide characters a
        // row and 68 of them make 4 rows (2 if counted by char).
        let mut wide = view("c1", (1, 1), Status::Open, State::Current);
        wide.thread.comments = vec![Comment {
            id: "c1-0".into(),
            author: "claude".into(),
            body: "漢".repeat(68),
            created_at: "2026-10-12T10:00:00Z".into(),
            revision: None,
        }];
        let mut a = panel_app(&[wide], 12); // 4 rows inside the panel
        send(&mut a, [press(KeyCode::Enter)]);

        for _ in 0..10 {
            send(&mut a, [ch('j')]);
        }

        let layout = a.panel_layout().unwrap();
        assert_eq!(layout.lines.len(), 7, "header, quote, author, 4 body rows");
        assert_eq!(panel(&a).1, 3);
        let last = format!("    {}", "漢".repeat(17));
        assert_eq!(layout.lines[3 + a.panel_inner_rows() - 1].text, last);
    }

    #[test]
    fn esc_q_and_ctrl_c_close_the_panel_without_quitting() {
        for close in [press(KeyCode::Esc), ch('q'), ctrl('c')] {
            let mut a = panel_app(&[chatty("c1", (2, 2), 1)], 20);
            send(&mut a, [ch('j'), press(KeyCode::Enter)]);

            assert_eq!(update(&mut a, close), None);

            assert_eq!(a.mode, Mode::Normal);
            assert_eq!(a.cursor, 1);
        }
    }

    #[test]
    fn the_cursor_cannot_drift_from_the_listed_line_inside_the_panel() {
        let mut a = panel_app(&[chatty("c1", (2, 2), 1), chatty("c2", (6, 6), 1)], 20);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);

        for event in [
            press(KeyCode::Enter),
            ch(']'),
            ch('['),
            ch('g'),
            ch('G'),
            ctrl('d'),
            ch('z'),
        ] {
            assert_eq!(update(&mut a, event), None);
        }

        assert_eq!(a.cursor, 1);
        assert_eq!(panel(&a), (0, 0));
    }

    #[test]
    fn the_panel_shrinks_the_view_keeps_the_cursor_visible_and_closing_restores() {
        let mut a = panel_app(&[chatty("c1", (6, 6), 1)], 12); // height 10
        typed(&mut a, "Ggjjjjj");
        assert_eq!(a.cursor, 5);

        send(&mut a, [press(KeyCode::Enter)]);

        assert_eq!(
            a.height(),
            4,
            "12 rows minus the banner, a 6-row panel and the footer"
        );
        assert!(a.top <= a.cursor && a.cursor < a.top + 4);
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.height(), 10);
    }

    #[test]
    fn a_panel_on_a_tiny_terminal_does_not_panic() {
        for rows in 0..=6 {
            let mut a = panel_app(&[chatty("c1", (1, 1), 3)], rows);
            send(
                &mut a,
                [press(KeyCode::Enter), ch('j'), press(KeyCode::Tab)],
            );
            assert!(a.height() >= 1);
        }
    }

    #[test]
    fn a_reread_that_drops_the_thread_closes_the_panel() {
        let mut a = panel_app(&[chatty("c1", (1, 1), 1)], 20);
        send(&mut a, [press(KeyCode::Enter)]);
        let text = same_text(&a);

        send(
            &mut a,
            [Event::Saved {
                id: "c9".into(),
                done: ThreadWrite::Added,
                view: text,
            }],
        );

        assert_eq!(a.mode, Mode::Normal);
    }

    fn in_panel_input() -> App {
        let mut a = panel_app(&[chatty("c1", (2, 2), 5)], 12);
        send(&mut a, [ch('j'), press(KeyCode::Enter), ch('j'), ch('a')]);
        a
    }

    #[test]
    fn a_enters_input_for_the_cursor_line_from_the_panel_only() {
        let mut a = in_panel_input();
        assert_eq!(draft(&a), ("", 0));
        assert_eq!(a.cursor, 1);

        let mut normal = panel_app(&[chatty("c1", (2, 2), 1)], 12);
        send(&mut normal, [ch('a')]);
        assert_eq!(normal.mode, Mode::Normal);

        typed(&mut a, "more a");
        assert_eq!(draft(&a), ("more a", 6), "a is text once typing");
    }

    #[test]
    fn ctrl_s_in_a_panel_input_saves_on_the_cursor_line() {
        let mut a = in_panel_input();
        typed(&mut a, "new");

        let effect = update(&mut a, ctrl('s'));

        assert_eq!(
            effect,
            Some(Effect::Add {
                start: 2,
                end: 2,
                body: "new".into()
            })
        );
    }

    #[test]
    fn saved_returns_to_the_panel_with_the_new_thread_selected() {
        let mut a = in_panel_input();
        typed(&mut a, "new");
        update(&mut a, ctrl('s'));
        let threads = vec![
            chatty("c1", (2, 2), 5),
            said_by(view("c9", (2, 2), Status::Open, State::Current), &["scott"]),
        ];

        let text = same_text(&a);
        update(
            &mut a,
            Event::Saved {
                id: "c9".into(),
                done: ThreadWrite::Added,
                view: View { threads, ..text },
            },
        );

        assert_eq!(listed(&a), 2);
        let start = a.panel_layout().unwrap().thread_starts[1];
        let max = a.panel_layout().unwrap().lines.len() - a.panel_inner_rows();
        assert_eq!(panel(&a), (1, start.min(max)));
        assert_eq!(a.status.as_deref(), Some("added c9"));
    }

    #[test]
    fn esc_and_ctrl_c_return_to_the_same_panel_view() {
        for cancel in [press(KeyCode::Esc), ctrl('c')] {
            let mut a = in_panel_input();
            typed(&mut a, "draft");
            let before = (panel_after_cancel(&a), a.cursor);

            update(&mut a, cancel.clone());
            update(&mut a, cancel);

            assert_eq!((panel(&a), a.cursor), (before.0, before.1));
        }
    }

    fn panel_after_cancel(a: &App) -> (usize, usize) {
        match &a.mode {
            Mode::Input {
                back: Back::Panel { selected, scroll },
                ..
            } => (*selected, *scroll),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failed_save_keeps_the_panel_input_and_its_draft() {
        let mut a = in_panel_input();
        typed(&mut a, "x");
        update(&mut a, ctrl('s'));

        update(&mut a, Event::SaveFailed("locked".into()));

        assert_eq!(draft(&a), ("x", 1));
        assert!(matches!(
            a.mode,
            Mode::Input {
                back: Back::Panel { .. },
                ..
            }
        ));
    }

    #[test]
    fn a_blank_draft_from_the_panel_stays_put() {
        let mut a = in_panel_input();
        typed(&mut a, "  ");

        assert_eq!(update(&mut a, ctrl('s')), None);
        assert_eq!(draft(&a), ("  ", 2));
    }

    // --- replying from the panel ---

    fn reply_target(a: &App) -> Option<&str> {
        match &a.mode {
            Mode::Input {
                target: Target::Reply(id),
                ..
            } => Some(id),
            Mode::Input { .. } => None,
            other => panic!("not in Input mode: {other:?}"),
        }
    }

    #[test]
    fn r_in_the_panel_opens_a_reply_to_the_selected_threads_id() {
        let mut a = two_threads_panel();

        assert_eq!(update(&mut a, ch('r')), None);

        assert_eq!(reply_target(&a), Some("c2"));
        assert_eq!(draft(&a), ("", 0));
        assert!(matches!(
            a.mode,
            Mode::Input {
                back: Back::Panel { selected: 1, .. },
                ..
            }
        ));
    }

    #[test]
    fn r_is_unbound_in_normal_and_text_in_input() {
        let mut normal = panel_app(&[chatty("c1", (2, 2), 1)], 12);
        typed(&mut normal, "jr");
        assert_eq!(normal.mode, Mode::Normal);

        let mut a = in_panel_input();
        typed(&mut a, "rrr");
        assert_eq!(draft(&a), ("rrr", 3));
        assert_eq!(reply_target(&a), None, "a is an add, not a reply");
    }

    #[test]
    fn ctrl_s_in_a_reply_asks_to_reply_to_that_thread_not_to_add() {
        let mut a = two_threads_panel();
        send(&mut a, [ch('r')]);
        typed(&mut a, "ok");

        let effect = update(&mut a, ctrl('s'));

        assert_eq!(
            effect,
            Some(Effect::Reply {
                id: "c2".into(),
                body: "ok".into()
            })
        );
        assert_eq!(draft(&a), ("ok", 2));
    }

    #[test]
    fn a_blank_reply_stays_put_and_esc_returns_to_the_same_panel_view() {
        for cancel in [press(KeyCode::Esc), ctrl('c')] {
            let mut a = two_threads_panel();
            let before = panel(&a);
            send(&mut a, [ch('r')]);
            typed(&mut a, "  ");
            assert_eq!(update(&mut a, ctrl('s')), None);
            assert_eq!(draft(&a), ("  ", 2));

            update(&mut a, cancel);

            assert_eq!(panel(&a), before);
        }
    }

    #[test]
    fn a_saved_reply_returns_to_the_panel_on_that_thread_and_says_replied() {
        let mut a = two_threads_panel();
        send(&mut a, [ch('r')]);
        typed(&mut a, "ok");
        update(&mut a, ctrl('s'));
        let threads = vec![
            chatty("c1", (2, 2), 1),
            said_by(
                view("c2", (2, 2), Status::Open, State::Current),
                &["claude", "scott"],
            ),
        ];

        let text = same_text(&a);
        update(
            &mut a,
            Event::Saved {
                id: "c2".into(),
                done: ThreadWrite::Replied,
                view: View { threads, ..text },
            },
        );

        assert_eq!(panel(&a).0, 1);
        assert_eq!(a.status.as_deref(), Some("replied c2"));
    }

    #[test]
    fn a_failed_reply_keeps_the_draft_the_target_and_the_panel_origin() {
        let mut a = two_threads_panel();
        send(&mut a, [ch('r')]);
        typed(&mut a, "ok");
        update(&mut a, ctrl('s'));

        update(&mut a, Event::SaveFailed("locked".into()));

        assert_eq!(draft(&a), ("ok", 2));
        assert_eq!(reply_target(&a), Some("c2"));
        assert_eq!(a.status.as_deref(), Some("locked"));
        assert!(matches!(
            a.mode,
            Mode::Input {
                back: Back::Panel { .. },
                ..
            }
        ));
    }

    // --- resolving and reopening from the panel ---

    /// Line 2 holds an open `c1` and a resolved `c2`; `c1` is selected.
    fn open_and_resolved_panel() -> App {
        let threads = [
            chatty("c1", (2, 2), 1),
            said_by(view("c2", (2, 2), resolved(), State::Current), &["claude"]),
        ];
        let mut a = panel_app(&threads, 20);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        a
    }

    #[test]
    fn altgr_characters_act_as_the_plain_key_in_the_panel() {
        let mut a = open_and_resolved_panel();
        let before = a.mode.clone();
        assert_eq!(update(&mut a, alt_graph('c')), None);
        assert_eq!(a.mode, before, "not ctrl+c");

        assert_eq!(
            update(&mut a, alt_graph('x')),
            Some(Effect::Resolve { id: "c1".into() })
        );
        send(&mut a, [alt_graph('r')]);
        assert!(matches!(a.mode, Mode::Input { .. }), "{:?}", a.mode);

        let mut a = open_and_resolved_panel();
        send(&mut a, [alt_graph('a')]);
        assert!(matches!(a.mode, Mode::Input { .. }), "{:?}", a.mode);
        let mut a = open_and_resolved_panel();
        send(&mut a, [alt_graph('?')]);
        assert!(matches!(a.mode, Mode::Overlay { .. }), "{:?}", a.mode);
    }

    #[test]
    fn x_resolves_the_selected_open_thread_without_changing_mode() {
        let mut a = open_and_resolved_panel();
        let before = a.mode.clone();

        assert_eq!(
            update(&mut a, ch('x')),
            Some(Effect::Resolve { id: "c1".into() })
        );

        assert_eq!(a.mode, before);
    }

    #[test]
    fn x_on_a_resolved_thread_does_nothing_but_say_so() {
        let mut a = open_and_resolved_panel();
        send(&mut a, [press(KeyCode::Tab)]);

        assert_eq!(update(&mut a, ch('x')), None);

        assert_eq!(a.status.as_deref(), Some("c2 is already resolved"));
        assert_eq!(panel(&a).0, 1);
    }

    #[test]
    fn o_reopens_a_resolved_thread_and_refuses_an_open_one() {
        let mut a = open_and_resolved_panel();

        assert_eq!(update(&mut a, ch('o')), None);
        assert_eq!(a.status.as_deref(), Some("c1 is already open"));

        send(&mut a, [press(KeyCode::Tab)]);
        assert_eq!(
            update(&mut a, ch('o')),
            Some(Effect::Reopen { id: "c2".into() })
        );
    }

    #[test]
    fn x_and_o_are_unbound_outside_the_panel_and_text_in_input() {
        let mut normal = panel_app(&[chatty("c1", (2, 2), 1)], 12);
        send(&mut normal, [ch('j')]);
        assert_eq!(update(&mut normal, ch('x')), None);
        assert_eq!(update(&mut normal, ch('o')), None);
        assert_eq!(normal.mode, Mode::Normal);

        let mut a = in_panel_input();
        assert_eq!(update(&mut a, ch('x')), None);
        assert_eq!(update(&mut a, ch('o')), None);
        assert_eq!(draft(&a), ("xo", 2));
    }

    #[test]
    fn saved_resolved_and_reopened_stay_on_that_thread_in_the_panel() {
        for (done, message) in [
            (ThreadWrite::Resolved, "resolved c2"),
            (ThreadWrite::Reopened, "reopened c2"),
        ] {
            let mut a = open_and_resolved_panel();
            send(&mut a, [press(KeyCode::Tab)]);
            let threads = vec![chatty("c1", (2, 2), 1), chatty("c2", (2, 2), 1)];

            let text = same_text(&a);
            update(
                &mut a,
                Event::Saved {
                    id: "c2".into(),
                    done,
                    view: View { threads, ..text },
                },
            );

            assert_eq!(panel(&a).0, 1);
            assert_eq!(a.status.as_deref(), Some(message));
        }
    }

    #[test]
    fn a_failed_resolve_leaves_the_panel_untouched() {
        let mut a = open_and_resolved_panel();
        update(&mut a, ch('x'));
        let before = a.mode.clone();

        update(&mut a, Event::SaveFailed("locked".into()));

        assert_eq!(a.mode, before);
        assert_eq!(a.status.as_deref(), Some("locked"));
    }
}
