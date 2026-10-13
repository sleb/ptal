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
        Mode::Normal | Mode::Panel { .. } => draw_footer(app, buf, bottom),
        Mode::Input { draft, cursor } => draw_input(app, draft, *cursor, buf, bottom),
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
    parts.push("enter:comment q:quit".to_string());
    buf.set_stringn(
        area.x,
        area.y,
        parts.join("  "),
        usize::from(area.width),
        Style::new(),
    );
}

fn draw_input(app: &App, draft: &str, cursor: usize, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let title = format!("Comment on line {} as {}", app.cursor() + 1, app.author());
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

    const W: u16 = 52;

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
plan.md  L1/12  enter:comment q:quit"
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
plan.md  L1/12  1 outdated  enter:comment q:quit"
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
        assert!(screen.ends_with("L30/30  enter:comment q:quit"));
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
            "(empty file)\n\n\nplan.md  L0/0  enter:comment q:quit"
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
