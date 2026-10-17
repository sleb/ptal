//! The comment box's text being edited: a pure value, no widgets.
//!
//! The text holds printable characters and `\n` only, so every byte of it
//! is drawn and its rows map 1:1 to byte ranges of it. The cursor is a byte
//! offset that always sits on a grapheme cluster boundary, so it can never
//! land inside a cluster that [`wrap`] refuses to split.

use std::ops::Range;

use super::wrap;

/// A comment being typed, and where the cursor is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    text: String,
    cursor: usize,
    /// The column, in cells, that consecutive [`Draft::up`] and
    /// [`Draft::down`] aim for; every other edit or move clears it.
    goal: Option<usize>,
    /// The first screen row the box shows; see [`Draft::clamp_scroll`].
    scroll: usize,
}

impl Draft {
    /// The text so far.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor as a byte offset into [`Draft::text`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The first of [`Draft::rows`] the box shows.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Scroll just enough that the cursor's row is among the `height` rows
    /// shown at `width`, and no blank rows sit below the text.
    pub fn clamp_scroll(&mut self, width: usize, height: usize) {
        let rows = self.rows(width).len();
        let row = self.cursor_at(width).0;
        let height = height.max(1);
        self.scroll = self
            .scroll
            .clamp((row + 1).saturating_sub(height), row)
            .min(rows.saturating_sub(height));
    }

    /// Insert `c` at the cursor and move past it. A control character is
    /// ignored. If `c` joins the cluster after it (a zero-width joiner, say),
    /// the cursor moves to the end of the joined cluster.
    pub fn insert(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.goal = None;
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        let cursor = self.cursor;
        if let Some(joined) = self
            .clusters()
            .into_iter()
            .find(|cluster| cluster.start < cursor && cursor < cluster.end)
        {
            self.cursor = joined.end;
        }
    }

    /// Start a new line at the cursor.
    pub fn newline(&mut self) {
        self.goal = None;
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    /// Move back over one cluster; a no-op at the start.
    pub fn left(&mut self) {
        self.goal = None;
        if let Some(before) = self.before() {
            self.cursor = before.start;
        }
    }

    /// Move forward over one cluster; a no-op at the end.
    pub fn right(&mut self) {
        self.goal = None;
        if let Some(after) = self.after() {
            self.cursor = after.end;
        }
    }

    /// Remove the cluster before the cursor; a no-op at the start.
    pub fn backspace(&mut self) {
        self.goal = None;
        if let Some(before) = self.before() {
            self.text.replace_range(before.clone(), "");
            self.cursor = before.start;
        }
    }

    /// Remove the cluster after the cursor; a no-op at the end.
    pub fn delete(&mut self) {
        self.goal = None;
        if let Some(after) = self.after() {
            self.text.replace_range(after, "");
        }
    }

    /// Move to the start of the cursor's line (just after the `\n` before
    /// it), whichever of its screen rows the cursor is on.
    pub fn home(&mut self) {
        self.goal = None;
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |at| at + 1);
    }

    /// Move to the end of the cursor's line (just before its `\n`).
    pub fn end(&mut self) {
        self.goal = None;
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |at| self.cursor + at);
    }

    /// The byte range of each screen row of the text at `width` cells: each
    /// line (split on `\n`, which is on no row) wrapped by [`wrap`]. An empty
    /// line, a trailing `\n` and an empty text each make one empty row.
    ///
    /// Whitespace at a soft break hangs on the end of its row, past `width`;
    /// a caller that wants the cursor's cell inside the box passes one less
    /// than the box's inner width.
    pub fn rows(&self, width: usize) -> Vec<Range<usize>> {
        let mut rows = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let shift = |row: Range<usize>| row.start + start..row.end + start;
            rows.extend(wrap::row_ranges(line, width).into_iter().map(shift));
            start += line.len() + 1;
        }
        rows
    }

    /// The cursor's (row, column) in [`Draft::rows`] at `width`, the column
    /// in cells. An offset at a soft break, the end of one row and the start
    /// of the next, shows at the start of the next. A cursor on hanging
    /// whitespace shows in cell `width`, the last one.
    pub fn cursor_at(&self, width: usize) -> (usize, usize) {
        let rows = self.rows(width);
        let row = rows
            .partition_point(|row| row.start <= self.cursor)
            .saturating_sub(1);
        let start = rows.get(row).map_or(0, |row| row.start);
        let col = wrap::width(&self.text[start..self.cursor]).min(width);
        (row, col)
    }

    /// Move to the screen row above at `width` (see [`Draft::rows`]); a
    /// no-op on the first row.
    pub fn up(&mut self, width: usize) {
        if let Some(row) = self.cursor_at(width).0.checked_sub(1) {
            self.move_to_row(row, width);
        }
    }

    /// Move to the screen row below at `width`; a no-op on the last row.
    pub fn down(&mut self, width: usize) {
        self.move_to_row(self.cursor_at(width).0 + 1, width);
    }

    /// Move to screen row `target` (if there is one) at the goal column:
    /// the last cluster boundary on it at or before that column. On a row
    /// that soft-wraps that is at most its last cluster's start, since its
    /// end shows on the next row.
    fn move_to_row(&mut self, target: usize, width: usize) {
        let rows = self.rows(width);
        let Some(row) = rows.get(target).cloned() else {
            return;
        };
        let goal = self.goal.unwrap_or(self.cursor_at(width).1);
        let soft = rows
            .get(target + 1)
            .is_some_and(|next| next.start == row.end);
        let mut landing = row.start;
        for cluster in self.clusters() {
            if cluster.start < row.start {
                continue;
            }
            let past = cluster.end > row.end || (soft && cluster.end == row.end);
            if past || wrap::width(&self.text[row.start..cluster.end]) > goal {
                break;
            }
            landing = cluster.end;
        }
        self.cursor = landing;
        self.goal = Some(goal);
    }

    /// The cluster ending at the cursor.
    fn before(&self) -> Option<Range<usize>> {
        self.clusters().into_iter().find(|c| c.end == self.cursor)
    }

    /// The cluster starting at the cursor.
    fn after(&self) -> Option<Range<usize>> {
        self.clusters().into_iter().find(|c| c.start == self.cursor)
    }

    /// The byte range of every cluster of the text, in order, each `\n`
    /// one cluster of its own (as Unicode segments it: nothing joins a line
    /// feed).
    fn clusters(&self) -> Vec<Range<usize>> {
        let mut clusters = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let shift = |c: Range<usize>| c.start + start..c.end + start;
            clusters.extend(wrap::clusters(line).into_iter().map(shift));
            start += line.len();
            if start < self.text.len() {
                clusters.push(start..start + 1);
                start += 1;
            }
        }
        clusters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Draft {
        let mut draft = Draft::default();
        text.chars().for_each(|c| draft.insert(c));
        draft
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut draft = typed("abc");
        draft.left();
        draft.left();
        draft.insert('x');

        assert_eq!(draft.text(), "axbc");
        assert_eq!(draft.cursor(), 2);
    }

    #[test]
    fn home_end_backspace_and_delete_edit_one_line() {
        let mut draft = typed("abc");

        draft.home();
        assert_eq!(draft.cursor(), 0);
        draft.backspace();
        assert_eq!(draft.text(), "abc", "backspace at the start is a no-op");
        draft.delete();
        assert_eq!((draft.text(), draft.cursor()), ("bc", 0));
        draft.end();
        assert_eq!(draft.cursor(), 2);
        draft.delete();
        assert_eq!(draft.text(), "bc", "delete at the end is a no-op");
        draft.backspace();
        assert_eq!((draft.text(), draft.cursor()), ("b", 1));
        draft.right();
        assert_eq!(draft.cursor(), 1, "right at the end is a no-op");
    }

    #[test]
    fn moves_and_deletes_take_a_whole_grapheme_cluster() {
        for cluster in ["e\u{301}", "👨\u{200d}👩\u{200d}👧"] {
            let mut draft = typed(&format!("a{cluster}b"));
            draft.home();
            draft.right();
            draft.right();
            assert_eq!(draft.cursor(), 1 + cluster.len(), "{cluster:?}");
            draft.left();
            assert_eq!(draft.cursor(), 1, "{cluster:?}");
            draft.delete();
            assert_eq!(draft.text(), "ab", "{cluster:?}");

            let mut draft = typed(&format!("a{cluster}"));
            draft.backspace();
            assert_eq!(draft.text(), "a", "{cluster:?}");
        }
    }

    #[test]
    fn a_char_joining_two_clusters_leaves_the_cursor_after_the_join() {
        let mut draft = typed("👨👩");
        draft.left();
        draft.insert('\u{200d}');

        assert_eq!(draft.cursor(), draft.text().len());
    }

    /// `text` typed, with every `\n` from [`Draft::newline`].
    fn lines(text: &str) -> Draft {
        let mut draft = Draft::default();
        for c in text.chars() {
            match c {
                '\n' => draft.newline(),
                c => draft.insert(c),
            }
        }
        draft
    }

    /// The text of each of `draft`'s rows at `width`.
    fn shown(draft: &Draft, width: usize) -> Vec<&str> {
        draft
            .rows(width)
            .into_iter()
            .map(|row| &draft.text()[row])
            .collect()
    }

    #[test]
    fn rows_wrap_each_line_and_keep_empty_ones() {
        assert_eq!(shown(&lines("one two three"), 8), ["one two ", "three"]);
        assert_eq!(shown(&lines("a\n\nb"), 8), ["a", "", "b"]);
        assert_eq!(shown(&lines("a\n"), 8), ["a", ""]);
        assert_eq!(shown(&lines(""), 8), [""]);
        assert_eq!(shown(&lines("漢字漢字"), 5), ["漢字", "漢字"]);
    }

    #[test]
    fn the_cursor_shows_on_its_row_at_its_cell() {
        let mut draft = lines("one two three");
        assert_eq!(draft.cursor_at(8), (1, 5), "at the end");
        (0..5).for_each(|_| draft.left());
        assert_eq!(
            draft.cursor_at(8),
            (1, 0),
            "a soft break shows on the next row"
        );
        draft.left();
        assert_eq!(draft.cursor_at(8), (0, 7));

        let draft = lines("a\nb");
        let mut after_newline = draft.clone();
        after_newline.left();
        assert_eq!(after_newline.cursor_at(8), (1, 0));
        let mut before_newline = after_newline.clone();
        before_newline.left();
        assert_eq!(before_newline.cursor_at(8), (0, 1));

        let mut wide = lines("漢a");
        wide.left();
        assert_eq!(wide.cursor_at(8), (0, 2), "a wide cluster counts two cells");
    }

    #[test]
    fn a_cursor_on_hanging_whitespace_stays_in_the_last_cell() {
        let mut draft = lines("aaaa bbbb   cccc");
        (0..5).for_each(|_| draft.left());

        assert_eq!(shown(&draft, 9)[0], "aaaa bbbb   ");
        assert_eq!(draft.cursor_at(9), (0, 9));
        draft.right();
        assert_eq!(draft.cursor_at(9), (1, 0));
    }

    /// `draft` with its cursor moved to the start of the text.
    fn at_start(mut draft: Draft, width: usize) -> Draft {
        for _ in 0..draft.rows(width).len() {
            draft.up(width);
        }
        draft.home();
        draft
    }

    #[test]
    fn up_and_down_move_by_screen_row_keeping_the_column() {
        let mut draft = lines("one two three\nfour");

        draft.up(8);
        assert_eq!((draft.cursor(), draft.cursor_at(8)), (12, (1, 4)));
        draft.up(8);
        assert_eq!((draft.cursor(), draft.cursor_at(8)), (4, (0, 4)));
        draft.down(8);
        assert_eq!(draft.cursor(), 12);
        draft.down(8);
        assert_eq!(draft.cursor(), 18);
    }

    #[test]
    fn the_column_sticks_across_a_short_row_until_another_key() {
        let mut draft = lines("aaaaaaaaaaaa\nbbb\ncccccccccccc");
        draft.left();
        draft.left();

        draft.up(20);
        assert_eq!(draft.cursor_at(20), (1, 3));
        draft.up(20);
        assert_eq!(draft.cursor_at(20), (0, 10), "back on the column it left");

        draft.down(20);
        draft.left();
        draft.up(20);
        assert_eq!(draft.cursor_at(20), (0, 2), "left reset the column");
    }

    #[test]
    fn onto_a_soft_wrapped_row_the_cursor_stops_before_the_break() {
        let mut draft = lines("abcdefghij\nabcd");

        draft.up(4);
        assert_eq!(draft.cursor(), 10, "end of `ij`, a whole line's end");
        draft.up(4);
        assert_eq!((draft.cursor(), draft.cursor_at(4)), (7, (1, 3)));
    }

    #[test]
    fn up_on_the_first_row_and_down_on_the_last_do_nothing() {
        let mut draft = lines("ab\ncd");

        draft.down(8);
        assert_eq!(draft.cursor(), 5);
        draft.up(8);
        draft.up(8);
        assert_eq!(draft.cursor(), 2);
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_the_whole_line() {
        let mut draft = lines("one two three\nfour");
        draft.up(8);

        draft.home();
        assert_eq!(draft.cursor(), 0);
        draft.end();
        assert_eq!(draft.cursor(), 13, "before the newline");
        draft.right();
        draft.end();
        assert_eq!(draft.cursor(), 18);
        draft.home();
        assert_eq!(draft.cursor(), 14);
    }

    #[test]
    fn every_boundary_is_reachable_by_down_then_right_and_editable() {
        let text = "First para, long enough to wrap.\n\nSecond 漢字 e\u{301} line.\nThird.";
        let width = 10;
        let start = at_start(lines(text), width);
        // Every cluster boundary, with the row it shows on.
        let mut boundaries = vec![(0, 0)];
        let mut walk = start.clone();
        while walk.cursor() < text.len() {
            walk.right();
            boundaries.push((walk.cursor(), walk.cursor_at(width).0));
        }
        assert!(boundaries.len() > 40, "{boundaries:?}");

        for &(target, row) in &boundaries {
            let mut draft = start.clone();
            (0..row).for_each(|_| draft.down(width));
            assert_eq!(draft.cursor_at(width).0, row, "{row} downs reach row {row}");
            assert!(
                draft.cursor() <= target,
                "{target} is not behind row {row}'s landing"
            );
            while draft.cursor() < target {
                draft.right();
            }

            assert_eq!(draft.cursor(), target);
            draft.insert('X');
            assert_eq!(
                draft.text()[target..].chars().next(),
                Some('X'),
                "at {target}"
            );
        }
    }

    #[test]
    fn control_characters_are_not_inserted() {
        let draft = typed("a\u{7}\tb\r");

        assert_eq!(draft.text(), "ab");
    }
}
