//! The app's state: opening it from a [`View`], applying a re-read so the
//! screen stays in place, and what the screen asks of it (the cursor, the
//! selection, the threads and markers, the changes, the banner).

use crate::anchor::{ChangeKind, Diff, LineRange, line_number};
use crate::model::{Entry, ReviewState, Side, State, Status};
use crate::review::{ReviewStatus, ThreadView, View};
use crate::tui::{highlight, table};

use super::{App, Back, Marker, Mode, Target, review_rows, state_name};

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
    pub(super) fn reread(
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

    pub(super) fn marker_of(&self, view: &ThreadView) -> Marker {
        match view.thread.status {
            Status::Open if view.thread.is_waiting_on(&self.author) => Marker::Waiting,
            Status::Open => Marker::Open,
            Status::Resolved { .. } => Marker::Resolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::Change;
    use crate::model::Verdict;
    use crate::review::ThreadWrite;
    use crate::tui::app::fixtures::*;
    use crate::tui::app::{Effect, Event, Overlay, update};
    use crate::tui::test_support::*;
    use ratatui::crossterm::event::KeyCode;

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
        typed(&mut a, "vjj");
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
