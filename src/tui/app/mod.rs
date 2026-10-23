//! TUI state and its pure transition function.
//!
//! [`update`] does no I/O and reads no clock: it turns an [`Event`] into a
//! state change and, at most, an [`Effect`] for the driver to perform.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::anchor::{Change, ChangeKind, Diff, LineRange, line_number};
use crate::model::{Entry, ReviewState, Side, State, Status, Verdict};
use crate::review::{ReviewStatus, ThreadView, ThreadWrite, View};

use super::draft::Draft;
use super::highlight::{self, Styling};
use super::panel::{self, Layout};
use super::{keys, search, table, wrap};

mod find;
mod input;
mod overlay;
mod threads;

use find::search_key;
use input::input_key;
use overlay::{overlay_key, review_rows, state_name};
use threads::panel_key;

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
    /// Typing a search after `/`, incrementally: every edit re-searches
    /// from `origin` and moves the cursor to the first matching line after
    /// it, like `n` would.
    Search {
        /// The query so far.
        query: String,
        /// The selection's fixed end when opened from Visual, which `enter`
        /// and `esc` return to; `None` returns to Normal.
        anchor: Option<usize>,
        /// The `(cursor, top)` when `/` was pressed: where every search
        /// starts and where `esc` restores.
        origin: (usize, usize),
        /// The line the query matched from `origin` and whether it wrapped;
        /// `None` when nothing matched (or the query is empty).
        hit: Option<(usize, bool)>,
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
        /// What saving the box writes.
        target: Target,
    },
    /// Reading a full-screen overlay, which replaces the whole screen.
    ///
    /// `back` is only ever Normal or the panel, and is where closing returns.
    Overlay {
        /// What the overlay shows.
        kind: Overlay,
        /// Where the overlay was opened from.
        back: Back,
        /// First visible row of its listing.
        scroll: usize,
    },
}

/// What a [`Mode::Overlay`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// The key listing, [`keys::HELP`].
    Help,
    /// The review's state and its latest log entry, note in full.
    Review,
}

impl Overlay {
    /// The overlay's title row.
    pub fn title(self) -> &'static str {
        match self {
            Overlay::Help => "Help",
            Overlay::Review => "Review",
        }
    }

    /// The key that opens the overlay, and closes it again.
    fn toggle(self) -> char {
        match self {
            Overlay::Help => '?',
            Overlay::Review => 'i',
        }
    }
}

impl Mode {
    /// An empty comment box that returns to `back` and writes `target`.
    fn input(back: Back, target: Target) -> Self {
        Mode::Input {
            draft: Draft::default(),
            discard_asked: false,
            back,
            target,
        }
    }
}

/// What a comment box writes when saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A new thread on the selection.
    NewThread,
    /// A reply to the thread with this id.
    ///
    /// Captured when `r` is pressed, so a re-read between the keypress and
    /// the write cannot retarget the reply.
    Reply(String),
    /// A hand-off to the agent; the text is its note, and a blank box hands
    /// off with none.
    HandOff,
    /// A close of the review; the text is its note.
    Close {
        /// The verdict chosen with `tab`; `None` until one is.
        verdict: Option<Verdict>,
        /// Whether a `ctrl+s` approving with open threads is waiting for a
        /// second one to confirm.
        confirm_asked: bool,
    },
}

impl Target {
    /// Whether saving a blank box writes: a hand-off or a close needs no
    /// note, a comment needs a body.
    fn saves_blank(&self) -> bool {
        matches!(self, Target::HandOff | Target::Close { .. })
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
    /// The driver wrote `done` to thread `id`, then re-read the file.
    Saved {
        /// The thread written to: the new thread for an add.
        id: String,
        /// Which write; it picks the footer message.
        done: ThreadWrite,
        /// The file as re-read after the write.
        view: View,
    },
    /// The driver wrote a hand-off, close or reopen, then re-read the file
    /// into `view`; `message` is the footer's.
    ReviewSaved { message: String, view: View },
    /// The driver could not perform the write. A pending draft is kept.
    SaveFailed(String),
    /// The driver noticed the file or the review changed on disk (the other
    /// side's writes) and re-read it. Nothing is written; the mode, a draft
    /// and the footer are kept.
    Refreshed(View),
    /// The driver's checks for the other side's writes have kept failing;
    /// the message goes in the footer like a failed save's.
    RefreshFailed(String),
    /// Text was pasted (the terminal's bracketed paste). In
    /// [`Mode::Input`] it goes into the draft in one step
    /// ([`Draft::paste`]); everywhere else it is ignored.
    Paste(String),
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
    /// Hand the turn to the agent with `note`, then answer with
    /// [`Event::ReviewSaved`] or [`Event::SaveFailed`].
    HandOff { note: Option<String> },
    /// Close the review with `verdict` (`None` if none was chosen, which the
    /// core refuses) and `note`; answered like [`Effect::HandOff`].
    Close {
        verdict: Option<Verdict>,
        note: Option<String>,
    },
    /// Reopen the closed review; answered like [`Effect::HandOff`].
    ReopenReview,
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
    /// Each line's markdown styling, from [`highlight::highlight`] at
    /// [`App::highlighted`] and again whenever a re-read changes the lines;
    /// empty when not highlighted.
    stylings: Vec<Styling>,
    /// Whether the file view is highlighted, so a re-read can restyle.
    highlight: bool,
    /// The hash of the content the screen shows, for writes that depend on
    /// it ([`crate::review::Reviews::against`]): the view's `sha256`.
    seen: String,
    pub(super) cursor: usize,
    pub(super) top: usize,
    /// Terminal width; the thread panel wraps to it.
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) mode: Mode,
    pub(super) status: Option<String>,
    /// The active search query (set by `enter` on a search), highlighted and
    /// repeated by `n` / `N`; cleared by `esc` in Normal.
    pub(super) search: Option<String>,
    /// The help listing laid out for `cols`.
    help: Vec<keys::Row>,
    /// The review overlay's listing laid out for `cols`, from `review`.
    review_rows: Vec<keys::Row>,
    /// Whose turn it is, or the verdict, and the latest log entry. Re-read
    /// from disk after each write, like the threads.
    review: ReviewStatus,
    /// The reviewed file's path as shown in the footer.
    file: String,
    /// Who the comments will be written as, shown in the input box.
    author: String,
    /// Every thread of the file, resolved and outdated included, in store
    /// order. Re-read from disk after each write; never patched locally.
    threads: Vec<ThreadView>,
    /// What changed since the human's last look, one hunk each, sorted.
    /// Re-read from disk after each write, like the threads.
    changes: Vec<Change>,
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
/// Terminal rows the review banner takes at the top, in every mode but an
/// overlay.
pub(super) const BANNER_ROWS: usize = 1;
/// Rows an overlay spends on its title and its footer.
pub(super) const OVERLAY_CHROME_ROWS: usize = 2;
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
    /// The screen over `view` (its lines, every thread, resolved ones
    /// too, the review, the changes since the last look, and the content
    /// hash, [`App::seen`]) on a terminal `cols` x `rows` cells, the cursor
    /// at the top. `author` is who comments are written as, and whose last
    /// word is not "waiting".
    ///
    /// The view goes in through the same re-read a refresh applies, from
    /// no text, so a fresh app and a refreshed one take the same state
    /// from a [`View`].
    pub fn open(view: View, author: &str, (cols, rows): (u16, u16)) -> Self {
        let mut app = App {
            tables: Vec::new(),
            stylings: Vec::new(),
            highlight: false,
            seen: String::new(),
            lines: Vec::new(),
            cursor: 0,
            top: 0,
            cols: usize::from(cols),
            rows: usize::from(rows),
            mode: Mode::Normal,
            status: None,
            search: None,
            help: Vec::new(),
            review_rows: Vec::new(),
            review: ReviewStatus::default(),
            file: String::new(),
            author: author.to_string(),
            threads: Vec::new(),
            changes: Vec::new(),
        };
        app.reread(view);
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

    fn set_review(&mut self, review: ReviewStatus) {
        self.review_rows = review_rows(&review, self.cols);
        self.review = review;
    }

    /// Show the file, its threads and the review as re-read into `view`,
    /// staying in place: the cursor, `top`, a selection's anchor and the
    /// search prompt's origin follow their text through an
    /// [`anchor::Diff`](Diff) of the old lines to the new (then the view
    /// clamp keeps them in bounds), and the panel keeps its thread by id. A
    /// draft is never touched; a close waiting to confirm asks again, since
    /// its count of open threads may be stale.
    fn reread(
        &mut self,
        View {
            revision: _,
            lines,
            sha256,
            threads,
            review,
            changes,
        }: View,
    ) {
        let selected = self.selected_thread_id();
        if lines != self.lines {
            // From no text (opening, or an empty file) there is no place to
            // keep, so the view starts at the top.
            if !self.lines.is_empty() {
                self.follow_text(&lines);
            }
            self.lines = lines;
            if self.highlight {
                self.stylings = highlight::highlight(&self.lines);
            }
            self.tables = table::align(&self.lines, self.text_width());
        }
        self.seen = sha256;
        self.threads = threads;
        self.set_review(review);
        self.changes = changes;
        if let Some(id) = selected {
            self.follow_thread(&id);
        }
        if let Mode::Input {
            target: Target::Close { confirm_asked, .. },
            ..
        } = &mut self.mode
        {
            *confirm_asked = false;
        }
    }

    /// Move every line position the state holds from the current lines to
    /// where the same text is in `new`.
    fn follow_text(&mut self, new: &[String]) {
        let old: Vec<&str> = self.lines.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        let diff = Diff::new(&old, &new);
        let map = |line: usize| {
            let n = line_number(line);
            let mapped = diff.map(LineRange { start: n, end: n });
            usize::try_from(mapped.range.start).map_or(0, |n| n.saturating_sub(1))
        };
        self.cursor = map(self.cursor);
        self.top = map(self.top);
        match &mut self.mode {
            Mode::Visual { anchor }
            | Mode::Input {
                back: Back::Visual { anchor },
                ..
            } => *anchor = map(*anchor),
            Mode::Search {
                anchor,
                origin,
                hit,
                ..
            } => {
                *anchor = anchor.map(map);
                *origin = (map(origin.0), map(origin.1));
                *hit = hit.map(|(line, wrapped)| (map(line), wrapped));
            }
            Mode::Normal | Mode::Panel { .. } | Mode::Input { .. } | Mode::Overlay { .. } => {}
        }
        let last = new.len().saturating_sub(1);
        if let Mode::Visual { anchor }
        | Mode::Input {
            back: Back::Visual { anchor },
            ..
        } = &mut self.mode
        {
            *anchor = (*anchor).min(last);
        }
    }

    /// The id of the thread selected in the panel, if it is open or a box
    /// opened from it will return to it.
    fn selected_thread_id(&self) -> Option<String> {
        let (Mode::Panel { selected, .. }
        | Mode::Input {
            back: Back::Panel { selected, .. },
            ..
        }) = self.mode
        else {
            return None;
        };
        self.threads_on(self.cursor)
            .get(selected)
            .map(|view| view.thread.id.clone())
    }

    /// Keep the panel (or the panel a box returns to) on thread `id`: by
    /// its new index on the cursor line, or, if it now covers other lines,
    /// by moving the cursor to its first line. A thread that is gone leaves
    /// the index to the clamp.
    fn follow_thread(&mut self, id: &str) {
        let on_cursor = |app: &Self| {
            app.threads_on(app.cursor)
                .iter()
                .position(|view| view.thread.id == id)
        };
        let mut index = on_cursor(self);
        if index.is_none()
            && let Some(view) = self.threads.iter().find(|view| view.thread.id == id)
        {
            let first = usize::try_from(view.thread.anchor.start_line).unwrap_or(1);
            self.cursor = first.saturating_sub(1).min(self.last_line());
            index = on_cursor(self);
        }
        if let (Some(index), Mode::Panel { selected, .. })
        | (
            Some(index),
            Mode::Input {
                back: Back::Panel { selected, .. },
                ..
            },
        ) = (index, &mut self.mode)
        {
            *selected = index;
        }
    }

    /// The hash of the content on screen: what a hand-off, a close or a new
    /// thread is written against.
    pub fn seen(&self) -> &str {
        &self.seen
    }

    /// How many threads are open.
    pub fn open_threads(&self) -> usize {
        self.threads
            .iter()
            .filter(|t| t.thread.status == Status::Open)
            .count()
    }

    /// Style the file view's markdown structure when `on`; plain otherwise.
    #[must_use]
    pub fn highlighted(mut self, on: bool) -> Self {
        self.highlight = on;
        self.stylings = if on {
            highlight::highlight(&self.lines)
        } else {
            Vec::new()
        };
        self
    }

    /// The review's state as last read.
    pub fn review(&self) -> &ReviewStatus {
        &self.review
    }

    /// The banner's text, `your turn`, `agent's turn` or `closed: <verdict>`
    /// then ` · <author>: <note>` with the latest entry's note (its first
    /// line, ending in `…` when there are more), and the key hint for what
    /// the human can do now.
    pub fn banner(&self) -> (String, &'static str) {
        let mut text = state_name(self.review.state);
        if let Some(Entry {
            author,
            note: Some(note),
            ..
        }) = &self.review.latest
        {
            let mut lines = note.lines();
            let first = lines.next().unwrap_or_default();
            let more = if lines.next().is_some() { "…" } else { "" };
            text.push_str(&format!(" · {author}: {first}{more}"));
        }
        let hint = match self.review.state {
            ReviewState::Turn(Side::Human) => "H:hand off C:close",
            ReviewState::Turn(Side::Agent) => "C:close",
            ReviewState::Closed(_) => "O:reopen",
        };
        (text, hint)
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
            | Mode::Search {
                anchor: Some(anchor),
                ..
            }
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

    /// What changed at 0-based `line` since the human's last look, if
    /// anything.
    pub fn change(&self, line: usize) -> Option<ChangeKind> {
        self.changes
            .iter()
            .find(|change| {
                let LineRange { start, end } = change.range;
                (start as usize..=end as usize).contains(&(line + 1))
            })
            .map(|change| change.kind)
    }

    /// How many changes (hunks) there are since the human's last look.
    pub fn changes(&self) -> usize {
        self.changes.len()
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
        match view.thread.status {
            Status::Open if view.thread.is_waiting_on(&self.author) => Marker::Waiting,
            Status::Open => Marker::Open,
            Status::Resolved { .. } => Marker::Resolved,
        }
    }

    /// Terminal rows below the file view in the current mode.
    pub(super) fn chrome_rows(&self) -> usize {
        match self.mode {
            Mode::Normal | Mode::Visual { .. } | Mode::Search { .. } => FOOTER_ROWS,
            Mode::Input { .. } => self.box_text_rows() + BOX_BORDER_ROWS,
            Mode::Panel { .. } => self.panel_rows() + FOOTER_ROWS,
            Mode::Overlay { .. } => self.rows,
        }
    }

    /// Listing rows an overlay shows: the screen less its title and footer
    /// rows.
    pub(super) fn overlay_body_rows(&self) -> usize {
        self.rows.saturating_sub(OVERLAY_CHROME_ROWS)
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

    /// Rows available to the file view, between the banner and the chrome
    /// below it; never zero, so the arithmetic in `clamp_view` and the
    /// half-page step stay total on tiny terminals.
    pub(super) fn height(&self) -> usize {
        self.rows
            .saturating_sub(BANNER_ROWS + self.chrome_rows())
            .max(1)
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
        self.help = keys::rows(self.cols);
        self.review_rows = review_rows(&self.review, self.cols);
    }

    /// The screen rows of 0-based `line` at the current width: its table
    /// rows if it has them, in the line's whole-line style only (layout
    /// moves the bytes inline spans point at), else the styled line wrapped
    /// by [`wrap::rows`]. A table is laid out or raw as a whole.
    fn wrapped(&self, line: usize) -> Vec<Line<'static>> {
        let styling = self.stylings.get(line);
        let query = self.highlight_query().unwrap_or_default();
        match &self.tables[line] {
            Some(rows) => {
                let style = styling.map_or(Style::new(), |styling| styling.line);
                rows.iter()
                    .map(|row| {
                        let shown = Line::from(Span::styled(row.clone(), style));
                        search::emphasize(shown, &search::find(row, query))
                    })
                    .collect()
            }
            None => {
                let raw = self.lines[line].as_str();
                let styled = styling.map_or_else(|| Line::raw(raw), |styling| styling.apply(raw));
                let styled = search::emphasize(styled, &search::find(raw, query));
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
        if matches!(self.mode, Mode::Overlay { .. }) {
            // The file is hidden, so its view stays as it was.
            self.clamp_overlay();
            return;
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

    /// Move to the start of the nearest change strictly after (or before)
    /// the cursor; stay put when there is none.
    fn jump_to_change(&mut self, forward: bool) {
        let starts = self
            .changes
            .iter()
            .map(|change| (change.range.start as usize).saturating_sub(1));
        let target = if forward {
            starts.into_iter().find(|&line| line > self.cursor)
        } else {
            starts.rev().find(|&line| line < self.cursor)
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
        Event::Saved { id, done, view } => {
            app.reread(view);
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
            app.status = Some(format!("{} {id}", done.as_str()));
            None
        }
        Event::ReviewSaved { message, view } => {
            app.reread(view);
            app.mode = Mode::Normal;
            app.status = Some(message);
            None
        }
        Event::SaveFailed(message) => {
            app.status = Some(message);
            None
        }
        Event::Refreshed(view) => {
            app.reread(view);
            None
        }
        Event::RefreshFailed(message) => {
            app.status = Some(message);
            None
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            app.status = None;
            match app.mode {
                Mode::Normal => normal_key(app, key),
                Mode::Visual { .. } => visual_key(app, key),
                Mode::Search { .. } => search_key(app, key),
                Mode::Panel { .. } => panel_key(app, key),
                Mode::Input { .. } => input_key(app, key),
                Mode::Overlay { .. } => overlay_key(app, key),
            }
        }
        Event::Key(_) => None,
        Event::Paste(text) => {
            if let Mode::Input {
                draft,
                discard_asked,
                ..
            } = &mut app.mode
            {
                app.status = None;
                *discard_asked = false;
                draft.paste(&text);
            }
            None
        }
    }
}

/// The modifiers of one key, read once so every handler agrees on them.
///
/// Terminals report AltGr as ctrl+alt, so a key with both held is text (it
/// types `[`, `{`, `@`, ...), not a chord: [`ctrl_only`](Self::ctrl_only) is
/// a real ctrl chord, [`ctrl`](Self::ctrl) is the looser test of modes that
/// ignore ctrl+alt letters. No handler reads `KeyEvent::modifiers` itself.
#[derive(Clone, Copy)]
struct Mods(KeyModifiers);

impl Mods {
    fn of(key: KeyEvent) -> Self {
        Self(key.modifiers)
    }

    /// Ctrl is held, AltGr included.
    fn ctrl(self) -> bool {
        self.0.contains(KeyModifiers::CONTROL)
    }

    /// Ctrl without alt: a real ctrl chord, never AltGr.
    fn ctrl_only(self) -> bool {
        self.ctrl() && !self.alt_gr()
    }

    /// Ctrl or alt is held.
    fn chord(self) -> bool {
        self.ctrl() || self.0.contains(KeyModifiers::ALT)
    }

    /// Ctrl and alt together: how terminals report AltGr.
    fn alt_gr(self) -> bool {
        self.0.contains(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }

    fn shift(self) -> bool {
        self.0.contains(KeyModifiers::SHIFT)
    }
}

/// Apply a key that moves the cursor; `false` if `key` is not one.
///
/// Normal and Visual share it, so the two modes move alike.
fn movement(app: &mut App, key: KeyEvent) -> bool {
    let mods = Mods::of(key);
    let (ctrl, chord, ctrl_only) = (mods.ctrl(), mods.chord(), mods.ctrl_only());
    // The letter aliases are plain keys only; ctrl+j is not `j`.
    let code = match key.code {
        KeyCode::Char('j') if !ctrl => KeyCode::Down,
        KeyCode::Char('k') if !ctrl => KeyCode::Up,
        KeyCode::Char('g') if !ctrl => KeyCode::Home,
        KeyCode::Char('G') if !ctrl => KeyCode::End,
        code => code,
    };
    match code {
        KeyCode::Char('d') if ctrl => app.scroll_half_page(true),
        KeyCode::Char('u') if ctrl => app.scroll_half_page(false),
        KeyCode::Down => app.move_to(app.cursor + 1),
        KeyCode::Up => app.move_to(app.cursor.saturating_sub(1)),
        KeyCode::Home => app.move_to(0),
        KeyCode::End => app.move_to(app.last_line()),
        KeyCode::Char(']') if !ctrl_only => app.jump_to_covered(true),
        KeyCode::Char('[') if !ctrl_only => app.jump_to_covered(false),
        KeyCode::Char('}') if !ctrl_only => app.jump_to_change(true),
        KeyCode::Char('{') if !ctrl_only => app.jump_to_change(false),
        KeyCode::Char('n') if !chord => app.repeat_search(true),
        KeyCode::Char('N') if !chord => app.repeat_search(false),
        KeyCode::PageDown => app.scroll_half_page(true),
        KeyCode::PageUp => app.scroll_half_page(false),
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
    let ctrl = Mods::of(key).ctrl();
    match key.code {
        KeyCode::Char('?') if !ctrl => app.open_overlay(Overlay::Help, Back::Normal),
        KeyCode::Char('i') if !ctrl => app.open_overlay(Overlay::Review, Back::Normal),
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
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::model::Verdict;
    use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::style::{Color, Modifier, Style};

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
    fn resize_reclamps_the_view_and_zero_height_does_not_panic() {
        let mut a = app(20, 12); // view height 10
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (19, 10));

        send(&mut a, [Event::Resize(40, 5)]); // view height 3
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

    #[test]
    fn opening_from_a_view_shows_its_text_threads_review_changes_and_hash() {
        let thread = said_by(
            view("c1", (2, 2), Status::Open, State::Current),
            &["claude"],
        );
        let a = App::open(
            View {
                sha256: "seen".into(),
                threads: vec![thread],
                review: handed_off("over to you"),
                changes: vec![change(1, 1, ChangeKind::Changed)],
                ..showing(vec!["one".into(), "two".into()], &[])
            },
            "scott",
            (80, 10),
        );

        assert_eq!(a.lines(), ["one", "two"]);
        assert_eq!((a.cursor, a.top), (0, 0));
        assert_eq!(a.marker(1), Some(Marker::Waiting));
        assert_eq!(a.review(), &handed_off("over to you"));
        assert_eq!(a.change(0), Some(ChangeKind::Changed));
        assert_eq!(a.seen(), "seen");
    }

    // --- wrapped rows ---

    fn wrapped(lines: &[&str], (cols, rows): (u16, u16)) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(showing(lines, &[]), "scott", (cols, rows))
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
            (14, 7), // view height 5
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

    #[test]
    fn j_scrolls_so_a_wrapped_cursor_line_is_fully_shown() {
        let mut a = wrapped(&[W3, W3, W3, "x"], (14, 9)); // height 7

        send(&mut a, [ch('j'), ch('j')]);

        assert_eq!(at(&a), (2, 1), "lines 1 and 2 take 6 of the 7 rows");
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn capital_g_shows_all_of_a_wrapped_last_line_with_no_blank_rows() {
        let mut lines = vec!["x"; 10];
        lines.push(W3);
        let mut a = wrapped(&lines, (15, 9)); // 2 digits, width 10, height 7

        send(&mut a, [ch('G')]);

        assert_eq!(at(&a), (10, 6), "4 short lines and the 3 rows of the last");
        assert_eq!(a.screen_rows().len(), 7);
    }

    #[test]
    fn a_cursor_line_taller_than_the_view_starts_at_the_top() {
        let mut a = wrapped(&["x", W3, "y"], (14, 4)); // height 2

        send(&mut a, [ch('j')]);

        assert_eq!(at(&a), (1, 1));
    }

    #[test]
    fn a_bracket_jump_past_wrapped_lines_shows_the_target() {
        let lines = [W3, W3, W3, "x", "y"].map(String::from).to_vec();
        let threads = [view("c1", (4, 4), Status::Open, State::Current)];
        let mut a = App::open(showing(lines, &threads), "scott", (14, 9)); // height 7

        send(&mut a, [ch(']')]);

        assert_eq!(at(&a), (3, 1));
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn opening_the_panel_below_wrapped_lines_keeps_the_cursor_line_shown() {
        let lines = [W3, W3, W3, W3, "t"].map(String::from).to_vec();
        let threads = [chatty("c1", (5, 5), 1)];
        let mut a = App::open(showing(lines, &threads), "scott", (14, 17)); // height 15
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
        let mut a = wrapped(&[W3, W3, W3, "x"], (80, 9));
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (3, 0));

        send(&mut a, [Event::Resize(14, 9)]);

        assert_eq!(a.screen_rows()[0].text.to_string(), "aaaa bbbb ");
        assert_eq!(at(&a), (3, 1));
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn half_a_page_counts_rows_not_lines() {
        // 2 digits, width 10: every line is 3 rows. Height 11, half page 5.
        let mut a = wrapped(&[W3; 20], (15, 13));

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
        let mut a = wrapped(&lines, (24, 6)); // text width 20, height 4

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
            (24, 5), // height 3
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
        let mut a = App::open(showing(Vec::new(), &[]), "scott", (80, 10));

        send(
            &mut a,
            [ch('j'), ch('k'), ch('G'), ch('g'), ctrl('d'), ctrl('u')],
        );

        assert_eq!(at(&a), (0, 0));
    }

    // --- Input mode ---

    #[test]
    fn mods_answer_for_each_modifier_combination() {
        use KeyModifiers as K;
        let of = |m| Mods::of(KeyEvent::new(KeyCode::Char('x'), m));
        // (mods, ctrl, ctrl_only, chord, alt_gr, shift)
        let table = [
            (K::NONE, false, false, false, false, false),
            (K::SHIFT, false, false, false, false, true),
            (K::CONTROL, true, true, true, false, false),
            (K::ALT, false, false, true, false, false),
            (K::CONTROL | K::ALT, true, false, true, true, false),
            (K::CONTROL | K::SHIFT, true, true, true, false, true),
        ];
        for (m, ctrl, ctrl_only, chord, alt_gr, shift) in table {
            let got = of(m);
            assert_eq!(
                (
                    got.ctrl(),
                    got.ctrl_only(),
                    got.chord(),
                    got.alt_gr(),
                    got.shift()
                ),
                (ctrl, ctrl_only, chord, alt_gr, shift),
                "{m:?}"
            );
        }
    }

    #[test]
    fn only_mods_reads_key_modifiers_outside_tests() {
        let src = include_str!("mod.rs");
        let production = &src[..src.find("#[cfg(test)]").expect("test module")];
        let reads = production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains(".modifiers"))
            .count();
        assert_eq!(reads, 1, "handlers must go through Mods::of");
    }

    // --- markers ---

    use crate::model::{State, Status};

    fn open_view(threads: &[ThreadView], n: usize) -> App {
        let lines = (1..=n).map(|i| i.to_string()).collect();
        App::open(showing(lines, threads), "scott", (80, 10)).labeled("plan.md")
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

        let a = App::open(
            showing(vec!["a".into(), "b".into()], &threads),
            "scott",
            (80, 10),
        );

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

        let text = same_text(&a);
        update(
            &mut a,
            Event::Saved {
                id: "c1".into(),
                done: ThreadWrite::Added,
                view: View { threads, ..text },
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

    // --- adding a thread from the panel ---

    #[test]
    fn input_box_shrinks_the_view_and_keeps_the_cursor_visible() {
        let mut a = app(20, 8); // view height 6 in Normal
        send(&mut a, [ch('j'), ch('j'), ch('j'), ch('j'), ch('j')]);
        assert_eq!(at(&a), (5, 0), "cursor on the last visible row");

        send(&mut a, [press(KeyCode::Enter)]);

        assert_eq!(
            a.height(),
            4,
            "8 rows minus the banner and a 3-row input box"
        );
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

    // --- the key listing and the handlers agree ---

    fn key_event(name: &str) -> KeyEvent {
        let none = KeyModifiers::NONE;
        match name {
            "enter" => KeyEvent::new(KeyCode::Enter, none),
            "esc" => KeyEvent::new(KeyCode::Esc, none),
            "tab" => KeyEvent::new(KeyCode::Tab, none),
            "shift+tab" => KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            "↓" => KeyEvent::new(KeyCode::Down, none),
            "↑" => KeyEvent::new(KeyCode::Up, none),
            "PgDn" => KeyEvent::new(KeyCode::PageDown, none),
            "PgUp" => KeyEvent::new(KeyCode::PageUp, none),
            "Home" => KeyEvent::new(KeyCode::Home, none),
            "End" => KeyEvent::new(KeyCode::End, none),
            _ => match name.strip_prefix("ctrl+") {
                Some(c) => KeyEvent::new(
                    KeyCode::Char(c.chars().next().unwrap()),
                    KeyModifiers::CONTROL,
                ),
                None => KeyEvent::new(KeyCode::Char(name.chars().next().unwrap()), none),
            },
        }
    }

    /// Every key worth trying: printable ASCII plain and with ctrl, and the
    /// named keys.
    fn candidates() -> Vec<KeyEvent> {
        let mut out = vec![];
        for c in ' '..='~' {
            out.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            out.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        }
        // Terminals report shift+tab as BackTab with shift held.
        out.push(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        for code in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            out.push(KeyEvent::new(code, KeyModifiers::NONE));
        }
        out
    }

    /// Whether pressing `key` in `fixture()` changes anything or asks for
    /// an effect.
    fn does_something(fixture: &dyn Fn() -> App, key: KeyEvent) -> bool {
        let mut a = fixture();
        let before = (snapshot(&a), a.status.clone());
        let effect = update(&mut a, Event::Key(key));
        effect.is_some() || (snapshot(&a), a.status.clone()) != before
    }

    /// Normal mode mid-file with an active search and threads either side.
    fn normal_fixture() -> App {
        let threads = [
            said_by(
                view("c1", (3, 3), Status::Open, State::Current),
                &["claude"],
            ),
            said_by(
                view("c2", (35, 35), Status::Open, State::Current),
                &["claude"],
            ),
        ];
        let lines = (1..=40).map(|i| format!("line {i}")).collect();
        let mut a = App::open(
            View {
                changes: vec![
                    change(2, 2, ChangeKind::Changed),
                    change(30, 31, ChangeKind::Changed),
                ],
                ..showing(lines, &threads)
            },
            "scott",
            (80, 10),
        );
        send(
            &mut a,
            [ch('/'), ch('2'), ch('9'), press(KeyCode::Enter), ch('g')],
        );
        send(&mut a, std::iter::repeat_with(|| ch('j')).take(8));
        a
    }

    /// The panel on a line with a long open thread and a resolved one,
    /// scrolled one row down, the open one selected.
    fn panel_fixture() -> App {
        let threads = [
            chatty("c1", (2, 2), 8),
            said_by(view("c2", (2, 2), resolved(), State::Current), &["claude"]),
        ];
        let mut a = panel_app(&threads, 12);
        send(&mut a, [ch('j'), press(KeyCode::Enter), ch('j')]);
        assert!(
            matches!(
                a.mode,
                Mode::Panel {
                    selected: 0,
                    scroll: 1
                }
            ),
            "{:?}",
            a.mode
        );
        a
    }

    fn listed_keys(title: &str) -> Vec<KeyEvent> {
        let section = keys::HELP.iter().find(|s| s.title == title).unwrap();
        section
            .entries
            .iter()
            .flat_map(|entry| keys::names(entry.keys))
            .map(key_event)
            .collect()
    }

    fn assert_listing_matches(title: &str, fixture: &dyn Fn() -> App) {
        let listed = listed_keys(title);
        for key in candidates() {
            if does_something(fixture, key) && !listed.contains(&key) {
                panic!("{title}: {key:?} does something but is not in the help listing");
            }
        }
        for key in &listed {
            assert!(
                does_something(fixture, *key),
                "{title}: {key:?} is listed but does nothing"
            );
        }
    }

    #[test]
    fn every_bound_normal_key_is_listed_and_every_listed_one_is_bound() {
        assert_listing_matches("Normal", &normal_fixture);
    }

    #[test]
    fn every_bound_panel_key_is_listed_and_every_listed_one_is_bound() {
        assert_listing_matches("Thread panel", &panel_fixture);
    }

    #[test]
    fn the_listed_keys_are_named_in_interfaces_md() {
        let doc = include_str!("../../../docs/interfaces.md");
        for section in keys::HELP {
            for entry in section.entries {
                for name in keys::names(entry.keys) {
                    let quoted = format!("`{name}`");
                    assert!(
                        doc.contains(&quoted),
                        "{name} ({}) is not in docs/interfaces.md",
                        entry.keys
                    );
                }
            }
        }
    }

    // --- the review state ---

    #[test]
    fn the_banner_takes_a_row_from_the_file_view() {
        let a = app(20, 10);

        assert_eq!(a.height(), 8, "10 rows less the banner and the footer");
    }

    // --- changes since the last look ---

    fn change_marks(a: &App) -> Vec<Option<ChangeKind>> {
        (0..a.lines.len()).map(|i| a.change(i)).collect()
    }

    #[test]
    fn changes_mark_their_lines_and_count_as_hunks() {
        let a = App::open(
            View {
                changes: vec![
                    change(2, 3, ChangeKind::Changed),
                    change(5, 5, ChangeKind::Removed),
                ],
                ..showing(numbered(5), &[])
            },
            "scott",
            (80, 10),
        );

        let (c, r) = (Some(ChangeKind::Changed), Some(ChangeKind::Removed));
        assert_eq!(change_marks(&a), [None, c, c, None, r], "0-based");
        assert_eq!(a.changes(), 2);
    }

    #[test]
    fn without_changes_nothing_is_marked() {
        let a = app(3, 10);

        assert_eq!(change_marks(&a), [None, None, None]);
        assert_eq!(a.changes(), 0);
    }

    #[test]
    fn a_review_write_replaces_the_changes_with_the_re_read_ones() {
        let mut a = App::open(
            View {
                changes: vec![change(1, 1, ChangeKind::Changed)],
                ..showing(numbered(3), &[])
            },
            "scott",
            (80, 10),
        );

        let text = same_text(&a);
        update(
            &mut a,
            Event::ReviewSaved {
                message: "handed off".into(),
                view: text,
            },
        );

        assert_eq!(change_marks(&a), [None, None, None]);
    }

    #[test]
    fn a_thread_write_replaces_the_changes_with_the_re_read_ones() {
        let mut a = App::open(
            View {
                changes: vec![change(1, 1, ChangeKind::Changed)],
                ..showing(numbered(3), &[])
            },
            "scott",
            (80, 10),
        );

        let text = same_text(&a);
        update(
            &mut a,
            Event::Saved {
                id: "c1".into(),
                done: ThreadWrite::Added,
                view: View {
                    changes: vec![change(3, 3, ChangeKind::Removed)],
                    ..text
                },
            },
        );

        assert_eq!(change_marks(&a), [None, None, Some(ChangeKind::Removed)]);
    }

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
        send(&mut a, [ch('j'), ch('}'), ch('{')]);
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

        send(&mut a, [ch('v'), ch('}')]);

        assert_eq!(a.selection(), (0, 2));
    }

    // --- live refresh: applying a re-read ---

    /// A re-read showing `lines` and `threads`, the review unchanged.
    fn view_of(a: &App, lines: Vec<String>, threads: Vec<ThreadView>) -> View {
        View {
            sha256: format!("sha of {}", lines.join("|")),
            lines,
            threads,
            review: a.review().clone(),
            ..View::default()
        }
    }

    fn refresh(a: &mut App, lines: Vec<String>, threads: Vec<ThreadView>) {
        let view = view_of(a, lines, threads);
        update(a, Event::Refreshed(view));
    }

    /// The view clamp's invariant: the whole cursor line is on screen.
    fn clamped(a: &App) -> bool {
        a.top <= a.cursor && a.cursor < a.lines.len() && cursor_line_shown(a)
    }

    #[test]
    fn lines_inserted_above_the_cursor_keep_it_on_its_text() {
        let mut a = app(20, 10);
        send(&mut a, (0..12).map(|_| ch('j')));
        let (cursor, top) = at(&a);
        let mut lines = numbered(20);
        lines.splice(0..0, ["new a".into(), "new b".into(), "new c".into()]);

        refresh(&mut a, lines, vec![]);

        assert_eq!(at(&a), (cursor + 3, top + 3));
        assert_eq!(a.lines()[a.cursor()], "line 13");
        assert!(clamped(&a));
    }

    #[test]
    fn an_edited_cursor_line_keeps_a_best_guess_and_a_shrunk_file_clamps() {
        let mut a = app(10, 20);
        send(&mut a, (0..4).map(|_| ch('j')));
        let mut lines = numbered(10);
        lines[4] = "LINE 5".into();

        refresh(&mut a, lines, vec![]);
        assert_eq!(a.cursor(), 4);

        send(&mut a, (0..4).map(|_| ch('j')));
        refresh(&mut a, numbered(3), vec![]);
        assert_eq!(a.cursor(), 2);
        assert!(clamped(&a));
    }

    #[test]
    fn a_shrink_mid_selection_maps_and_clamps_the_anchor() {
        let mut a = app(10, 20);
        send(&mut a, (0..6).map(|_| ch('j')));
        send(&mut a, [ch('v'), ch('j'), ch('j')]);
        assert_eq!(a.selection(), (6, 8));

        refresh(&mut a, numbered(4), vec![]);

        assert_eq!(a.mode, Mode::Visual { anchor: 3 });
        assert_eq!(a.selection(), (3, 3));
        assert!(clamped(&a));
    }

    #[test]
    fn a_selections_box_keeps_its_draft_and_its_range_follows_the_text() {
        let mut a = app(10, 20);
        send(
            &mut a,
            [ch('j'), ch('j'), ch('v'), ch('j'), press(KeyCode::Enter)],
        );
        typed(&mut a, "half");
        let mut lines = numbered(10);
        lines.insert(0, "new".into());

        refresh(&mut a, lines, vec![]);

        assert_eq!(draft(&a).0, "half");
        assert_eq!(a.selection(), (3, 4));
        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 4,
                end: 5,
                body: "half".into()
            })
        );
    }

    #[test]
    fn half_typed_boxes_survive_a_refresh() {
        let threads = vec![view("c1", (1, 1), Status::Open, State::Current)];
        for open in [
            &[press(KeyCode::Enter), ch('r')][..],
            &[ch('H')],
            &[ch('C'), press(KeyCode::Tab)],
            &[ch('j'), press(KeyCode::Enter)],
        ] {
            let mut a = App::open(showing(numbered(5), &threads), "scott", (80, 20));
            send(&mut a, open.iter().cloned());
            typed(&mut a, "draft");
            let before = a.mode.clone();

            refresh(&mut a, numbered(5), threads.clone());

            assert_eq!(a.mode, before, "{open:?}");
        }
    }

    #[test]
    fn a_refresh_keeps_a_discard_ask_and_clears_a_close_confirm() {
        let threads = vec![view("c1", (1, 1), Status::Open, State::Current)];
        let mut a = App::open(showing(numbered(5), &threads), "scott", (80, 20));
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "x");
        update(&mut a, press(KeyCode::Esc));
        refresh(&mut a, numbered(5), threads.clone());
        assert!(matches!(
            a.mode,
            Mode::Input {
                discard_asked: true,
                ..
            }
        ));

        let mut a = App::open(showing(numbered(5), &threads), "scott", (80, 20));
        send(&mut a, [ch('C'), press(KeyCode::Tab)]);
        assert_eq!(update(&mut a, ctrl('s')), None, "asks to confirm");
        refresh(&mut a, numbered(5), threads.clone());

        assert!(matches!(
            a.mode,
            Mode::Input {
                target: Target::Close {
                    verdict: Some(Verdict::Approved),
                    confirm_asked: false,
                },
                ..
            }
        ));
        assert_eq!(update(&mut a, ctrl('s')), None, "asks again");
    }

    fn selected_id(a: &App) -> Option<String> {
        let Mode::Panel { selected, .. } = a.mode else {
            return None;
        };
        a.threads_on(a.cursor())
            .get(selected)
            .map(|view| view.thread.id.clone())
    }

    #[test]
    fn the_panel_keeps_its_thread_by_id_across_a_refresh() {
        let c1 = view("c1", (1, 1), Status::Open, State::Current);
        let c3 = view("c3", (1, 1), Status::Open, State::Current);
        let mut a = App::open(
            showing(numbered(5), &[c1.clone(), c3.clone()]),
            "scott",
            (80, 20),
        );
        send(&mut a, [press(KeyCode::Enter), press(KeyCode::Tab)]);
        assert_eq!(selected_id(&a).as_deref(), Some("c3"));

        let c2 = view("c2", (1, 1), Status::Open, State::Current);
        refresh(&mut a, numbered(5), vec![c1.clone(), c2, c3]);
        assert_eq!(selected_id(&a).as_deref(), Some("c3"));

        refresh(&mut a, numbered(5), vec![c1.clone()]);
        assert_eq!(selected_id(&a).as_deref(), Some("c1"), "clamped");

        refresh(&mut a, numbered(5), vec![]);
        assert_eq!(a.mode, Mode::Normal, "no threads left on the line");
    }

    #[test]
    fn a_reply_box_from_the_panel_returns_to_its_thread_after_a_refresh() {
        let c1 = view("c1", (1, 1), Status::Open, State::Current);
        let c3 = view("c3", (1, 1), Status::Open, State::Current);
        let mut a = App::open(
            showing(numbered(5), &[c1.clone(), c3.clone()]),
            "scott",
            (80, 20),
        );
        send(
            &mut a,
            [press(KeyCode::Enter), press(KeyCode::Tab), ch('r')],
        );
        typed(&mut a, "x");

        let c2 = view("c2", (1, 1), Status::Open, State::Current);
        refresh(&mut a, numbered(5), vec![c1, c2, c3]);
        send(&mut a, [press(KeyCode::Esc), press(KeyCode::Esc)]);

        assert_eq!(selected_id(&a).as_deref(), Some("c3"));
    }

    #[test]
    fn the_panel_follows_its_thread_to_the_line_it_moved_to() {
        let c1 = view("c1", (1, 1), Status::Open, State::Current);
        let c2 = view("c2", (1, 1), Status::Open, State::Current);
        let mut a = App::open(showing(numbered(5), &[c1.clone(), c2]), "scott", (80, 20));
        send(&mut a, [press(KeyCode::Enter), press(KeyCode::Tab)]);

        let moved = view("c2", (3, 3), Status::Open, State::Outdated);
        refresh(&mut a, numbered(5), vec![c1, moved]);

        assert_eq!(a.cursor(), 2);
        assert_eq!(selected_id(&a).as_deref(), Some("c2"));
    }

    #[test]
    fn a_refresh_shows_the_new_state_changes_and_layout() {
        let mut a =
            App::open(showing(vec!["# Title".into()], &[]), "scott", (80, 20)).highlighted(true);
        update(&mut a, ch('i'));
        let lines: Vec<String> = ["# Title", "", "| a | b |", "|---|---|", "| 1 | 2 |"]
            .map(String::from)
            .to_vec();
        let view = View {
            review: handed_off("Your go"),
            changes: vec![Change {
                range: LineRange { start: 3, end: 5 },
                kind: ChangeKind::Changed,
            }],
            ..view_of(&a, lines, vec![])
        };

        update(&mut a, Event::Refreshed(view));

        assert!(a.banner().0.contains("Your go"), "{:?}", a.banner());
        assert!(
            a.overlay_rows(Overlay::Review)
                .iter()
                .any(|row| format!("{row:?}").contains("Your go"))
        );
        assert_eq!(a.change(2), Some(ChangeKind::Changed));
        assert_eq!(a.changes(), 1);
        assert!(a.tables[2].is_some(), "the new table is laid out");
        assert_eq!(a.stylings.len(), 5, "highlighting follows the new text");
    }

    #[test]
    fn a_refresh_maps_the_search_prompts_origin() {
        let mut a = app(20, 10);
        send(&mut a, (0..5).map(|_| ch('j')));
        update(&mut a, ch('/'));
        let mut lines = numbered(20);
        lines.splice(0..0, ["x".into(), "y".into()]);

        refresh(&mut a, lines, vec![]);
        update(&mut a, press(KeyCode::Esc));

        assert_eq!(a.lines()[a.cursor()], "line 6");
    }

    #[test]
    fn text_arriving_in_an_empty_view_starts_at_the_top() {
        let mut a = app(0, 10);

        refresh(&mut a, numbered(30), vec![]);

        assert_eq!(at(&a), (0, 0));
    }

    #[test]
    fn a_refresh_records_the_content_seen() {
        let mut a = app(3, 10);

        update(
            &mut a,
            Event::Refreshed(View {
                sha256: "seen".into(),
                ..showing(numbered(4), &[])
            }),
        );

        assert_eq!(a.seen(), "seen");
    }

    #[test]
    fn a_saves_reread_applies_the_new_text_too() {
        let mut a = app(5, 20);
        send(&mut a, [ch('j'), ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "x");
        update(&mut a, ctrl('s'));
        let mut lines = numbered(5);
        lines.insert(0, "new".into());
        let view = view_of(&a, lines.clone(), vec![]);

        update(
            &mut a,
            Event::Saved {
                id: "c1".into(),
                done: ThreadWrite::Added,
                view,
            },
        );
        assert_eq!(a.lines(), lines);
        assert_eq!(a.lines()[a.cursor()], "line 3");

        lines.insert(0, "newer".into());
        let view = view_of(&a, lines.clone(), vec![]);
        update(
            &mut a,
            Event::ReviewSaved {
                message: "handed off".into(),
                view,
            },
        );
        assert_eq!(a.lines(), lines);
        assert_eq!(a.lines()[a.cursor()], "line 3");
    }
}
