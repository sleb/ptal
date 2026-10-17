//! TUI state and its pure transition function.
//!
//! [`update`] does no I/O and reads no clock: it turns an [`Event`] into a
//! state change and, at most, an [`Effect`] for the driver to perform.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::model::{State, Status};
use crate::review::ThreadView;

use super::draft::Draft;
use super::highlight::{self, Styling};
use super::panel::{self, Layout};
use super::{table, wrap};

/// What the user is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Scrolling the file.
    Normal,
    /// Selecting a range of whole lines, from `anchor` to the cursor.
    ///
    /// The cursor is the selection's moving end, so every movement key
    /// extends the selection and the view keeps that end on screen.
    Visual {
        /// The 0-based file line where the selection started.
        anchor: usize,
    },
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
        /// The comment so far, and the cursor in it.
        draft: Draft,
        /// The mode this box was opened from, which `esc` returns to.
        back: Back,
        /// Whether an `esc` (or `ctrl+c`) on a draft that is not blank is
        /// waiting for a second one to discard it.
        discard_asked: bool,
        /// The id of the thread being replied to; `None` adds a new thread.
        ///
        /// Captured when `r` is pressed, so a re-read between the keypress
        /// and the write cannot retarget the reply.
        reply_to: Option<String>,
    },
}

impl Mode {
    /// An empty comment box that returns to `back`, adding a thread or, with
    /// `reply_to`, replying to that thread.
    fn input(back: Back, reply_to: Option<String>) -> Self {
        Mode::Input {
            draft: Draft::default(),
            discard_asked: false,
            back,
            reply_to,
        }
    }
}

/// Where a comment box returns to on `esc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Back {
    /// Normal mode; a successful save stays there too.
    Normal,
    /// The panel at this view; a successful save returns to it, on the
    /// written thread.
    Panel {
        /// The selected thread's index.
        selected: usize,
        /// The panel's scroll.
        scroll: usize,
    },
    /// The selection from `anchor` to the cursor, kept as it was while
    /// typing; a successful save goes to Normal.
    Visual {
        /// The selection's fixed end.
        anchor: usize,
    },
}

impl Back {
    /// The mode `esc` returns to.
    fn mode(self) -> Mode {
        match self {
            Back::Normal => Mode::Normal,
            Back::Panel { selected, scroll } => Mode::Panel { selected, scroll },
            Back::Visual { anchor } => Mode::Visual { anchor },
        }
    }
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
    /// Add a thread on lines `start..=end` (1-based, `start <= end`; equal
    /// for one line) with `body`, then answer with [`Event::Saved`] or
    /// [`Event::SaveFailed`].
    Add { start: u32, end: u32, body: String },
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
    /// Each line's table rows from [`table::align`] at `text_width()`.
    /// Recomputed by `relayout` whenever the text width can change (on open
    /// and resize); anything else that changes it must call `relayout` too.
    tables: Vec<Option<Vec<String>>>,
    /// Each line's markdown styling, from [`highlight::highlight`] once at
    /// [`App::highlighted`] (the file never changes in a session); empty
    /// when not highlighted.
    stylings: Vec<Styling>,
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
    /// The part of the line this row shows, as displayed and styled.
    pub text: Line<'static>,
}

/// Terminal rows taken by the footer in Normal mode.
const FOOTER_ROWS: usize = 1;
/// Rows the comment box's border takes; the box replaces the footer.
const BOX_BORDER_ROWS: usize = 2;
/// The comment box shows at most this many rows of its draft.
const BOX_MAX_TEXT_ROWS: usize = 10;
/// The thread panel is half the terminal, but never fewer rows than this
/// (border included).
const PANEL_MIN_ROWS: usize = 4;
/// Rows the panel's border takes from its content.
const PANEL_BORDER_ROWS: usize = 2;
/// Columns the comment box's border takes from its text.
const BOX_BORDER_COLS: usize = 2;
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
            tables: Vec::new(),
            stylings: Vec::new(),
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
        app.relayout();
        app.clamp_view();
        app
    }

    /// Label the screen with the file path.
    #[must_use]
    pub fn labeled(mut self, file: impl Into<String>) -> Self {
        self.file = file.into();
        self
    }

    /// Style the file view's markdown structure when `on`; plain otherwise.
    #[must_use]
    pub fn highlighted(mut self, on: bool) -> Self {
        self.stylings = if on {
            highlight::highlight(&self.lines)
        } else {
            Vec::new()
        };
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

    /// The selected 0-based lines `(first, last)`, in file order: the range
    /// between the anchor and the cursor in [`Mode::Visual`] and in a
    /// comment box opened from it, else the cursor line alone.
    pub fn selection(&self) -> (usize, usize) {
        match self.mode {
            Mode::Visual { anchor }
            | Mode::Input {
                back: Back::Visual { anchor },
                ..
            } => (anchor.min(self.cursor), anchor.max(self.cursor)),
            _ => (self.cursor, self.cursor),
        }
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
            Mode::Normal | Mode::Visual { .. } => FOOTER_ROWS,
            Mode::Input { .. } => self.box_text_rows() + BOX_BORDER_ROWS,
            Mode::Panel { .. } => self.panel_rows() + FOOTER_ROWS,
        }
    }

    /// Cells the comment box wraps its draft at: the box's inner width less
    /// one, so a cursor after a full row still has a cell to stand on.
    pub(super) fn draft_width(&self) -> usize {
        self.cols.saturating_sub(BOX_BORDER_COLS + 1)
    }

    /// Draft rows the comment box shows: all of them, at least 1, but at
    /// most 10 and at most `rows / 2 - 2` (so the file view keeps at least
    /// half the terminal), never fewer than 1. 0 outside `Input`.
    pub(super) fn box_text_rows(&self) -> usize {
        let Mode::Input { draft, .. } = &self.mode else {
            return 0;
        };
        let cap = (self.rows / 2)
            .saturating_sub(BOX_BORDER_ROWS)
            .clamp(1, BOX_MAX_TEXT_ROWS);
        draft.rows(self.draft_width()).len().clamp(1, cap)
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

    /// Lay out the tables again for the current text width.
    fn relayout(&mut self) {
        self.tables = table::align(&self.lines, self.text_width());
    }

    /// The screen rows of 0-based `line` at the current width: its table
    /// rows if it has them, in the line's whole-line style only (layout
    /// moves the bytes inline spans point at), else the styled line wrapped
    /// by [`wrap::rows`]. A table is laid out or raw as a whole.
    fn wrapped(&self, line: usize) -> Vec<Line<'static>> {
        let styling = self.stylings.get(line);
        match &self.tables[line] {
            Some(rows) => {
                let style = styling.map_or(Style::new(), |styling| styling.line);
                rows.iter()
                    .map(|row| Line::from(Span::styled(row.clone(), style)))
                    .collect()
            }
            None => {
                let raw = self.lines[line].as_str();
                let styled = styling.map_or_else(|| Line::raw(raw), |styling| styling.apply(raw));
                wrap::rows(&styled, self.text_width())
            }
        }
    }

    /// Screen rows 0-based `line` takes at the current width.
    fn rows_of(&self, line: usize) -> usize {
        match self.tables.get(line) {
            Some(Some(rows)) => rows.len(),
            Some(None) => wrap::row_count(&self.lines[line], self.text_width()),
            None => 0,
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
    /// below the last line while earlier lines could fill them. In `Input`,
    /// also keep the draft's cursor row inside the box.
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
        let (width, rows) = (self.draft_width(), self.box_text_rows());
        if let Mode::Input { draft, .. } = &mut self.mode {
            draft.clamp_scroll(width, rows);
        }
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
            app.relayout();
            None
        }
        Event::Saved { id, done, threads } => {
            app.threads = threads;
            let from_panel = matches!(
                app.mode,
                Mode::Panel { .. }
                    | Mode::Input {
                        back: Back::Panel { .. },
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
                Mode::Visual { .. } => visual_key(app, key),
                Mode::Panel { .. } => panel_key(app, key),
                Mode::Input { .. } => input_key(app, key),
            }
        }
        Event::Key(_) => None,
    }
}

/// Apply a key that moves the cursor; `false` if `key` is not one.
///
/// Normal and Visual share it, so the two modes move alike.
fn movement(app: &mut App, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('d') if ctrl => app.scroll_half_page(true),
        KeyCode::Char('u') if ctrl => app.scroll_half_page(false),
        KeyCode::Char('j') | KeyCode::Down => app.move_to(app.cursor + 1),
        KeyCode::Char('k') | KeyCode::Up => app.move_to(app.cursor.saturating_sub(1)),
        KeyCode::Char('g') | KeyCode::Home => app.move_to(0),
        KeyCode::Char('G') | KeyCode::End => app.move_to(app.last_line()),
        KeyCode::Char(']') => app.jump_to_covered(true),
        KeyCode::Char('[') => app.jump_to_covered(false),
        KeyCode::PageDown => app.scroll_half_page(true),
        KeyCode::PageUp => app.scroll_half_page(false),
        _ => return false,
    }
    true
}

/// `q` or `ctrl+c`: quit in Normal mode. Every sub-mode that is not typing
/// leaves on these or `esc` instead, so one stray `q` cannot quit.
fn is_quit(key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Char('c') if ctrl)
        || matches!(key.code, KeyCode::Char('q') if !ctrl)
}

/// [`is_quit`] or `esc`: back to Normal from Visual or the panel.
fn is_leave(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || is_quit(key)
}

fn normal_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    if is_quit(key) {
        return Some(Effect::Quit);
    }
    if movement(app, key) {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('v' | 'V') if !ctrl && !app.lines.is_empty() => {
            app.mode = Mode::Visual { anchor: app.cursor };
        }
        KeyCode::Enter if !app.threads_on(app.cursor).is_empty() => app.open_panel(),
        KeyCode::Enter if !app.lines.is_empty() => {
            app.mode = Mode::input(Back::Normal, None);
        }
        _ => {}
    }
    None
}

/// Keys in [`Mode::Visual`]: movement extends the selection and `enter`
/// opens a comment box for it; nothing here quits or writes.
fn visual_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
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
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('v' | 'V') if !ctrl => app.mode = Mode::Normal,
        KeyCode::Enter => {
            app.mode = Mode::input(Back::Visual { anchor }, None);
        }
        _ => {}
    }
    None
}

fn panel_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let Mode::Panel { selected, scroll } = app.mode else {
        return None;
    };
    if is_leave(key) {
        app.mode = Mode::Normal;
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let count = app.threads_on(app.cursor).len().max(1);
    match key.code {
        KeyCode::Char('a') if !ctrl => {
            app.mode = Mode::input(Back::Panel { selected, scroll }, None);
        }
        KeyCode::Char('r') if !ctrl => {
            let id = app
                .threads_on(app.cursor)
                .get(selected)
                .map(|view| view.thread.id.clone());
            if let Some(id) = id {
                app.mode = Mode::input(Back::Panel { selected, scroll }, Some(id));
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

/// What a save stores of `draft`: the text less trailing whitespace and
/// less leading blank lines, keeping the indent of its first line of text.
fn saved_body(draft: &str) -> &str {
    let text = draft.trim_end();
    let first = text.find(|c: char| !c.is_whitespace()).unwrap_or(0);
    let line = text[..first].rfind('\n').map_or(0, |at| at + 1);
    &text[line..]
}

/// A key in the comment box: `ctrl+s` (either case, as some terminals
/// report it with shift or caps lock) saves a draft that is not blank (as
/// typed, newlines kept, less trailing whitespace and leading blank lines), `enter` starts a new
/// line, and the rest edit the draft. `esc` or `ctrl+c` returns to where the
/// box came from: at once for a blank draft, else on a second press, any
/// other key in between keeping the draft.
fn input_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    let width = app.draft_width();
    let Mode::Input {
        draft,
        discard_asked,
        back,
        reply_to,
    } = &mut app.mode
    else {
        return None;
    };
    let back = back.mode();
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let blank = draft.text().trim().is_empty();
    let cancel = key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c'));
    if cancel {
        if blank || *discard_asked {
            app.mode = back;
        } else {
            *discard_asked = true;
        }
        return None;
    }
    *discard_asked = false;
    match key.code {
        KeyCode::Char('s' | 'S') if ctrl && blank => {}
        KeyCode::Char('s' | 'S') if ctrl => {
            let body = saved_body(draft.text()).to_owned();
            return Some(match reply_to {
                Some(id) => Effect::Reply {
                    id: id.clone(),
                    body,
                },
                None => {
                    let (first, last) = app.selection();
                    let one_based = |line: usize| u32::try_from(line + 1).unwrap_or(u32::MAX);
                    Effect::Add {
                        start: one_based(first),
                        end: one_based(last),
                        body,
                    }
                }
            });
        }
        KeyCode::Char(c) if !ctrl => draft.insert(c),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::style::{Color, Modifier, Style};

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

    /// The screen rows as (line, first, text), styles dropped.
    fn plain_rows(a: &App) -> Vec<(usize, bool, String)> {
        a.screen_rows()
            .into_iter()
            .map(|row| (row.line, row.first, row.text.to_string()))
            .collect()
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

        let row = |line, first, text: &str| (line, first, text.to_string());
        assert_eq!(
            plain_rows(&a),
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

        assert_eq!(a.screen_rows()[0].text.to_string(), "aaaa bbbb ");
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

    // --- highlighting ---

    fn highlighted(lines: &[&str], size: (u16, u16)) -> App {
        wrapped(lines, size).highlighted(true)
    }

    /// Each span of screen row `at` as its text and its full style.
    fn styled_row(a: &App, at: usize) -> Vec<(String, Style)> {
        let row = &a.screen_rows()[at].text;
        row.spans
            .iter()
            .map(|span| (span.content.to_string(), row.style.patch(span.style)))
            .collect()
    }

    fn bold() -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    #[test]
    fn a_highlighted_app_styles_its_rows_and_an_unhighlighted_one_does_not() {
        let lines = ["# Title", "plain **bold**"];
        let heading = Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD);

        let a = highlighted(&lines, (80, 10));
        assert_eq!(styled_row(&a, 0), [("# Title".into(), heading)]);
        assert_eq!(
            styled_row(&a, 1),
            [("plain ".into(), Style::new()), ("**bold**".into(), bold())]
        );

        for a in [
            wrapped(&lines, (80, 10)),
            wrapped(&lines, (80, 10)).highlighted(false),
        ] {
            for at in 0..2 {
                assert!(
                    styled_row(&a, at)
                        .iter()
                        .all(|(_, style)| *style == Style::new())
                );
            }
        }
    }

    #[test]
    fn a_wrapped_styled_line_keeps_its_style_on_its_continuation_rows() {
        let a = highlighted(&["**aaaa bbbb cccc**"], (12, 10)); // text width 10

        assert_eq!(
            plain_rows(&a),
            [
                (0, true, "**aaaa ".to_string()),
                (0, false, "bbbb ".to_string()),
                (0, false, "cccc**".to_string()),
            ]
        );
        for at in 0..3 {
            assert!(styled_row(&a, at).iter().all(|(_, style)| *style == bold()));
        }
    }

    #[test]
    fn a_laid_out_table_gets_only_its_line_styles_and_a_raw_one_gets_inline_ones() {
        let table = ["| **h** | b |", "|---|---|", "| **x** | y |"];

        let a = highlighted(&table, (40, 10));
        assert!(styled_row(&a, 0).iter().all(|(_, style)| *style == bold()));
        assert_eq!(plain_rows(&a)[2].2, " **x** │ y ");
        assert!(
            styled_row(&a, 2)
                .iter()
                .all(|(_, style)| *style == Style::new())
        );

        let narrow = highlighted(&table, (10, 12)); // text width 6: raw
        assert_eq!(plain_rows(&narrow)[0].2, "| ");
        let body_rows = narrow
            .screen_rows()
            .iter()
            .filter(|row| row.line == 2)
            .count();
        let first_body = narrow.screen_rows().iter().position(|row| row.line == 2);
        let first_body = first_body.expect("the body line is shown");
        assert!(
            (first_body..first_body + body_rows)
                .any(|at| styled_row(&narrow, at).contains(&("**x**".into(), bold())))
        );
    }

    #[test]
    fn highlighting_changes_no_text_layout_or_behavior() {
        let file = [
            "# Plan **now**",
            "",
            "- a list item with `code` that goes on and on and on",
            "> a quote with *emphasis* and [a link](http://example.com/long/path)",
            "```rust",
            "fn main() { let x = **y; }",
            "```",
            "| a | **b** |",
            "|---|---|",
            "| 1 | a cell long enough to wrap in a narrow view |",
            "Setext",
            "---",
            "> \u{301}abcd a combining accent right after a quote marker",
        ];
        let keys = [ch('j'), ch('j'), ch('G'), ch('k'), press(KeyCode::Enter)];
        for cols in [12, 20, 40, 80] {
            for rows in [4, 10] {
                let mut off = wrapped(&file, (cols, rows));
                let mut on = highlighted(&file, (cols, rows));
                assert_eq!(plain_rows(&on), plain_rows(&off), "{cols}x{rows}");
                for line in 0..file.len() {
                    assert_eq!(
                        on.rows_of(line),
                        on.wrapped(line).len(),
                        "{cols}x{rows} line {line}"
                    );
                }
                for key in keys.clone() {
                    assert_eq!(update(&mut on, key.clone()), update(&mut off, key));
                    assert_eq!((at(&on), plain_rows(&on)), (at(&off), plain_rows(&off)));
                }
                typed(&mut on, "x");
                typed(&mut off, "x");
                assert_eq!(
                    update(&mut on, press(KeyCode::Enter)),
                    update(&mut off, press(KeyCode::Enter))
                );
            }
        }
    }

    // --- tables ---

    /// Prose around a table that aligns 21 cells wide; its raw lines are at
    /// most 16.
    const TABLE: [&str; 5] = ["intro", "|a|bbbbbbbb|", "|-|-|", "|cccccccc|d|", "outro"];

    #[test]
    fn a_table_line_is_one_screen_row_showing_its_aligned_row() {
        let a = wrapped(&TABLE, (40, 10));

        let row = |line, text: &str| (line, true, text.to_string());
        assert_eq!(
            plain_rows(&a),
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
    fn a_table_too_narrow_for_its_minimum_widths_shows_raw_and_resizing_switches_it() {
        let texts = |a: &App| -> Vec<String> {
            a.screen_rows()
                .into_iter()
                .map(|row| row.text.to_string())
                .collect()
        };
        let mut a = wrapped(&TABLE, (19, 10)); // text width 15
        assert_eq!(texts(&a), TABLE);

        send(&mut a, [Event::Resize(40, 10)]);
        assert_eq!(texts(&a)[1], " a        │ bbbbbbbb ");
        send(&mut a, [Event::Resize(19, 10)]);
        assert_eq!(texts(&a), TABLE);
    }

    /// A decision table with a long cell, then prose. 4 lines, so the gutter
    /// is 4 columns; at 24 columns the text width is 20 and the long cell
    /// wraps into 3 rows; at 80 it fits on one.
    const DECISION: [&str; 4] = [
        "| Q | Decision |",
        "|---|---|",
        "| 1 | use the water filling rule for widths |",
        "outro",
    ];

    fn texts(a: &App) -> Vec<String> {
        a.screen_rows()
            .into_iter()
            .map(|row| row.text.to_string())
            .collect()
    }

    #[test]
    fn a_table_too_wide_for_the_view_wraps_its_cells_inside_their_columns() {
        let a = wrapped(&DECISION, (24, 10));

        let row = |line, first, text: &str| (line, first, text.to_string());
        assert_eq!(
            plain_rows(&a),
            [
                row(0, true, " Q │ Decision       "),
                row(1, true, "───┼────────────────"),
                row(2, true, " 1 │ use the water  "),
                row(2, false, "   │ filling rule   "),
                row(2, false, "   │ for widths     "),
                row(3, true, "outro"),
            ]
        );
    }

    #[test]
    fn resizing_reflows_a_table_between_one_row_wrapped_and_raw() {
        let mut a = wrapped(&DECISION, (80, 10));
        assert_eq!(texts(&a)[2], " 1 │ use the water filling rule for widths ");

        send(&mut a, [Event::Resize(24, 10)]);
        assert_eq!(
            texts(&a)[2..5],
            [
                " 1 │ use the water  ",
                "   │ filling rule   ",
                "   │ for widths     "
            ]
        );
        send(&mut a, [Event::Resize(14, 10)]); // text width 10: below 1 + 10
        assert_eq!(texts(&a)[0], "| Q | ");
        send(&mut a, [Event::Resize(80, 10)]);
        assert_eq!(a.screen_rows().len(), 4);
    }

    #[test]
    fn j_moves_over_a_wrapped_table_line_in_one_step_and_enter_comments_on_it() {
        let mut a = wrapped(&DECISION, (24, 10));

        send(&mut a, [ch('j'), ch('j')]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('j')]);
        assert_eq!(a.cursor, 3);
        send(&mut a, [ch('k'), press(KeyCode::Enter)]);
        typed(&mut a, "why?");

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 3,
                end: 3,
                body: "why?".into()
            })
        );
    }

    #[test]
    fn the_clamp_counts_the_rows_of_a_wrapped_table_line() {
        let mut lines = vec!["x"; 6];
        lines.extend(&DECISION[..3]);
        let mut a = wrapped(&lines, (24, 5)); // text width 20, height 4

        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (8, 7), "the delimiter and the body's 3 rows");
        assert!(cursor_line_shown(&a));
        send(&mut a, [ch('g'), ctrl('d'), ctrl('d'), ctrl('d')]);
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn j_moves_through_a_table_by_file_line_and_enter_comments_on_one() {
        let mut a = wrapped(&TABLE, (40, 10));

        send(&mut a, [ch('j'), ch('j')]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "why?");

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 4,
                end: 4,
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
        let wide_row = format!("| 1 | {long} |");
        let mut mixed: Vec<&str> = (0..40)
            .map(|i| match i % 4 {
                0 => W3,
                1 => "short",
                2 => long.as_str(),
                _ => "",
            })
            .collect();
        mixed.extend(["| id | why |", "|---|---|", &wide_row, "| 2 | ok |"]);
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

    /// The draft's text and its cursor as a `char` index.
    fn draft(app: &App) -> (&str, usize) {
        match &app.mode {
            Mode::Input { draft, .. } => {
                let text = draft.text();
                (text, text[..draft.cursor()].chars().count())
            }
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
        let mut a = App::open(vec!["x".into()], &[], "scott", (13, 20));
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
        update(&mut a, ctrl('s'));
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
    fn panel_scroll_clamps_to_rows_wrapped_by_cell_width() {
        // 40 cols: 34 cells after the body indent, so 17 wide characters a
        // row and 68 of them make 4 rows (2 if counted by char).
        let mut wide = view("c1", (1, 1), Status::Open, State::Current);
        wide.thread.comments = vec![Comment {
            id: "c1-0".into(),
            author: "claude".into(),
            body: "漢".repeat(68),
            created_at: "2026-10-12T10:00:00Z".into(),
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

    /// `a`'s draft scroll and its cursor's screen row.
    fn box_scroll(a: &App) -> (usize, usize) {
        match &a.mode {
            Mode::Input { draft, .. } => (draft.scroll(), draft.cursor_at(a.draft_width()).0),
            other => panic!("not in Input mode: {other:?}"),
        }
    }

    /// `n` draft lines, `x` on each, in a box on line 1.
    fn box_of(n: usize, rows: u16) -> App {
        let mut a = app(30, rows);
        send(&mut a, [press(KeyCode::Enter)]);
        for i in 0..n {
            if i > 0 {
                send(&mut a, [press(KeyCode::Enter)]);
            }
            typed(&mut a, "x");
        }
        a
    }

    #[test]
    fn the_box_grows_with_its_draft_up_to_a_cap() {
        assert_eq!(box_of(0, 40).chrome_rows(), 3, "an empty draft is one row");
        assert_eq!(box_of(1, 40).chrome_rows(), 3);
        assert_eq!(box_of(4, 40).chrome_rows(), 6);
        assert_eq!(box_of(10, 40).chrome_rows(), 12);
        assert_eq!(box_of(14, 40).chrome_rows(), 12, "at most 10 text rows");
        assert_eq!(box_of(14, 16).chrome_rows(), 8, "at most rows / 2 - 2");
        assert_eq!(box_of(3, 4).chrome_rows(), 3, "at least one text row");
    }

    #[test]
    fn a_draft_past_the_cap_scrolls_to_keep_the_cursor_row_shown() {
        let mut a = box_of(14, 40);
        assert_eq!(box_scroll(&a), (4, 13), "rows 4..14 shown");

        send(&mut a, (0..13).map(|_| press(KeyCode::Up)));
        assert_eq!(box_scroll(&a), (0, 0));
        send(&mut a, (0..10).map(|_| press(KeyCode::Down)));
        assert_eq!(box_scroll(&a), (1, 10), "scrolled just enough");
    }

    #[test]
    fn a_resize_rewraps_the_draft_and_reclamps_its_scroll() {
        let mut a = app(30, 40);
        send(&mut a, [press(KeyCode::Enter)]);
        typed(&mut a, &"word ".repeat(24));
        send(&mut a, [press(KeyCode::Home)]);
        assert_eq!(a.chrome_rows(), 4, "120 cells at 77 is two rows");

        update(&mut a, Event::Resize(13, 40));
        assert_eq!(a.chrome_rows(), 12, "two words a row, capped at 10");
        assert_eq!(box_scroll(&a), (0, 0));
        send(&mut a, [press(KeyCode::End)]);
        assert_eq!(box_scroll(&a), (2, 11));

        update(&mut a, Event::Resize(80, 40));
        assert_eq!(box_scroll(&a), (0, 1), "no blank rows below the text");
        update(&mut a, Event::Resize(80, 0));
        update(&mut a, Event::Resize(0, 0));
    }

    #[test]
    fn the_commented_lines_stay_shown_as_the_box_grows() {
        let mut one = app(30, 24);
        send(&mut one, (0..20).map(|_| ch('j')));
        send(&mut one, [press(KeyCode::Enter)]);
        let mut range = app(30, 24);
        send(&mut range, (0..18).map(|_| ch('j')));
        send(
            &mut range,
            [ch('v'), ch('j'), ch('j'), press(KeyCode::Enter)],
        );
        for a in [&mut one, &mut range] {
            for _ in 0..12 {
                send(a, [ch('x'), press(KeyCode::Enter)]);
                assert!(cursor_line_shown(a), "{:?}", at(a));
            }
            assert_eq!(a.chrome_rows(), 12);
        }
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
                back: Back::Panel { selected: 1, .. },
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

    // --- visual selection ---

    /// `n` lines with the cursor on 0-based `line`, then `v`.
    fn visual_at(n: usize, rows: u16, line: usize) -> App {
        let mut a = app(n, rows);
        send(&mut a, (0..line).map(|_| ch('j')));
        send(&mut a, [ch('v')]);
        a
    }

    #[test]
    fn v_starts_a_selection_of_the_cursor_line() {
        let a = visual_at(10, 12, 3);

        assert_eq!(a.mode, Mode::Visual { anchor: 3 });
        assert_eq!(a.selection(), (3, 3));
    }

    #[test]
    fn outside_visual_the_selection_is_the_cursor_line() {
        let mut a = app(10, 12);
        send(&mut a, [ch('j'), ch('j')]);

        assert_eq!(a.selection(), (2, 2));
    }

    #[test]
    fn j_and_k_extend_the_selection_either_way_and_across_the_anchor() {
        let mut down = visual_at(10, 12, 3);
        send(&mut down, [ch('j'), ch('j')]);
        assert_eq!(down.selection(), (3, 5));
        assert_eq!(down.cursor, 5, "the cursor is the moving end");

        let mut up = visual_at(10, 12, 3);
        send(&mut up, [ch('k'), ch('k')]);
        assert_eq!(up.selection(), (1, 3));

        send(&mut up, (0..4).map(|_| press(KeyCode::Down)));
        assert_eq!(up.selection(), (3, 5), "crossed the anchor");
        send(&mut up, [press(KeyCode::Up)]);
        assert_eq!(up.selection(), (3, 4));
        assert_eq!(up.mode, Mode::Visual { anchor: 3 });
    }

    #[test]
    fn every_movement_key_moves_the_end_and_the_view_follows_it() {
        let mut a = visual_at(20, 6, 2); // view height 5

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
        let mut a = App::open(lines, &threads, "scott", (40, 12));
        send(&mut a, [ch('j'), ch('v')]);

        send(&mut a, [ch(']')]);
        assert_eq!(a.selection(), (1, 6));
        send(&mut a, [ch('G'), ch('[')]);
        assert_eq!(a.selection(), (1, 7));
    }

    #[test]
    fn capital_v_selects_exactly_as_v_does() {
        let mut a = app(10, 12);
        send(&mut a, [ch('j'), ch('V'), ch('j')]);

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
        let mut a = App::open(vec![], &[], "scott", (40, 10));

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
                reply_to: None,
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

        update(
            &mut a,
            Event::Saved {
                id: "c4".into(),
                done: Done::Added,
                threads,
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
}
