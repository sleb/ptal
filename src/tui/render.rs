//! Drawing: a pure function of [`App`] onto a ratatui frame.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Widget};

use super::app::{App, Marker, Mode, ScreenRow};

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
    let digits = app.digits();
    for (screen_row, row) in app.screen_rows().into_iter().zip(area.rows()) {
        let ScreenRow {
            line,
            first,
            mut text,
        } = screen_row;
        let on_cursor = line == app.cursor();
        let style = if on_cursor {
            Style::new().bg(Color::DarkGray)
        } else {
            Style::new()
        };
        if on_cursor {
            // Faint text on the cursor background is hard to read.
            for span in &mut text.spans {
                span.style = span.style.remove_modifier(Modifier::DIM);
            }
        }
        buf.set_style(row, style);
        let number = if first {
            (line + 1).to_string()
        } else {
            String::new()
        };
        let gutter = format!("  {number:>digits$}│");
        let (x, _) = buf.set_stringn(row.x, row.y, gutter, usize::from(row.width), style);
        let room = row.right().saturating_sub(x);
        if room > 0 {
            // Patches the text's styles over the row's, so the cursor `bg`
            // shows under highlighted text.
            buf.set_line(x, row.y, &text, room);
        }
        if let Some(marker) = app.marker(line).filter(|_| first) {
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
        Mode::Panel { .. } if app.selected_is_resolved() => {
            "tab:next a:add r:reply o:reopen j/k:scroll esc:close"
        }
        Mode::Panel { .. } => "tab:next a:add r:reply x:resolve j/k:scroll esc:close",
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

    const W: u16 = 70;

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

    const PANEL_FOOTER: &str =
        "plan.md  L1/12  tab:next a:add r:reply x:resolve j/k:scroll esc:close";

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
    fn a_wide_comment_wraps_in_full_inside_the_panel_border() {
        let wide = "漢".repeat(40); // 80 cells; 64 fit after the indent
        let threads = [talk(
            "c1",
            (1, 1),
            "line 1",
            Status::Open,
            State::Current,
            &[("scott", &wide)],
        )];
        let app = panel_app(&threads);

        let terminal = draw(&app, W, 16);

        let text = screen(&terminal);
        assert_eq!(text.matches('漢').count(), 40, "{text}");
        let buffer = terminal.backend().buffer();
        for y in 8..14 {
            assert_eq!(buffer[(W - 1, y)].symbol(), "│", "row {y}\n{text}");
        }
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
    fn the_panel_footer_offers_resolve_on_open_threads_and_reopen_on_resolved() {
        let open = [talk(
            "c1",
            (1, 1),
            "a",
            Status::Open,
            State::Current,
            &[("claude", "hi")],
        )];
        let done = [talk(
            "c1",
            (1, 1),
            "a",
            Status::Resolved {
                resolved_by: "claude".into(),
                resolved_at: "2026-10-12T10:00:00Z".into(),
            },
            State::Current,
            &[("claude", "hi")],
        )];

        let open = screen(&draw(&panel_app(&open), W, 16));
        let done = screen(&draw(&panel_app(&done), W, 16));

        assert!(
            open.ends_with("a:add r:reply x:resolve j/k:scroll esc:close"),
            "{open}"
        );
        assert!(
            done.ends_with("a:add r:reply o:reopen j/k:scroll esc:close"),
            "{done}"
        );
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

    /// An app on a terminal `cols` wide, so wrapping matches the drawing.
    fn narrow(lines: &[&str], threads: &[ThreadView], (cols, rows): (u16, u16)) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(lines, threads, "scott", (cols, rows)).labeled("plan.md")
    }

    const LONG: &str = "a\tbravo charlie delta echo foxtrot";

    #[test]
    fn long_lines_wrap_with_the_number_on_the_first_row_only() {
        let app = narrow(&[LONG, "short"], &[], (20, 5));

        let screen = screen(&draw(&app, 20, 5));

        assert_eq!(
            screen,
            "  1│a    bravo
   │charlie delta
   │echo foxtrot
  2│short
plan.md  L1/2  enter"
        );
    }

    #[test]
    fn the_cursor_highlight_covers_every_row_of_a_wrapped_line() {
        let mut app = narrow(&["one", LONG, "three"], &[], (20, 6));
        key(&mut app, KeyCode::Char('j'));

        let terminal = draw(&app, 20, 6);

        let buffer = terminal.backend().buffer();
        let bg = |y: u16| buffer[(5, y)].bg;
        for y in 1..=3 {
            assert_eq!(bg(y), Color::DarkGray, "row {y}");
        }
        assert_eq!(bg(0), Color::Reset);
        assert_eq!(bg(4), Color::Reset);
    }

    #[test]
    fn a_thread_glyph_shows_on_a_wrapped_lines_first_row_only() {
        let threads = [open_thread("c1", (1, 1), State::Current)];
        let app = narrow(&[LONG, "short"], &threads, (20, 5));

        let terminal = draw(&app, 20, 5);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "●");
        assert_eq!(buffer[(0, 1)].symbol(), " ");
        assert_eq!(buffer[(0, 2)].symbol(), " ");
    }

    const TABLE: [&str; 5] = [
        "before",
        "| Q | Decision |",
        "|---|---|",
        "| a | b |",
        "after",
    ];

    #[test]
    fn a_table_shows_aligned_in_place_with_a_number_on_every_row() {
        let app = narrow(&TABLE, &[], (40, 6));

        let screen = screen(&draw(&app, 40, 6));

        let file: Vec<&str> = screen.lines().take(5).collect();
        assert_eq!(
            file,
            [
                "  1│before",
                "  2│ Q │ Decision",
                "  3│───┼──────────",
                "  4│ a │ b",
                "  5│after",
            ]
        );
    }

    #[test]
    fn a_table_rows_glyph_and_the_cursor_highlight_cover_that_row_alone() {
        let threads = [open_thread("c1", (4, 4), State::Current)];
        let mut app = narrow(&TABLE, &threads, (40, 6));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));

        let terminal = draw(&app, 40, 6);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 3)].symbol(), "●");
        for y in [0, 1, 2, 4] {
            assert_eq!(buffer[(0, y)].symbol(), " ", "row {y}");
        }
        let bg = |y: u16| buffer[(5, y)].bg;
        assert_eq!(bg(2), Color::DarkGray);
        for y in [0, 1, 3, 4] {
            assert_eq!(bg(y), Color::Reset, "row {y}");
        }
    }

    /// At 30 columns the text width is 26 and the long cell wraps in 3 rows.
    const WIDE_TABLE: [&str; 5] = [
        "before",
        "| Q | Decision |",
        "|---|---|",
        "| 1 | use the water filling rule for widths |",
        "after",
    ];

    #[test]
    fn a_wide_table_wraps_its_cells_in_their_columns_numbered_once_per_line() {
        let app = narrow(&WIDE_TABLE, &[], (30, 8));

        let screen = screen(&draw(&app, 30, 8));

        assert_eq!(
            screen,
            "  1│before
  2│ Q │ Decision
  3│───┼──────────────────────
  4│ 1 │ use the water
   │   │ filling rule for
   │   │ widths
  5│after
plan.md  L1/5  enter:thread/co"
        );
    }

    #[test]
    fn a_wrapped_table_lines_glyph_is_on_its_first_row_and_its_highlight_on_all() {
        let threads = [open_thread("c1", (4, 4), State::Current)];
        let mut app = narrow(&WIDE_TABLE, &threads, (30, 8));
        for _ in 0..3 {
            key(&mut app, KeyCode::Char('j'));
        }

        let terminal = draw(&app, 30, 8);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 3)].symbol(), "●");
        for y in [0, 1, 2, 4, 5, 6] {
            assert_eq!(buffer[(0, y)].symbol(), " ", "row {y}");
        }
        let bg = |y: u16| buffer[(5, y)].bg;
        for y in 3..=5 {
            assert_eq!(bg(y), Color::DarkGray, "row {y}");
        }
        for y in [0, 1, 2, 6] {
            assert_eq!(bg(y), Color::Reset, "row {y}");
        }
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

    fn highlighted(lines: &[&str]) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(lines, &[], "scott", (W, 6)).highlighted(true)
    }

    #[test]
    fn a_highlighted_heading_is_bold_magenta_in_its_text_and_not_its_gutter() {
        let app = highlighted(&["x", "# Title"]); // cursor on line 1

        let terminal = draw(&app, W, 6);

        let buffer = terminal.backend().buffer();
        assert_eq!(screen(&terminal).lines().nth(1), Some("  2│# Title"));
        for x in 4..11 {
            assert_eq!(buffer[(x, 1)].fg, Color::Magenta, "x {x}");
            assert!(buffer[(x, 1)].modifier.contains(Modifier::BOLD), "x {x}");
        }
        for x in 0..4 {
            assert_eq!(buffer[(x, 1)].fg, Color::Reset, "gutter x {x}");
            assert!(
                !buffer[(x, 1)].modifier.contains(Modifier::BOLD),
                "gutter x {x}"
            );
        }
    }

    #[test]
    fn the_cursor_highlight_shows_under_highlighted_text() {
        let app = highlighted(&["# Title", "x"]);

        let terminal = draw(&app, W, 6);

        let cell = &terminal.backend().buffer()[(4, 0)];
        assert_eq!((cell.fg, cell.bg), (Color::Magenta, Color::DarkGray));
        assert_eq!(terminal.backend().buffer()[(20, 0)].bg, Color::DarkGray);
    }

    #[test]
    fn a_fenced_block_has_dim_fences_and_a_cyan_body() {
        let app = highlighted(&["x", "```", "code", "```"]); // cursor on "x"

        let terminal = draw(&app, W, 6);

        let buffer = terminal.backend().buffer();
        for y in [1, 3] {
            assert!(buffer[(4, y)].modifier.contains(Modifier::DIM), "fence {y}");
        }
        assert_eq!(buffer[(4, 2)].fg, Color::Cyan);
        assert_eq!(buffer[(7, 2)].fg, Color::Cyan);
    }

    #[test]
    fn nothing_on_the_cursor_row_is_dim_so_it_stays_legible() {
        let app = highlighted(&["```", "code", "```", "> quote"]);

        let terminal = draw(&app, W, 6);

        let buffer = terminal.backend().buffer();
        let cell = &buffer[(4, 0)];
        assert!(!cell.modifier.contains(Modifier::DIM));
        assert_eq!(cell.bg, Color::DarkGray);
        assert!(
            buffer[(4, 2)].modifier.contains(Modifier::DIM),
            "off the cursor row"
        );
        assert!(
            buffer[(4, 3)].modifier.contains(Modifier::DIM),
            "quote marker"
        );
    }
}
