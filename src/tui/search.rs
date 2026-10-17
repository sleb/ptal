//! Pure text search for the file view: where a query matches in a line, and
//! the same matches marked in a styled line.
//!
//! Matching is smart-case: case-insensitive unless the query has an
//! uppercase character. Case is folded one `char` at a time
//! (`c.to_lowercase().next()`), so every match is a valid byte range of the
//! original line; foldings that change the char count (`ß` and `SS`) do not
//! match.

use std::ops::Range;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// `c` folded for a case-insensitive compare: its first lowercase char.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The non-overlapping byte ranges of `line` where `query` matches, left to
/// right. Empty when `query` is empty or absent from `line`.
pub fn find(line: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let exact = query.chars().any(char::is_uppercase);
    let same = |a: char, b: char| if exact { a == b } else { fold(a) == fold(b) };
    let needle = query.chars();
    let len = needle.clone().count();
    let mut found = Vec::new();
    let mut starts = line.char_indices().peekable();
    while let Some(&(start, _)) = starts.peek() {
        let mut rest = line[start..].char_indices();
        let hit = needle
            .clone()
            .all(|n| rest.next().is_some_and(|(_, h)| same(h, n)));
        if hit {
            let end = start + rest.offset();
            found.push(start..end);
            // Skip the match so matches do not overlap.
            for _ in 0..len {
                starts.next();
            }
        } else {
            starts.next();
        }
    }
    found
}

/// `line` with the text under `ranges` (sorted, non-overlapping byte ranges
/// into its concatenated text, as [`find`] returns) reversed on top of its
/// existing style. The text itself is unchanged.
pub fn emphasize<'a>(line: Line<'a>, ranges: &[Range<usize>]) -> Line<'a> {
    if ranges.is_empty() {
        return line;
    }
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut base = 0;
    let (style, alignment) = (line.style, line.alignment);
    for span in line.spans {
        let text = span.content.as_ref();
        let end = base + text.len();
        let mut cuts = vec![0, text.len()];
        for range in ranges {
            for edge in [range.start, range.end] {
                if (base..=end).contains(&edge) {
                    cuts.push(edge - base);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for cut in cuts.windows(2) {
            let (from, to) = (base + cut[0], base + cut[1]);
            let marked = ranges.iter().any(|r| r.start <= from && to <= r.end);
            let style = if marked {
                span.style
                    .patch(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                span.style
            };
            spans.push(Span::styled(text[cut[0]..cut[1]].to_owned(), style));
        }
        base = end;
    }
    Line {
        spans,
        style,
        alignment,
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn slices<'a>(line: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|r| &line[r.clone()]).collect()
    }

    #[test]
    fn find_returns_every_match_left_to_right() {
        let line = "plan a plan";
        assert_eq!(find(line, "plan"), vec![0..4, 7..11]);
    }

    #[test]
    fn no_match_and_an_empty_query_find_nothing() {
        assert!(find("hello", "zzz").is_empty());
        assert!(find("hello", "").is_empty());
        assert!(find("", "a").is_empty());
    }

    #[test]
    fn a_lowercase_query_ignores_case_and_an_uppercase_one_does_not() {
        let line = "Plan PLAN plan";
        assert_eq!(find(line, "plan").len(), 3);
        assert_eq!(slices(line, &find(line, "Plan")), vec!["Plan"]);
    }

    #[test]
    fn multibyte_text_gives_char_boundary_ranges() {
        for (line, query) in [("café é", "é"), ("日本語 日本", "日本"), ("a😀b😀", "😀")]
        {
            let found = find(line, query);
            assert!(!found.is_empty());
            assert!(slices(line, &found).iter().all(|s| *s == query));
        }
        assert_eq!(slices("CAFÉ", &find("CAFÉ", "é")), vec!["É"]);
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find("aaa", "aa"), vec![0..2]);
        assert_eq!(find("aaaa", "aa"), vec![0..2, 2..4]);
    }

    fn marked(line: &Line) -> Vec<(String, bool)> {
        line.spans
            .iter()
            .map(|s| {
                (
                    s.content.to_string(),
                    s.style.add_modifier.contains(Modifier::REVERSED),
                )
            })
            .collect()
    }

    #[test]
    fn emphasize_reverses_exactly_the_matches_and_keeps_the_text() {
        let line = Line::raw("a plan b");
        let out = emphasize(line, &[2..6]);
        assert_eq!(
            marked(&out),
            vec![
                ("a ".into(), false),
                ("plan".into(), true),
                (" b".into(), false)
            ]
        );
    }

    #[test]
    fn emphasize_splits_across_span_boundaries_and_keeps_styles() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::styled("ab", red), Span::raw("cd")]);
        let out = emphasize(line, &[1..3]);
        assert_eq!(
            marked(&out),
            vec![
                ("a".into(), false),
                ("b".into(), true),
                ("c".into(), true),
                ("d".into(), false)
            ]
        );
        assert_eq!(out.spans[1].style.fg, Some(Color::Red));
        assert_eq!(out.spans[2].style.fg, None);
    }

    #[test]
    fn emphasize_with_no_ranges_leaves_the_line_unchanged() {
        let line = Line::raw("abc");
        assert_eq!(emphasize(line.clone(), &[]), line);
    }

    #[test]
    fn emphasize_keeps_the_lines_own_style() {
        let line = Line::raw("abc").style(Style::new().fg(Color::Blue));
        let out = emphasize(line, &[0..1]);
        assert_eq!(out.style.fg, Some(Color::Blue));
    }
}
