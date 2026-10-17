//! Word-wrapping a line of text into screen rows. This is the only place the
//! TUI wraps text: file lines, table cells, thread panel comments and the
//! comment being typed.
//!
//! - A tab is four spaces; control characters are dropped.
//! - Width is measured exactly as ratatui draws: grapheme clusters of the
//!   whole line (as [`Span::styled_graphemes`] yields them), each
//!   [`CellWidth::cell_width`] cells wide. A cluster is never split, not even
//!   where it straddles two spans, and a wide one that does not fit moves to
//!   the next row.
//! - A row breaks after the last whitespace that fits. A run with no
//!   whitespace that is wider than a row is broken at the width.
//! - Whitespace at a break stays on the end of the row it follows, even past
//!   the width (it is clipped when drawn), so continuation rows never start
//!   with the space they broke on and the rows join back to the displayed
//!   line.
//! - Every cluster keeps its style (a straddling one takes the style of its
//!   first byte): wrapping never restyles text, and a styled line wraps into
//!   exactly the rows its plain text does.

use std::mem;
use std::ops::Range;

use ratatui::buffer::CellWidth;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// `line` as displayed, split into rows at most `width` cells wide (not
/// counting trailing whitespace). An empty line is one empty row. Every row
/// holds at least one cluster, so a cluster wider than `width` gets a row to
/// itself and a `width` of 0 still makes progress.
///
/// Each cluster keeps the style it has in `line` (its span's style patched
/// over the line's), so a span cut by a break goes on in its style on the
/// next row. A tab takes the style of the span it is in. The rows' text is
/// [`plain_rows`] of the line's text.
pub fn rows(line: &Line, width: usize) -> Vec<Line<'static>> {
    let mut text = String::new();
    // Where each span's expanded text ends in `text`, and its style.
    let mut ends: Vec<(usize, Style)> = Vec::with_capacity(line.spans.len());
    for span in &line.spans {
        text.push_str(&expand_tabs(&span.content));
        ends.push((text.len(), line.style.patch(span.style)));
    }
    let style_at = |at: usize| {
        let span = ends.partition_point(|&(end, _)| end <= at);
        ends.get(span).map_or(Style::new(), |&(_, style)| style)
    };
    split(&text, width)
        .iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for &symbol in row {
                let style = style_at(offset_in(&text, symbol));
                match spans.last_mut() {
                    Some(span) if span.style == style => span.content.to_mut().push_str(symbol),
                    _ => spans.push(Span::styled(symbol.to_owned(), style)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// [`rows`] of unstyled `text`, as strings.
pub fn plain_rows(text: &str, width: usize) -> Vec<String> {
    split(&expand_tabs(text), width)
        .iter()
        .map(|row| row.concat())
        .collect()
}

/// How many rows [`rows`] (or [`plain_rows`]) makes of `text`, without
/// building them.
pub fn row_count(text: &str, width: usize) -> usize {
    split(&expand_tabs(text), width).len()
}

/// The byte range in `text` of each row of [`plain_rows`], for `text` with
/// no tabs or control characters (which [`plain_rows`] would expand or
/// drop). An empty `text` is one empty row, `0..0`.
pub(super) fn row_ranges(text: &str, width: usize) -> Vec<Range<usize>> {
    split(text, width)
        .iter()
        .map(|row| match (row.first(), row.last()) {
            (Some(first), Some(last)) => offset_in(text, first)..offset_in(text, last) + last.len(),
            _ => 0..0,
        })
        .collect()
}

/// The clusters of `text` (tabs already expanded) on each row: the one
/// wrapping algorithm behind [`rows`], [`plain_rows`], [`row_count`] and
/// [`row_ranges`].
fn split(text: &str, width: usize) -> Vec<Vec<&str>> {
    let mut rows = Vec::new();
    let mut row: Vec<&str> = Vec::new();
    let mut used = 0;
    // Index in `row` just after its last whitespace, and the cells used up
    // to there.
    let mut break_at: Option<(usize, usize)> = None;
    each_cluster(text, |cluster| {
        let symbol = &text[cluster];
        let cells = usize::from(symbol.cell_width());
        if symbol.chars().all(char::is_whitespace) {
            row.push(symbol);
            used += cells;
            break_at = Some((row.len(), used));
            return;
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
        row.push(symbol);
        used += cells;
    });
    rows.push(row);
    rows
}

/// The byte range in `text` of each grapheme cluster ratatui draws, in
/// order, as [`each_cluster`] yields them. Control characters are not
/// drawn, so they are in no range.
pub(super) fn clusters(text: &str) -> Vec<Range<usize>> {
    let mut clusters = Vec::new();
    each_cluster(text, |cluster| clusters.push(cluster));
    clusters
}

/// Call `f` with the byte range of each grapheme cluster of `text` that
/// ratatui draws, in order, without collecting them: the one segmentation
/// behind [`split`], [`clusters`] and so the draft's cursor.
fn each_cluster(text: &str, mut f: impl FnMut(Range<usize>)) {
    for grapheme in Span::raw(text).styled_graphemes(Style::new()) {
        let at = offset_in(text, grapheme.symbol);
        f(at..at + grapheme.symbol.len());
    }
}

/// Byte offset of `part`, a slice of `text`, within `text`.
fn offset_in(text: &str, part: &str) -> usize {
    part.as_ptr() as usize - text.as_ptr() as usize
}

/// Cells `text` takes on one unbounded row, by the same measure as [`rows`]:
/// a tab is four cells, control characters take none.
pub(super) fn width(text: &str) -> usize {
    Span::raw(expand_tabs(text).as_str())
        .styled_graphemes(Style::new())
        .map(|g| usize::from(g.symbol.cell_width()))
        .sum()
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    /// The rows of an unstyled `line`, as text.
    fn plain(line: &str, width: usize) -> Vec<String> {
        rows(&Line::raw(line), width)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Each displayed cluster of `rows` with its style, row by row.
    fn clusters(rows: &[Line]) -> Vec<Vec<(String, Style)>> {
        rows.iter()
            .map(|row| {
                row.styled_graphemes(Style::new())
                    .map(|g| (g.symbol.to_string(), g.style))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_span_split_by_a_wrap_keeps_its_style_on_the_next_row() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::raw("plain "),
            Span::styled("bold words here", bold),
        ]);

        let rows = rows(&line, 8);

        let text: Vec<String> = rows.iter().map(ToString::to_string).collect();
        assert_eq!(text, ["plain ", "bold ", "words ", "here"]);
        for (row, clusters) in clusters(&rows).iter().enumerate() {
            for (symbol, style) in clusters {
                let want = if row == 0 { Style::new() } else { bold };
                assert_eq!(*style, want, "{symbol:?} on row {row}");
            }
        }
    }

    #[test]
    fn a_tab_in_a_styled_span_expands_to_four_spaces_in_that_style() {
        let code = Style::new().fg(ratatui::style::Color::Cyan);
        let line = Line::from(vec![Span::raw("a"), Span::styled("\tb", code)]);

        let rows = rows(&line, 20);

        assert_eq!(
            clusters(&rows),
            [[
                ("a".to_string(), Style::new()),
                (" ".to_string(), code),
                (" ".to_string(), code),
                (" ".to_string(), code),
                (" ".to_string(), code),
                ("b".to_string(), code),
            ]]
        );
    }

    #[test]
    fn a_cluster_straddling_a_span_boundary_is_not_split() {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let italic = Style::new().add_modifier(Modifier::ITALIC);
        let line = Line::from(vec![
            Span::styled("> ", dim),
            Span::styled("\u{301}abcd", italic),
        ]);

        let rows = rows(&line, 3);

        let text: Vec<String> = rows.iter().map(ToString::to_string).collect();
        assert_eq!(text, plain("> \u{301}abcd", 3));
        assert_eq!(row_count("> \u{301}abcd", 3), rows.len());
        assert_eq!(
            clusters(&rows)[0][1],
            (" \u{301}".to_string(), dim),
            "styled as its first byte"
        );
    }

    #[test]
    fn a_line_that_fits_is_one_row_and_an_empty_line_is_one_empty_row() {
        assert_eq!(plain("short", 10), ["short"]);
        assert_eq!(plain("exactly10!", 10), ["exactly10!"]);
        assert_eq!(plain("", 10), [""]);
    }

    #[test]
    fn a_break_falls_after_the_last_space_that_fits_and_the_space_hangs() {
        assert_eq!(plain("aaaa bbbb cccc", 9), ["aaaa bbbb ", "cccc"]);
        assert_eq!(plain("aaaa bbbb cccc", 10), ["aaaa bbbb ", "cccc"]);
        assert_eq!(plain("aa bb cc dd", 5), ["aa bb ", "cc dd"]);
    }

    #[test]
    fn a_word_wider_than_a_row_is_hard_broken_at_the_width() {
        assert_eq!(plain("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(plain("ab abcdefghij", 4), ["ab ", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn indentation_and_runs_of_spaces_are_kept() {
        assert_eq!(plain("    indented text", 12), ["    indented ", "text"]);
        assert_eq!(plain("a   b", 10), ["a   b"]);
    }

    #[test]
    fn a_tab_is_four_spaces_and_wrapping_counts_them() {
        assert_eq!(plain("a\tb", 10), ["a    b"]);
        assert_eq!(plain("\tabcdef", 8), ["    ", "abcdef"]);
    }

    #[test]
    fn wide_clusters_count_two_cells_and_move_whole_to_the_next_row() {
        assert_eq!(plain("漢字漢字", 5), ["漢字", "漢字"]);
        assert_eq!(plain("a漢字", 4), ["a漢", "字"]);
    }

    #[test]
    fn a_grapheme_cluster_is_never_split() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let accented = "e\u{301}";
        assert_eq!(plain(&format!("{family}{family}"), 3), [family, family]);
        assert_eq!(
            plain(&accented.repeat(3), 2),
            [accented.repeat(2), accented.to_string()]
        );
    }

    #[test]
    fn control_characters_are_dropped_as_ratatui_drops_them() {
        assert_eq!(plain("a\u{7}b\rc\u{1b}d", 10), ["abcd"]);
    }

    #[test]
    fn width_measures_as_drawn_with_tabs_as_four_and_controls_dropped() {
        assert_eq!(width("a\t漢"), 7);
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(width("a\u{7}b"), 2);
    }

    #[test]
    fn widths_zero_and_one_make_progress_and_a_wide_cluster_gets_its_own_row() {
        assert_eq!(plain("ab c", 0), ["a", "b ", "c"]);
        assert_eq!(plain("ab c", 1), ["a", "b ", "c"]);
        assert_eq!(plain("漢a字", 1), ["漢", "a", "字"]);
        assert_eq!(plain("\u{301}", 0), ["\u{301}"]);
    }

    const CORPUS: &[&str] = &[
        "The quick brown fox jumps over the lazy dog, twice.",
        "    - an indented list item that goes on for a while",
        "\tTabbed\tcolumns\tline",
        "averyveryverylongwordwithnospacesatallinsideit and more",
        "漢字かなカナ混じりの文、そして English words too",
        "emoji 👨\u{200d}👩\u{200d}👧 and e\u{301} accents 🎉🎉🎉",
        "> \u{301}abcd",
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

    #[test]
    fn styling_a_line_never_changes_how_it_wraps() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        for line in CORPUS {
            for (cut, _) in line.char_indices() {
                let (head, tail) = line.split_at(cut);
                let styled = Line::from(vec![Span::raw(head), Span::styled(tail, bold)]);
                for width in 1..=12 {
                    let text: Vec<String> = rows(&styled, width)
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    assert_eq!(
                        text,
                        plain_rows(line, width),
                        "{line:?} cut at {cut}, width {width}"
                    );
                    assert_eq!(row_count(line, width), text.len());
                }
            }
        }
    }

    #[test]
    fn row_ranges_cut_the_text_into_exactly_the_plain_rows() {
        for line in CORPUS
            .iter()
            .filter(|line| !line.contains(char::is_control))
        {
            for width in 1..=12 {
                let cut: Vec<&str> = row_ranges(line, width)
                    .into_iter()
                    .map(|range| &line[range])
                    .collect();

                assert_eq!(cut, plain_rows(line, width), "{line:?} at {width}");
            }
        }
    }

    #[test]
    fn rows_join_back_to_the_line_and_each_fits_the_width() {
        for line in CORPUS {
            for width in 1..=12 {
                let rows = plain(line, width);

                assert_eq!(rows.concat(), displayed(line), "{line:?} at {width}");
                for row in &rows {
                    let shown = row.trim_end();
                    let one_cluster = Span::raw(shown).styled_graphemes(Style::new()).count() == 1;
                    assert!(
                        super::width(shown) <= width || one_cluster,
                        "{row:?} overflows {width} in {line:?}"
                    );
                    let first = Span::raw(row.as_str())
                        .styled_graphemes(Style::new())
                        .next()
                        .map(|g| g.symbol.to_string());
                    assert!(
                        row == &rows[0]
                            || !first.is_some_and(|g| g.chars().all(char::is_whitespace)),
                        "continuation {row:?} starts with whitespace in {line:?}"
                    );
                }
            }
        }
    }
}
