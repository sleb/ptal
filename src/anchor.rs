//! Pure mapping of line ranges between two versions of a file.
//!
//! A thread is anchored to a range in the revision it was created on.
//! [`map_range`] diffs that revision against the current one (Myers, by whole
//! lines) and says where the range is now:
//!
//! - every anchored line is unchanged and still contiguous → [`State::Current`]
//!   at the new line numbers;
//! - otherwise → [`State::Outdated`] at a best-guess range, always within the
//!   new file.
//!
//! [`Diff`] computes the diff once to map many ranges, and lists what changed
//! with [`Diff::changes`]; [`clamp`] keeps a range inside a text without
//! diffing.
//!
//! No I/O, no clock, deterministic. Callers split the text into lines; the
//! core uses [`str::lines`] everywhere (see [`crate::review::View::lines`]).

use similar::{Algorithm, DiffOp, capture_diff_slices};

use crate::model::State;

/// A 1-based, inclusive line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

/// Where a range lands in the new text, and whether it still reads the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapped {
    /// In bounds of the new text: `1 <= start <= end <= max(new.len(), 1)`.
    pub range: LineRange,
    pub state: State,
}

/// Map `range` of `old` to the matching lines of `new`.
///
/// The result is [`State::Current`] only if every line of `range` is
/// unchanged and the lines are still contiguous, so the mapped lines of `new`
/// are byte-identical to `range` of `old`. A line inserted inside the range
/// makes it [`State::Outdated`].
///
/// An outdated range is a best guess: each end follows the diff, and an end
/// on an edited or deleted line goes to where that change starts in `new`.
/// It is [`clamp`]ed to the new text; an empty `new` gives `1-1`. A `range`
/// outside `old` is outdated, never a panic.
///
/// To map several ranges between the same two texts, build one [`Diff`].
///
/// # Examples
/// ```
/// # use ptal::anchor::{map_range, LineRange};
/// # use ptal::model::State;
/// let old = ["a", "b", "c"];
/// let new = ["new", "a", "b", "c"];
///
/// let mapped = map_range(&old, &new, LineRange { start: 2, end: 3 });
/// assert_eq!(mapped.range, LineRange { start: 3, end: 4 });
/// assert_eq!(mapped.state, State::Current);
/// ```
pub fn map_range(old: &[&str], new: &[&str], range: LineRange) -> Mapped {
    Diff::new(old, new).map(range)
}

/// One hunk of the new text that differs from the old: see [`Diff::changes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// In bounds of the new text; one line for [`ChangeKind::Removed`].
    pub range: LineRange,
    pub kind: ChangeKind,
}

/// What a [`Change`] marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The lines were added or edited.
    Changed,
    /// Text was removed just before this line, or after it if it is the last.
    Removed,
}

/// `range` moved inside a text of `len` lines: `1 <= start <= end <=
/// max(len, 1)`. A range already inside is unchanged.
pub fn clamp(range: LineRange, len: usize) -> LineRange {
    let max = u32::try_from(len.max(1)).unwrap_or(u32::MAX);
    let start = range.start.clamp(1, max);
    let end = range.end.clamp(1, max).max(start);
    LineRange { start, end }
}

/// The line diff of two texts, computed once and used to map any number of
/// ranges with [`Diff::map`], as [`map_range`] does for one.
#[derive(Debug, Clone)]
pub struct Diff {
    ops: Vec<DiffOp>,
    new_len: usize,
}

impl Diff {
    /// Diff `old` against `new` by whole lines (Myers).
    pub fn new(old: &[&str], new: &[&str]) -> Self {
        Self {
            ops: capture_diff_slices(Algorithm::Myers, old, new),
            new_len: new.len(),
        }
    }

    /// Map `range` of the old text; see [`map_range`].
    pub fn map(&self, range: LineRange) -> Mapped {
        let first = range.start.saturating_sub(1) as usize;
        let last = range.end.saturating_sub(1) as usize;
        let (new_first, start_kept) = self.position(first);
        let (new_last, end_kept) = self.position(last);
        let current = range.start >= 1
            && range.start <= range.end
            && start_kept
            && end_kept
            && new_last - new_first == last - first
            && self.unchanged(first, last);

        let guess = LineRange {
            start: line_number(new_first),
            end: line_number(new_last),
        };
        Mapped {
            range: clamp(guess, self.new_len),
            state: if current {
                State::Current
            } else {
                State::Outdated
            },
        }
    }

    /// What changed in the new text, one [`Change`] per hunk (a maximal run
    /// of adjacent changed lines), in order.
    ///
    /// A hunk with new lines is [`ChangeKind::Changed`] over them. A pure
    /// deletion is [`ChangeKind::Removed`] at the line that now follows the
    /// removed text, or at the last line if it was removed at the end; with
    /// no lines left there is nothing to mark, and removals before and after
    /// a sole remaining line are one mark on it. The changes are sorted,
    /// disjoint and in bounds of the new text, and identical texts have none.
    ///
    /// # Examples
    /// ```
    /// # use ptal::anchor::{Change, ChangeKind, Diff, LineRange};
    /// let old = ["a", "b", "c", "d"];
    /// let new = ["a", "B", "d"];
    ///
    /// assert_eq!(
    ///     Diff::new(&old, &new).changes(),
    ///     [Change { range: LineRange { start: 2, end: 2 }, kind: ChangeKind::Changed }],
    /// );
    /// ```
    pub fn changes(&self) -> Vec<Change> {
        let mut changes = Vec::new();
        let mut ops = self.ops.iter().peekable();
        while let Some(op) = ops.next() {
            if matches!(op, DiffOp::Equal { .. }) {
                continue;
            }
            let mut new = op.new_range();
            while let Some(next) = ops.next_if(|op| !matches!(op, DiffOp::Equal { .. })) {
                new.end = next.new_range().end;
            }
            let change = if new.is_empty() {
                if self.new_len == 0 {
                    continue;
                }
                let line = line_number(new.start.min(self.new_len - 1));
                Change {
                    range: LineRange {
                        start: line,
                        end: line,
                    },
                    kind: ChangeKind::Removed,
                }
            } else {
                Change {
                    range: LineRange {
                        start: line_number(new.start),
                        end: line_number(new.end - 1),
                    },
                    kind: ChangeKind::Changed,
                }
            };
            // Removals before and after the only line left both mark it.
            if changes.last() != Some(&change) {
                changes.push(change);
            }
        }
        changes
    }

    /// Whether every op touching 0-based old lines `first..=last` is equal.
    fn unchanged(&self, first: usize, last: usize) -> bool {
        self.ops
            .iter()
            .filter(|op| {
                let old = op.old_range();
                old.start <= last && old.end > first
            })
            .all(|op| matches!(op, DiffOp::Equal { .. }))
    }

    /// Where 0-based line `i` of the old text goes in the new text, and
    /// whether it is unchanged there. An edited or deleted line goes to the
    /// start of its change in the new text; a line past the end goes past the
    /// end.
    fn position(&self, i: usize) -> (usize, bool) {
        for op in &self.ops {
            if !op.old_range().contains(&i) {
                continue;
            }
            return match *op {
                DiffOp::Equal {
                    old_index,
                    new_index,
                    ..
                } => (new_index + (i - old_index), true),
                _ => (op.new_range().start, false),
            };
        }
        (self.new_len, false)
    }
}

/// The 1-based line number of 0-based index `i`, saturating at
/// `u32::MAX`: the one index-to-line conversion, the TUI's too.
pub(crate) fn line_number(i: usize) -> u32 {
    u32::try_from(i + 1).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_number_is_the_index_plus_one_saturating() {
        assert_eq!(line_number(0), 1);
        assert_eq!(line_number(41), 42);
        assert_eq!(line_number(u32::MAX as usize), u32::MAX);
    }

    fn range(start: u32, end: u32) -> LineRange {
        LineRange { start, end }
    }

    fn map(old: &[&str], new: &[&str], start: u32, end: u32) -> (LineRange, State) {
        let mapped = map_range(old, new, range(start, end));
        (mapped.range, mapped.state)
    }

    const OLD: [&str; 4] = ["one", "two", "three", "four"];

    #[test]
    fn identical_texts_keep_the_range_current() {
        assert_eq!(map(&OLD, &OLD, 2, 3), (range(2, 3), State::Current));
    }

    #[test]
    fn lines_inserted_above_shift_the_range_and_keep_it_current() {
        let new = ["zero", "half", "one", "two", "three", "four"];

        assert_eq!(map(&OLD, &new, 2, 3), (range(4, 5), State::Current));
    }

    #[test]
    fn lines_deleted_above_shift_the_range_and_keep_it_current() {
        let new = ["two", "three", "four"];

        assert_eq!(map(&OLD, &new, 2, 3), (range(1, 2), State::Current));
    }

    #[test]
    fn lines_appended_below_leave_the_range_unchanged() {
        let new = ["one", "two", "three", "four", "five"];

        assert_eq!(map(&OLD, &new, 2, 3), (range(2, 3), State::Current));
    }

    #[test]
    fn an_edited_anchored_line_is_outdated_at_the_edit() {
        let new = ["zero", "one", "TWO", "three", "four"];

        assert_eq!(map(&OLD, &new, 2, 3), (range(3, 4), State::Outdated));
    }

    #[test]
    fn a_deleted_anchored_range_is_outdated_and_in_bounds() {
        let new = ["one", "four"];
        assert_eq!(map(&OLD, &new, 2, 3), (range(2, 2), State::Outdated));

        let truncated = ["one", "two"];
        assert_eq!(map(&OLD, &truncated, 3, 4), (range(2, 2), State::Outdated));
    }

    #[test]
    fn a_line_inserted_inside_the_range_is_outdated() {
        let new = ["one", "two", "inserted", "three", "four"];

        assert_eq!(map(&OLD, &new, 2, 3), (range(2, 4), State::Outdated));
    }

    #[test]
    fn an_empty_new_file_is_outdated_at_line_one() {
        assert_eq!(map(&OLD, &[], 2, 3), (range(1, 1), State::Outdated));
    }

    #[test]
    fn a_range_on_the_last_line_maps_correctly() {
        let new = ["zero", "one", "two", "three", "four"];

        assert_eq!(map(&OLD, &new, 4, 4), (range(5, 5), State::Current));
        assert_eq!(map(&OLD, &OLD, 1, 4), (range(1, 4), State::Current));
    }

    #[test]
    fn a_range_outside_the_old_text_is_outdated_not_a_panic() {
        assert_eq!(map(&OLD, &OLD, 4, 9), (range(4, 4), State::Outdated));
        assert_eq!(map(&OLD, &OLD, 0, 1), (range(1, 1), State::Outdated));
        assert_eq!(map(&OLD, &OLD, 3, 2).1, State::Outdated);
    }

    #[test]
    fn one_diff_maps_many_ranges_like_map_range() {
        let new = ["zero", "one", "TWO", "three", "four"];
        let diff = Diff::new(&OLD, &new);

        for r in ranges(OLD.len()) {
            assert_eq!(diff.map(r), map_range(&OLD, &new, r), "{r:?}");
        }
    }

    #[test]
    fn clamp_keeps_a_range_inside_a_text_of_len_lines() {
        assert_eq!(clamp(range(2, 3), 4), range(2, 3));
        assert_eq!(clamp(range(3, 9), 4), range(3, 4));
        assert_eq!(clamp(range(7, 9), 4), range(4, 4));
        assert_eq!(clamp(range(0, 0), 4), range(1, 1));
        assert_eq!(clamp(range(3, 2), 4), range(3, 3));
        assert_eq!(clamp(range(2, 3), 0), range(1, 1));
    }

    fn changes(old: &[&str], new: &[&str]) -> Vec<Change> {
        Diff::new(old, new).changes()
    }

    fn changed(start: u32, end: u32) -> Change {
        Change {
            range: range(start, end),
            kind: ChangeKind::Changed,
        }
    }

    fn removed(line: u32) -> Change {
        Change {
            range: range(line, line),
            kind: ChangeKind::Removed,
        }
    }

    #[test]
    fn identical_texts_have_no_changes() {
        assert_eq!(changes(&OLD, &OLD), vec![]);
    }

    #[test]
    fn a_line_inserted_in_the_middle_is_changed() {
        let new = ["one", "two", "new", "three", "four"];

        assert_eq!(changes(&OLD, &new), vec![changed(3, 3)]);
    }

    #[test]
    fn an_edited_line_is_changed() {
        let new = ["one", "TWO", "three", "four"];

        assert_eq!(changes(&OLD, &new), vec![changed(2, 2)]);
    }

    #[test]
    fn two_lines_replaced_by_three_are_one_change() {
        let new = ["one", "x", "y", "z", "four"];

        assert_eq!(changes(&OLD, &new), vec![changed(2, 4)]);
    }

    #[test]
    fn a_deleted_middle_line_is_removed_at_the_following_line() {
        let new = ["one", "three", "four"];

        assert_eq!(changes(&OLD, &new), vec![removed(2)]);
    }

    #[test]
    fn a_deleted_last_line_is_removed_at_the_new_last_line() {
        let new = ["one", "two", "three"];

        assert_eq!(changes(&OLD, &new), vec![removed(3)]);
    }

    #[test]
    fn deleting_everything_has_no_line_to_mark() {
        assert_eq!(changes(&OLD, &[]), vec![]);
    }

    #[test]
    fn removals_around_the_only_line_left_are_one_mark() {
        assert_eq!(changes(&["a", "b", "a"], &["b"]), vec![removed(1)]);
    }

    #[test]
    fn separate_edits_are_separate_changes_in_order() {
        let new = ["ONE", "two", "three", "FOUR"];

        assert_eq!(changes(&OLD, &new), vec![changed(1, 1), changed(4, 4)]);
    }

    #[test]
    fn an_edit_next_to_a_deletion_is_one_change() {
        let new = ["one", "TWO", "four"];

        assert_eq!(changes(&OLD, &new), vec![changed(2, 2)]);
    }

    // --- properties, exhaustive over a small corpus ---

    /// Every text of 0-4 lines over the alphabet {a, b, c}.
    fn corpus() -> Vec<Vec<&'static str>> {
        let mut texts = vec![vec![]];
        let mut frontier = vec![vec![]];
        for _ in 0..4 {
            let mut next = Vec::new();
            for text in &frontier {
                for line in ["a", "b", "c"] {
                    let mut longer: Vec<&str> = text.clone();
                    longer.push(line);
                    next.push(longer);
                }
            }
            texts.extend(next.iter().cloned());
            frontier = next;
        }
        texts
    }

    /// Every valid range of a text of `len` lines.
    fn ranges(len: usize) -> impl Iterator<Item = LineRange> {
        let len = len as u32;
        (1..=len).flat_map(move |s| (s..=len).map(move |e| range(s, e)))
    }

    #[test]
    fn corpus_covers_every_short_text() {
        assert_eq!(corpus().len(), 1 + 3 + 9 + 27 + 81);
    }

    #[test]
    fn property_identity_maps_every_range_unchanged() {
        for text in corpus() {
            for r in ranges(text.len()) {
                let mapped = map_range(&text, &text, r);
                assert_eq!(
                    mapped,
                    Mapped {
                        range: r,
                        state: State::Current
                    }
                );
            }
        }
    }

    #[test]
    fn property_mapped_ranges_are_in_bounds_and_current_is_byte_identical() {
        let corpus = corpus();
        for old in &corpus {
            for new in &corpus {
                for r in ranges(old.len()) {
                    let Mapped { range: m, state } = map_range(old, new, r);
                    let max = new.len().max(1) as u32;
                    assert!(
                        1 <= m.start && m.start <= m.end && m.end <= max,
                        "{old:?} -> {new:?} {r:?}: {m:?}"
                    );
                    if state == State::Current {
                        let was = &old[r.start as usize - 1..r.end as usize];
                        let now = &new[m.start as usize - 1..m.end as usize];
                        assert_eq!(was, now, "{old:?} -> {new:?} {r:?}: {m:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn property_changes_are_sorted_disjoint_and_in_bounds() {
        let corpus = corpus();
        for old in &corpus {
            for new in &corpus {
                let found = changes(old, new);
                let mut after = 0;
                for c in &found {
                    let r = c.range;
                    assert!(
                        after < r.start && r.start <= r.end && r.end as usize <= new.len(),
                        "{old:?} -> {new:?}: {found:?}"
                    );
                    if c.kind == ChangeKind::Removed {
                        assert_eq!(r.start, r.end, "{old:?} -> {new:?}: {found:?}");
                    }
                    after = r.end;
                }
                // Identical texts, and only they, have no changes, unless
                // nothing is left to mark.
                assert_eq!(
                    found.is_empty(),
                    old == new || new.is_empty(),
                    "{old:?} -> {new:?}: {found:?}"
                );
                // The lines not marked changed were kept: in order, they
                // are a subsequence of the old text.
                let kept = new.iter().enumerate().filter(|(i, _)| {
                    !found.iter().any(|c| {
                        c.kind == ChangeKind::Changed
                            && (c.range.start as usize..=c.range.end as usize).contains(&(i + 1))
                    })
                });
                let mut rest = old.iter();
                for (_, line) in kept {
                    assert!(rest.any(|o| o == line), "{old:?} -> {new:?}: {found:?}");
                }
            }
        }
    }
}
