//! Searching: the `/` prompt, [`Mode::Search`], and `n` / `N`.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::tui::search;

use super::{App, Effect, Mode, Mods};

impl App {
    /// The first line with a match for `query` strictly after (or before)
    /// `from`, wrapping past either end, and whether it wrapped. A lone
    /// matching line is `from` itself, wrapped. `None` when nothing matches.
    fn next_match(&self, query: &str, from: usize, forward: bool) -> Option<(usize, bool)> {
        let count = self.lines.len();
        (1..=count).find_map(|step| {
            let (line, wrapped) = if forward {
                ((from + step) % count, from + step >= count)
            } else {
                ((from + count - step % count) % count, step > from)
            };
            self.line_matches(line, query).then_some((line, wrapped))
        })
    }

    /// Whether 0-based `line` matches `query` as shown: a table line on its
    /// laid-out rows, so a jump always lands on something highlighted; any
    /// other line on its raw text.
    fn line_matches(&self, line: usize, query: &str) -> bool {
        match &self.tables[line] {
            Some(rows) => rows.iter().any(|row| !search::find(row, query).is_empty()),
            None => !search::find(&self.lines[line], query).is_empty(),
        }
    }

    /// Re-run the live search of `Mode::Search` from its origin.
    fn research(&mut self) {
        let Mode::Search { query, origin, .. } = &self.mode else {
            return;
        };
        let (origin, query) = (*origin, query.clone());
        self.top = origin.1;
        let found = if query.is_empty() {
            None
        } else {
            self.next_match(&query, origin.0, true)
        };
        if let Mode::Search { hit, .. } = &mut self.mode {
            *hit = found;
        }
        self.move_to(found.map_or(origin.0, |(line, _)| line));
    }

    /// What the search prompt says about the live query: `" [no match]"` or
    /// `" [wrapped]"`, else nothing.
    pub fn search_note(&self) -> &'static str {
        match &self.mode {
            Mode::Search { query, .. } if query.is_empty() => "",
            Mode::Search { hit: None, .. } => " [no match]",
            Mode::Search {
                hit: Some((_, true)),
                ..
            } => " [wrapped]",
            _ => "",
        }
    }

    /// The active search query, if one was accepted with `enter`.
    pub fn active_search(&self) -> Option<&str> {
        self.search.as_deref()
    }

    /// The query whose matches the file view marks: the live one while
    /// typing, else the active search.
    pub fn highlight_query(&self) -> Option<&str> {
        match &self.mode {
            Mode::Search { query, .. } => Some(query.as_str()),
            _ => self.search.as_deref(),
        }
        .filter(|query| !query.is_empty())
    }

    /// Move to the next (previous) line matching the active search, saying
    /// so when there is none, no search, or the jump wrapped.
    pub(super) fn repeat_search(&mut self, forward: bool) {
        let Some(query) = self.search.clone() else {
            self.status = Some("no search".into());
            return;
        };
        match self.next_match(&query, self.cursor, forward) {
            None => self.status = Some(format!("no match: {query}")),
            Some((line, wrapped)) => {
                if wrapped {
                    self.status = Some("search wrapped".into());
                }
                self.move_to(line);
            }
        }
    }

    /// Open the search prompt from Normal (`anchor` `None`) or Visual.
    pub(super) fn start_search(&mut self, anchor: Option<usize>) {
        if self.lines.is_empty() {
            return;
        }
        self.mode = Mode::Search {
            query: String::new(),
            anchor,
            origin: (self.cursor, self.top),
            hit: None,
        };
    }
}

/// Keys in [`Mode::Search`]: text edits the query (and re-searches), `enter`
/// keeps the position and makes the query the active search, `esc` and
/// `ctrl+c` restore the cursor and view.
pub(super) fn search_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Search {
        query,
        anchor,
        origin,
        ..
    } = &mut app.mode
    else {
        return None;
    };
    let (anchor, origin) = (*anchor, *origin);
    let ctrl = Mods::of(key).ctrl();
    let back = anchor.map_or(Mode::Normal, |anchor| Mode::Visual { anchor });
    match key.code {
        KeyCode::Esc | KeyCode::Char('c') if key.code == KeyCode::Esc || ctrl => {
            app.mode = back;
            app.top = origin.1;
            app.move_to(origin.0);
        }
        // Like vim: backspace on an empty prompt leaves it.
        KeyCode::Backspace if query.is_empty() => {
            app.mode = back;
        }
        KeyCode::Enter => {
            let query = std::mem::take(query);
            app.mode = back;
            if !query.is_empty() {
                app.search = Some(query);
            }
        }
        KeyCode::Backspace => {
            query.pop();
            app.research();
        }
        KeyCode::Char(c) if !ctrl => {
            query.push(c);
            app.research();
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Event;
    use crate::tui::app::fixtures::*;
    use crate::tui::test_support::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyModifiers};

    fn searching(lines: &[&str], rows: u16) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(showing(lines, &[]), "scott", (40, rows))
    }

    fn type_query(a: &mut App, text: &str) {
        send(a, text.chars().map(ch));
    }

    const PLAN: [&str; 6] = ["alpha", "beta", "gamma plan", "delta", "plan b", "omega"];

    #[test]
    fn next_match_goes_strictly_after_wraps_and_finds_a_lone_line() {
        let a = searching(&PLAN, 10);
        assert_eq!(a.next_match("plan", 0, true), Some((2, false)));
        assert_eq!(a.next_match("plan", 2, true), Some((4, false)));
        assert_eq!(a.next_match("plan", 4, true), Some((2, true)));
        assert_eq!(a.next_match("plan", 2, false), Some((4, true)));
        assert_eq!(a.next_match("plan", 4, false), Some((2, false)));
        assert_eq!(a.next_match("omega", 5, true), Some((5, true)));
        assert_eq!(a.next_match("omega", 5, false), Some((5, true)));
        assert_eq!(a.next_match("zzz", 0, true), None);
        assert_eq!(searching(&[], 10).next_match("a", 0, true), None);
    }

    #[test]
    fn typing_a_search_moves_the_cursor_live_and_enter_keeps_it() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        assert!(matches!(a.mode, Mode::Search { .. }));
        type_query(&mut a, "pla");
        assert_eq!(a.cursor, 2);
        type_query(&mut a, "n b");
        assert_eq!(a.cursor, 4);
        send(
            &mut a,
            [press(KeyCode::Backspace), press(KeyCode::Backspace)],
        );
        assert_eq!(a.cursor, 2);
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.cursor, 2);
        assert_eq!(a.search.as_deref(), Some("plan"));
    }

    #[test]
    fn a_typo_returns_to_the_origin_and_an_uppercase_char_makes_it_exact() {
        let mut a = searching(&PLAN, 10);
        typed(&mut a, "j/");
        type_query(&mut a, "plan");
        assert_eq!(a.cursor, 2);
        type_query(&mut a, "x");
        assert_eq!(a.cursor, 1);
        assert_eq!(a.search_note(), " [no match]");
        send(
            &mut a,
            [press(KeyCode::Backspace), press(KeyCode::Backspace)],
        );
        type_query(&mut a, "N");
        assert_eq!(a.cursor, 1);
        assert_eq!(a.search_note(), " [no match]");
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(a.cursor, 1);
        assert_eq!(a.search.as_deref(), Some("plaN"));
    }

    #[test]
    fn a_match_reached_by_wrapping_says_so_in_the_prompt() {
        let mut a = searching(&PLAN, 10);
        typed(&mut a, "G/");
        type_query(&mut a, "alpha");
        assert_eq!(a.cursor, 0);
        assert_eq!(a.search_note(), " [wrapped]");
    }

    #[test]
    fn esc_and_ctrl_c_restore_the_cursor_and_view_and_set_no_search() {
        for cancel in [press(KeyCode::Esc), ctrl('c')] {
            let mut a = searching(&PLAN, 4);
            typed(&mut a, "j/");
            let origin = at(&a);
            type_query(&mut a, "omega");
            assert_eq!(a.cursor, 5);
            assert_ne!(a.top, origin.1);
            send(&mut a, [cancel]);
            assert_eq!(a.mode, Mode::Normal);
            assert_eq!(at(&a), origin);
            assert_eq!(a.search, None);
        }
    }

    #[test]
    fn an_empty_enter_cancels_and_keeps_the_old_search() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "plan");
        send(
            &mut a,
            [press(KeyCode::Enter), ch('/'), press(KeyCode::Enter)],
        );
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.search.as_deref(), Some("plan"));
    }

    #[test]
    fn a_search_with_no_match_is_still_the_active_search() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "zzz");
        send(&mut a, [press(KeyCode::Enter), ch('n')]);
        assert_eq!(a.cursor, 0);
        assert_eq!(a.status(), Some("no match: zzz"));
    }

    #[test]
    fn commands_and_ctrl_chords_are_text_in_the_prompt() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/'), ch('q'), ch('j'), ctrl('d'), ch('/')]);
        let Mode::Search { query, .. } = &a.mode else {
            panic!("left the prompt");
        };
        assert_eq!(query, "qj/");
    }

    #[test]
    fn slash_on_an_empty_file_does_nothing_and_releases_are_ignored() {
        let mut a = searching(&[], 10);
        send(&mut a, [ch('/')]);
        assert_eq!(a.mode, Mode::Normal);
        let mut a = searching(&PLAN, 10);
        let mut release = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        send(&mut a, [Event::Key(release)]);
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn n_and_capital_n_cycle_and_wrap_with_a_status() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "plan");
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('n')]);
        assert_eq!((a.cursor, a.status()), (4, None));
        send(&mut a, [ch('n')]);
        assert_eq!((a.cursor, a.status()), (2, Some("search wrapped")));
        send(&mut a, [ch('N')]);
        assert_eq!((a.cursor, a.status()), (4, Some("search wrapped")));
        send(&mut a, [ch('N')]);
        assert_eq!((a.cursor, a.status()), (2, None));
    }

    #[test]
    fn n_with_no_search_says_so_and_does_not_move() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('n')]);
        assert_eq!((a.cursor, a.status()), (0, Some("no search")));
    }

    #[test]
    fn esc_in_normal_clears_the_search() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "plan");
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(a.highlight_query(), Some("plan"));
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!((a.search.as_deref(), a.highlight_query()), (None, None));
    }

    #[test]
    fn search_extends_a_visual_selection_and_esc_returns_to_it() {
        let mut a = searching(&PLAN, 10);
        typed(&mut a, "v/");
        type_query(&mut a, "plan");
        assert_eq!(a.selection(), (0, 2));
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(a.mode, Mode::Visual { anchor: 0 });
        send(&mut a, [ch('n')]);
        assert_eq!(a.selection(), (0, 4));
        send(&mut a, [ch('/')]);
        type_query(&mut a, "omega");
        assert_eq!(a.selection(), (0, 5));
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.mode, Mode::Visual { anchor: 0 });
        assert_eq!(a.selection(), (0, 4));
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.search.as_deref(), Some("plan"));
    }

    #[test]
    fn the_live_and_active_queries_mark_matches_on_every_row() {
        let mut a = searching(&["one plan two plan three"], 6);
        let reversed = |a: &App| -> Vec<String> {
            a.screen_rows()
                .iter()
                .flat_map(|row| row.text.spans.iter())
                .filter(|s| {
                    s.style
                        .add_modifier
                        .contains(ratatui::style::Modifier::REVERSED)
                })
                .map(|s| s.content.to_string())
                .collect()
        };
        assert!(reversed(&a).is_empty());
        send(&mut a, [ch('/')]);
        type_query(&mut a, "plan");
        assert_eq!(reversed(&a), ["plan", "plan"]);
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(reversed(&a), ["plan", "plan"]);
        send(&mut a, [press(KeyCode::Esc)]);
        assert!(reversed(&a).is_empty());
    }

    #[test]
    fn a_resize_while_typing_keeps_the_cursor_line_shown() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "omega");
        send(&mut a, [Event::Resize(40, 2)]);
        assert!(a.top <= a.cursor);
    }

    #[test]
    fn altgr_characters_are_text_in_the_prompt() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/'), alt_graph('@'), alt_graph('ł')]);
        let Mode::Search { query, .. } = &a.mode else {
            panic!("left the prompt");
        };
        assert_eq!(query, "@ł");
    }

    #[test]
    fn n_with_a_modifier_is_not_a_search_repeat() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "plan");
        send(&mut a, [press(KeyCode::Enter)]);
        for event in [
            ctrl('n'),
            Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)),
            Event::Key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::CONTROL)),
        ] {
            send(&mut a, [event]);
            assert_eq!((a.cursor, a.status()), (2, None));
        }
    }

    #[test]
    fn altgr_n_is_a_search_repeat() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('/')]);
        type_query(&mut a, "plan");
        send(&mut a, [press(KeyCode::Enter)]);
        let first = a.cursor;

        send(&mut a, [alt_graph('n')]);

        assert_ne!(a.cursor, first);
    }

    #[test]
    fn backspace_on_an_empty_query_cancels_the_prompt() {
        let mut a = searching(&PLAN, 10);
        send(&mut a, [ch('j'), ch('/'), press(KeyCode::Backspace)]);
        assert_eq!((a.mode.clone(), a.cursor), (Mode::Normal, 1));
        send(&mut a, [ch('v'), ch('/'), press(KeyCode::Backspace)]);
        assert_eq!(a.mode, Mode::Visual { anchor: 1 });
    }

    #[test]
    fn a_table_line_matches_on_what_is_shown_so_every_jump_is_highlighted() {
        let lines = ["intro", "| a | longer |", "|---|---|", "| bb | c |", "end"];
        let mut a = searching(&lines, 10);
        // Layout pads the cells, so the source's `a | l` is not on screen.
        send(&mut a, [ch('/')]);
        type_query(&mut a, "a | l");
        assert_eq!(a.cursor, 0);
        assert_eq!(a.search_note(), " [no match]");
        send(&mut a, [press(KeyCode::Esc), ch('/')]);
        type_query(&mut a, "c");
        let marked = a
            .screen_rows()
            .iter()
            .filter(|row| {
                row.text.spans.iter().any(|s| {
                    s.style
                        .add_modifier
                        .contains(ratatui::style::Modifier::REVERSED)
                })
            })
            .map(|row| row.line)
            .collect::<Vec<_>>();
        assert!(marked.contains(&a.cursor), "{marked:?} vs {}", a.cursor);
    }
}
