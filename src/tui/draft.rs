//! The comment box's text being edited: a pure value, no widgets.
//!
//! The text holds printable characters, `\t` and `\n` only, so none of it
//! is dropped by [`wrap`] and its rows map 1:1 to byte ranges of it. A tab
//! comes only from a paste ([`Draft::paste`]); it is one cluster, four
//! cells wide, and is drawn as four spaces. (A
//! zero-width cluster, such as a zero-width space, has a range but takes no
//! cell, so the cursor seems to stay put as it steps over one.) The cursor
//! is a byte offset that always sits on a grapheme cluster boundary, so it
//! can never land inside a cluster that [`wrap`] refuses to split.

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
        let rows = self.rows(width);
        let row = self.cursor_row(&rows);
        let rows = rows.len();
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
        self.snap();
    }

    /// Insert pasted `text` at the cursor in one step and move past it.
    /// Every line break (`\r\n`, `\r` or `\n`) becomes `\n` and tabs are
    /// kept; other control characters are dropped. If the pasted text joins
    /// a cluster either side of it, the cursor is left on a boundary, as for
    /// [`Draft::insert`]. A paste with nothing left to insert changes
    /// nothing.
    pub fn paste(&mut self, text: &str) {
        let mut kept = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    chars.next_if_eq(&'\n');
                    kept.push('\n');
                }
                '\n' | '\t' => kept.push(c),
                c if c.is_control() => {}
                c => kept.push(c),
            }
        }
        let text = kept;
        if text.is_empty() {
            return;
        }
        self.goal = None;
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.snap();
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

    /// Remove the cluster before the cursor; a no-op at the start. If the
    /// clusters either side then join (two halves of a flag, say), the
    /// cursor moves to the end of the joined one.
    pub fn backspace(&mut self) {
        self.goal = None;
        if let Some(before) = self.before() {
            self.text.replace_range(before.clone(), "");
            self.cursor = before.start;
            self.snap();
        }
    }

    /// Remove the cluster after the cursor; a no-op at the end. A join is
    /// handled as for [`Draft::backspace`].
    pub fn delete(&mut self) {
        self.goal = None;
        if let Some(after) = self.after() {
            self.text.replace_range(after, "");
            self.snap();
        }
    }

    /// Move a cursor that an edit left inside a cluster to that cluster's
    /// end, so it is on a boundary again.
    fn snap(&mut self) {
        let cursor = self.cursor;
        if let Some(joined) = self
            .clusters()
            .into_iter()
            .find(|cluster| cluster.start < cursor && cursor < cluster.end)
        {
            self.cursor = joined.end;
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
        let row = self.cursor_row(&rows);
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
        // Cells from the row's start to the end of `cluster`.
        let mut used = 0;
        for cluster in self.clusters() {
            if cluster.start < row.start {
                continue;
            }
            used += wrap::width(&self.text[cluster.clone()]);
            let past = cluster.end > row.end || (soft && cluster.end == row.end);
            if past || used > goal {
                break;
            }
            landing = cluster.end;
        }
        self.cursor = landing;
        self.goal = Some(goal);
    }

    /// Which of `rows` (from [`Draft::rows`]) the cursor shows on: the last
    /// one starting at or before it, so a soft break shows on the next row.
    fn cursor_row(&self, rows: &[Range<usize>]) -> usize {
        rows.partition_point(|row| row.start <= self.cursor)
            .saturating_sub(1)
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

    /// `text` typed, with every `\n` from [`Draft::newline`] and every tab
    /// pasted (it cannot be typed).
    fn lines(text: &str) -> Draft {
        let mut draft = Draft::default();
        for c in text.chars() {
            match c {
                '\n' => draft.newline(),
                '\t' => draft.paste("\t"),
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
        let text = "First para, long enough to wrap.\n\nSecond 漢字 e\u{301} line.\n\tTabbed\tline.\nThird.";
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
    fn removing_what_kept_two_clusters_apart_leaves_the_cursor_on_a_boundary() {
        // Two regional indicators pair into one flag once nothing is between them.
        let (us, a) = ("\u{1f1fa}", "\u{1f1f8}");
        let mut back = typed(&format!("{us}x{a}"));
        back.left();
        back.backspace();
        let mut del = typed(&format!("{us}x{a}"));
        del.left();
        del.left();
        del.delete();

        for draft in [back, del] {
            assert_eq!(draft.text(), format!("{us}{a}"));
            assert!(
                [0, draft.text().len()].contains(&draft.cursor()),
                "cursor {} inside the flag",
                draft.cursor()
            );
        }
    }

    #[test]
    fn a_tab_is_four_cells_and_moves_and_deletes_as_one_cluster() {
        let mut draft = lines("\tab");
        draft.home();
        draft.right();
        assert_eq!((draft.cursor(), draft.cursor_at(20)), (1, (0, 4)));
        draft.left();
        assert_eq!(draft.cursor(), 0);
        draft.delete();
        assert_eq!(draft.text(), "ab");

        let mut draft = lines("a\t");
        draft.backspace();
        assert_eq!((draft.text(), draft.cursor()), ("a", 1));
    }

    #[test]
    fn up_and_down_keep_the_column_across_a_tab_indented_line() {
        let mut draft = lines("abcdef\n\tx\nabcdef");
        draft.left();

        draft.up(20);
        assert_eq!(draft.cursor_at(20), (1, 5), "end of `\tx`");
        draft.up(20);
        assert_eq!(draft.cursor_at(20), (0, 5));
        draft.down(20);
        draft.home();
        draft.right();
        draft.down(20);
        assert_eq!(draft.cursor_at(20), (2, 4), "under the tab's end");
    }

    #[test]
    fn a_tab_indented_line_wraps_with_the_tab_counted_four() {
        assert_eq!(shown(&lines("\tab cd"), 6), ["\tab ", "cd"]);
    }

    #[test]
    fn a_paste_makes_every_line_break_a_newline_and_keeps_tabs() {
        let mut draft = Draft::default();
        draft.paste("a\r\nb\rc\nd\te");

        assert_eq!(draft.text(), "a\nb\nc\nd\te");
        assert_eq!(draft.cursor(), draft.text().len());
    }

    #[test]
    fn a_paste_drops_other_control_characters() {
        let mut draft = Draft::default();
        draft.paste("a\u{7}b\u{1b}c\u{7f}d\u{85}e\u{2028}");

        assert_eq!(draft.text(), "abcde\u{2028}");
    }

    #[test]
    fn a_paste_goes_in_at_the_cursor_and_the_cursor_ends_after_it() {
        let mut draft = typed("ad");
        draft.left();
        draft.paste("b\nc");

        assert_eq!(draft.text(), "ab\ncd");
        assert_eq!(draft.cursor(), 4);
    }

    #[test]
    fn a_paste_clears_the_sticky_column() {
        let mut draft = lines("aaaaaaaa\nb\naaaaaaaa");
        draft.up(20);
        draft.paste("x");
        draft.up(20);

        assert_eq!(draft.cursor_at(20), (0, 2), "column of `bx`, not 8");
    }

    #[test]
    fn an_empty_paste_changes_nothing() {
        let mut draft = typed("ab");
        draft.left();
        let before = draft.clone();
        draft.paste("");
        draft.paste("\u{7}\u{1b}");

        assert_eq!(draft.text(), before.text());
        assert_eq!(draft.cursor(), before.cursor());
    }

    #[test]
    fn a_paste_joining_the_clusters_around_it_leaves_the_cursor_on_a_boundary() {
        let mut lead = typed("eb");
        lead.left();
        lead.paste("\u{301}x");
        assert_eq!((lead.text(), lead.cursor()), ("e\u{301}xb", 4));

        let (us, a) = ("\u{1f1fa}", "\u{1f1f8}");
        let mut trail = typed(a);
        trail.home();
        trail.paste(us);
        assert_eq!(trail.cursor(), trail.text().len(), "after the joined flag");
    }

    #[test]
    fn control_characters_are_not_inserted() {
        let draft = typed("a\u{7}\tb\r");

        assert_eq!(draft.text(), "ab");
    }
}
