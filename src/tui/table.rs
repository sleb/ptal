//! Aligning markdown tables in place. This is the only place tables are
//! parsed.
//!
//! Each source line of a table becomes one aligned row, so line numbers,
//! the cursor and anchors keep mapping one-to-one to file lines. Cells are
//! trimmed, padded to their column (measured as [`wrap::rows`] measures) and
//! joined by ` │ `; each row starts and ends with one space. The delimiter
//! row shows as `─` runs joined by `┼`, as wide as the other rows.
//!
//! Too wide for the view: when a table's natural width does not fit, its
//! columns share the view max-min fairly (narrow columns keep their natural
//! width, the rest split what is left), but no column gets fewer than
//! `MIN_COLUMN` cells, or its natural width if that is narrower. A view too
//! narrow for those minimums shows the table raw.
//!
//! What counts as a table, a subset of GitHub-flavored markdown:
//!
//! - A header line with an unescaped `|`, then a delimiter line with an
//!   unescaped `|` whose cells are each `-`s with an optional `:` at either
//!   end (`:--` left, the default; `--:` right; `:-:` center). Header and
//!   delimiter have the same cell count.
//! - One leading and one trailing pipe are optional. `\|` is a pipe inside a
//!   cell and shows bare. Inline markup shows as written.
//! - Lines indented four or more columns are code, not table lines; up to
//!   three spaces of indentation are dropped.
//! - The body runs to the first blank line, line without an unescaped `|`,
//!   or fence opener. Unlike GFM, a pipeless line is never a row.
//! - If any body row's cell count differs from the header's, the whole block
//!   stays raw (GFM would pad or truncate).
//! - Lines inside ```` ``` ```` or `~~~` fences are never tables. A fence
//!   closes on a line of the same character, at least as long as the opener;
//!   an unclosed fence runs to the end of the file.

use super::wrap;

/// How a column's cells sit in its width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// One entry per line of `lines`: its aligned row if it is a line of a
/// well-formed table, else `None`. All rows of one table are equally wide.
pub fn align(lines: &[String]) -> Vec<Option<String>> {
    let mut shown = vec![None; lines.len()];
    let mut fence: Option<Fence> = None;
    let mut at = 0;
    while at < lines.len() {
        let line = &lines[at];
        if let Some(open) = fence {
            if open.is_closed_by(line) {
                fence = None;
            }
            at += 1;
        } else if let Some(open) = Fence::opened_by(line) {
            fence = Some(open);
            at += 1;
        } else if let Some((aligns, end)) = table_at(lines, at) {
            let block = &lines[at..end];
            let columns = aligns.len();
            if block.iter().all(|row| split(row).len() == columns) {
                for (slot, row) in shown[at..end].iter_mut().zip(format(block, &aligns)) {
                    *slot = Some(row);
                }
            }
            at = end;
        } else {
            at += 1;
        }
    }
    shown
}

/// Where the delimiter row sits in a table block.
const DELIMITER: usize = 1;

/// The aligned rows of a well-formed table: `block` is its header, delimiter
/// and body lines, every one with as many cells as `aligns`.
fn format(block: &[String], aligns: &[Align]) -> Vec<String> {
    let cells: Vec<Vec<String>> = block.iter().map(|row| split(row)).collect();
    let widths: Vec<usize> = (0..aligns.len())
        .map(|column| {
            cells
                .iter()
                .enumerate()
                .filter(|&(at, _)| at != DELIMITER)
                .map(|(_, row)| wrap::width(&row[column]))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    cells
        .iter()
        .enumerate()
        .map(|(at, row)| {
            if at == DELIMITER {
                return widths
                    .iter()
                    .map(|&w| "─".repeat(w + 2))
                    .collect::<Vec<_>>()
                    .join("┼");
            }
            let padded: Vec<String> = row
                .iter()
                .zip(&widths)
                .zip(aligns)
                .map(|((cell, &w), &align)| pad(cell, w, align))
                .collect();
            format!(" {} ", padded.join(" │ "))
        })
        .collect()
}

/// The narrowest a column is squeezed to when its table is too wide for the
/// view; a column naturally narrower than this keeps its natural width.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by align in the next step")
)]
const MIN_COLUMN: usize = 10;

/// Column widths for a table whose columns are `natural` cells wide, laid out
/// in `width` cells (cell padding and ` │ ` separators included), or `None`
/// if the columns cannot all get their minimum, `min(natural, MIN_COLUMN)`.
///
/// Columns that fit are kept at their natural width; otherwise the space is
/// shared max-min fairly: narrowest first, a column keeps its natural width
/// while that is no more than an equal share of what is left, and the
/// columns left over split the rest equally, a leftover cell each to the
/// leftmost of them.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by align in the next step")
)]
fn widths(natural: &[usize], width: usize) -> Option<Vec<usize>> {
    let mut space = width.checked_sub((3 * natural.len()).saturating_sub(1))?;
    let minimum: usize = natural.iter().map(|&n| n.min(MIN_COLUMN)).sum();
    if space < minimum {
        return None;
    }
    let mut widths = natural.to_vec();
    let mut by_width: Vec<usize> = (0..natural.len()).collect();
    by_width.sort_by_key(|&column| natural[column]);
    let mut unsettled = by_width.as_slice();
    while let Some((&column, rest)) = unsettled.split_first()
        && natural[column] * unsettled.len() <= space
    {
        space -= natural[column];
        unsettled = rest;
    }
    let mut shared = unsettled.to_vec();
    shared.sort_unstable();
    for (at, &column) in shared.iter().enumerate() {
        widths[column] = space / shared.len() + usize::from(at < space % shared.len());
    }
    Some(widths)
}

/// `cell` padded with spaces to `width` cells. A center column's odd
/// leftover space goes on the right.
fn pad(cell: &str, width: usize, align: Align) -> String {
    let extra = width.saturating_sub(wrap::width(cell));
    let left = match align {
        Align::Left => 0,
        Align::Center => extra / 2,
        Align::Right => extra,
    };
    format!("{}{cell}{}", " ".repeat(left), " ".repeat(extra - left))
}

/// The column alignments and the end (exclusive) of the table whose header
/// is `lines[at]`, if one starts there. The body runs to the first line that
/// cannot be a row; its cell counts are not checked here.
fn table_at(lines: &[String], at: usize) -> Option<(Vec<Align>, usize)> {
    let header = unindented(&lines[at]).filter(|line| has_pipe(line))?;
    let aligns = delimiter(unindented(lines.get(at + 1)?)?)?;
    if split(header).len() != aligns.len() {
        return None;
    }
    let body = lines[at + 2..]
        .iter()
        .take_while(|line| {
            unindented(line).is_some_and(|row| has_pipe(row) && Fence::opened_by(row).is_none())
        })
        .count();
    Some((aligns, at + 2 + body))
}

/// `line` without its indentation, or `None` if it is indented four columns
/// or more (indented code).
fn unindented(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ');
    (line.len() - rest.len() < 4 && !rest.starts_with('\t')).then_some(rest)
}

/// An open code fence: its character and how many of them opened it.
#[derive(Debug, Clone, Copy)]
struct Fence {
    mark: char,
    len: usize,
}

impl Fence {
    /// The fence `line` opens, if it is a ```` ``` ```` or `~~~` opener.
    fn opened_by(line: &str) -> Option<Self> {
        let rest = unindented(line)?;
        let mark = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
        let len = rest.chars().take_while(|&c| c == mark).count();
        let info = &rest[len..];
        (len >= 3 && !(mark == '`' && info.contains('`'))).then_some(Self { mark, len })
    }

    /// Whether `line` closes this fence: the same character, at least as
    /// many, and nothing else.
    fn is_closed_by(self, line: &str) -> bool {
        unindented(line).is_some_and(|rest| {
            let rest = rest.trim_end();
            rest.chars().count() >= self.len && rest.chars().all(|c| c == self.mark)
        })
    }
}

/// Whether `line` has a `|` that is not escaped as `\|`.
fn has_pipe(line: &str) -> bool {
    line.match_indices('|')
        .any(|(at, _)| !line[..at].ends_with('\\'))
}

/// The trimmed cells of a table row. One leading and one trailing unescaped
/// pipe are dropped; `\|` is a pipe inside a cell and is shown bare.
fn split(line: &str) -> Vec<String> {
    let mut row = line.trim();
    row = row.strip_prefix('|').unwrap_or(row);
    if row.ends_with('|') && !row.ends_with("\\|") {
        row = &row[..row.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    cells.push(cell);
    cells.iter().map(|cell| cell.trim().to_owned()).collect()
}

/// The column alignments of a delimiter row such as `|---|:-:|`, or `None`
/// if `line` is not one: it needs an unescaped pipe, and every cell must be
/// one or more `-` with an optional `:` at either end.
fn delimiter(line: &str) -> Option<Vec<Align>> {
    if !has_pipe(line) {
        return None;
    }
    split(line)
        .iter()
        .map(|cell| {
            let left = cell.starts_with(':');
            let right = cell.len() > 1 && cell.ends_with(':');
            let dashes = &cell[usize::from(left)..cell.len() - usize::from(right)];
            if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::wrap::width;

    #[test]
    fn split_trims_cells_and_drops_one_outer_pipe_each_side() {
        assert_eq!(split("| a | b |"), ["a", "b"]);
        assert_eq!(split("a | b"), ["a", "b"]);
        assert_eq!(split("| |  |"), ["", ""]);
    }

    #[test]
    fn an_escaped_pipe_stays_inside_its_cell_and_shows_bare() {
        assert_eq!(split(r"| a \| b |"), ["a | b"]);
    }

    #[test]
    fn delimiter_reads_each_columns_alignment() {
        use Align::{Center, Left, Right};
        assert_eq!(
            delimiter("|---|:-:|--:|:--|"),
            Some(vec![Left, Center, Right, Left])
        );
        assert_eq!(delimiter("| - |"), Some(vec![Left]));
    }

    #[test]
    fn a_delimiter_needs_a_pipe_and_only_dashes_and_colons() {
        assert_eq!(delimiter("---"), None);
        assert_eq!(delimiter("|-x-|"), None);
        assert_eq!(delimiter("| |"), None);
    }

    #[test]
    fn natural_widths_that_fit_are_kept() {
        assert_eq!(widths(&[1, 8], 21), Some(vec![1, 8]));
    }

    #[test]
    fn narrow_columns_keep_their_width_and_the_rest_share_what_is_left() {
        assert_eq!(widths(&[10, 60, 300], 108), Some(vec![10, 45, 45]));
        assert_eq!(widths(&[3, 300], 50), Some(vec![3, 42]));
    }

    #[test]
    fn a_leftover_cell_goes_to_the_leftmost_shared_column() {
        // Space for cells: 26 - 5 = 21.
        assert_eq!(widths(&[100, 100], 26), Some(vec![11, 10]));
    }

    #[test]
    fn below_the_minimum_column_widths_there_is_no_layout() {
        assert_eq!(widths(&[8, 8], 15), None);
        assert_eq!(widths(&[30, 30], 25), Some(vec![10, 10]));
        assert_eq!(widths(&[30, 30, 1], 29), Some(vec![10, 10, 1]));
        assert_eq!(widths(&[30, 30, 1], 28), None);
        assert_eq!(widths(&[5, 5, 5], 3), None);
    }

    #[test]
    fn allocated_widths_fit_respect_the_minimums_and_use_all_space_when_short() {
        let naturals: [&[usize]; 7] = [
            &[1],
            &[40],
            &[1, 8],
            &[10, 60, 300],
            &[3, 300, 7, 25],
            &[9, 11, 10, 12],
            &[200, 1, 200, 1, 200],
        ];
        for natural in naturals {
            for width in 1..=80 {
                let Some(got) = widths(natural, width) else {
                    continue;
                };
                let space = width - (3 * natural.len() - 1);
                let used: usize = got.iter().sum();
                assert!(used <= space, "{natural:?} at {width}: {got:?}");
                for (&w, &n) in got.iter().zip(natural) {
                    assert!(
                        w <= n && w >= n.min(MIN_COLUMN),
                        "{natural:?} at {width}: {got:?}"
                    );
                }
                if got.iter().zip(natural).any(|(w, n)| w < n) {
                    assert_eq!(used, space, "{natural:?} at {width}: {got:?}");
                }
            }
        }
    }

    /// Which lines of `text` [`align`] treats as table rows.
    fn found(text: &str) -> Vec<bool> {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        align(&lines).iter().map(Option::is_some).collect()
    }

    #[test]
    fn a_well_formed_table_is_found_and_the_prose_around_it_is_not() {
        let text = "intro\n| a | b |\n|---|---|\n| 1 | 2 |\noutro";

        assert_eq!(found(text), [false, true, true, true, false]);
    }

    #[test]
    fn the_body_ends_at_a_blank_line_or_a_line_without_a_pipe() {
        let text = "| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |\nprose\n| 3 |";

        assert_eq!(
            found(text),
            [true, true, true, false, true, true, true, false, false]
        );
    }

    #[test]
    fn a_header_and_delimiter_alone_are_a_table() {
        assert_eq!(found("| a | b |\n|---|---|"), [true, true]);
    }

    #[test]
    fn a_ragged_body_row_leaves_the_whole_block_raw() {
        assert_eq!(
            found("| a | b |\n|---|---|\n| 1 | 2 |\n| 1 | 2 | 3 |"),
            [false; 4]
        );
        assert_eq!(found("| a | b |\n|---|---|\n| 1 |\n| 1 | 2 |"), [false; 4]);
    }

    #[test]
    fn a_header_delimiter_count_mismatch_or_a_setext_heading_is_not_a_table() {
        assert_eq!(found("| a | b |\n|---|\n| 1 | 2 |"), [false; 3]);
        assert_eq!(found("text\n---"), [false; 2]);
        assert_eq!(found("a | b\n---"), [false; 2]);
    }

    #[test]
    fn tables_inside_fences_are_not_tables() {
        let table = "| a |\n|---|\n| 1 |";
        let backticks = format!("```\n{table}\n```\n{table}");
        let tildes = format!("~~~~\n{table}\n```\n~~~~\n{table}");

        assert_eq!(
            found(&backticks),
            [false, false, false, false, false, true, true, true]
        );
        assert_eq!(
            found(&tildes),
            [false, false, false, false, false, false, true, true, true]
        );
        assert_eq!(found(&format!("```rust\n{table}")), [false; 4]);
    }

    #[test]
    fn four_spaces_of_indentation_is_code_and_up_to_three_is_dropped() {
        assert_eq!(found("    | a |\n    |---|\n    | 1 |"), [false; 3]);
        assert_eq!(found("  | a |\n  |---|\n  | 1 |"), [true; 3]);
    }

    /// [`align`]'s rows for `text`, which must all be table lines.
    fn shown(text: &str) -> Vec<String> {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        align(&lines)
            .into_iter()
            .map(|row| row.expect("every line is a table row"))
            .collect()
    }

    #[test]
    fn cells_are_padded_to_their_column_and_joined_by_bars() {
        assert_eq!(
            shown("| Q | Decision |\n|---|---|\n| a | b |"),
            [" Q │ Decision ", "───┼──────────", " a │ b        "]
        );
    }

    #[test]
    fn indentation_is_dropped_and_escaped_pipes_show_bare() {
        assert_eq!(
            shown("  | a |\n  |---|\n  | x \\| y |"),
            [" a     ", "───────", " x | y "]
        );
    }

    #[test]
    fn right_and_center_columns_pad_on_the_correct_side() {
        assert_eq!(
            shown("| right | center |\n|--:|:-:|\n| 1 | 12 |\n| 22 | 1 |"),
            [
                " right │ center ",
                "───────┼────────",
                "     1 │   12   ",
                "    22 │   1    ",
            ]
        );
    }

    #[test]
    fn wide_characters_pad_by_cells_not_chars() {
        let family = "👨\u{200d}👩\u{200d}👧";
        assert_eq!(
            shown(&format!("| k |\n|---|\n| 漢字 |\n| abcd |\n| {family} |")),
            [
                " k    ",
                "──────",
                " 漢字 ",
                " abcd ",
                &format!(" {family}   "),
            ]
        );
    }

    #[test]
    fn every_row_of_a_table_is_equally_wide_and_each_line_has_one_entry() {
        let corpus = [
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "| 漢字 | x |\n|:-:|--:|\n| a | かなカナ |",
            "| 👨\u{200d}👩\u{200d}👧 | 🎉 |\n|---|---|\n| e\u{301} | |",
            "| | |\n|-|-|\n| | |",
            "| esc \\| aped |\n|---|\n| \\| |",
            "| one |\n|---|\n| column |\n| rows |",
            "prose\n| a |\n|---|\n\nmore prose",
        ];
        for text in corpus {
            let lines: Vec<String> = text.lines().map(str::to_owned).collect();
            let rows = align(&lines);

            assert_eq!(rows.len(), lines.len(), "{text:?}");
            let widths: Vec<usize> = rows.iter().flatten().map(|row| width(row)).collect();
            assert!(!widths.is_empty(), "{text:?}");
            assert!(
                widths.windows(2).all(|w| w[0] == w[1]),
                "{text:?}: {widths:?}"
            );
        }
    }
}
