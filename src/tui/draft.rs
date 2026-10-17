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

    /// Insert `c` at the cursor and move past it. A control character is
    /// ignored. If `c` joins the cluster after it (a zero-width joiner, say),
    /// the cursor moves to the end of the joined cluster.
    pub fn insert(&mut self, c: char) {
        if c.is_control() {
            return;
        }
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
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    /// Move back over one cluster; a no-op at the start.
    pub fn left(&mut self) {
        if let Some(before) = self.before() {
            self.cursor = before.start;
        }
    }

    /// Move forward over one cluster; a no-op at the end.
    pub fn right(&mut self) {
        if let Some(after) = self.after() {
            self.cursor = after.end;
        }
    }

    /// Remove the cluster before the cursor; a no-op at the start.
    pub fn backspace(&mut self) {
        if let Some(before) = self.before() {
            self.text.replace_range(before.clone(), "");
            self.cursor = before.start;
        }
    }

    /// Remove the cluster after the cursor; a no-op at the end.
    pub fn delete(&mut self) {
        if let Some(after) = self.after() {
            self.text.replace_range(after, "");
        }
    }

    /// Move to the start of the text.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Move to the end of the text.
    pub fn end(&mut self) {
        self.cursor = self.text.len();
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

    #[test]
    fn control_characters_are_not_inserted() {
        let draft = typed("a\u{7}\tb\r");

        assert_eq!(draft.text(), "ab");
    }
}
