//! Word-wrapping a file line into screen rows. This is the only place file
//! lines are wrapped.
//!
//! - A tab is four spaces; control characters are dropped.
//! - Width is measured exactly as ratatui draws: grapheme clusters from
//!   [`Span::styled_graphemes`], each [`CellWidth::cell_width`] cells wide. A
//!   cluster is never split, and a wide one that does not fit moves to the
//!   next row.
//! - A row breaks after the last whitespace that fits. A run with no
//!   whitespace that is wider than a row is broken at the width.
//! - Whitespace at a break stays on the end of the row it follows, even past
//!   the width (it is clipped when drawn), so continuation rows never start
//!   with the space they broke on and the rows join back to the displayed
//!   line.

use std::mem;

use ratatui::buffer::CellWidth;
use ratatui::style::Style;
use ratatui::text::Span;

/// `line` as displayed, split into rows at most `width` cells wide (not
/// counting trailing whitespace). An empty line is one empty row. Every row
/// holds at least one cluster, so a cluster wider than `width` gets a row to
/// itself and a `width` of 0 still makes progress.
pub fn rows(line: &str, width: usize) -> Vec<String> {
    let text = line.replace('\t', "    ");
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    // Byte offset in `row` just after its last whitespace, and the cells used
    // up to there.
    let mut break_at: Option<(usize, usize)> = None;
    for grapheme in Span::raw(text.as_str()).styled_graphemes(Style::new()) {
        let symbol = grapheme.symbol;
        let cells = usize::from(symbol.cell_width());
        if symbol.chars().all(char::is_whitespace) {
            row.push_str(symbol);
            used += cells;
            break_at = Some((row.len(), used));
            continue;
        }
        if used + cells > width
            && let Some((at, used_at)) = break_at.take()
        {
            let tail = row.split_off(at);
            rows.push(mem::replace(&mut row, tail));
            used -= used_at;
        }
        if used + cells > width && !row.is_empty() {
            rows.push(mem::take(&mut row));
            used = 0;
        }
        row.push_str(symbol);
        used += cells;
    }
    rows.push(row);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_that_fits_is_one_row_and_an_empty_line_is_one_empty_row() {
        assert_eq!(rows("short", 10), ["short"]);
        assert_eq!(rows("exactly10!", 10), ["exactly10!"]);
        assert_eq!(rows("", 10), [""]);
    }

    #[test]
    fn a_break_falls_after_the_last_space_that_fits_and_the_space_hangs() {
        assert_eq!(rows("aaaa bbbb cccc", 9), ["aaaa bbbb ", "cccc"]);
        assert_eq!(rows("aaaa bbbb cccc", 10), ["aaaa bbbb ", "cccc"]);
        assert_eq!(rows("aa bb cc dd", 5), ["aa bb ", "cc dd"]);
    }

    #[test]
    fn a_word_wider_than_a_row_is_hard_broken_at_the_width() {
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(rows("ab abcdefghij", 4), ["ab ", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn indentation_and_runs_of_spaces_are_kept() {
        assert_eq!(rows("    indented text", 12), ["    indented ", "text"]);
        assert_eq!(rows("a   b", 10), ["a   b"]);
    }

    #[test]
    fn a_tab_is_four_spaces_and_wrapping_counts_them() {
        assert_eq!(rows("a\tb", 10), ["a    b"]);
        assert_eq!(rows("\tabcdef", 8), ["    ", "abcdef"]);
    }

    #[test]
    fn wide_clusters_count_two_cells_and_move_whole_to_the_next_row() {
        assert_eq!(rows("漢字漢字", 5), ["漢字", "漢字"]);
        assert_eq!(rows("a漢字", 4), ["a漢", "字"]);
    }

    #[test]
    fn a_grapheme_cluster_is_never_split() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let accented = "e\u{301}";
        assert_eq!(rows(&format!("{family}{family}"), 3), [family, family]);
        assert_eq!(
            rows(&accented.repeat(3), 2),
            [accented.repeat(2), accented.to_string()]
        );
    }

    #[test]
    fn control_characters_are_dropped_as_ratatui_drops_them() {
        assert_eq!(rows("a\u{7}b\rc\u{1b}d", 10), ["abcd"]);
    }

    #[test]
    fn widths_zero_and_one_make_progress_and_a_wide_cluster_gets_its_own_row() {
        assert_eq!(rows("ab c", 0), ["a", "b ", "c"]);
        assert_eq!(rows("ab c", 1), ["a", "b ", "c"]);
        assert_eq!(rows("漢a字", 1), ["漢", "a", "字"]);
        assert_eq!(rows("\u{301}", 0), ["\u{301}"]);
    }

    const CORPUS: &[&str] = &[
        "The quick brown fox jumps over the lazy dog, twice.",
        "    - an indented list item that goes on for a while",
        "\tTabbed\tcolumns\tline",
        "averyveryverylongwordwithnospacesatallinsideit and more",
        "漢字かなカナ混じりの文、そして English words too",
        "emoji 👨\u{200d}👩\u{200d}👧 and e\u{301} accents 🎉🎉🎉",
        "trailing spaces   ",
        "  a  b  c  ",
        "",
    ];

    /// What ratatui would draw for `line` on one unbounded row.
    fn displayed(line: &str) -> String {
        line.replace('\t', "    ")
            .chars()
            .filter(|c| !c.is_control())
            .collect()
    }

    fn cells(text: &str) -> usize {
        Span::raw(text)
            .styled_graphemes(Style::new())
            .map(|g| usize::from(g.symbol.cell_width()))
            .sum()
    }

    #[test]
    fn rows_join_back_to_the_line_and_each_fits_the_width() {
        for line in CORPUS {
            for width in 1..=12 {
                let rows = rows(line, width);

                assert_eq!(rows.concat(), displayed(line), "{line:?} at {width}");
                for row in &rows {
                    let shown = row.trim_end();
                    let one_cluster = Span::raw(shown).styled_graphemes(Style::new()).count() == 1;
                    assert!(
                        cells(shown) <= width || one_cluster,
                        "{row:?} overflows {width} in {line:?}"
                    );
                    assert!(
                        row == &rows[0] || !row.starts_with(' '),
                        "continuation {row:?} starts with a space in {line:?}"
                    );
                }
            }
        }
    }
}
