//! TUI state and its pure transition function.
//!
//! [`update`] does no I/O and reads no clock: it turns an [`Event`] into a
//! state change and, at most, an [`Effect`] for the driver to perform.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::review::ThreadView;

/// What the user is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Scrolling the file.
    Normal,
    /// Typing a comment on the cursor line.
    Input {
        /// The comment so far.
        draft: String,
        /// Insertion point as a `char` index into `draft`.
        cursor: usize,
    },
}

/// An input to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A key was pressed (or released/repeated; `update` filters).
    Key(KeyEvent),
    /// The terminal is now `width` x `height` cells.
    Resize(u16, u16),
    /// The driver saved the pending comment as thread `id`; `threads` is the
    /// re-read view of the file's threads.
    Saved {
        id: String,
        threads: Vec<ThreadView>,
    },
    /// The driver could not save the pending comment.
    SaveFailed(String),
}

/// Something `update` wants the driver to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Add a thread on `line` (1-based) with `body`, then answer with
    /// [`Event::Saved`] or [`Event::SaveFailed`].
    Save { line: u32, body: String },
    /// Leave the TUI.
    Quit,
}

/// Everything the screen shows.
#[derive(Debug, Clone)]
pub struct App {
    pub(super) lines: Vec<String>,
    pub(super) cursor: usize,
    pub(super) top: usize,
    pub(super) rows: usize,
    pub(super) mode: Mode,
    pub(super) status: Option<String>,
}

/// Terminal rows taken by the footer in Normal mode.
const FOOTER_ROWS: usize = 1;

impl App {
    /// A view over `lines` on a terminal `rows` tall.
    pub fn new(lines: Vec<String>, rows: u16) -> Self {
        let mut app = App {
            lines,
            cursor: 0,
            top: 0,
            rows: usize::from(rows),
            mode: Mode::Normal,
            status: None,
        };
        app.clamp_view();
        app
    }

    /// The file's lines as displayed.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The 0-based line the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The 0-based first visible line.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The current mode.
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// A one-line message for the footer (a save result), if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Rows available to the file view; never zero, so the arithmetic in
    /// `clamp_view` and the half-page step stay total on tiny terminals.
    pub(super) fn height(&self) -> usize {
        self.rows.saturating_sub(FOOTER_ROWS).max(1)
    }

    fn last_line(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    /// Restore `top <= cursor < top + height` and never leave blank rows
    /// below the last line while earlier lines could fill them.
    fn clamp_view(&mut self) {
        let height = self.height();
        self.cursor = self.cursor.min(self.last_line());
        self.top = self.top.min(self.lines.len().saturating_sub(height));
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + height {
            self.top = self.cursor + 1 - height;
        }
    }

    fn move_to(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.clamp_view();
    }

    fn scroll_half_page(&mut self, down: bool) {
        let step = (self.height() / 2).max(1);
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

/// Apply `event` to `app`; return the effect the driver must perform.
pub fn update(app: &mut App, event: Event) -> Option<Effect> {
    match event {
        Event::Resize(_, rows) => {
            app.rows = usize::from(rows);
            app.clamp_view();
            None
        }
        Event::Saved { id, .. } => {
            app.mode = Mode::Normal;
            app.status = Some(format!("added {id}"));
            None
        }
        Event::SaveFailed(message) => {
            app.status = Some(message);
            None
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            app.status = None;
            match app.mode {
                Mode::Normal => normal_key(app, key),
                Mode::Input { .. } => input_key(app, key),
            }
        }
        Event::Key(_) => None,
    }
}

fn normal_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => return Some(Effect::Quit),
        KeyCode::Char('d') if ctrl => app.scroll_half_page(true),
        KeyCode::Char('u') if ctrl => app.scroll_half_page(false),
        KeyCode::Char('q') if !ctrl => return Some(Effect::Quit),
        KeyCode::Char('j') | KeyCode::Down => app.move_to(app.cursor + 1),
        KeyCode::Char('k') | KeyCode::Up => app.move_to(app.cursor.saturating_sub(1)),
        KeyCode::Char('g') | KeyCode::Home => app.move_to(0),
        KeyCode::Char('G') | KeyCode::End => app.move_to(app.last_line()),
        KeyCode::PageDown => app.scroll_half_page(true),
        KeyCode::PageUp => app.scroll_half_page(false),
        KeyCode::Enter if !app.lines.is_empty() => {
            app.mode = Mode::Input {
                draft: String::new(),
                cursor: 0,
            };
        }
        _ => {}
    }
    None
}

fn input_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Input { draft, cursor } = &mut app.mode else {
        return None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let len = draft.chars().count();
    match key.code {
        KeyCode::Char('c') if ctrl => app.mode = Mode::Normal,
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter if draft.trim().is_empty() => {}
        KeyCode::Enter => {
            return Some(Effect::Save {
                line: u32::try_from(app.cursor + 1).unwrap_or(u32::MAX),
                body: draft.clone(),
            });
        }
        KeyCode::Char(c) if !ctrl => {
            draft.insert(byte_index(draft, *cursor), c);
            *cursor += 1;
        }
        KeyCode::Backspace if *cursor > 0 => {
            *cursor -= 1;
            draft.remove(byte_index(draft, *cursor));
        }
        KeyCode::Delete if *cursor < len => {
            draft.remove(byte_index(draft, *cursor));
        }
        KeyCode::Left => *cursor = cursor.saturating_sub(1),
        KeyCode::Right => *cursor = (*cursor + 1).min(len),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = len,
        _ => {}
    }
    None
}

/// Byte offset of the `index`th `char` of `text` (its length if past the end).
fn byte_index(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ch(c: char) -> Event {
        press(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    /// `n` lines on a terminal of `rows` rows (one is the footer).
    fn app(n: usize, rows: u16) -> App {
        let lines = (1..=n).map(|i| format!("line {i}")).collect();
        App::new(lines, rows)
    }

    fn send(app: &mut App, events: impl IntoIterator<Item = Event>) {
        for e in events {
            update(app, e);
        }
    }

    fn at(app: &App) -> (usize, usize) {
        (app.cursor, app.top)
    }

    #[test]
    fn j_and_k_move_the_cursor_and_clamp_at_both_ends() {
        let mut a = app(3, 10);

        send(&mut a, [ch('k')]);
        assert_eq!(a.cursor, 0);
        send(&mut a, [ch('j'), ch('j'), ch('j'), ch('j')]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('k')]);
        assert_eq!(a.cursor, 1);
    }

    #[test]
    fn g_and_capital_g_jump_to_first_and_last_line() {
        let mut a = app(20, 6); // view height 5

        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (19, 15));
        send(&mut a, [ch('g')]);
        assert_eq!(at(&a), (0, 0));
    }

    #[test]
    fn ctrl_d_and_ctrl_u_move_half_a_page_and_the_view_follows() {
        let mut a = app(20, 11); // view height 10, half page 5

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
        let mut a = app(5, 2); // view height 1

        send(&mut a, [ctrl('d')]);

        assert_eq!(at(&a), (1, 1));
    }

    #[test]
    fn cursor_stays_inside_the_view_through_a_scripted_walk() {
        let mut a = app(30, 8); // view height 7
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
    fn resize_reclamps_the_view_and_zero_height_does_not_panic() {
        let mut a = app(20, 11);
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (19, 10));

        send(&mut a, [Event::Resize(40, 4)]); // view height 3
        assert_eq!(at(&a), (19, 17));
        send(&mut a, [Event::Resize(40, 0), ch('k'), ctrl('d'), ch('g')]);
        assert_eq!(a.cursor, 0);
        send(&mut a, [Event::Resize(40, 1), ch('G')]);
        assert_eq!(a.cursor, 19);
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
        let mut a = App::new(Vec::new(), 10);

        send(
            &mut a,
            [ch('j'), ch('k'), ch('G'), ch('g'), ctrl('d'), ctrl('u')],
        );

        assert_eq!(at(&a), (0, 0));
    }

    // --- Input mode ---

    fn typed(app: &mut App, text: &str) {
        send(app, text.chars().map(ch));
    }

    fn draft(app: &App) -> (&str, usize) {
        match &app.mode {
            Mode::Input { draft, cursor } => (draft, *cursor),
            Mode::Normal => panic!("not in Input mode"),
        }
    }

    fn input_app() -> App {
        let mut a = app(5, 10);
        send(&mut a, [ch('j'), ch('j'), press(KeyCode::Enter)]);
        a
    }

    #[test]
    fn enter_opens_an_empty_draft_for_the_current_line() {
        let a = input_app();

        assert_eq!(draft(&a), ("", 0));
        assert_eq!(a.cursor, 2);
    }

    #[test]
    fn enter_on_an_empty_file_is_a_no_op() {
        let mut a = App::new(Vec::new(), 10);

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
    fn esc_discards_the_draft_and_keeps_the_cursor() {
        let mut a = input_app();
        typed(&mut a, "hello");

        assert_eq!(update(&mut a, press(KeyCode::Esc)), None);

        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(a.cursor, 2);
        send(&mut a, [press(KeyCode::Enter)]);
        assert_eq!(draft(&a), ("", 0));
    }

    #[test]
    fn enter_on_a_blank_draft_stays_in_input_with_no_effect() {
        let mut a = input_app();
        typed(&mut a, "  ");

        assert_eq!(update(&mut a, press(KeyCode::Enter)), None);

        assert_eq!(draft(&a), ("  ", 2));
    }

    #[test]
    fn enter_with_text_asks_to_save_on_a_one_based_line_and_waits() {
        let mut a = input_app();
        typed(&mut a, "why?");

        let effect = update(&mut a, press(KeyCode::Enter));

        assert_eq!(
            effect,
            Some(Effect::Save {
                line: 3,
                body: "why?".into()
            })
        );
        assert_eq!(draft(&a), ("why?", 4));
    }

    #[test]
    fn saved_returns_to_normal_and_says_what_was_added() {
        let mut a = input_app();
        typed(&mut a, "why?");
        update(&mut a, press(KeyCode::Enter));

        update(
            &mut a,
            Event::Saved {
                id: "c3".into(),
                threads: vec![],
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
        update(&mut a, press(KeyCode::Enter));

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

    #[test]
    fn ctrl_c_cancels_input_and_a_second_one_quits() {
        let mut a = input_app();
        typed(&mut a, "draft");

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
}
