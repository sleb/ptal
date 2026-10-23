//! TUI state and its pure transition function.
//!
//! [`update`] does no I/O and reads no clock: it turns an [`Event`] into a
//! state change and, at most, an [`Effect`] for the driver to perform.
//!
//! The driver never patches this state after a write: it re-reads the one
//! [`View`] and sends it back, so the screen shows what is on disk. After
//! every event the cursor line is on screen (wrapped rows counted), and in
//! [`Mode::Input`] the draft's cursor row is in the box.
//!
//! This module holds the types, [`update`] and the modifier helper; each
//! submodule adds an `impl App` block for one concern:
//!
//! - `state`: opening from a [`View`], applying re-reads, and what the
//!   screen asks of the state (cursor, selection, threads, markers,
//!   changes, banner).
//! - `layout`: the screen's rows and columns, the file view's wrapped rows,
//!   and the view clamps.
//! - `normal`: Normal and Visual keys and the movement they share.
//! - `find`: the `/` prompt and `n` / `N`.
//! - `threads`: the thread panel.
//! - `input`: the comment box and what saving it writes.
//! - `overlay`: the help and review overlays.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::Line;

use crate::anchor::Change;
use crate::model::Verdict;
use crate::review::{ReviewStatus, ThreadView, ThreadWrite, View};

use super::draft::Draft;
use super::highlight::Styling;
use super::keys;

mod find;
mod input;
mod layout;
mod normal;
mod overlay;
mod state;
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
    Overlay {
        /// What the overlay shows.
        kind: Overlay,
        /// Where the overlay was opened from, and closing returns.
        back: OverlayBack,
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

/// Where a [`Mode::Overlay`] returns to when it closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayBack {
    /// Normal mode.
    Normal,
    /// The panel at this view.
    Panel {
        /// The selected thread's index.
        selected: usize,
        /// The panel's scroll.
        scroll: usize,
    },
}

impl OverlayBack {
    /// The mode closing returns to.
    fn mode(self) -> Mode {
        match self {
            OverlayBack::Normal => Mode::Normal,
            OverlayBack::Panel { selected, scroll } => Mode::Panel { selected, scroll },
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
    /// Each line's table rows from [`table::align`](super::table::align) at `text_width()`.
    /// Recomputed by `relayout` whenever the text width can change (on open
    /// and resize); anything else that changes it must call `relayout` too.
    tables: Vec<Option<Vec<String>>>,
    /// Each line's markdown styling, from [`highlight::highlight`](super::highlight::highlight) at
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
/// Terminals report AltGr as ctrl+alt, so [`Mods::of`] reads a character
/// typed with both held as that character with no modifiers: every mode acts
/// on it as the plain key (`[`, `{`, `@`, ... type or bind as usual) and no
/// ctrl chord fires from it. Named keys keep ctrl+alt, since AltGr never
/// types one. This is a trade-off: a terminal that reports AltGr with no
/// modifiers (most Unix ones) makes ctrl+alt+letter a real chord, and that
/// chord acts as the bare letter. Nothing is bound to ctrl+alt. No handler reads `KeyEvent::modifiers` itself.
#[derive(Clone, Copy)]
struct Mods(KeyModifiers);

impl Mods {
    fn of(key: KeyEvent) -> Self {
        let both = KeyModifiers::CONTROL | KeyModifiers::ALT;
        let mut mods = key.modifiers;
        if matches!(key.code, KeyCode::Char(_)) && mods.contains(both) {
            mods.remove(both);
        }
        Self(mods)
    }

    /// Ctrl is held: a real chord, never AltGr.
    fn ctrl(self) -> bool {
        self.0.contains(KeyModifiers::CONTROL)
    }

    /// Ctrl or alt is held, AltGr excluded.
    fn chord(self) -> bool {
        self.ctrl() || self.0.contains(KeyModifiers::ALT)
    }

    fn shift(self) -> bool {
        self.0.contains(KeyModifiers::SHIFT)
    }
}

/// What a scrolling key asks for, whatever it scrolls.
///
/// The file view, the overlays and the panel read their scroll keys
/// through [`Scroll::of`], so the keys and their modifier tests are one
/// table; each mode decides what an intent moves (the panel takes only
/// [`Up`](Self::Up) and [`Down`](Self::Down)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scroll {
    /// `k` or `↑`.
    Up,
    /// `j` or `↓`.
    Down,
    /// `ctrl+u` or `PgUp`.
    HalfUp,
    /// `ctrl+d` or `PgDn`.
    HalfDown,
    /// `g` or `Home`.
    Top,
    /// `G` or `End`.
    Bottom,
}

impl Scroll {
    /// The intent of `key`, if it scrolls. Every key but the half pages is
    /// a plain one (ctrl+j is not `j`, and ctrl+↓ is a terminal's word jump,
    /// not `↓`); `PgDn` and `PgUp` take any modifiers.
    fn of(key: KeyEvent) -> Option<Self> {
        let ctrl = Mods::of(key).ctrl();
        Some(match key.code {
            KeyCode::Char('k') | KeyCode::Up if !ctrl => Scroll::Up,
            KeyCode::Char('j') | KeyCode::Down if !ctrl => Scroll::Down,
            KeyCode::Char('u') if ctrl => Scroll::HalfUp,
            KeyCode::Char('d') if ctrl => Scroll::HalfDown,
            KeyCode::PageUp => Scroll::HalfUp,
            KeyCode::PageDown => Scroll::HalfDown,
            KeyCode::Char('g') | KeyCode::Home if !ctrl => Scroll::Top,
            KeyCode::Char('G') | KeyCode::End if !ctrl => Scroll::Bottom,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::anchor::ChangeKind;
    use crate::model::{State, Status};
    use crate::tui::test_support::*;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    // --- modifiers ---

    #[test]
    fn mods_answer_for_each_modifier_combination() {
        use KeyModifiers as K;
        let of = |code, m| Mods::of(KeyEvent::new(code, m));
        let c = KeyCode::Char('x');
        // (key, mods, ctrl, chord, shift)
        let table = [
            (c, K::NONE, false, false, false),
            (c, K::SHIFT, false, false, true),
            (c, K::CONTROL, true, true, false),
            (c, K::ALT, false, true, false),
            (c, K::CONTROL | K::SHIFT, true, true, true),
            // A character typed with ctrl+alt is AltGr text: no modifiers.
            (c, K::CONTROL | K::ALT, false, false, false),
            (c, K::CONTROL | K::ALT | K::SHIFT, false, false, true),
            // AltGr never types a named key, so that stays a ctrl chord.
            (KeyCode::Down, K::CONTROL | K::ALT, true, true, false),
            (KeyCode::Home, K::CONTROL | K::ALT, true, true, false),
            (KeyCode::Enter, K::CONTROL | K::ALT, true, true, false),
            (KeyCode::Tab, K::CONTROL | K::ALT, true, true, false),
            (KeyCode::Esc, K::CONTROL | K::ALT, true, true, false),
            (KeyCode::Backspace, K::CONTROL | K::ALT, true, true, false),
        ];
        for (code, m, ctrl, chord, shift) in table {
            let got = of(code, m);
            assert_eq!(
                (got.ctrl(), got.chord(), got.shift()),
                (ctrl, chord, shift),
                "{code:?} {m:?}"
            );
        }
    }

    #[test]
    fn only_mods_reads_key_modifiers_outside_tests() {
        // Every file of the module, found by walking its directory so a new
        // submodule (nested or not) cannot escape the scan; each is cut at
        // its test module, not at the first `#[cfg(test)]` item.
        let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tui/app")];
        let mut reads = 0;
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                let tests = src.find("#[cfg(test)]\nmod tests {");
                let production = tests.map_or(&src[..], |at| &src[..at]);
                reads += production
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .filter(|line| line.contains(".modifiers"))
                    .count();
            }
        }
        assert_eq!(reads, 1, "handlers must go through Mods::of");
    }

    #[test]
    fn ctrl_arrows_scroll_nothing_in_any_mode() {
        let ctrl_key = |code, m| Event::Key(KeyEvent::new(code, m));
        let named = [KeyCode::Down, KeyCode::Up, KeyCode::Home, KeyCode::End];
        let normal = || {
            let mut a = app(20, 10);
            typed(&mut a, "jj");
            a
        };
        let help = || {
            let mut a = app(5, 4);
            typed(&mut a, "?j");
            a
        };
        let panel = || {
            let mut a = panel_app(&[chatty("c1", (2, 2), 8)], 12);
            send(&mut a, [ch('j'), press(KeyCode::Enter), ch('j')]);
            a
        };
        for (name, fixture) in [
            ("normal", &normal as &dyn Fn() -> App),
            ("help", &help),
            ("panel", &panel),
        ] {
            for code in named {
                for m in [
                    KeyModifiers::CONTROL,
                    KeyModifiers::CONTROL | KeyModifiers::ALT,
                ] {
                    let mut a = fixture();
                    let before = (a.mode.clone(), at(&a));
                    send(&mut a, [ctrl_key(code, m)]);
                    assert_eq!((a.mode.clone(), at(&a)), before, "{name}: {m:?}+{code:?}");
                }
            }
        }
    }

    #[test]
    fn scroll_of_maps_each_key_plain_and_with_ctrl() {
        use KeyModifiers as K;
        let of = |code, m| Scroll::of(KeyEvent::new(code, m));
        let c = KeyCode::Char;
        // (key, plain, with ctrl)
        let table = [
            (c('j'), Some(Scroll::Down), None),
            (KeyCode::Down, Some(Scroll::Down), None),
            (c('k'), Some(Scroll::Up), None),
            (KeyCode::Up, Some(Scroll::Up), None),
            (c('d'), None, Some(Scroll::HalfDown)),
            (
                KeyCode::PageDown,
                Some(Scroll::HalfDown),
                Some(Scroll::HalfDown),
            ),
            (c('u'), None, Some(Scroll::HalfUp)),
            (KeyCode::PageUp, Some(Scroll::HalfUp), Some(Scroll::HalfUp)),
            (c('g'), Some(Scroll::Top), None),
            (KeyCode::Home, Some(Scroll::Top), None),
            (c('G'), Some(Scroll::Bottom), None),
            (KeyCode::End, Some(Scroll::Bottom), None),
            (c('x'), None, None),
            (KeyCode::Enter, None, None),
        ];
        for (code, plain, with_ctrl) in table {
            assert_eq!(of(code, K::NONE), plain, "{code:?}");
            assert_eq!(of(code, K::CONTROL), with_ctrl, "ctrl+{code:?}");
        }
        // AltGr text is the plain key, never a ctrl chord.
        assert_eq!(of(c('d'), K::CONTROL | K::ALT), None);
        assert_eq!(of(c('j'), K::CONTROL | K::ALT), Some(Scroll::Down));
    }

    // --- the key listing and the handlers agree ---

    fn key(name: &str) -> KeyEvent {
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
            .map(key)
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
}
