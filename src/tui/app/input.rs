//! The comment box, [`Mode::Input`]: editing a draft and what saving it
//! writes.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::anchor::line_number;
use crate::model::Verdict;

use super::{App, Effect, Mode, Mods, Target};

/// What a save stores of `draft`: the text less trailing whitespace and
/// less leading blank lines, keeping the indent of its first line of text.
fn saved_body(draft: &str) -> &str {
    let text = draft.trim_end();
    let first = text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
    let line = text[..first].rfind('\n').map_or(0, |at| at + 1);
    &text[line..]
}

/// A key in the comment box: `ctrl+s` (either case, as some terminals
/// report it with shift or caps lock) saves, as [`save`] decides; `tab` picks
/// a close's verdict; `enter` starts a new line; the rest edit the draft.
/// `esc` or `ctrl+c` returns to where the box came from: at once for a
/// blank draft, else on a second press, any other key in between keeping
/// the draft (and cancelling a pending confirmation). Only ctrl without alt
/// makes a chord: AltGr characters (ctrl+alt) are inserted as text.
pub(super) fn input_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let ctrl_only = Mods::of(key).ctrl_only();
    let save = ctrl_only && matches!(key.code, KeyCode::Char('s' | 'S'));
    // Only a save needs these; computed before the mode is borrowed.
    let (selection, open_threads) = if save {
        (app.selection(), app.open_threads())
    } else {
        ((0, 0), 0)
    };
    let width = app.draft_width();
    let Mode::Input {
        draft,
        discard_asked,
        back,
        target,
    } = &mut app.mode
    else {
        return None;
    };
    // Any key but a save cancels a pending confirmation, `esc` included.
    if let Target::Close { confirm_asked, .. } = target
        && !save
    {
        *confirm_asked = false;
    }
    let cancel = key.code == KeyCode::Esc || (ctrl_only && key.code == KeyCode::Char('c'));
    if cancel {
        if draft.text().trim().is_empty() || *discard_asked {
            app.mode = back.mode();
        } else {
            *discard_asked = true;
        }
        return None;
    }
    *discard_asked = false;
    match key.code {
        _ if save => return save_box(draft.text(), target, selection, open_threads),
        KeyCode::Tab => {
            if let Target::Close { verdict, .. } = target {
                *verdict = Some(match verdict {
                    Some(Verdict::Approved) => Verdict::Dropped,
                    None | Some(Verdict::Dropped) => Verdict::Approved,
                });
            }
        }
        KeyCode::Char(c) if !ctrl_only => draft.insert(c),
        KeyCode::Enter => draft.newline(),
        KeyCode::Backspace => draft.backspace(),
        KeyCode::Delete => draft.delete(),
        KeyCode::Left => draft.left(),
        KeyCode::Right => draft.right(),
        KeyCode::Up => draft.up(width),
        KeyCode::Down => draft.down(width),
        KeyCode::Home => draft.home(),
        KeyCode::End => draft.end(),
        _ => {}
    }
    None
}

/// What `ctrl+s` in a box writing `target` asks for, with `text` as typed:
/// the text as [`saved_body`] trims it, as the body of a comment (on the
/// 0-based `selection`, for a new thread) or as the note of a hand-off or
/// close (none when blank). A blank comment saves nothing. Approving with
/// `open_threads` asks first: the first press sets `confirm_asked` and
/// returns `None`, a second one closes.
fn save_box(
    text: &str,
    target: &mut Target,
    (first, last): (usize, usize),
    open_threads: usize,
) -> Option<Effect> {
    let body = saved_body(text);
    if body.trim().is_empty() && !target.saves_blank() {
        return None;
    }
    let note = (!body.trim().is_empty()).then(|| body.to_owned());
    let body = body.to_owned();
    Some(match target {
        Target::Reply(id) => Effect::Reply {
            id: id.clone(),
            body,
        },
        Target::NewThread => Effect::Add {
            start: line_number(first),
            end: line_number(last),
            body,
        },
        Target::HandOff => Effect::HandOff { note },
        Target::Close {
            verdict,
            confirm_asked,
        } => {
            if *verdict == Some(Verdict::Approved) && open_threads > 0 && !*confirm_asked {
                *confirm_asked = true;
                return None;
            }
            Effect::Close {
                verdict: *verdict,
                note,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ReviewState, Side, State, Status};
    use crate::review::{ThreadWrite, View};
    use crate::tui::app::fixtures::*;
    use crate::tui::app::{Event, update};
    use ratatui::crossterm::event::KeyModifiers;

    #[test]
    fn enter_opens_an_empty_draft_for_the_current_line() {
        let a = input_app();

        assert_eq!(draft(&a), ("", 0));
        assert_eq!(a.cursor, 2);
    }

    #[test]
    fn enter_on_an_empty_file_is_a_no_op() {
        let mut a = App::open(showing(Vec::new(), &[]), "scott", (80, 10));

        assert_eq!(update(&mut a, press(KeyCode::Enter)), None);
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn typing_inserts_and_backspace_removes_whole_chars() {
        let mut a = input_app();

        typed(&mut a, "aé😀");
        assert_eq!(draft(&a), ("aé😀", 3));
        send(&mut a, [press(KeyCode::Backspace)]);
        assert_eq!(draft(&a), ("aé", 2));
        send(
            &mut a,
            [press(KeyCode::Backspace), press(KeyCode::Backspace)],
        );
        send(&mut a, [press(KeyCode::Backspace)]);
        assert_eq!(draft(&a), ("", 0));
    }

    #[test]
    fn arrows_home_end_and_delete_edit_mid_draft_without_splitting_chars() {
        let mut a = input_app();
        typed(&mut a, "aé😀z");

        send(&mut a, [press(KeyCode::Left), press(KeyCode::Left)]);
        assert_eq!(draft(&a).1, 2);
        typed(&mut a, "X");
        assert_eq!(draft(&a), ("aéX😀z", 3));
        send(&mut a, [press(KeyCode::Delete)]);
        assert_eq!(draft(&a), ("aéXz", 3));
        send(&mut a, [press(KeyCode::Backspace)]);
        assert_eq!(draft(&a), ("aéz", 2));
        send(&mut a, [press(KeyCode::Home), press(KeyCode::Left)]);
        assert_eq!(draft(&a).1, 0);
        send(&mut a, [press(KeyCode::Backspace)]);
        assert_eq!(draft(&a), ("aéz", 0), "backspace at the start is a no-op");
        send(&mut a, [press(KeyCode::End), press(KeyCode::Right)]);
        assert_eq!(draft(&a).1, 3);
        send(&mut a, [press(KeyCode::Delete)]);
        assert_eq!(draft(&a), ("aéz", 3), "delete at the end is a no-op");
    }

    #[test]
    fn esc_twice_discards_the_draft_and_keeps_the_cursor() {
        let mut a = input_app();
        typed(&mut a, "hello");

        assert_eq!(update(&mut a, press(KeyCode::Esc)), None);
        assert_eq!(draft(&a), ("hello", 5), "the first esc only asks");
        assert!(discard_asked(&a));
        assert_eq!(update(&mut a, press(KeyCode::Esc)), None);

        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.cursor, 2);
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(draft(&a), ("", 0));
    }

    #[test]
    fn ctrl_s_on_a_blank_draft_stays_in_input_with_no_effect() {
        let mut a = input_app();
        typed(&mut a, "  ");

        assert_eq!(update(&mut a, ctrl('s')), None);

        assert_eq!(draft(&a), ("  ", 2));
    }

    #[test]
    fn ctrl_s_with_text_asks_to_save_on_a_one_based_line_and_waits() {
        let mut a = input_app();
        typed(&mut a, "why?");

        let effect = update(&mut a, ctrl('s'));

        assert_eq!(
            effect,
            Some(Effect::Add {
                start: 3,
                end: 3,
                body: "why?".into()
            })
        );
        assert_eq!(draft(&a), ("why?", 4));
    }

    #[test]
    fn saved_returns_to_normal_and_says_what_was_added() {
        let mut a = input_app();
        typed(&mut a, "why?");
        update(&mut a, ctrl('s'));

        let text = same_text(&a);
        update(
            &mut a,
            Event::Saved {
                id: "c3".into(),
                done: ThreadWrite::Added,
                view: text,
            },
        );

        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.status.as_deref(), Some("added c3"));
        send(&mut a, [ch('j')]);
        assert_eq!(a.status, None, "the flash clears on the next key");
    }

    #[test]
    fn save_failure_keeps_the_draft_and_shows_the_message() {
        let mut a = input_app();
        typed(&mut a, "why?");
        update(&mut a, ctrl('s'));

        update(&mut a, Event::SaveFailed("review is locked".into()));

        assert_eq!(draft(&a), ("why?", 4));
        assert_eq!(a.status.as_deref(), Some("review is locked"));
    }

    #[test]
    fn normal_commands_are_just_text_in_input_mode() {
        let mut a = input_app();
        let mut events: Vec<Event> = "jqgG".chars().map(ch).collect();
        events.extend([press(KeyCode::Up), press(KeyCode::PageDown)]);

        for event in events {
            assert_eq!(update(&mut a, event), None);
        }

        assert_eq!(draft(&a), ("jqgG", 4));
        assert_eq!(a.cursor, 2);
    }

    /// Whether the box is asking before it discards its draft.
    fn discard_asked(a: &App) -> bool {
        matches!(
            a.mode,
            Mode::Input {
                discard_asked: true,
                ..
            }
        )
    }

    #[test]
    fn a_blank_draft_is_cancelled_by_one_esc() {
        for cancel in [press(KeyCode::Esc), ctrl('c')] {
            let mut a = input_app();
            send(&mut a, [ch(' '), press(KeyCode::Enter)]);

            assert_eq!(update(&mut a, cancel), None);

            assert_eq!(a.mode, Mode::Normal);
        }
    }

    #[test]
    fn esc_or_ctrl_c_after_either_one_discards() {
        for (first, second) in [
            (press(KeyCode::Esc), ctrl('c')),
            (ctrl('c'), press(KeyCode::Esc)),
            (ctrl('c'), ctrl('c')),
        ] {
            let mut a = input_app();
            typed(&mut a, "x");

            send(&mut a, [first]);
            assert!(discard_asked(&a));
            send(&mut a, [second]);

            assert_eq!(a.mode, Mode::Normal);
        }
    }

    #[test]
    fn any_other_key_keeps_the_draft_and_stops_asking() {
        for key in [
            ch('y'),
            press(KeyCode::Left),
            press(KeyCode::Enter),
            ctrl('d'),
        ] {
            let mut a = input_app();
            typed(&mut a, "x");
            send(&mut a, [press(KeyCode::Esc), key.clone()]);
            assert!(!discard_asked(&a), "{key:?}");

            send(&mut a, [press(KeyCode::Esc)]);

            assert!(
                discard_asked(&a),
                "{key:?}: asks again rather than discarding"
            );
            assert!(draft(&a).0.starts_with('x'), "{key:?}");
        }
    }

    fn paste(text: &str) -> Event {
        Event::Paste(text.into())
    }

    #[test]
    fn a_paste_inserts_at_the_cursor_and_the_box_grows_to_fit() {
        let mut a = app(5, 40);
        send(&mut a, [press(KeyCode::Enter)]);
        typed(&mut a, "az");
        send(&mut a, [press(KeyCode::Left)]);

        assert_eq!(update(&mut a, paste("b\n\tc\r\ny\n")), None);

        assert_eq!(draft(&a), ("ab\n\tc\ny\nz", 8));
        assert_eq!(a.chrome_rows(), 6, "four text rows");
    }

    #[test]
    fn a_paste_of_control_keys_neither_saves_cancels_nor_leaves_the_box() {
        let mut a = input_app();
        typed(&mut a, "x");

        assert_eq!(update(&mut a, paste("y\u{13}\u{1b}\u{3}\u{1b}\r")), None);

        assert_eq!(draft(&a), ("xy\n", 3));
        assert!(!discard_asked(&a));
    }

    #[test]
    fn a_paste_keeps_the_draft_and_stops_asking_to_discard() {
        let mut a = input_app();
        typed(&mut a, "x");
        send(&mut a, [press(KeyCode::Esc), paste("y")]);
        assert!(!discard_asked(&a));

        send(&mut a, [press(KeyCode::Esc)]);

        assert!(discard_asked(&a), "asks again rather than discarding");
        assert_eq!(draft(&a).0, "xy");
    }

    #[test]
    fn a_paste_clears_a_failed_save_message() {
        let mut a = input_app();
        typed(&mut a, "why?");
        update(&mut a, ctrl('s'));
        update(&mut a, Event::SaveFailed("review is locked".into()));

        update(&mut a, paste("!"));

        assert_eq!(a.status, None);
        assert_eq!(draft(&a).0, "why?!");
    }

    #[test]
    fn outside_the_comment_box_a_paste_does_nothing() {
        let threads = [chatty("c1", (2, 2), 1)];
        let normal = panel_app(&threads, 20);
        let mut visual = normal.clone();
        send(&mut visual, [ch('v'), ch('j')]);
        let mut search = normal.clone();
        send(&mut search, [ch('/'), ch('l')]);
        let mut panel = normal.clone();
        send(&mut panel, [ch('j'), press(KeyCode::Enter)]);
        let mut help = normal.clone();
        send(&mut help, [ch('?')]);

        for mut a in [normal, visual, search, panel, help] {
            a.status = Some("flash".into());
            let before = a.clone();

            assert_eq!(
                update(&mut a, paste("q\nj\u{3}")),
                None,
                "{:?}",
                before.mode
            );

            assert_eq!(a.mode, before.mode);
            assert_eq!(at(&a), at(&before), "{:?}", before.mode);
            assert_eq!(a.status, before.status, "{:?}", before.mode);
        }
    }

    #[test]
    fn ctrl_c_twice_cancels_input_and_a_third_quits() {
        let mut a = input_app();
        typed(&mut a, "draft");

        assert_eq!(update(&mut a, ctrl('c')), None);
        assert_eq!(update(&mut a, ctrl('c')), None);
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(update(&mut a, ctrl('c')), Some(Effect::Quit));
    }

    #[test]
    fn ctrl_chords_do_not_insert_text() {
        let mut a = input_app();

        send(&mut a, [ctrl('d'), ctrl('u')]);

        assert_eq!(draft(&a), ("", 0));
    }

    #[test]
    fn altgr_characters_are_text_in_the_comment_box() {
        let mut a = input_app();

        send(&mut a, [alt_graph('{'), alt_graph('@'), alt_graph('ł')]);

        assert_eq!(draft(&a), ("{@ł", 3));
    }

    #[test]
    fn ctrl_s_saves_and_ctrl_c_cancels_with_and_without_shift() {
        for mods in [
            KeyModifiers::CONTROL,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            let chord = |c| Event::Key(KeyEvent::new(KeyCode::Char(c), mods));
            let mut a = input_app();
            typed(&mut a, "why?");
            let effect = update(&mut a, chord('s'));
            assert!(matches!(effect, Some(Effect::Add { .. })), "{effect:?}");

            let mut a = input_app();
            send(&mut a, [chord('c')]);
            assert_eq!(a.mode, Mode::Normal);
        }
    }

    #[test]
    fn altgr_s_and_c_are_text_not_save_or_cancel() {
        let mut a = input_app();

        assert_eq!(update(&mut a, alt_graph('s')), None);
        send(&mut a, [alt_graph('c')]);

        assert_eq!(draft(&a), ("sc", 2));
    }

    #[test]
    fn enter_starts_a_new_line_and_asks_for_nothing() {
        let mut a = input_app();
        typed(&mut a, "a");

        assert_eq!(update(&mut a, press(KeyCode::Enter)), None);
        typed(&mut a, "b");

        assert_eq!(draft(&a), ("a\nb", 3));
    }

    #[test]
    fn ctrl_s_saves_every_line_as_typed() {
        let lines = [ch('a'), press(KeyCode::Enter), ch('b')];
        let mut one = input_app();
        let mut range = visual_at(10, 12, 3);
        send(&mut range, [ch('j'), press(KeyCode::Enter)]);
        let mut reply = two_threads_panel();
        send(&mut reply, [ch('r')]);
        for a in [&mut one, &mut range, &mut reply] {
            send(a, lines.clone());
        }

        let body = || "a\nb".to_string();
        assert_eq!(
            update(&mut one, ctrl('s')),
            Some(Effect::Add {
                start: 3,
                end: 3,
                body: body()
            })
        );
        assert_eq!(
            update(&mut range, ctrl('s')),
            Some(Effect::Add {
                start: 4,
                end: 5,
                body: body()
            })
        );
        assert_eq!(
            update(&mut reply, ctrl('s')),
            Some(Effect::Reply {
                id: "c2".into(),
                body: body()
            })
        );
    }

    #[test]
    fn ctrl_s_drops_trailing_blank_lines_and_keeps_the_rest() {
        let mut a = input_app();
        typed(&mut a, "  fix this");
        send(&mut a, [press(KeyCode::Enter), press(KeyCode::Enter)]);
        typed(&mut a, "please ");
        send(
            &mut a,
            [press(KeyCode::Enter), ch(' '), press(KeyCode::Enter)],
        );

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 3,
                end: 3,
                body: "  fix this\n\nplease".into()
            })
        );
        assert_eq!(
            draft(&a).0,
            "  fix this\n\nplease \n \n",
            "the draft is untouched"
        );
    }

    #[test]
    fn ctrl_s_drops_leading_blank_lines_but_keeps_the_first_lines_indent() {
        let mut a = input_app();
        send(
            &mut a,
            [press(KeyCode::Enter), ch(' '), press(KeyCode::Enter)],
        );
        typed(&mut a, "  indented");

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 3,
                end: 3,
                body: "  indented".into()
            })
        );
    }

    #[test]
    fn ctrl_with_a_capital_s_saves_too() {
        let mut a = input_app();
        typed(&mut a, "why?");

        let effect = update(&mut a, ctrl('S'));

        assert!(matches!(effect, Some(Effect::Add { .. })), "{effect:?}");
    }

    #[test]
    fn ctrl_s_on_only_newlines_and_spaces_does_nothing() {
        let mut a = input_app();
        send(
            &mut a,
            [press(KeyCode::Enter), ch(' '), press(KeyCode::Enter)],
        );

        assert_eq!(update(&mut a, ctrl('s')), None);
        assert_eq!(draft(&a), ("\n \n", 3));
    }

    #[test]
    fn up_and_down_move_between_the_drafts_rows_at_the_box_width() {
        // 13 columns: an 11-cell inner box, rows wrapped at 10.
        let mut a = App::open(showing(vec!["x".into()], &[]), "scott", (13, 20));
        send(&mut a, [press(KeyCode::Enter)]);
        typed(&mut a, "aaaa bbbb cccc");

        send(&mut a, [press(KeyCode::Up)]);
        assert_eq!(draft(&a).1, 4, "from `cc|cc` up to `aaaa|`");
        send(&mut a, [press(KeyCode::Down)]);
        assert_eq!(draft(&a).1, 14);
    }

    #[test]
    fn backspace_at_the_start_of_a_line_joins_it_to_the_one_above() {
        let mut a = input_app();
        send(&mut a, [ch('a'), press(KeyCode::Enter), ch('b')]);

        send(&mut a, [press(KeyCode::Left), press(KeyCode::Backspace)]);

        assert_eq!(draft(&a), ("ab", 1));
    }

    // --- hand off, close, reopen ---

    fn target(a: &App) -> &Target {
        match &a.mode {
            Mode::Input { target, .. } => target,
            other => panic!("not in Input mode: {other:?}"),
        }
    }

    fn shift(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT))
    }

    #[test]
    fn capital_h_opens_a_hand_off_box_that_saves_even_when_blank() {
        let mut a = app(5, 10);

        send(&mut a, [shift('H')]);
        assert_eq!(target(&a), &Target::HandOff);
        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::HandOff { note: None })
        );

        let mut a = app(5, 10);
        send(&mut a, [shift('H')]);
        typed(&mut a, "\nall yours  ");
        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::HandOff {
                note: Some("all yours".into())
            })
        );
    }

    #[test]
    fn esc_leaves_a_hand_off_box_like_a_comment() {
        let mut a = app(5, 10);
        send(&mut a, [shift('H'), press(KeyCode::Esc)]);
        assert_eq!(a.mode, Mode::Normal);

        send(&mut a, [shift('H')]);
        typed(&mut a, "x");
        send(&mut a, [press(KeyCode::Esc)]);
        assert!(discard_asked(&a));
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.mode, Mode::Normal);
    }

    fn verdict(a: &App) -> Option<Verdict> {
        match target(a) {
            Target::Close { verdict, .. } => *verdict,
            other => panic!("not a close: {other:?}"),
        }
    }

    #[test]
    fn capital_c_opens_a_close_box_and_tab_picks_the_verdict() {
        let mut a = app(5, 10);

        send(&mut a, [shift('C')]);
        assert_eq!(verdict(&a), None);
        send(&mut a, [press(KeyCode::Tab)]);
        assert_eq!(verdict(&a), Some(Verdict::Approved));
        send(&mut a, [press(KeyCode::Tab)]);
        assert_eq!(verdict(&a), Some(Verdict::Dropped));
        send(&mut a, [press(KeyCode::Tab)]);
        assert_eq!(verdict(&a), Some(Verdict::Approved));
    }

    #[test]
    fn a_close_with_no_verdict_still_asks_the_core() {
        let mut a = app(5, 10);
        send(&mut a, [shift('C')]);
        typed(&mut a, "done");

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Close {
                verdict: None,
                note: Some("done".into())
            })
        );
    }

    fn with_an_open_thread() -> App {
        let threads = [said_by(
            view("c1", (1, 1), Status::Open, State::Current),
            &["claude"],
        )];
        App::open(
            showing(vec!["one".into(), "two".into()], &threads),
            "scott",
            (80, 10),
        )
    }

    #[test]
    fn approving_with_open_threads_asks_once_more_and_any_other_key_cancels_the_ask() {
        let mut a = with_an_open_thread();
        send(&mut a, [shift('C'), press(KeyCode::Tab)]);

        assert_eq!(update(&mut a, ctrl('s')), None, "the first press asks");
        assert!(matches!(
            target(&a),
            Target::Close {
                confirm_asked: true,
                ..
            }
        ));
        typed(&mut a, "x");
        assert!(matches!(
            target(&a),
            Target::Close {
                confirm_asked: false,
                ..
            }
        ));
        assert_eq!(update(&mut a, ctrl('s')), None, "asked again");
        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Close {
                verdict: Some(Verdict::Approved),
                note: Some("x".into())
            })
        );
    }

    #[test]
    fn dropping_or_approving_with_no_open_threads_closes_at_once() {
        let mut a = with_an_open_thread();
        send(
            &mut a,
            [shift('C'), press(KeyCode::Tab), press(KeyCode::Tab)],
        );
        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Close {
                verdict: Some(Verdict::Dropped),
                note: None
            })
        );

        let mut a = app(5, 10);
        send(&mut a, [shift('C'), press(KeyCode::Tab)]);
        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Close {
                verdict: Some(Verdict::Approved),
                note: None
            })
        );
    }

    #[test]
    fn capital_o_asks_to_reopen_the_review() {
        let mut a = app(5, 10);

        assert_eq!(update(&mut a, shift('O')), Some(Effect::ReopenReview));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn a_review_write_updates_the_state_and_returns_to_normal() {
        let mut a = app(5, 10);
        send(&mut a, [shift('H')]);

        let text = same_text(&a);
        update(
            &mut a,
            Event::ReviewSaved {
                message: "handed off".into(),
                view: View {
                    review: handed_off("ok"),
                    ..text
                },
            },
        );

        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.status(), Some("handed off"));
        assert_eq!(a.review().state, ReviewState::Turn(Side::Agent));
        assert!(a.banner().0.starts_with("agent's turn · scott: ok"));
    }

    #[test]
    fn an_esc_between_the_two_presses_cancels_the_approve_confirmation() {
        let mut a = with_an_open_thread();
        send(&mut a, [shift('C'), press(KeyCode::Tab)]);
        typed(&mut a, "x");
        assert_eq!(update(&mut a, ctrl('s')), None, "the first press asks");

        send(&mut a, [press(KeyCode::Esc)]);

        assert!(matches!(
            target(&a),
            Target::Close {
                confirm_asked: false,
                ..
            }
        ));
        assert_eq!(update(&mut a, ctrl('s')), None, "asked again");
    }
}
