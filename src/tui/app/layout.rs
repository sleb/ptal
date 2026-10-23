//! Layout: how the screen's rows and columns are shared between the file
//! view and the chrome around it, the file view's wrapped rows, and the
//! clamps that keep the cursor, the panel and the comment box in view.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::panel::{self, Layout};
use crate::tui::{keys, search, table, wrap};

use super::{
    App, BANNER_ROWS, BOX_BORDER_COLS, BOX_BORDER_ROWS, BOX_MAX_TEXT_ROWS, FOOTER_ROWS,
    GUTTER_EXTRA, Mode, OVERLAY_CHROME_ROWS, PANEL_BORDER_ROWS, PANEL_MIN_ROWS, ScreenRow,
    review_rows,
};

impl App {
    /// Terminal rows below the file view in the current mode.
    pub(in crate::tui) fn chrome_rows(&self) -> usize {
        match self.mode {
            Mode::Normal | Mode::Visual { .. } | Mode::Search { .. } => FOOTER_ROWS,
            Mode::Input { .. } => self.box_text_rows() + BOX_BORDER_ROWS,
            Mode::Panel { .. } => self.panel_rows() + FOOTER_ROWS,
            Mode::Overlay { .. } => self.rows,
        }
    }

    /// Listing rows an overlay shows: the screen less its title and footer
    /// rows.
    pub(in crate::tui) fn overlay_body_rows(&self) -> usize {
        self.rows.saturating_sub(OVERLAY_CHROME_ROWS)
    }

    /// Cells the comment box wraps its draft at: the box's inner width less
    /// one, so a cursor after a full row still has a cell to stand on.
    pub(in crate::tui) fn draft_width(&self) -> usize {
        self.cols.saturating_sub(BOX_BORDER_COLS + 1)
    }

    /// Draft rows the comment box shows: all of them, at least 1, but at
    /// most 10 and at most `rows / 2 - 2` (so the file view keeps at least
    /// half the terminal), never fewer than 1. 0 outside `Input`.
    pub(in crate::tui) fn box_text_rows(&self) -> usize {
        let Mode::Input { draft, .. } = &self.mode else {
            return 0;
        };
        let cap = (self.rows / 2)
            .saturating_sub(BOX_BORDER_ROWS)
            .clamp(1, BOX_MAX_TEXT_ROWS);
        draft.rows(self.draft_width()).len().clamp(1, cap)
    }

    /// Rows of the bordered panel, border included.
    fn panel_rows(&self) -> usize {
        (self.rows / 2).max(PANEL_MIN_ROWS)
    }

    /// Rows of panel content, inside the border.
    pub(in crate::tui) fn panel_inner_rows(&self) -> usize {
        self.panel_rows().saturating_sub(PANEL_BORDER_ROWS)
    }

    /// The panel's wrapped rows, when it is open.
    pub fn panel_layout(&self) -> Option<Layout> {
        let Mode::Panel { selected, .. } = self.mode else {
            return None;
        };
        let threads = self.threads_on(self.cursor);
        let width = self.cols.saturating_sub(PANEL_BORDER_ROWS);
        Some(panel::layout(&threads, selected, width))
    }

    /// Rows available to the file view, between the banner and the chrome
    /// below it; never zero, so the arithmetic in `clamp_view` and the
    /// half-page step stay total on tiny terminals.
    pub(in crate::tui) fn height(&self) -> usize {
        self.rows
            .saturating_sub(BANNER_ROWS + self.chrome_rows())
            .max(1)
    }

    /// Digits in the largest line number; the gutter right-aligns numbers to
    /// this width.
    pub(in crate::tui) fn digits(&self) -> usize {
        self.lines.len().to_string().len()
    }

    /// Columns left for file text after the gutter (glyph, space, number,
    /// `│`); at least 1.
    pub fn text_width(&self) -> usize {
        self.cols
            .saturating_sub(self.digits() + GUTTER_EXTRA)
            .max(1)
    }

    /// The rows the file view shows, from `top`, at most `height` of them.
    /// Each file line is one or more rows: its shown text wrapped by
    /// [`wrap::rows`].
    pub fn screen_rows(&self) -> Vec<ScreenRow> {
        (self.top..self.lines.len())
            .flat_map(|line| {
                self.wrapped(line)
                    .into_iter()
                    .enumerate()
                    .map(move |(i, text)| ScreenRow {
                        line,
                        first: i == 0,
                        text,
                    })
            })
            .take(self.height())
            .collect()
    }

    pub(super) fn last_line(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    /// Lay out the tables again for the current text width.
    pub(super) fn relayout(&mut self) {
        self.tables = table::align(&self.lines, self.text_width());
        self.help = keys::rows(self.cols);
        self.review_rows = review_rows(&self.review, self.cols);
    }

    /// The screen rows of 0-based `line` at the current width: its table
    /// rows if it has them, in the line's whole-line style only (layout
    /// moves the bytes inline spans point at), else the styled line wrapped
    /// by [`wrap::rows`]. A table is laid out or raw as a whole.
    fn wrapped(&self, line: usize) -> Vec<Line<'static>> {
        let styling = self.stylings.get(line);
        let query = self.highlight_query().unwrap_or_default();
        match &self.tables[line] {
            Some(rows) => {
                let style = styling.map_or(Style::new(), |styling| styling.line);
                rows.iter()
                    .map(|row| {
                        let shown = Line::from(Span::styled(row.clone(), style));
                        search::emphasize(shown, &search::find(row, query))
                    })
                    .collect()
            }
            None => {
                let raw = self.lines[line].as_str();
                let styled = styling.map_or_else(|| Line::raw(raw), |styling| styling.apply(raw));
                let styled = search::emphasize(styled, &search::find(raw, query));
                wrap::rows(&styled, self.text_width())
            }
        }
    }

    /// Screen rows 0-based `line` takes at the current width.
    pub(super) fn rows_of(&self, line: usize) -> usize {
        match self.tables.get(line) {
            Some(Some(rows)) => rows.len(),
            Some(None) => wrap::row_count(&self.lines[line], self.text_width()),
            None => 0,
        }
    }

    /// The first line of the longest run ending at `end` that fits in
    /// `height` rows; `end` itself when it alone is taller. Touches at most
    /// `height` lines.
    fn fit_back(&self, end: usize, height: usize) -> usize {
        let mut first = end;
        let mut used = self.rows_of(end);
        while first > 0 {
            let above = self.rows_of(first - 1);
            if used + above > height {
                break;
            }
            used += above;
            first -= 1;
        }
        first
    }

    /// Keep the whole cursor line on screen (`top <= cursor` and lines
    /// `top..=cursor` fit in `height` rows; only a cursor line taller than
    /// the view is cut off, with `top == cursor`), and never leave blank rows
    /// below the last line while earlier lines could fill them. In `Input`,
    /// also keep the draft's cursor row inside the box.
    pub(super) fn clamp_view(&mut self) {
        if matches!(self.mode, Mode::Panel { .. }) && self.threads_on(self.cursor).is_empty() {
            self.mode = Mode::Normal;
        }
        if matches!(self.mode, Mode::Overlay { .. }) {
            // The file is hidden, so its view stays as it was.
            self.clamp_overlay();
            return;
        }
        let height = self.height();
        self.cursor = self.cursor.min(self.last_line());
        self.top = self
            .top
            .min(self.fit_back(self.last_line(), height))
            .clamp(self.fit_back(self.cursor, height), self.cursor);
        self.clamp_panel();
        let (width, rows) = (self.draft_width(), self.box_text_rows());
        if let Mode::Input { draft, .. } = &mut self.mode {
            draft.clamp_scroll(width, rows);
        }
    }

    /// Keep the panel's selection and scroll inside its content.
    fn clamp_panel(&mut self) {
        let Some(layout) = self.panel_layout() else {
            return;
        };
        let last = layout.thread_starts.len().saturating_sub(1);
        let max_scroll = layout.lines.len().saturating_sub(self.panel_inner_rows());
        if let Mode::Panel { selected, scroll } = &mut self.mode {
            *selected = (*selected).min(last);
            *scroll = (*scroll).min(max_scroll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{State, Status};
    use crate::tui::app::fixtures::*;
    use crate::tui::app::{Effect, Event, update};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn resize_reclamps_the_view_and_zero_height_does_not_panic() {
        let mut a = app(20, 12); // view height 10
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (19, 10));

        send(&mut a, [Event::Resize(40, 5)]); // view height 3
        assert_eq!(at(&a), (19, 17));
        send(&mut a, [Event::Resize(40, 0), ch('k'), ctrl('d'), ch('g')]);
        assert_eq!(a.cursor, 0);
        send(&mut a, [Event::Resize(40, 1), ch('G')]);
        assert_eq!(a.cursor, 19);
    }

    #[test]
    fn open_and_resize_record_the_terminal_width_too() {
        let mut a = app(3, 10);
        assert_eq!((a.cols, a.rows), (80, 10));

        send(&mut a, [Event::Resize(33, 7)]);

        assert_eq!((a.cols, a.rows), (33, 7));
    }

    // --- wrapped rows ---

    fn wrapped(lines: &[&str], (cols, rows): (u16, u16)) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(showing(lines, &[]), "scott", (cols, rows))
    }

    /// The screen rows as (line, first, text), styles dropped.
    fn plain_rows(a: &App) -> Vec<(usize, bool, String)> {
        a.screen_rows()
            .into_iter()
            .map(|row| (row.line, row.first, row.text.to_string()))
            .collect()
    }

    #[test]
    fn text_width_is_the_columns_after_the_gutter_and_at_least_one() {
        assert_eq!(app(12, 10).text_width(), 80 - 5, "2 digits + 3");
        assert_eq!(wrapped(&["a"; 12], (20, 10)).text_width(), 15);
        assert_eq!(wrapped(&["a"; 3], (20, 10)).text_width(), 16);
        assert_eq!(wrapped(&["a"; 3], (3, 10)).text_width(), 1);
        assert_eq!(wrapped(&[], (20, 10)).text_width(), 16);
    }

    #[test]
    fn screen_rows_map_each_row_to_its_file_line_and_fill_the_view() {
        // Gutter 4, text width 10; the second line wraps into 3 rows.
        let a = wrapped(
            &["short", "aaaa bbbb cccc dddd eeee", "x", "y"],
            (14, 7), // view height 5
        );

        let row = |line, first, text: &str| (line, first, text.to_string());
        assert_eq!(
            plain_rows(&a),
            [
                row(0, true, "short"),
                row(1, true, "aaaa bbbb "),
                row(1, false, "cccc dddd "),
                row(1, false, "eeee"),
                row(2, true, "x"),
            ]
        );
    }

    /// Wraps into 3 rows at text width 10; one row at 80 columns.
    const W3: &str = "aaaa bbbb cccc dddd eeee";

    /// Screen rows of lines `from..=to` at the app's current width.
    fn rows_between(a: &App, from: usize, to: usize) -> usize {
        (from..=to).map(|line| a.rows_of(line)).sum()
    }

    #[test]
    fn j_scrolls_so_a_wrapped_cursor_line_is_fully_shown() {
        let mut a = wrapped(&[W3, W3, W3, "x"], (14, 9)); // height 7

        send(&mut a, [ch('j'), ch('j')]);

        assert_eq!(at(&a), (2, 1), "lines 1 and 2 take 6 of the 7 rows");
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn capital_g_shows_all_of_a_wrapped_last_line_with_no_blank_rows() {
        let mut lines = vec!["x"; 10];
        lines.push(W3);
        let mut a = wrapped(&lines, (15, 9)); // 2 digits, width 10, height 7

        send(&mut a, [ch('G')]);

        assert_eq!(at(&a), (10, 6), "4 short lines and the 3 rows of the last");
        assert_eq!(a.screen_rows().len(), 7);
    }

    #[test]
    fn a_cursor_line_taller_than_the_view_starts_at_the_top() {
        let mut a = wrapped(&["x", W3, "y"], (14, 4)); // height 2

        send(&mut a, [ch('j')]);

        assert_eq!(at(&a), (1, 1));
    }

    #[test]
    fn a_bracket_jump_past_wrapped_lines_shows_the_target() {
        let lines = [W3, W3, W3, "x", "y"].map(String::from).to_vec();
        let threads = [view("c1", (4, 4), Status::Open, State::Current)];
        let mut a = App::open(showing(lines, &threads), "scott", (14, 9)); // height 7

        send(&mut a, [ch(']')]);

        assert_eq!(at(&a), (3, 1));
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn opening_the_panel_below_wrapped_lines_keeps_the_cursor_line_shown() {
        let lines = [W3, W3, W3, W3, "t"].map(String::from).to_vec();
        let threads = [chatty("c1", (5, 5), 1)];
        let mut a = App::open(showing(lines, &threads), "scott", (14, 17)); // height 15
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (4, 0));

        send(&mut a, [press(KeyCode::Enter)]); // height 7

        assert!(cursor_line_shown(&a));
        assert_eq!(a.top, 2);
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(at(&a), (4, 0), "everything fits again");
    }

    #[test]
    fn resizing_narrower_rewraps_and_keeps_the_cursor_line_shown() {
        let mut a = wrapped(&[W3, W3, W3, "x"], (80, 9));
        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (3, 0));

        send(&mut a, [Event::Resize(14, 9)]);

        assert_eq!(a.screen_rows()[0].text.to_string(), "aaaa bbbb ");
        assert_eq!(at(&a), (3, 1));
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn half_a_page_counts_rows_not_lines() {
        // 2 digits, width 10: every line is 3 rows. Height 11, half page 5.
        let mut a = wrapped(&[W3; 20], (15, 13));

        send(&mut a, [ctrl('d')]);
        assert_eq!(at(&a), (1, 1), "a second line would make 6 rows");
        send(&mut a, [ctrl('d'), ctrl('d')]);
        assert_eq!(at(&a), (3, 3));
        send(&mut a, [ctrl('u')]);
        assert_eq!(at(&a), (2, 2));
    }

    // --- highlighting ---

    fn highlighted(lines: &[&str], size: (u16, u16)) -> App {
        wrapped(lines, size).highlighted(true)
    }

    /// Each span of screen row `at` as its text and its full style.
    fn styled_row(a: &App, at: usize) -> Vec<(String, Style)> {
        let row = &a.screen_rows()[at].text;
        row.spans
            .iter()
            .map(|span| (span.content.to_string(), row.style.patch(span.style)))
            .collect()
    }

    fn bold() -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    #[test]
    fn a_highlighted_app_styles_its_rows_and_an_unhighlighted_one_does_not() {
        let lines = ["# Title", "plain **bold**"];
        let heading = Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD);

        let a = highlighted(&lines, (80, 10));
        assert_eq!(styled_row(&a, 0), [("# Title".into(), heading)]);
        assert_eq!(
            styled_row(&a, 1),
            [("plain ".into(), Style::new()), ("**bold**".into(), bold())]
        );

        for a in [
            wrapped(&lines, (80, 10)),
            wrapped(&lines, (80, 10)).highlighted(false),
        ] {
            for at in 0..2 {
                assert!(
                    styled_row(&a, at)
                        .iter()
                        .all(|(_, style)| *style == Style::new())
                );
            }
        }
    }

    #[test]
    fn a_wrapped_styled_line_keeps_its_style_on_its_continuation_rows() {
        let a = highlighted(&["**aaaa bbbb cccc**"], (12, 10)); // text width 10

        assert_eq!(
            plain_rows(&a),
            [
                (0, true, "**aaaa ".to_string()),
                (0, false, "bbbb ".to_string()),
                (0, false, "cccc**".to_string()),
            ]
        );
        for at in 0..3 {
            assert!(styled_row(&a, at).iter().all(|(_, style)| *style == bold()));
        }
    }

    #[test]
    fn a_laid_out_table_gets_only_its_line_styles_and_a_raw_one_gets_inline_ones() {
        let table = ["| **h** | b |", "|---|---|", "| **x** | y |"];

        let a = highlighted(&table, (40, 10));
        assert!(styled_row(&a, 0).iter().all(|(_, style)| *style == bold()));
        assert_eq!(plain_rows(&a)[2].2, " **x** │ y ");
        assert!(
            styled_row(&a, 2)
                .iter()
                .all(|(_, style)| *style == Style::new())
        );

        let narrow = highlighted(&table, (10, 12)); // text width 6: raw
        assert_eq!(plain_rows(&narrow)[0].2, "| ");
        let body_rows = narrow
            .screen_rows()
            .iter()
            .filter(|row| row.line == 2)
            .count();
        let first_body = narrow.screen_rows().iter().position(|row| row.line == 2);
        let first_body = first_body.expect("the body line is shown");
        assert!(
            (first_body..first_body + body_rows)
                .any(|at| styled_row(&narrow, at).contains(&("**x**".into(), bold())))
        );
    }

    #[test]
    fn highlighting_changes_no_text_layout_or_behavior() {
        let file = [
            "# Plan **now**",
            "",
            "- a list item with `code` that goes on and on and on",
            "> a quote with *emphasis* and [a link](http://example.com/long/path)",
            "```rust",
            "fn main() { let x = **y; }",
            "```",
            "| a | **b** |",
            "|---|---|",
            "| 1 | a cell long enough to wrap in a narrow view |",
            "Setext",
            "---",
            "> \u{301}abcd a combining accent right after a quote marker",
        ];
        let keys = [ch('j'), ch('j'), ch('G'), ch('k'), press(KeyCode::Enter)];
        for cols in [12, 20, 40, 80] {
            for rows in [4, 10] {
                let mut off = wrapped(&file, (cols, rows));
                let mut on = highlighted(&file, (cols, rows));
                assert_eq!(plain_rows(&on), plain_rows(&off), "{cols}x{rows}");
                for line in 0..file.len() {
                    assert_eq!(
                        on.rows_of(line),
                        on.wrapped(line).len(),
                        "{cols}x{rows} line {line}"
                    );
                }
                for key in keys.clone() {
                    assert_eq!(update(&mut on, key.clone()), update(&mut off, key));
                    assert_eq!((at(&on), plain_rows(&on)), (at(&off), plain_rows(&off)));
                }
                typed(&mut on, "x");
                typed(&mut off, "x");
                assert_eq!(
                    update(&mut on, press(KeyCode::Enter)),
                    update(&mut off, press(KeyCode::Enter))
                );
            }
        }
    }

    // --- tables ---

    /// Prose around a table that aligns 21 cells wide; its raw lines are at
    /// most 16.
    const TABLE: [&str; 5] = ["intro", "|a|bbbbbbbb|", "|-|-|", "|cccccccc|d|", "outro"];

    #[test]
    fn a_table_line_is_one_screen_row_showing_its_aligned_row() {
        let a = wrapped(&TABLE, (40, 10));

        let row = |line, text: &str| (line, true, text.to_string());
        assert_eq!(
            plain_rows(&a),
            [
                row(0, "intro"),
                row(1, " a        │ bbbbbbbb "),
                row(2, "──────────┼──────────"),
                row(3, " cccccccc │ d        "),
                row(4, "outro"),
            ]
        );
    }

    #[test]
    fn a_table_too_narrow_for_its_minimum_widths_shows_raw_and_resizing_switches_it() {
        let texts = |a: &App| -> Vec<String> {
            a.screen_rows()
                .into_iter()
                .map(|row| row.text.to_string())
                .collect()
        };
        let mut a = wrapped(&TABLE, (19, 10)); // text width 15
        assert_eq!(texts(&a), TABLE);

        send(&mut a, [Event::Resize(40, 10)]);
        assert_eq!(texts(&a)[1], " a        │ bbbbbbbb ");
        send(&mut a, [Event::Resize(19, 10)]);
        assert_eq!(texts(&a), TABLE);
    }

    /// A decision table with a long cell, then prose. 4 lines, so the gutter
    /// is 4 columns; at 24 columns the text width is 20 and the long cell
    /// wraps into 3 rows; at 80 it fits on one.
    const DECISION: [&str; 4] = [
        "| Q | Decision |",
        "|---|---|",
        "| 1 | use the water filling rule for widths |",
        "outro",
    ];

    fn texts(a: &App) -> Vec<String> {
        a.screen_rows()
            .into_iter()
            .map(|row| row.text.to_string())
            .collect()
    }

    #[test]
    fn a_table_too_wide_for_the_view_wraps_its_cells_inside_their_columns() {
        let a = wrapped(&DECISION, (24, 10));

        let row = |line, first, text: &str| (line, first, text.to_string());
        assert_eq!(
            plain_rows(&a),
            [
                row(0, true, " Q │ Decision       "),
                row(1, true, "───┼────────────────"),
                row(2, true, " 1 │ use the water  "),
                row(2, false, "   │ filling rule   "),
                row(2, false, "   │ for widths     "),
                row(3, true, "outro"),
            ]
        );
    }

    #[test]
    fn resizing_reflows_a_table_between_one_row_wrapped_and_raw() {
        let mut a = wrapped(&DECISION, (80, 10));
        assert_eq!(texts(&a)[2], " 1 │ use the water filling rule for widths ");

        send(&mut a, [Event::Resize(24, 10)]);
        assert_eq!(
            texts(&a)[2..5],
            [
                " 1 │ use the water  ",
                "   │ filling rule   ",
                "   │ for widths     "
            ]
        );
        send(&mut a, [Event::Resize(14, 10)]); // text width 10: below 1 + 10
        assert_eq!(texts(&a)[0], "| Q | ");
        send(&mut a, [Event::Resize(80, 10)]);
        assert_eq!(a.screen_rows().len(), 4);
    }

    #[test]
    fn j_moves_over_a_wrapped_table_line_in_one_step_and_enter_comments_on_it() {
        let mut a = wrapped(&DECISION, (24, 10));

        send(&mut a, [ch('j'), ch('j')]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('j')]);
        assert_eq!(a.cursor, 3);
        send(&mut a, [ch('k'), press(KeyCode::Enter)]);
        typed(&mut a, "why?");

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 3,
                end: 3,
                body: "why?".into()
            })
        );
    }

    #[test]
    fn the_clamp_counts_the_rows_of_a_wrapped_table_line() {
        let mut lines = vec!["x"; 6];
        lines.extend(&DECISION[..3]);
        let mut a = wrapped(&lines, (24, 6)); // text width 20, height 4

        send(&mut a, [ch('G')]);
        assert_eq!(at(&a), (8, 7), "the delimiter and the body's 3 rows");
        assert!(cursor_line_shown(&a));
        send(&mut a, [ch('g'), ctrl('d'), ctrl('d'), ctrl('d')]);
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn j_moves_through_a_table_by_file_line_and_enter_comments_on_one() {
        let mut a = wrapped(&TABLE, (40, 10));

        send(&mut a, [ch('j'), ch('j')]);
        assert_eq!(a.cursor, 2);
        send(&mut a, [ch('j'), press(KeyCode::Enter)]);
        typed(&mut a, "why?");

        assert_eq!(
            update(&mut a, ctrl('s')),
            Some(Effect::Add {
                start: 4,
                end: 4,
                body: "why?".into()
            })
        );
    }

    #[test]
    fn the_view_is_clamped_by_the_shown_rows_not_the_raw_ones() {
        // Text width 20: the raw header wraps into 2 rows, aligned it is 1.
        let mut a = wrapped(
            &["| a                        | b |", "|---|---|", "| c | d |"],
            (24, 5), // height 3
        );

        send(&mut a, [ch('G')]);

        assert_eq!(at(&a), (2, 0), "the aligned table fits in 3 rows");
        assert!(cursor_line_shown(&a));
    }

    #[test]
    fn the_cursor_line_stays_shown_through_a_walk_over_wrapped_lines() {
        let long = "word ".repeat(30);
        let wide_row = format!("| 1 | {long} |");
        let mut mixed: Vec<&str> = (0..40)
            .map(|i| match i % 4 {
                0 => W3,
                1 => "short",
                2 => long.as_str(),
                _ => "",
            })
            .collect();
        mixed.extend(["| id | why |", "|---|---|", &wide_row, "| 2 | ok |"]);
        let walk = "jjjjjjjjjjjjGkkkkkkkgjjjjjj]]][[";
        let mut events: Vec<Event> = walk.chars().map(ch).collect();
        events.extend([ctrl('d'), ctrl('u'), ctrl('d'), ctrl('d'), ch('G')]);
        events.extend([ctrl('u'), ctrl('u'), ctrl('d')]);

        for cols in [12, 14, 20, 40, 80] {
            for rows in [4, 8, 13] {
                let mut a = wrapped(&mixed, (cols, rows));
                for e in &events {
                    update(&mut a, e.clone());
                    let height = a.height();
                    assert!(
                        a.top <= a.cursor
                            && (rows_between(&a, a.top, a.cursor) <= height || a.top == a.cursor),
                        "{cols}x{rows} after {e:?}: top {} cursor {}",
                        a.top,
                        a.cursor
                    );
                }
            }
        }
    }

    // --- adding a thread from the panel ---

    #[test]
    fn input_box_shrinks_the_view_and_keeps_the_cursor_visible() {
        let mut a = app(20, 8); // view height 6 in Normal
        send(&mut a, [ch('j'), ch('j'), ch('j'), ch('j'), ch('j')]);
        assert_eq!(at(&a), (5, 0), "cursor on the last visible row");

        send(&mut a, [press(KeyCode::Enter)]);

        assert_eq!(
            a.height(),
            4,
            "8 rows minus the banner and a 3-row input box"
        );
        assert!(a.top <= a.cursor && a.cursor < a.top + 4, "{:?}", at(&a));
        send(&mut a, [press(KeyCode::Esc)]);
        assert_eq!(a.height(), 6);
        assert!(a.top <= a.cursor && a.cursor < a.top + 6, "{:?}", at(&a));
    }

    /// `a`'s draft scroll and its cursor's screen row.
    fn box_scroll(a: &App) -> (usize, usize) {
        match &a.mode {
            Mode::Input { draft, .. } => (draft.scroll(), draft.cursor_at(a.draft_width()).0),
            other => panic!("not in Input mode: {other:?}"),
        }
    }

    /// `n` draft lines, `x` on each, in a box on line 1.
    fn box_of(n: usize, rows: u16) -> App {
        let mut a = app(30, rows);
        send(&mut a, [press(KeyCode::Enter)]);
        for i in 0..n {
            if i > 0 {
                send(&mut a, [press(KeyCode::Enter)]);
            }
            typed(&mut a, "x");
        }
        a
    }

    #[test]
    fn the_box_grows_with_its_draft_up_to_a_cap() {
        assert_eq!(box_of(0, 40).chrome_rows(), 3, "an empty draft is one row");
        assert_eq!(box_of(1, 40).chrome_rows(), 3);
        assert_eq!(box_of(4, 40).chrome_rows(), 6);
        assert_eq!(box_of(10, 40).chrome_rows(), 12);
        assert_eq!(box_of(14, 40).chrome_rows(), 12, "at most 10 text rows");
        assert_eq!(box_of(14, 16).chrome_rows(), 8, "at most rows / 2 - 2");
        assert_eq!(box_of(3, 4).chrome_rows(), 3, "at least one text row");
    }

    #[test]
    fn a_draft_past_the_cap_scrolls_to_keep_the_cursor_row_shown() {
        let mut a = box_of(14, 40);
        assert_eq!(box_scroll(&a), (4, 13), "rows 4..14 shown");

        send(&mut a, (0..13).map(|_| press(KeyCode::Up)));
        assert_eq!(box_scroll(&a), (0, 0));
        send(&mut a, (0..10).map(|_| press(KeyCode::Down)));
        assert_eq!(box_scroll(&a), (1, 10), "scrolled just enough");
    }

    #[test]
    fn a_resize_rewraps_the_draft_and_reclamps_its_scroll() {
        let mut a = app(30, 40);
        send(&mut a, [press(KeyCode::Enter)]);
        typed(&mut a, &"word ".repeat(24));
        send(&mut a, [press(KeyCode::Home)]);
        assert_eq!(a.chrome_rows(), 4, "120 cells at 77 is two rows");

        update(&mut a, Event::Resize(13, 40));
        assert_eq!(a.chrome_rows(), 12, "two words a row, capped at 10");
        assert_eq!(box_scroll(&a), (0, 0));
        send(&mut a, [press(KeyCode::End)]);
        assert_eq!(box_scroll(&a), (2, 11));

        update(&mut a, Event::Resize(80, 40));
        assert_eq!(box_scroll(&a), (0, 1), "no blank rows below the text");
        update(&mut a, Event::Resize(80, 0));
        update(&mut a, Event::Resize(0, 0));
    }

    #[test]
    fn the_commented_lines_stay_shown_as_the_box_grows() {
        let mut one = app(30, 24);
        send(&mut one, (0..20).map(|_| ch('j')));
        send(&mut one, [press(KeyCode::Enter)]);
        let mut range = app(30, 24);
        send(&mut range, (0..18).map(|_| ch('j')));
        send(
            &mut range,
            [ch('v'), ch('j'), ch('j'), press(KeyCode::Enter)],
        );
        for a in [&mut one, &mut range] {
            for _ in 0..12 {
                send(a, [ch('x'), press(KeyCode::Enter)]);
                assert!(cursor_line_shown(a), "{:?}", at(a));
            }
            assert_eq!(a.chrome_rows(), 12);
        }
    }

    // --- the review state ---

    #[test]
    fn the_banner_takes_a_row_from_the_file_view() {
        let a = app(20, 10);

        assert_eq!(a.height(), 8, "10 rows less the banner and the footer");
    }
}
