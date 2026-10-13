//! Drawing: a pure function of [`App`] onto a ratatui frame.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Widget};

use super::app::{App, Marker, Mode};

/// Draw `app` onto `frame`. Reads state only; never mutates it.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    if area.is_empty() {
        return;
    }
    let buf = frame.buffer_mut();
    let chrome = u16::try_from(app.chrome_rows())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let view = Rect {
        height: area.height - chrome,
        ..area
    };
    let bottom = Rect {
        y: area.y + view.height,
        height: chrome,
        ..area
    };

    draw_file(app, buf, view);
    match app.mode() {
        Mode::Normal => draw_footer(app, buf, bottom),
        Mode::Panel { scroll, .. } => {
            // The footer is the panel's last row; the box takes the rest.
            let rows = bottom.height.saturating_sub(1);
            let panel = Rect {
                height: rows,
                ..bottom
            };
            let footer = Rect {
                y: bottom.y + rows,
                height: bottom.height - rows,
                ..bottom
            };
            draw_panel(app, *scroll, buf, panel);
            draw_footer(app, buf, footer);
        }
        Mode::Input {
            draft,
            cursor,
            reply_to,
            ..
        } => draw_input(app, draft, *cursor, reply_to.as_deref(), buf, bottom),
    }
}

const TAB: &str = "    ";

fn draw_file(app: &App, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    if app.lines().is_empty() {
        buf.set_stringn(
            area.x,
            area.y,
            "(empty file)",
            usize::from(area.width),
            Style::new(),
        );
        return;
    }
    let digits = app.lines().len().to_string().len();
    let rows = app
        .lines()
        .iter()
        .enumerate()
        .skip(app.top())
        .zip(area.rows());
    for ((index, line), row) in rows {
        let style = if index == app.cursor() {
            Style::new().bg(Color::DarkGray)
        } else {
            Style::new()
        };
        buf.set_style(row, style);
        let text = line.replace('\t', TAB);
        let row_text = format!("  {:>digits$}│{text}", index + 1);
        buf.set_stringn(row.x, row.y, row_text, usize::from(row.width), style);
        if let Some(marker) = app.marker(index) {
            let (glyph, color) = glyph(marker);
            buf.set_stringn(row.x, row.y, glyph, 1, style.patch(color));
        }
    }
}

/// The gutter glyph and the style it adds. The glyph differs by marker, not
/// just the color, so it survives a monochrome terminal.
fn glyph(marker: Marker) -> (&'static str, Style) {
    match marker {
        Marker::Waiting => ("◆", Style::new().fg(Color::Yellow)),
        Marker::Open => ("●", Style::new()),
        Marker::Resolved => ("○", Style::new().add_modifier(Modifier::DIM)),
    }
}

fn draw_footer(app: &App, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let mut parts = vec![match app.status() {
        Some(status) => status.to_string(),
        None => app.file().to_string(),
    }];
    let position = if app.lines().is_empty() {
        0
    } else {
        app.cursor() + 1
    };
    parts.push(format!("L{position}/{}", app.lines().len()));
    if app.outdated() > 0 {
        parts.push(format!("{} outdated", app.outdated()));
    }
    let hints = match app.mode() {
        Mode::Panel { .. } => "tab:next a:add r:reply j/k:scroll esc:close",
        _ => "enter:thread/comment ]/[:jump q:quit",
    };
    parts.push(hints.to_string());
    buf.set_stringn(
        area.x,
        area.y,
        parts.join("  "),
        usize::from(area.width),
        Style::new(),
    );
}

fn draw_panel(app: &App, scroll: usize, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let block = Block::bordered().title(format!("Threads on line {}", app.cursor() + 1));
    let inner = block.inner(area);
    block.render(area, buf);
    let Some(layout) = app.panel_layout() else {
        return;
    };
    let rows = layout.lines.iter().skip(scroll).zip(inner.rows());
    for (line, row) in rows {
        let style = if line.dim {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new()
        };
        buf.set_stringn(row.x, row.y, &line.text, usize::from(row.width), style);
    }
}

fn draw_input(
    app: &App,
    draft: &str,
    cursor: usize,
    reply_to: Option<&str>,
    buf: &mut Buffer,
    area: Rect,
) {
    if area.is_empty() {
        return;
    }
    let title = match reply_to {
        Some(id) => format!("Reply to {id} as {}", app.author()),
        None => format!("Comment on line {} as {}", app.cursor() + 1, app.author()),
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.is_empty() {
        return;
    }
    // One row, so scroll sideways to keep the cursor cell in view.
    let width = usize::from(inner.width);
    let first = (cursor + 1).saturating_sub(width);
    let mut shown = draft.chars().skip(first).take(width);
    for (offset, x) in (inner.left()..inner.right()).enumerate() {
        let style = if first + offset == cursor {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        let symbol = shown.next().unwrap_or(' ');
        buf[(x, inner.y)].set_char(symbol).set_style(style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Anchor, State, Status, Thread};
    use crate::review::ThreadView;
    use crate::tui::app::{Event, update};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::{Color, Modifier};

    const W: u16 = 66;

    fn lines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    fn app_with(lines: Vec<String>, threads: &[ThreadView], rows: u16) -> App {
        App::open(lines, threads, "scott", (W, rows)).labeled("plan.md")
    }

    fn draw(app: &App, width: u16, rows: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, rows)).unwrap();
        terminal.draw(|frame| render(app, frame)).unwrap();
        terminal
    }

    /// The screen as text, one line per row, trailing spaces trimmed.
    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(app: &mut App, code: KeyCode) {
        update(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    fn open_thread(id: &str, (start, end): (u32, u32), state: State) -> ThreadView {
        ThreadView {
            thread: Thread {
                id: id.into(),
                anchor: Anchor {
                    start_line: start,
                    end_line: end,
                    quote: String::new(),
                },
                status: Status::Open,
                comments: vec![],
            },
            state,
        }
    }

    fn talk(
        id: &str,
        range: (u32, u32),
        quote: &str,
        status: Status,
        state: State,
        said: &[(&str, &str)],
    ) -> ThreadView {
        use crate::model::Comment;
        let mut view = open_thread(id, range, state);
        view.thread.anchor.quote = quote.into();
        view.thread.status = status;
        view.thread.comments = said
            .iter()
            .enumerate()
            .map(|(i, (author, body))| Comment {
                id: format!("{id}-{i}"),
                author: (*author).into(),
                body: (*body).into(),
                created_at: format!("2026-10-12T09:{:02}:30Z", 30 + i),
            })
            .collect();
        view
    }

    fn resolved_by_claude() -> Status {
        Status::Resolved {
            resolved_by: "claude".into(),
            resolved_at: "2026-10-12T10:00:00Z".into(),
        }
    }

    /// A panel row: border, `text` padded to the box's inner width, border.
    fn boxed(text: &str) -> String {
        format!("│{text:<inner$}│", inner = usize::from(W) - 2)
    }

    fn box_top(title: &str) -> String {
        format!(
            "┌{title}{}┐",
            "─".repeat(usize::from(W) - 2 - title.chars().count())
        )
    }

    fn box_bottom() -> String {
        format!("└{}┘", "─".repeat(usize::from(W) - 2))
    }

    fn panel_app(threads: &[ThreadView]) -> App {
        let mut app = app_with(lines(12), threads, 16);
        key(&mut app, KeyCode::Enter);
        app
    }

    const PANEL_FOOTER: &str = "plan.md  L1/12  tab:next a:add r:reply j/k:scroll esc:close";

    #[test]
    fn the_panel_shows_the_thread_in_a_box_above_the_footer() {
        let threads = [talk(
            "c1",
            (1, 1),
            "line 1",
            Status::Open,
            State::Current,
            &[("scott", "Why this?"), ("claude", "Because.")],
        )];
        let app = panel_app(&threads);

        let screen = screen(&draw(&app, W, 16));

        let expected = [
            "◆  1│line 1",
            "   2│line 2",
            "   3│line 3",
            "   4│line 4",
            "   5│line 5",
            "   6│line 6",
            "   7│line 7",
            &box_top("Threads on line 1"),
            &boxed("▶ c1 open"),
            &boxed("  > line 1"),
            &boxed("  scott · 2026-10-12 09:30Z"),
            &boxed("    Why this?"),
            &boxed("  claude · 2026-10-12 09:31Z"),
            &boxed("    Because."),
            &box_bottom(),
            PANEL_FOOTER,
        ]
        .join("\n");
        assert_eq!(screen, expected);
    }

    #[test]
    fn two_threads_stack_and_the_marker_follows_the_selection() {
        let threads = [
            talk(
                "c1",
                (1, 1),
                "a",
                Status::Open,
                State::Current,
                &[("scott", "mine")],
            ),
            talk(
                "c2",
                (1, 1),
                "a",
                Status::Open,
                State::Current,
                &[("claude", "theirs")],
            ),
        ];
        let mut app = panel_app(&threads);

        let first = screen(&draw(&app, W, 16));
        assert!(first.contains("│  c1 open"), "{first}");
        assert!(
            first.contains("│▶ c2 open"),
            "waiting thread starts selected"
        );

        key(&mut app, KeyCode::Tab);
        let second = screen(&draw(&app, W, 16));
        assert!(second.contains("│▶ c1 open"), "{second}");
        assert!(second.contains("│  c2 open"));
    }

    #[test]
    fn a_resolved_thread_is_dim_and_says_who_resolved_it() {
        let threads = [talk(
            "c1",
            (1, 1),
            "a",
            resolved_by_claude(),
            State::Current,
            &[("scott", "hm")],
        )];
        let app = panel_app(&threads);

        let terminal = draw(&app, W, 16);

        let text = screen(&terminal);
        assert!(text.contains("│▶ c1 resolved by claude"), "{text}");
        let buffer = terminal.backend().buffer();
        // Row 8 is the header; column 1 is its marker.
        assert!(buffer[(1, 8)].modifier.contains(Modifier::DIM));
        assert!(!buffer[(0, 8)].modifier.contains(Modifier::DIM), "border");
    }

    #[test]
    fn an_outdated_thread_is_labeled_in_the_panel() {
        let threads = [talk(
            "c1",
            (1, 1),
            "old text",
            Status::Open,
            State::Outdated,
            &[("scott", "x")],
        )];
        let app = panel_app(&threads);

        let text = screen(&draw(&app, W, 16));

        assert!(text.contains("│▶ c1 open · outdated"), "{text}");
        assert!(text.contains("│  > old text"));
    }

    #[test]
    fn a_scrolled_panel_starts_at_its_scroll_row() {
        let threads = [talk(
            "c1",
            (1, 1),
            "line 1",
            Status::Open,
            State::Current,
            &[
                ("scott", "Why this?"),
                ("claude", "Because."),
                ("scott", "Ok."),
            ],
        )];
        let mut app = panel_app(&threads);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));

        let text = screen(&draw(&app, W, 16));

        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[8], boxed("  scott · 2026-10-12 09:30Z"));
        assert!(!text.contains("▶ c1 open"));
    }

    #[test]
    fn typing_from_the_panel_swaps_the_box_for_the_input_and_keeps_the_file() {
        let threads = [talk(
            "c1",
            (1, 1),
            "a",
            Status::Open,
            State::Current,
            &[("claude", "hi")],
        )];
        let mut app = panel_app(&threads);
        key(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "ok");

        let text = screen(&draw(&app, W, 16));

        assert!(text.contains("┌Comment on line 1 as scott"), "{text}");
        assert!(text.contains("│ok"));
        assert!(!text.contains("Threads on line"));
        assert!(text.starts_with("◆  1│line 1"));
    }

    #[test]
    fn replying_titles_the_box_with_the_thread_and_author() {
        let threads = [talk(
            "c1",
            (1, 1),
            "a",
            Status::Open,
            State::Current,
            &[("claude", "hi")],
        )];
        let mut app = panel_app(&threads);
        key(&mut app, KeyCode::Char('r'));
        type_text(&mut app, "ok");

        let text = screen(&draw(&app, W, 16));

        assert!(text.contains("┌Reply to c1 as scott"), "{text}");
        assert!(!text.contains("Comment on line"));
        assert!(text.contains("│ok"));
    }

    #[test]
    fn the_cursor_line_stays_highlighted_above_the_panel() {
        let threads = [talk(
            "c1",
            (3, 3),
            "line 3",
            Status::Open,
            State::Current,
            &[("claude", "hi")],
        )];
        let mut app = app_with(lines(12), &threads, 16);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);

        let terminal = draw(&app, W, 16);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(6, 2)].bg, Color::DarkGray);
        assert_eq!(buffer[(6, 1)].bg, Color::Reset);
    }

    #[test]
    fn normal_mode_shows_gutter_text_and_a_footer() {
        let app = app_with(lines(12), &[], 8);

        let screen = screen(&draw(&app, W, 8));

        assert_eq!(
            screen,
            "   1│line 1
   2│line 2
   3│line 3
   4│line 4
   5│line 5
   6│line 6
   7│line 7
plan.md  L1/12  enter:thread/comment ]/[:jump q:quit"
        );
    }

    #[test]
    fn marked_lines_show_a_dot_and_outdated_threads_are_counted() {
        let threads = [
            open_thread("c1", (2, 3), State::Current),
            open_thread("c2", (6, 6), State::Outdated),
        ];
        let app = app_with(lines(12), &threads, 8);

        let screen = screen(&draw(&app, W, 8));

        assert_eq!(
            screen,
            "   1│line 1
●  2│line 2
●  3│line 3
   4│line 4
   5│line 5
●  6│line 6
   7│line 7
plan.md  L1/12  1 outdated  enter:thread/comment ]/[:jump q:quit"
        );
    }

    #[test]
    fn gutter_glyph_and_style_follow_the_marker() {
        use crate::model::Comment;
        let said = |mut v: ThreadView, author: &str| {
            v.thread.comments = vec![Comment {
                id: "x".into(),
                author: author.into(),
                body: "b".into(),
                created_at: "2026-10-12T10:00:00Z".into(),
            }];
            v
        };
        let mut resolved = said(open_thread("c3", (3, 3), State::Current), "claude");
        resolved.thread.status = Status::Resolved {
            resolved_by: "claude".into(),
            resolved_at: "2026-10-12T10:00:00Z".into(),
        };
        let threads = [
            said(open_thread("c1", (1, 1), State::Current), "scott"),
            said(open_thread("c2", (2, 2), State::Current), "claude"),
            resolved,
        ];
        let app = app_with(lines(4), &threads, 8);

        let terminal = draw(&app, W, 8);

        let buffer = terminal.backend().buffer();
        let cell = |y: u16| &buffer[(0, y)];
        assert_eq!(cell(0).symbol(), "●");
        assert_eq!(cell(1).symbol(), "◆");
        assert_eq!(cell(1).fg, Color::Yellow);
        assert_eq!(cell(2).symbol(), "○");
        assert!(cell(2).modifier.contains(Modifier::DIM));
        assert_eq!(cell(3).symbol(), " ");
    }

    #[test]
    fn input_mode_draws_a_titled_box_over_the_footer_with_the_file_above() {
        let mut app = app_with(lines(12), &[], 8);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);
        type_text(&mut app, "why?");

        let screen = screen(&draw(&app, W, 8));

        let title = "Comment on line 3 as scott";
        let top = format!("┌{title}{}┐", "─".repeat(usize::from(W) - 2 - title.len()));
        let bottom = format!("└{}┘", "─".repeat(usize::from(W) - 2));
        assert_eq!(
            screen,
            format!(
                "   1│line 1\n   2│line 2\n   3│line 3\n   4│line 4\n   5│line 5\n{top}\n│why?{}│\n{bottom}",
                " ".repeat(usize::from(W) - 2 - 4)
            )
        );
    }

    #[test]
    fn input_cursor_is_a_reversed_cell_mid_draft_and_at_the_end() {
        let mut app = app_with(lines(3), &[], 8);
        key(&mut app, KeyCode::Enter);
        type_text(&mut app, "abc");
        let reversed = |terminal: &Terminal<TestBackend>, x: u16| {
            // 8 rows: the box fills rows 5..8, its text row is 6.
            terminal.backend().buffer()[(x, 6)]
                .modifier
                .contains(Modifier::REVERSED)
        };

        let at_end = draw(&app, W, 8);
        assert!(reversed(&at_end, 4), "the space after `abc`");
        assert!(!reversed(&at_end, 3));

        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Left);
        let mid = draw(&app, W, 8);
        assert!(reversed(&mid, 2), "on `b`");
        assert!(!reversed(&mid, 4));
        assert_eq!(mid.backend().buffer()[(2, 6)].symbol(), "b");
    }

    #[test]
    fn a_scrolled_view_starts_at_top() {
        let mut app = app_with(lines(30), &[], 8);
        key(&mut app, KeyCode::Char('G'));

        let screen = screen(&draw(&app, W, 8));

        let first = screen.lines().next().unwrap();
        assert_eq!(first, "  24│line 24");
        assert!(screen.ends_with("L30/30  enter:thread/comment ]/[:jump q:quit"));
    }

    #[test]
    fn long_lines_are_truncated_and_tabs_become_four_spaces() {
        let long = format!("a\tb{}", "x".repeat(40));
        let app = app_with(vec![long, "short".into()], &[], 4);

        let screen = screen(&draw(&app, 20, 4));

        assert_eq!(screen.lines().next().unwrap(), "  1│a    bxxxxxxxxxx");
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        for width in 0..=3 {
            for rows in 0..=3 {
                let mut app =
                    app_with(lines(5), &[open_thread("c1", (1, 2), State::Current)], rows);
                draw(&app, width, rows);
                key(&mut app, KeyCode::Enter);
                draw(&app, width, rows);
                key(&mut app, KeyCode::Char('a'));
                type_text(&mut app, "hello");
                draw(&app, width, rows);
            }
        }
    }

    #[test]
    fn an_empty_file_says_so() {
        let app = app_with(Vec::new(), &[], 4);

        let screen = screen(&draw(&app, W, 4));

        assert_eq!(
            screen,
            "(empty file)\n\n\nplan.md  L0/0  enter:thread/comment ]/[:jump q:quit"
        );
    }

    #[test]
    fn only_the_cursor_line_is_highlighted() {
        let mut app = app_with(lines(5), &[], 8);
        key(&mut app, KeyCode::Char('j'));

        let terminal = draw(&app, W, 8);

        let buffer = terminal.backend().buffer();
        let bg = |y: u16| buffer[(5, y)].bg;
        assert_eq!(bg(1), Color::DarkGray);
        for y in [0, 2, 3, 4] {
            assert_eq!(bg(y), Color::Reset, "row {y}");
        }
    }
}
