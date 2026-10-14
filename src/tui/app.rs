//! TUI state and its pure transition function.
//!
//! [`update`] does no I/O and reads no clock: it turns an [`Event`] into a
//! state change and, at most, an [`Effect`] for the driver to perform.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::model::{State, Status};
use crate::review::ThreadView;

use super::panel::{self, Layout};
use super::{table, wrap};

/// What the user is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Scrolling the file.
    Normal,
    /// Reading the threads on the cursor line.
    ///
    /// Which threads those are is recomputed from the app's threads and the
    /// cursor, never cached, so a re-read cannot leave stale ids behind.
    Panel {
        /// Index into the cursor line's threads.
        selected: usize,
        /// First visible row of the panel's content.
        scroll: usize,
    },
    /// Typing a comment: a new thread on the cursor line, or a reply.
    Input {
        /// The comment so far.
        draft: String,
        /// Insertion point as a `char` index into `draft`.
        cursor: usize,
        /// The panel's `(selected, scroll)` this box was opened from, which
        /// `esc` and a successful save return to.
        from_panel: Option<(usize, usize)>,
        /// The id of the thread being replied to; `None` adds a new thread.
        ///
        /// Captured when `r` is pressed, so a re-read between the keypress
        /// and the write cannot retarget the reply.
        reply_to: Option<String>,
    },
}

/// An input to [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A key was pressed (or released/repeated; `update` filters).
    Key(KeyEvent),
    /// The terminal is now `width` x `height` cells.
    Resize(u16, u16),
    /// The driver wrote `done` to thread `id`; `threads` is the re-read view
    /// of the file's threads.
    Saved {
        /// The thread written to: the new thread for an add.
        id: String,
        done: Done,
        threads: Vec<ThreadView>,
    },
    /// The driver could not perform the write. A pending draft is kept.
    SaveFailed(String),
}

/// Which write an [`Event::Saved`] reports; it picks the footer message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    /// A new thread was added.
    Added,
    /// A comment was appended to a thread.
    Replied,
    /// A thread was resolved.
    Resolved,
    /// A thread was reopened.
    Reopened,
}

impl Done {
    fn verb(self) -> &'static str {
        match self {
            Done::Added => "added",
            Done::Replied => "replied",
            Done::Resolved => "resolved",
            Done::Reopened => "reopened",
        }
    }
}

/// Something `update` wants the driver to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Add a thread on `line` (1-based) with `body`, then answer with
    /// [`Event::Saved`] or [`Event::SaveFailed`].
    Add { line: u32, body: String },
    /// Append `body` to thread `id`; answered like [`Effect::Add`].
    Reply { id: String, body: String },
    /// Resolve thread `id` with no note; answered like [`Effect::Add`].
    Resolve { id: String },
    /// Reopen thread `id` with no note; answered like [`Effect::Add`].
    Reopen { id: String },
    /// Leave the TUI.
    Quit,
}

/// Everything the screen shows.
#[derive(Debug, Clone)]
pub struct App {
    pub(super) lines: Vec<String>,
    /// Each line's aligned table row, from [`table::align`]; computed once,
    /// since the file never changes under the TUI.
    tables: Vec<Option<String>>,
    pub(super) cursor: usize,
    pub(super) top: usize,
    /// Terminal width; the thread panel wraps to it.
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) mode: Mode,
    pub(super) status: Option<String>,
    /// The reviewed file's path as shown in the footer.
    file: String,
    /// Who the comments will be written as, shown in the input box.
    author: String,
    /// Every thread of the file, resolved and outdated included, in store
    /// order. Re-read from disk after each write; never patched locally.
    threads: Vec<ThreadView>,
}

/// What a gutter cell says about the threads covering a line.
///
/// Declared strongest first, so the marker for a line is the minimum over
/// the threads covering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    /// An open thread whose last comment is not by the current author.
    Waiting,
    /// An open thread whose last word is the current author's.
    Open,
    /// Only resolved threads cover the line.
    Resolved,
}

/// One screen row of the file view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRow {
    /// The 0-based file line this row shows part of.
    pub line: usize,
    /// Whether this is the line's first row (which carries its gutter).
    pub first: bool,
    /// The part of the line this row shows, as displayed.
    pub text: String,
}

/// Terminal rows taken by the footer in Normal mode.
const FOOTER_ROWS: usize = 1;
/// Terminal rows taken by the bordered input box (it replaces the footer).
const INPUT_BOX_ROWS: usize = 3;
/// The thread panel is half the terminal, but never fewer rows than this
/// (border included).
const PANEL_MIN_ROWS: usize = 4;
/// Rows the panel's border takes from its content.
const PANEL_BORDER_ROWS: usize = 2;
/// Gutter columns besides the line number: the glyph, a space, and `│`.
const GUTTER_EXTRA: usize = 3;

impl App {
    /// A view over `lines` on a terminal `cols` x `rows` cells, with gutter
    /// markers for `threads` (every thread, resolved ones too). `author` is
    /// who comments are written as, and whose last word is not "waiting".
    pub fn open(
        lines: Vec<String>,
        threads: &[ThreadView],
        author: &str,
        (cols, rows): (u16, u16),
    ) -> Self {
        let mut app = App {
            tables: table::align(&lines),
            lines,
            cursor: 0,
            top: 0,
            cols: usize::from(cols),
            rows: usize::from(rows),
            mode: Mode::Normal,
            status: None,
            file: String::new(),
            author: author.to_string(),
            threads: threads.to_vec(),
        };
        app.clamp_view();
        app
    }

    /// Label the screen with the file path.
    #[must_use]
    pub fn labeled(mut self, file: impl Into<String>) -> Self {
        self.file = file.into();
        self
    }

    /// The file path shown in the footer.
    pub fn file(&self) -> &str {
        &self.file
    }

    /// The author comments are written as.
    pub fn author(&self) -> &str {
        &self.author
    }

    /// The file's lines as read. What the view shows is [`App::screen_rows`].
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

    /// The gutter marker for 0-based `line`, if any thread covers it.
    ///
    /// A thread covers its stored range whatever its `state`, so outdated
    /// threads stay reachable.
    pub fn marker(&self, line: usize) -> Option<Marker> {
        self.threads_on(line)
            .into_iter()
            .map(|view| self.marker_of(view))
            .min()
    }

    /// Every thread covering 0-based `line`, in store order.
    pub fn threads_on(&self, line: usize) -> Vec<&ThreadView> {
        self.threads
            .iter()
            .filter(|view| self.covers(view, line))
            .collect()
    }

    /// How many open threads are outdated.
    pub fn outdated(&self) -> usize {
        self.threads
            .iter()
            .filter(|t| t.thread.status == Status::Open && t.state == State::Outdated)
            .count()
    }

    /// Whether `view`'s stored range, clamped to the file, holds `line`.
    fn covers(&self, view: &ThreadView, line: usize) -> bool {
        let anchor = &view.thread.anchor;
        let first = (anchor.start_line as usize).saturating_sub(1);
        let last = (anchor.end_line as usize).min(self.lines.len());
        (first..last).contains(&line)
    }

    fn marker_of(&self, view: &ThreadView) -> Marker {
        let open = view.thread.status == Status::Open;
        let theirs = view
            .thread
            .comments
            .last()
            .is_some_and(|comment| comment.author != self.author);
        match (open, theirs) {
            (true, true) => Marker::Waiting,
            (true, false) => Marker::Open,
            (false, _) => Marker::Resolved,
        }
    }

    /// Terminal rows below the file view in the current mode.
    pub(super) fn chrome_rows(&self) -> usize {
        match self.mode {
            Mode::Normal => FOOTER_ROWS,
            Mode::Input { .. } => INPUT_BOX_ROWS,
            Mode::Panel { .. } => self.panel_rows() + FOOTER_ROWS,
        }
    }

    /// Whether the panel's selected thread is resolved.
    pub fn selected_is_resolved(&self) -> bool {
        let Mode::Panel { selected, .. } = self.mode else {
            return false;
        };
        self.threads_on(self.cursor)
            .get(selected)
            .is_some_and(|view| view.thread.status != Status::Open)
    }

    /// Rows of the bordered panel, border included.
    fn panel_rows(&self) -> usize {
        (self.rows / 2).max(PANEL_MIN_ROWS)
    }

    /// Rows of panel content, inside the border.
    pub(super) fn panel_inner_rows(&self) -> usize {
        self.panel_rows().saturating_sub(PANEL_BORDER_ROWS)
    }

    /// The panel's wrapped rows, when it is open.
    pub fn panel_layout(&self) -> Option<Layout> {
        let Mode::Panel { selected, .. } = self.mode else {
            return None;
        };
        let threads = self.threads_on(self.cursor);
        let width = self.cols.saturating_sub(PANEL_BORDER_ROWS);
        Some(panel::layout(&threads, selected, width))
    }

    /// Rows available to the file view; never zero, so the arithmetic in
    /// `clamp_view` and the half-page step stay total on tiny terminals.
    pub(super) fn height(&self) -> usize {
        self.rows.saturating_sub(self.chrome_rows()).max(1)
    }

    /// Digits in the largest line number; the gutter right-aligns numbers to
    /// this width.
    pub(super) fn digits(&self) -> usize {
        self.lines.len().to_string().len()
    }

    /// Columns left for file text after the gutter (glyph, space, number,
    /// `│`); at least 1.
    pub fn text_width(&self) -> usize {
        self.cols
            .saturating_sub(self.digits() + GUTTER_EXTRA)
            .max(1)
    }

    /// The rows the file view shows, from `top`, at most `height` of them.
    /// Each file line is one or more rows: its shown text wrapped by
    /// [`wrap::rows`].
    pub fn screen_rows(&self) -> Vec<ScreenRow> {
        (self.top..self.lines.len())
            .flat_map(|line| {
                self.wrapped(line)
                    .into_iter()
                    .enumerate()
                    .map(move |(i, text)| ScreenRow {
                        line,
                        first: i == 0,
                        text,
                    })
            })
            .take(self.height())
            .collect()
    }

    fn last_line(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    /// What 0-based `line` shows: its aligned table row if it has one that
    /// fits the text width, else the raw line. A table's rows are equally
    /// wide, so a table is aligned or raw as a whole.
    fn shown(&self, line: usize) -> &str {
        match &self.tables[line] {
            Some(row) if wrap::width(row) <= self.text_width() => row,
            _ => &self.lines[line],
        }
    }

    /// The screen rows of 0-based `line`'s shown text at the current width.
    fn wrapped(&self, line: usize) -> Vec<String> {
        wrap::rows(self.shown(line), self.text_width())
    }

    /// Screen rows 0-based `line` takes at the current width.
    fn rows_of(&self, line: usize) -> usize {
        if line < self.lines.len() {
            self.wrapped(line).len()
        } else {
            0
        }
    }

    /// The first line of the longest run ending at `end` that fits in
    /// `height` rows; `end` itself when it alone is taller. Touches at most
    /// `height` lines.
    fn fit_back(&self, end: usize, height: usize) -> usize {
        let mut first = end;
        let mut used = self.rows_of(end);
        while first > 0 {
            let above = self.rows_of(first - 1);
            if used + above > height {
                break;
            }
            used += above;
            first -= 1;
        }
        first
    }

    /// Keep the whole cursor line on screen (`top <= cursor` and lines
    /// `top..=cursor` fit in `height` rows; only a cursor line taller than
    /// the view is cut off, with `top == cursor`), and never leave blank rows
    /// below the last line while earlier lines could fill them.
    fn clamp_view(&mut self) {
        if matches!(self.mode, Mode::Panel { .. }) && self.threads_on(self.cursor).is_empty() {
            self.mode = Mode::Normal;
        }
        let height = self.height();
        self.cursor = self.cursor.min(self.last_line());
        self.top = self
            .top
            .min(self.fit_back(self.last_line(), height))
            .clamp(self.fit_back(self.cursor, height), self.cursor);
        self.clamp_panel();
    }

    /// Keep the panel's selection and scroll inside its content.
    fn clamp_panel(&mut self) {
        let Some(layout) = self.panel_layout() else {
            return;
        };
        let last = layout.thread_starts.len().saturating_sub(1);
        let max_scroll = layout.lines.len().saturating_sub(self.panel_inner_rows());
        if let Mode::Panel { selected, scroll } = &mut self.mode {
            *selected = (*selected).min(last);
            *scroll = (*scroll).min(max_scroll);
        }
    }

    /// Open the panel on the cursor line's threads, selecting the first one
    /// waiting on the author, else the first.
    fn open_panel(&mut self) {
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
    fn back_to_panel(&mut self, id: &str) {
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

    fn move_to(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.clamp_view();
    }

    /// Move to the nearest covered line strictly after (or before) the
    /// cursor; stay put when there is none.
    fn jump_to_covered(&mut self, forward: bool) {
        let covered = |line: &usize| self.marker(*line).is_some();
        let target = if forward {
            (self.cursor + 1..self.lines.len()).find(covered)
        } else {
            (0..self.cursor).rev().find(covered)
        };
        if let Some(line) = target {
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

/// Apply `event` to `app`; return the effect the driver must perform.
pub fn update(app: &mut App, event: Event) -> Option<Effect> {
    let effect = apply(app, event);
    // Resizes and mode changes both alter the view height.
    app.clamp_view();
    effect
}

fn apply(app: &mut App, event: Event) -> Option<Effect> {
    match event {
        Event::Resize(cols, rows) => {
            app.cols = usize::from(cols);
            app.rows = usize::from(rows);
            None
        }
        Event::Saved { id, done, threads } => {
            app.threads = threads;
            let from_panel = matches!(
                app.mode,
                Mode::Panel { .. }
                    | Mode::Input {
                        from_panel: Some(_),
                        ..
                    }
            );
            app.mode = Mode::Normal;
            if from_panel {
                app.back_to_panel(&id);
            }
            app.status = Some(format!("{} {id}", done.verb()));
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
                Mode::Panel { .. } => panel_key(app, key),
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
        KeyCode::Char(']') => app.jump_to_covered(true),
        KeyCode::Char('[') => app.jump_to_covered(false),
        KeyCode::PageDown => app.scroll_half_page(true),
        KeyCode::PageUp => app.scroll_half_page(false),
        KeyCode::Enter if !app.threads_on(app.cursor).is_empty() => app.open_panel(),
        KeyCode::Enter if !app.lines.is_empty() => {
            app.mode = Mode::Input {
                draft: String::new(),
                cursor: 0,
                from_panel: None,
                reply_to: None,
            };
        }
        _ => {}
    }
    None
}

fn panel_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Panel { selected, scroll } = app.mode else {
        return None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let count = app.threads_on(app.cursor).len().max(1);
    match key.code {
        KeyCode::Char('c') if ctrl => app.mode = Mode::Normal,
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Char('q') if !ctrl => app.mode = Mode::Normal,
        KeyCode::Char('a') if !ctrl => {
            app.mode = Mode::Input {
                draft: String::new(),
                cursor: 0,
                from_panel: Some((selected, scroll)),
                reply_to: None,
            };
        }
        KeyCode::Char('r') if !ctrl => {
            let id = app
                .threads_on(app.cursor)
                .get(selected)
                .map(|view| view.thread.id.clone());
            if let Some(id) = id {
                app.mode = Mode::Input {
                    draft: String::new(),
                    cursor: 0,
                    from_panel: Some((selected, scroll)),
                    reply_to: Some(id),
                };
            }
        }
        KeyCode::Char('x') if !ctrl => return app.change_status(selected, true),
        KeyCode::Char('o') if !ctrl => return app.change_status(selected, false),
        KeyCode::Tab if !shift => app.select_thread((selected + 1) % count),
        KeyCode::Tab | KeyCode::BackTab => app.select_thread((selected + count - 1) % count),
        KeyCode::Char('j') | KeyCode::Down if !ctrl => {
            app.mode = Mode::Panel {
                selected,
                scroll: scroll + 1,
            };
        }
        KeyCode::Char('k') | KeyCode::Up if !ctrl => {
            app.mode = Mode::Panel {
                selected,
                scroll: scroll.saturating_sub(1),
            };
        }
        _ => {}
    }
    None
}

fn input_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Input {
        draft,
        cursor,
        from_panel,
        reply_to,
    } = &mut app.mode
    else {
        return None;
    };
    let back = from_panel.map_or(Mode::Normal, |(selected, scroll)| Mode::Panel {
        selected,
        scroll,
    });
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let len = draft.chars().count();
    match key.code {
        KeyCode::Char('c') if ctrl => app.mode = back,
        KeyCode::Esc => app.mode = back,
        KeyCode::Enter if draft.trim().is_empty() => {}
        KeyCode::Enter => {
            let body = draft.clone();
            return Some(match reply_to {
                Some(id) => Effect::Reply {
                    id: id.clone(),
                    body,
                },
                None => Effect::Add {
                    line: u32::try_from(app.cursor + 1).unwrap_or(u32::MAX),
                    body,
                },
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
        App::open(lines, &[], "scott", (80, rows))
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
    fn open_and_resize_record_the_terminal_width_too() {
        let mut a = app(3, 10);
        assert_eq!((a.cols, a.rows), (80, 10));

        send(&mut a, [Event::Resize(33, 7)]);

        assert_eq!((a.cols, a.rows), (33, 7));
    }

    // --- wrapped rows ---

    fn wrapped(lines: &[&str], (cols, rows): (u16, u16)) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(lines, &[], "scott", (cols, rows))
    }

    #[test]
    fn text_width_is_the_columns_after_the_gutter_and_at_least_one() {
        assert_eq!(app(12, 10).text_width(), 80 - 5, "2 digits + 3");
        assert_eq!(wrapped(&["a"; 12], (20, 10)).text_width(), 15);
        assert_eq!(wrapped(&["a"; 3], (20, 10)).text_width(), 16);
        assert_eq!(wrapped(&["a"; 3], (3, 10)).text_width(), 1);
        assert_eq!(wrapped(&[], (20, 10)).text_width(), 16);
    }

    #[test]
    fn screen_rows_map_each_row_to_its_file_line_and_fill_the_view() {
        // Gutter 4, text width 10; the second line wraps into 3 rows.
        let a = wrapped(
            &["short", "aaaa bbbb cccc dddd eeee", "x", "y"],
            (14, 6), // view height 5
        );

        let row = |line, first, text: &str| ScreenRow {
            line,
            first,
            text: text.into(),
        };
        assert_eq!(
            a.screen_rows(),
            [
                row(0, true, "short"),
                row(1, true, "aaaa bbbb "),
                row(1, false, "cccc dddd "),
                row(1, false, "eeee"),
                row(2, true, "x"),
            ]
        );
    }

    /// Wraps into 3 rows at text width 10; one row at 80 columns.
    const W3: &str = "aaaa bbbb cccc dddd eeee";

    /// Screen rows of lines `from..=to` at the app's current width.
    fn rows_between(a: &App, from: usize, to: usize) -> usize {
        (from..=to).map(|line| a.rows_of(line)).sum()
    }

    /// Whether every row of the cursor line is on screen.
    fn cursor_line_shown(a: &App) -> bool {
        let shown = a
            .screen_rows()
            .iter()
            .filter(|r| r.line == a.cursor)
            .count();
        shown == a.rows_of(a.cursor)
    }

    #[test]
    fn j_scrolls_so_a_wrapped_cursor_line_is_fully_shown() {
        let mut a = wrapped(&[W3, W3, W3, "x"], (14, 8)); // height 7

        send(&mut a, [ch('j'), ch('j')]);

        assert_eq!(at(&a), (2, 1), "lines 1 and 2 take 6 of the 7 rows");
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn capital_g_shows_all_of_a_wrapped_last_line_with_no_blank_rows() {
        let mut lines = vec!["x"; 10];
        lines.push(W3);
        let mut a = wrapped(&lines, (15, 8)); // 2 digits, width 10, height 7

        send(&mut a, [ch('G')]);

        assert_eq!(at(&a), (10, 6), "4 short lines and the 3 rows of the last");
        assert_eq!(a.screen_rows().len(), 7);
    }

    #[test]
    fn a_cursor_line_taller_than_the_view_starts_at_the_top() {
        let mut a = wrapped(&["x", W3, "y"], (14, 3)); // height 2

        send(&mut a, [ch('j')]);

        assert_eq!(at(&a), (1, 1));
    }

    #[test]
    fn a_bracket_jump_past_wrapped_lines_shows_the_target() {
        let lines = [W3, W3, W3, "x", "y"].map(String::from).to_vec();
        let threads = [view("c1", (4, 4), Status::Open, State::Current)];
        let mut a = App::open(lines, &threads, "scott", (14, 8)); // height 7

        send(&mut a, [ch(']')]);

        assert_eq!(at(&a), (3, 1));
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn opening_the_panel_below_wrapped_lines_keeps_the_cursor_line_shown() {
        let lines = [W3, W3, W3, W3, "t"].map(String::from).to_vec();
        let threads = [chatty("c1", (5, 5), 1)];
        let mut a = App::open(lines, &threads, "scott", (14, 16)); // height 15
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (4, 0));

        send(&mut a, [press(KeyCode::Enter)]); // height 7

        assert!(cursor_line_shown(&a));
        assert_eq!(a.top, 2);
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(at(&a), (4, 0), "everything fits again");
    }

    #[test]
    fn resizing_narrower_rewraps_and_keeps_the_cursor_line_shown() {
        let mut a = wrapped(&[W3, W3, W3, "x"], (80, 8));
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (3, 0));

        send(&mut a, [Event::Resize(14, 8)]);

        assert_eq!(a.screen_rows()[0].text, "aaaa bbbb ");
        assert_eq!(at(&a), (3, 1));
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn half_a_page_counts_rows_not_lines() {
        // 2 digits, width 10: every line is 3 rows. Height 11, half page 5.
        let mut a = wrapped(&[W3; 20], (15, 12));

        send(&mut a, [ctrl('d')]);
        assert_eq!(at(&a), (1, 1), "a second line would make 6 rows");
        send(&mut a, [ctrl('d'), ctrl('d')]);
        assert_eq!(at(&a), (3, 3));
        send(&mut a, [ctrl('u')]);
        assert_eq!(at(&a), (2, 2));
    }

    // --- tables ---

    /// Prose around a table that aligns 21 cells wide; its raw lines are at
    /// most 16.
    const TABLE: [&str; 5] = ["intro", "|a|bbbbbbbb|", "|-|-|", "|cccccccc|d|", "outro"];

    #[test]
    fn a_table_line_is_one_screen_row_showing_its_aligned_row() {
        let a = wrapped(&TABLE, (40, 10));

        let row = |line, text: &str| ScreenRow {
            line,
            first: true,
            text: text.into(),
        };
        assert_eq!(
            a.screen_rows(),
            [
                row(0, "intro"),
                row(1, " a        │ bbbbbbbb "),
                row(2, "──────────┼──────────"),
                row(3, " cccccccc │ d        "),
                row(4, "outro"),
            ]
        );
    }

    #[test]
    fn a_table_too_wide_for_the_view_shows_raw_and_resizing_switches_it() {
        let texts =
            |a: &App| -> Vec<String> { a.screen_rows().into_iter().map(|row| row.text).collect() };
        let mut a = wrapped(&TABLE, (19, 10)); // text width 15
        assert_eq!(texts(&a), TABLE);

        send(&mut a, [Event::Resize(40, 10)]);
        assert_eq!(texts(&a)[1], " a        │ bbbbbbbb ");
        send(&mut a, [Event::Resize(19, 10)]);
        assert_eq!(texts(&a), TABLE);
    }

    #[test]
    fn j_moves_through_a_table_by_file_line_and_enter_comments_on_one() {
        let mut a = wrapped(&TABLE, (40, 10));

        send(&mut a, [ch('j'), ch('j')]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "why?");

        assert_eq!(
            update(&mut a, press(KeyCode::Enter)),
            Some(Effect::Add {
                line: 4,
                body: "why?".into()
            })
        );
    }

    #[test]
    fn the_view_is_clamped_by_the_shown_rows_not_the_raw_ones() {
        // Text width 20: the raw header wraps into 2 rows, aligned it is 1.
        let mut a = wrapped(
            &["| a                        | b |", "|---|---|", "| c | d |"],
            (24, 4), // height 3
        );

        send(&mut a, [ch('G')]);

        assert_eq!(at(&a), (2, 0), "the aligned table fits in 3 rows");
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn the_cursor_line_stays_shown_through_a_walk_over_wrapped_lines() {
        let long = "word ".repeat(30);
        let mixed: Vec<&str> = (0..40)
            .map(|i| match i % 4 {
                0 => W3,
                1 => "short",
                2 => long.as_str(),
                _ => "",
            })
            .collect();
        let walk = "jjjjjjjjjjjjGkkkkkkkgjjjjjj]]][[";
        let mut events: Vec<Event> = walk.chars().map(ch).collect();
        events.extend([ctrl('d'), ctrl('u'), ctrl('d'), ctrl('d'), ch('G')]);
        events.extend([ctrl('u'), ctrl('u'), ctrl('d')]);

        for cols in [12, 14, 20, 40, 80] {
            for rows in [4, 8, 13] {
                let mut a = wrapped(&mixed, (cols, rows));
                for e in &events {
                    update(&mut a, e.clone());
                    let height = a.height();
                    assert!(
                        a.top <= a.cursor
                            && (rows_between(&a, a.top, a.cursor) <= height || a.top == a.cursor),
                        "{cols}x{rows} after {e:?}: top {} cursor {}",
                        a.top,
                        a.cursor
                    );
                }
            }
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
        let mut a = App::open(Vec::new(), &[], "scott", (80, 10));

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
            Mode::Input { draft, cursor, .. } => (draft, *cursor),
            _ => panic!("not in Input mode"),
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
        let mut a = App::open(Vec::new(), &[], "scott", (80, 10));

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
            Some(Effect::Add {
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
                done: Done::Added,
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

    // --- markers ---

    use crate::model::{Anchor, Comment, State, Status, Thread};

    fn view(id: &str, (start, end): (u32, u32), status: Status, state: State) -> ThreadView {
        ThreadView {
            thread: Thread {
                id: id.into(),
                anchor: Anchor {
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
    fn said_by(mut v: ThreadView, authors: &[&str]) -> ThreadView {
        v.thread.comments = authors
            .iter()
            .enumerate()
            .map(|(i, author)| Comment {
                id: format!("{}-{i}", v.thread.id),
                author: (*author).into(),
                body: format!("body {i}"),
                created_at: "2026-10-12T10:00:00Z".into(),
            })
            .collect();
        v
    }

    fn resolved() -> Status {
        Status::Resolved {
            resolved_by: "claude".into(),
            resolved_at: "2026-10-12T10:00:00Z".into(),
        }
    }

    fn open_view(threads: &[ThreadView], n: usize) -> App {
        let lines = (1..=n).map(|i| i.to_string()).collect();
        App::open(lines, threads, "scott", (80, 10)).labeled("plan.md")
    }

    fn markers(a: &App) -> Vec<Option<Marker>> {
        (0..a.lines.len()).map(|i| a.marker(i)).collect()
    }

    #[test]
    fn an_open_current_thread_marks_its_range_as_open() {
        let threads = [said_by(
            view("c1", (2, 3), Status::Open, State::Current),
            &["scott"],
        )];

        let a = open_view(&threads, 4);

        let open = Some(Marker::Open);
        assert_eq!(markers(&a), [None, open, open, None], "0-based 2..=3");
    }

    #[test]
    fn a_thread_whose_last_comment_is_by_someone_else_is_waiting_on_you() {
        let threads = [
            said_by(
                view("c1", (1, 1), Status::Open, State::Current),
                &["scott", "claude"],
            ),
            said_by(
                view("c2", (2, 2), Status::Open, State::Current),
                &["claude", "scott"],
            ),
            view("c3", (3, 3), Status::Open, State::Current),
        ];

        let a = open_view(&threads, 3);

        assert_eq!(
            markers(&a),
            [
                Some(Marker::Waiting),
                Some(Marker::Open),
                Some(Marker::Open)
            ],
            "my own last word, or no comments at all, is not waiting"
        );
    }

    #[test]
    fn resolved_only_lines_are_resolved_and_open_threads_outrank_them() {
        let threads = [
            said_by(view("c1", (1, 2), resolved(), State::Current), &["claude"]),
            said_by(view("c2", (2, 3), Status::Open, State::Current), &["scott"]),
            said_by(
                view("c3", (3, 3), Status::Open, State::Current),
                &["claude"],
            ),
        ];

        let a = open_view(&threads, 4);

        assert_eq!(
            markers(&a),
            [
                Some(Marker::Resolved),
                Some(Marker::Open),
                Some(Marker::Waiting),
                None
            ]
        );
    }

    #[test]
    fn a_resolved_thread_is_never_waiting() {
        let threads = [said_by(
            view("c1", (1, 1), resolved(), State::Current),
            &["scott", "claude"],
        )];

        let a = open_view(&threads, 1);

        assert_eq!(a.marker(0), Some(Marker::Resolved));
    }

    #[test]
    fn outdated_threads_cover_their_stored_range_and_only_open_ones_are_counted() {
        let threads = [
            said_by(
                view("c1", (2, 2), Status::Open, State::Outdated),
                &["scott"],
            ),
            said_by(view("c2", (4, 4), resolved(), State::Outdated), &["scott"]),
            said_by(view("c3", (1, 1), Status::Open, State::Current), &["scott"]),
        ];

        let a = open_view(&threads, 4);

        assert_eq!(
            markers(&a),
            [
                Some(Marker::Open),
                Some(Marker::Open),
                None,
                Some(Marker::Resolved)
            ]
        );
        assert_eq!(a.outdated(), 1);
    }

    #[test]
    fn a_range_past_the_end_of_the_file_cannot_index_out_of_range() {
        let threads = [view("c1", (2, 99), Status::Open, State::Current)];

        let a = App::open(vec!["a".into(), "b".into()], &threads, "scott", (80, 10));

        assert_eq!(a.marker(1), Some(Marker::Open));
        assert_eq!(a.marker(2), None);
        assert!(a.threads_on(5).is_empty());
    }

    #[test]
    fn threads_on_lists_covering_threads_in_store_order() {
        let threads = [
            view("c1", (1, 3), resolved(), State::Current),
            view("c2", (3, 3), Status::Open, State::Current),
            view("c3", (2, 2), Status::Open, State::Outdated),
        ];

        let a = open_view(&threads, 3);

        let ids = |line| -> Vec<&str> {
            a.threads_on(line)
                .iter()
                .map(|t| t.thread.id.as_str())
                .collect()
        };
        assert_eq!(ids(0), ["c1"]);
        assert_eq!(ids(1), ["c1", "c3"]);
        assert_eq!(ids(2), ["c1", "c2"]);
    }

    #[test]
    fn saved_replaces_everything_with_the_reread_threads() {
        let mut a = app(5, 10);
        send(&mut a, [press(KeyCode::Enter)]);
        typed(&mut a, "x");
        update(&mut a, press(KeyCode::Enter));
        let threads = vec![
            view("c1", (4, 5), Status::Open, State::Current),
            view("c2", (1, 1), Status::Open, State::Outdated),
        ];

        update(
            &mut a,
            Event::Saved {
                id: "c1".into(),
                done: Done::Added,
                threads,
            },
        );

        assert_eq!(a.marker(3), Some(Marker::Open));
        assert_eq!(a.marker(4), Some(Marker::Open));
        assert_eq!(a.marker(0), Some(Marker::Open), "outdated, by stored range");
        assert_eq!(a.outdated(), 1);
    }

    // --- ] and [ ---

    fn jump_app() -> App {
        let threads = [
            view("c1", (3, 3), Status::Open, State::Current),
            view("c2", (6, 7), resolved(), State::Current),
            view("c3", (10, 10), Status::Open, State::Outdated),
        ];
        let lines = (1..=12).map(|i| i.to_string()).collect();
        App::open(lines, &threads, "scott", (80, 6))
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

        send(&mut a, [ch(']'), ch(']'), ch(']'), ch(']')]);

        assert_eq!(a.cursor, 9);
        assert!(a.top <= a.cursor && a.cursor < a.top + a.height());
    }

    #[test]
    fn brackets_do_nothing_without_threads_and_are_text_in_input() {
        let mut none = app(5, 10);
        send(&mut none, [ch(']'), ch('[')]);
        assert_eq!(none.cursor, 0);

        let mut a = input_app();
        typed(&mut a, "][");
        assert_eq!(draft(&a), ("][", 2));
        assert_eq!(a.cursor, 2);
    }

    // --- thread panel ---

    /// A thread with `n` single-line comments by `claude`.
    fn chatty(id: &str, range: (u32, u32), n: usize) -> ThreadView {
        let authors = vec!["claude"; n];
        said_by(view(id, range, Status::Open, State::Current), &authors)
    }

    fn panel_app(threads: &[ThreadView], rows: u16) -> App {
        let lines = (1..=10).map(|i| format!("line {i}")).collect();
        App::open(lines, threads, "scott", (40, rows))
    }

    fn panel(a: &App) -> (usize, usize) {
        match a.mode {
            Mode::Panel { selected, scroll } => (selected, scroll),
            _ => panic!("not in Panel mode: {:?}", a.mode),
        }
    }

    fn shift_tab() -> Event {
        Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
    }

    fn listed(a: &App) -> usize {
        a.panel_layout().unwrap().thread_starts.len()
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
        let mut a = panel_app(&[chatty("c1", (6, 6), 1)], 12); // height 11
        send(&mut a, [ch('G'), ch('g')]);
        send(&mut a, [ch('j'), ch('j'), ch('j'), ch('j'), ch('j')]);
        assert_eq!(a.cursor, 5);

        send(&mut a, [press(KeyCode::Enter)]);

        assert_eq!(a.height(), 5, "12 rows minus a 6-row panel and the footer");
        assert!(a.top <= a.cursor && a.cursor < a.top + 5);
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.height(), 11);
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

        send(
            &mut a,
            [Event::Saved {
                id: "c9".into(),
                done: Done::Added,
                threads: vec![],
            }],
        );

        assert_eq!(a.mode, Mode::Normal);
    }

    // --- adding a thread from the panel ---

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
    fn enter_in_a_panel_input_saves_on_the_cursor_line() {
        let mut a = in_panel_input();
        typed(&mut a, "new");

        let effect = update(&mut a, press(KeyCode::Enter));

        assert_eq!(
            effect,
            Some(Effect::Add {
                line: 2,
                body: "new".into()
            })
        );
    }

    #[test]
    fn saved_returns_to_the_panel_with_the_new_thread_selected() {
        let mut a = in_panel_input();
        typed(&mut a, "new");
        update(&mut a, press(KeyCode::Enter));
        let threads = vec![
            chatty("c1", (2, 2), 5),
            said_by(view("c9", (2, 2), Status::Open, State::Current), &["scott"]),
        ];

        update(
            &mut a,
            Event::Saved {
                id: "c9".into(),
                done: Done::Added,
                threads,
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

            update(&mut a, cancel);

            assert_eq!((panel(&a), a.cursor), (before.0, before.1));
        }
    }

    fn panel_after_cancel(a: &App) -> (usize, usize) {
        match &a.mode {
            Mode::Input {
                from_panel: Some(p),
                ..
            } => *p,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failed_save_keeps_the_panel_input_and_its_draft() {
        let mut a = in_panel_input();
        typed(&mut a, "x");
        update(&mut a, press(KeyCode::Enter));

        update(&mut a, Event::SaveFailed("locked".into()));

        assert_eq!(draft(&a), ("x", 1));
        assert!(matches!(
            a.mode,
            Mode::Input {
                from_panel: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_blank_draft_from_the_panel_stays_put() {
        let mut a = in_panel_input();
        typed(&mut a, "  ");

        assert_eq!(update(&mut a, press(KeyCode::Enter)), None);
        assert_eq!(draft(&a), ("  ", 2));
    }

    #[test]
    fn input_box_shrinks_the_view_and_keeps_the_cursor_visible() {
        let mut a = app(20, 7); // view height 6 in Normal
        send(&mut a, [ch('j'), ch('j'), ch('j'), ch('j'), ch('j')]);
        assert_eq!(at(&a), (5, 0), "cursor on the last visible row");

        send(&mut a, [press(KeyCode::Enter)]);

        assert_eq!(a.height(), 4, "7 rows minus a 3-row input box");
        assert!(a.top <= a.cursor && a.cursor < a.top + 4, "{:?}", at(&a));
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.height(), 6);
        assert!(a.top <= a.cursor && a.cursor < a.top + 6, "{:?}", at(&a));
    }

    // --- replying from the panel ---

    /// The panel on line 2 holding `c1` (by claude) and `c2` (by claude),
    /// with `c2` selected.
    fn two_threads_panel() -> App {
        let threads = [chatty("c1", (2, 2), 1), chatty("c2", (2, 2), 1)];
        let mut a = panel_app(&threads, 20);
        send(
            &mut a,
            [ch('j'), press(KeyCode::Enter), press(KeyCode::Tab)],
        );
        a
    }

    fn reply_target(a: &App) -> Option<&str> {
        match &a.mode {
            Mode::Input { reply_to, .. } => reply_to.as_deref(),
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
                from_panel: Some((1, _)),
                ..
            }
        ));
    }

    #[test]
    fn r_is_unbound_in_normal_and_text_in_input() {
        let mut normal = panel_app(&[chatty("c1", (2, 2), 1)], 12);
        send(&mut normal, [ch('j'), ch('r')]);
        assert_eq!(normal.mode, Mode::Normal);

        let mut a = in_panel_input();
        typed(&mut a, "rrr");
        assert_eq!(draft(&a), ("rrr", 3));
        assert_eq!(reply_target(&a), None, "a is an add, not a reply");
    }

    #[test]
    fn enter_in_a_reply_asks_to_reply_to_that_thread_not_to_add() {
        let mut a = two_threads_panel();
        send(&mut a, [ch('r')]);
        typed(&mut a, "ok");

        let effect = update(&mut a, press(KeyCode::Enter));

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
            assert_eq!(update(&mut a, press(KeyCode::Enter)), None);
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
        update(&mut a, press(KeyCode::Enter));
        let threads = vec![
            chatty("c1", (2, 2), 1),
            said_by(
                view("c2", (2, 2), Status::Open, State::Current),
                &["claude", "scott"],
            ),
        ];

        update(
            &mut a,
            Event::Saved {
                id: "c2".into(),
                done: Done::Replied,
                threads,
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
        update(&mut a, press(KeyCode::Enter));

        update(&mut a, Event::SaveFailed("locked".into()));

        assert_eq!(draft(&a), ("ok", 2));
        assert_eq!(reply_target(&a), Some("c2"));
        assert_eq!(a.status.as_deref(), Some("locked"));
        assert!(matches!(
            a.mode,
            Mode::Input {
                from_panel: Some(_),
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
        for (done, text) in [
            (Done::Resolved, "resolved c2"),
            (Done::Reopened, "reopened c2"),
        ] {
            let mut a = open_and_resolved_panel();
            send(&mut a, [press(KeyCode::Tab)]);
            let threads = vec![chatty("c1", (2, 2), 1), chatty("c2", (2, 2), 1)];

            update(
                &mut a,
                Event::Saved {
                    id: "c2".into(),
                    done,
                    threads,
                },
            );

            assert_eq!(panel(&a).0, 1);
            assert_eq!(a.status.as_deref(), Some(text));
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
