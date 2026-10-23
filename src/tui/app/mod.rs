//! TUI state and its pure transition function.
//!
//! [`update`] does no I/O and reads no clock: it turns an [`Event`] into a
//! state change and, at most, an [`Effect`] for the driver to perform.

use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::Line;

use crate::anchor::{Change, ChangeKind, Diff, LineRange, line_number};
use crate::model::{Entry, ReviewState, Side, State, Status, Verdict};
use crate::review::{ReviewStatus, ThreadView, ThreadWrite, View};

use super::draft::Draft;
use super::highlight::{self, Styling};
use super::{keys, table};

mod find;
mod input;
mod layout;
mod normal;
mod overlay;
mod threads;

use find::search_key;
use input::input_key;
use normal::{is_leave, normal_key, visual_key};
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

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::model::Verdict;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

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
