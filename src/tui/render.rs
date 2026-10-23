//! Drawing: a pure function of [`App`] onto a ratatui frame.

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Widget};

use super::app::{App, BANNER_ROWS, Marker, Mode, OVERLAY_CHROME_ROWS, Overlay, ScreenRow, Target};
use super::draft::Draft;
use super::wrap;
use crate::anchor::ChangeKind;
use crate::model::Verdict;

/// Draw `app` onto `frame`. Reads state only; never mutates it.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    if area.is_empty() {
        return;
    }
    let buf = frame.buffer_mut();
    if let Mode::Overlay { kind, scroll, .. } = app.mode() {
        draw_overlay(app, *kind, *scroll, buf, area);
        return;
    }
    let banner = Rect {
        height: u16::try_from(BANNER_ROWS).unwrap_or(1).min(area.height),
        ..area
    };
    let below = Rect {
        y: area.y + banner.height,
        height: area.height - banner.height,
        ..area
    };
    let chrome = u16::try_from(app.chrome_rows())
        .unwrap_or(u16::MAX)
        .min(below.height);
    let view = Rect {
        height: below.height - chrome,
        ..below
    };
    let bottom = Rect {
        y: below.y + view.height,
        height: chrome,
        ..below
    };

    draw_banner(app, buf, banner);
    draw_file(app, buf, view);
    match app.mode() {
        Mode::Normal | Mode::Visual { .. } => draw_footer(app, buf, bottom),
        Mode::Search { query, .. } => draw_prompt(app, query, buf, bottom),
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
        Mode::Overlay { .. } => {}
        Mode::Input {
            draft,
            discard_asked,
            target,
            ..
        } => {
            let border = box_border(app, *discard_asked, target, bottom.width);
            draw_input(app, draft, border, target, buf, bottom);
        }
    }
}

fn draw_file(app: &App, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    if app.lines().is_empty() {
        let width = usize::from(area.width);
        let empty = wrap::cut("(empty file)", width);
        buf.set_stringn(area.x, area.y, empty, width, Style::new());
        return;
    }
    let digits = app.digits();
    let (first_selected, last_selected) = app.selection();
    for (screen_row, row) in app.screen_rows().into_iter().zip(area.rows()) {
        let ScreenRow {
            line,
            first,
            mut text,
        } = screen_row;
        let selected = (first_selected..=last_selected).contains(&line);
        let style = if selected {
            Style::new().bg(Color::DarkGray)
        } else {
            Style::new()
        };
        if selected {
            // Faint text on the selection background is hard to read.
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
        if let Some(kind) = app.change(line).filter(|_| first) {
            let (glyph, color) = change_glyph(kind);
            buf.set_stringn(row.x + 1, row.y, glyph, 1, style.patch(color));
        }
    }
}

/// The gutter's second column for a line changed since the human's last
/// look. Like [`glyph`], the shape differs, not just the color.
fn change_glyph(kind: ChangeKind) -> (&'static str, Style) {
    match kind {
        ChangeKind::Changed => ("┃", Style::new().fg(Color::Green)),
        ChangeKind::Removed => ("-", Style::new().fg(Color::Red)),
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

/// Room the banner's text keeps beside the key hint before the hint is
/// dropped.
const MIN_BANNER_TEXT: usize = 12;

/// The review banner: its text from the left, [`wrap::cut`] to leave room
/// for the key hint flush right; on a row too narrow for both, the text
/// alone.
fn draw_banner(app: &App, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let (text, hint) = app.banner();
    let text = one_line(&text);
    let hint_width = wrap::width(hint);
    let room = width.saturating_sub(hint_width + 1);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    if room < MIN_BANNER_TEXT {
        buf.set_stringn(area.x, area.y, wrap::cut(&text, width), width, bold);
        return;
    }
    buf.set_stringn(area.x, area.y, wrap::cut(&text, room), room, bold);
    let x = area.right() - u16::try_from(hint_width).unwrap_or(area.width);
    buf.set_stringn(x, area.y, hint, hint_width, Style::new());
}

/// The search prompt on the footer row: `/query` and a note, cut to show
/// the tail of a query too wide for the row.
fn draw_prompt(app: &App, query: &str, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    // The query has first claim on the row; the note shows only if it fits
    // whole beside it.
    let text = wrap::tail(&format!("/{query}"), width);
    let left = width.saturating_sub(wrap::width(&text));
    let note = app.search_note();
    let text = if wrap::width(note) <= left {
        text + note
    } else {
        text
    };
    buf.set_stringn(area.x, area.y, text, width, Style::new());
}

fn draw_footer(app: &App, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let mut parts = Vec::new();
    let position = if app.lines().is_empty() {
        0
    } else {
        app.cursor() + 1
    };
    parts.push(format!("L{position}/{}", app.lines().len()));
    if let Mode::Visual { .. } = app.mode() {
        let (first, last) = app.selection();
        parts.push(format!("visual {}-{}", first + 1, last + 1));
    }
    if app.outdated() > 0 {
        parts.push(format!("{} outdated", app.outdated()));
    }
    match app.changes() {
        0 => {}
        1 => parts.push("1 change".to_string()),
        n => parts.push(format!("{n} changes")),
    }
    let hints = match app.mode() {
        Mode::Panel { .. } if app.selected_is_resolved() => {
            "tab:next a:add r:reply o:reopen j/k:scroll esc:close ?:help"
        }
        Mode::Panel { .. } => "tab:next a:add r:reply x:resolve j/k:scroll esc:close ?:help",
        Mode::Visual { .. } => "esc:cancel enter:comment j/k:extend",
        _ => "enter:thread/comment ]/[:jump q:quit v:select /:search ?:help",
    };
    let counts = parts.join("  ");
    // The search and the hints are cut first, before the path gives way.
    let rest = match app.active_search() {
        Some(query) => format!("/{query}  {hints}"),
        None => hints.to_string(),
    };
    let width = usize::from(area.width);
    let footer = match app.status() {
        Some(status) => format!("{}  {counts}  {rest}", one_line(status)),
        None => footer_row(app.file(), &counts, &rest, width),
    };
    let footer = wrap::cut(&footer, width);
    buf.set_stringn(area.x, area.y, footer, width, Style::new());
}

/// `path`, `counts` and `rest` (the search and the hints) on a footer row
/// of `width` cells, for the caller to [`wrap::cut`] at the end. While
/// `path` and `counts` fit, the row is all three, so only `rest` is cut.
/// Otherwise `rest` is dropped and the path gives way: it keeps its end
/// after an ellipsis ([`wrap::tail`]), down to the ellipsis alone, so the
/// counts show.
fn footer_row(path: &str, counts: &str, rest: &str, width: usize) -> String {
    let lead = format!("{path}  {counts}");
    if wrap::width(&lead) <= width {
        return format!("{lead}  {rest}");
    }
    let room = width.saturating_sub(wrap::width(counts) + 2);
    format!("{}  {counts}", wrap::tail(path, room.max(1)))
}

/// Overlay `kind` over the whole screen: a title row, its listing from
/// `scroll`, and a footer row saying how to scroll and close.
fn draw_overlay(app: &App, kind: Overlay, scroll: usize, buf: &mut Buffer, area: Rect) {
    let width = usize::from(area.width);
    let mut rows = area.rows();
    if let Some(row) = rows.next() {
        let title = Style::new().add_modifier(Modifier::BOLD);
        buf.set_stringn(row.x, row.y, kind.title(), width, title);
    }
    let listing = app.overlay_rows(kind);
    let body = usize::from(area.height).saturating_sub(OVERLAY_CHROME_ROWS);
    for (row, line) in rows.by_ref().take(body).zip(listing.iter().skip(scroll)) {
        let style = if line.heading {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        buf.set_stringn(row.x, row.y, &line.text, width, style);
    }
    if area.height >= 2 {
        let y = area.y + area.height - 1;
        let hint = match kind {
            Overlay::Help => "j/k g/G:scroll  ?/esc:close",
            Overlay::Review => "j/k g/G:scroll  i/esc:close",
        };
        let hint = wrap::cut(hint, width);
        buf.set_stringn(area.x, y, hint, width, Style::new());
    }
}

fn draw_panel(app: &App, scroll: usize, buf: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let title = format!("Threads on line {}", app.cursor() + 1);
    let block = Block::bordered().title(fit_title(&title, area.width));
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

/// A box `width` cells wide's top-border `title`, [`wrap::cut`] to fit
/// between the corners.
fn fit_title(title: &str, width: u16) -> Cow<'_, str> {
    wrap::cut(title, border_room(width))
}

/// The cells between the corners of a box `width` cells wide: the room for
/// its border titles, top and bottom.
fn border_room(width: u16) -> usize {
    usize::from(width.saturating_sub(2))
}

/// The key hint on the comment box's bottom border.
const INPUT_HINT: &str = "ctrl+s:save enter:newline esc:cancel";
/// [`INPUT_HINT`] for a box too narrow for it: how to save and cancel.
const INPUT_HINT_SHORT: &str = "ctrl+s:save esc:cancel";
/// [`INPUT_HINT`] in a hand-off box, and its short form.
const HAND_OFF_HINTS: [&str; 2] = [
    "ctrl+s:hand off enter:newline esc:cancel",
    "ctrl+s:hand off esc:cancel",
];
/// [`INPUT_HINT`] in a close box, and its short form.
const CLOSE_HINTS: [&str; 2] = [
    "tab:verdict ctrl+s:close esc:cancel",
    "tab:verdict ctrl+s:close",
];
/// The bottom border while an `esc` waits to discard a draft.
const DISCARD_HINT: &str = "esc:discard the draft  any other key:keep it";
/// [`DISCARD_HINT`] for a box too narrow for it.
const DISCARD_HINT_SHORT: &str = "esc:discard other:keep";
/// The discard warning for a box too narrow even for [`DISCARD_HINT_SHORT`].
const DISCARD_HINT_TINY: &str = "esc:discard?";

/// The text on the comment box's bottom border and its style, for a box
/// writing `target` `width` cells wide. A pending [`App::status`] (a failed
/// save's error) wins, red and bold, unless [`one_line`] leaves it empty;
/// then the discard prompt while `discard_asked`; then, for a close asking
/// to confirm, how many open threads stay open; else the key hint for
/// `target`. Hints pick the longest form that
/// fits inside the corners, falling back to the shortest. The error is put on
/// one row by [`one_line`]. Whatever is chosen is [`wrap::cut`] to fit
/// between the corners, so text that does not fit ends in `…`.
fn box_border(
    app: &App,
    discard_asked: bool,
    target: &Target,
    width: u16,
) -> (Cow<'static, str>, Style) {
    let room = border_room(width);
    let error = app.status().map(one_line).filter(|error| !error.is_empty());
    if let Some(error) = error {
        // `one_line` leaves no tabs or control characters, so an error that
        // fits comes back borrowed and is kept as is.
        let cut = match wrap::cut(&error, room) {
            Cow::Owned(cut) => Some(cut),
            Cow::Borrowed(_) => None,
        };
        return (
            Cow::Owned(cut.unwrap_or(error)),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
    }
    if !discard_asked
        && let Target::Close {
            confirm_asked: true,
            ..
        } = target
    {
        let ask = match app.open_threads() {
            1 => "1 open thread stays open: ctrl+s again to approve".to_string(),
            n => format!("{n} open threads stay open: ctrl+s again to approve"),
        };
        let style = Style::new().add_modifier(Modifier::BOLD);
        return (Cow::Owned(wrap::cut(&ask, room).into_owned()), style);
    }
    let hints: &[&str] = match target {
        _ if discard_asked => &[DISCARD_HINT, DISCARD_HINT_SHORT, DISCARD_HINT_TINY],
        Target::NewThread | Target::Reply(_) => &[INPUT_HINT, INPUT_HINT_SHORT],
        Target::HandOff => &HAND_OFF_HINTS,
        Target::Close { .. } => &CLOSE_HINTS,
    };
    let hint = hints
        .iter()
        .find(|hint| wrap::width(hint) <= room)
        .or(hints.last())
        .copied()
        .unwrap_or_default();
    (wrap::cut(hint, room), Style::new())
}

/// `text` on one row: each run of whitespace and control characters (a
/// `\r\n`, a blank line, a tab) becomes one space, and leading and trailing
/// ones go. Otherwise ratatui drops the control characters and a multi-line
/// error's words run together, and every blank cell pushes the message's
/// tail closer to the corner.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The comment box: the draft's rows from its scroll, wrapped at
/// [`App::draft_width`] exactly as `update` counts them (a tab drawn as four
/// spaces, the cells the cursor counts it as), and the cursor as a
/// reversed cell, with `border` (text and style, from [`box_border`]) on the
/// bottom border.
fn draw_input(
    app: &App,
    draft: &Draft,
    (hint, hint_style): (Cow<'_, str>, Style),
    target: &Target,
    buf: &mut Buffer,
    area: Rect,
) {
    if area.is_empty() {
        return;
    }
    let title = match target {
        Target::Reply(id) => format!("Reply to {id} as {}", app.author()),
        Target::HandOff => "Hand off to the agent (note optional)".to_string(),
        Target::Close { verdict, .. } => {
            let verdict = verdict.map_or("?", Verdict::as_str);
            format!("Close as {verdict} (tab: approved / dropped)")
        }
        Target::NewThread => match app.selection() {
            (first, last) if first == last => {
                format!("Comment on line {} as {}", first + 1, app.author())
            }
            (first, last) => format!(
                "Comment on lines {}-{} as {}",
                first + 1,
                last + 1,
                app.author()
            ),
        },
    };
    let block = Block::bordered()
        .title(fit_title(&title, area.width))
        .title_bottom(Line::styled(hint, hint_style));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.is_empty() {
        return;
    }
    let width = app.draft_width();
    let rows = draft.rows(width);
    for (y, row) in (inner.top()..inner.bottom()).zip(rows.into_iter().skip(draft.scroll())) {
        let text = wrap::expand_tabs(&draft.text()[row]);
        buf.set_stringn(inner.x, y, text, usize::from(inner.width), Style::new());
    }
    let (row, col) = draft.cursor_at(width);
    let y = row
        .checked_sub(draft.scroll())
        .and_then(|row| u16::try_from(row).ok())
        .map(|row| inner.y.saturating_add(row));
    let x = u16::try_from(col).map(|col| inner.x.saturating_add(col));
    if let (Some(y), Ok(x)) = (y, x)
        && y < inner.bottom()
        && x < inner.right()
    {
        buf[(x, y)].set_style(Style::new().add_modifier(Modifier::REVERSED));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ReviewState, Side, State, Status, Verdict};
    use crate::review::{ReviewStatus, ThreadView, View};
    use crate::tui::app::{Event, update};
    use crate::tui::keys;
    use crate::tui::test_support::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};

    const W: u16 = 80;
    /// The screen row of the file view's first row, below the banner.
    const TOP: u16 = 1;

    fn app_with(lines: Vec<String>, threads: &[ThreadView], rows: u16) -> App {
        App::open(showing(lines, threads), "scott", (W, rows)).labeled("plan.md")
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
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `left`, then `right` flush against the right edge of `width` cells.
    fn spread(left: &str, right: &str, width: usize) -> String {
        let pad = width - left.chars().count() - right.chars().count();
        format!("{left}{}{right}", " ".repeat(pad))
    }

    /// A fresh review's banner, the top row, `width` cells wide.
    fn banner_row(width: u16) -> String {
        spread("your turn", "H:hand off C:close", usize::from(width))
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

    /// The thread panel opened on line 1 of twelve lines, at 80×16.
    fn opened_panel(threads: &[ThreadView]) -> App {
        let mut app = app_with(numbered(12), threads, 16);
        send(&mut app, [press(KeyCode::Enter)]);
        app
    }

    const PANEL_FOOTER: &str =
        "plan.md  L1/12  tab:next a:add r:reply x:resolve j/k:scroll esc:close ?:help";

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
        let app = opened_panel(&threads);

        let screen = screen(&draw(&app, W, 16));

        let expected = [
            &banner_row(W),
            "◆  1│line 1",
            "   2│line 2",
            "   3│line 3",
            "   4│line 4",
            "   5│line 5",
            "   6│line 6",
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
        let mut app = opened_panel(&threads);

        let first = screen(&draw(&app, W, 16));
        assert!(first.contains("│  c1 open"), "{first}");
        assert!(
            first.contains("│▶ c2 open"),
            "waiting thread starts selected"
        );

        send(&mut app, [press(KeyCode::Tab)]);
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
            resolved(),
            State::Current,
            &[("scott", "hm")],
        )];
        let app = opened_panel(&threads);

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
        let app = opened_panel(&threads);

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
        let mut app = opened_panel(&threads);
        typed(&mut app, "jj");

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
        let app = opened_panel(&threads);

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
        let mut app = opened_panel(&threads);
        send(&mut app, [ch('a')]);
        typed(&mut app, "ok");

        let text = screen(&draw(&app, W, 16));

        assert!(text.contains("┌Comment on line 1 as scott"), "{text}");
        assert!(text.contains("│ok"));
        assert!(!text.contains("Threads on line"));
        assert_eq!(text.lines().nth(1), Some("◆  1│line 1"), "{text}");
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
                resolved_revision: None,
            },
            State::Current,
            &[("claude", "hi")],
        )];

        let open = screen(&draw(&opened_panel(&open), W, 16));
        let done = screen(&draw(&opened_panel(&done), W, 16));

        assert!(
            open.ends_with("a:add r:reply x:resolve j/k:scroll esc:close ?:help"),
            "{open}"
        );
        assert!(
            done.ends_with("a:add r:reply o:reopen j/k:scroll esc:close ?:help"),
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
        let mut app = opened_panel(&threads);
        send(&mut app, [ch('r')]);
        typed(&mut app, "ok");

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
        let mut app = app_with(numbered(12), &threads, 16);
        typed(&mut app, "jj");
        send(&mut app, [press(KeyCode::Enter)]);

        let terminal = draw(&app, W, 16);

        let buffer = terminal.backend().buffer();
        // Line 3 is row 3, below the banner.
        assert_eq!(buffer[(6, 3)].bg, Color::DarkGray);
        assert_eq!(buffer[(6, 2)].bg, Color::Reset);
    }

    #[test]
    fn normal_mode_shows_gutter_text_and_a_footer() {
        let app = app_with(numbered(12), &[], 8);

        let screen = screen(&draw(&app, W, 8));

        assert_eq!(
            screen,
            format!(
                "{}
   1│line 1
   2│line 2
   3│line 3
   4│line 4
   5│line 5
   6│line 6
plan.md  L1/12  enter:thread/comment ]/[:jump q:quit v:select /:search ?:help",
                banner_row(W)
            )
        );
    }

    #[test]
    fn marked_lines_show_a_dot_and_outdated_threads_are_counted() {
        let threads = [
            view("c1", (2, 3), Status::Open, State::Current),
            view("c2", (6, 6), Status::Open, State::Outdated),
        ];
        let app = app_with(numbered(12), &threads, 8);

        let screen = screen(&draw(&app, W, 8));

        assert_eq!(
            screen,
            format!(
                "{}
   1│line 1
●  2│line 2
●  3│line 3
   4│line 4
   5│line 5
●  6│line 6
plan.md  L1/12  1 outdated  enter:thread/comment ]/[:jump q:quit v:select /:sea…",
                banner_row(W)
            )
        );
    }

    use crate::anchor::ChangeKind;

    #[test]
    fn changed_lines_and_removal_points_are_marked_and_counted() {
        let threads = [
            view("c1", (2, 2), Status::Open, State::Current),
            view("c2", (5, 5), Status::Open, State::Outdated),
        ];
        let app = App::open(
            View {
                changes: vec![
                    change(2, 3, ChangeKind::Changed),
                    change(5, 5, ChangeKind::Removed),
                ],
                ..showing(numbered(12), &threads)
            },
            "scott",
            (W, 8),
        )
        .labeled("plan.md");

        let screen = screen(&draw(&app, W, 8));

        assert_eq!(
            screen,
            format!(
                "{}
   1│line 1
●┃ 2│line 2
 ┃ 3│line 3
   4│line 4
●- 5│line 5
   6│line 6
plan.md  L1/12  1 outdated  2 changes  enter:thread/comment ]/[:jump q:quit v:s…",
                banner_row(W)
            )
        );
    }

    #[test]
    fn change_glyphs_differ_in_color_and_keep_the_cursor_background() {
        let app = App::open(
            View {
                changes: vec![
                    change(1, 1, ChangeKind::Changed),
                    change(3, 3, ChangeKind::Removed),
                ],
                ..showing(numbered(4), &[])
            },
            "scott",
            (W, 8),
        )
        .labeled("plan.md");

        let terminal = draw(&app, W, 8);

        let buffer = terminal.backend().buffer();
        let cell = |n: u16| &buffer[(1, n + 1)];
        assert_eq!(cell(0).fg, Color::Green);
        assert_eq!(cell(0).bg, Color::DarkGray, "the cursor line");
        assert_eq!(cell(2).symbol(), "-");
        assert_eq!(cell(2).fg, Color::Red);
        assert_eq!(cell(2).bg, Color::Reset);
    }

    #[test]
    fn a_wrapped_changed_line_is_marked_on_its_first_row_only() {
        let lines = vec!["aaaa bbbb cccc dddd".to_string(), "x".to_string()];
        let app = App::open(
            View {
                changes: vec![change(1, 1, ChangeKind::Changed)],
                ..showing(lines, &[])
            },
            "scott",
            (14, 6),
        )
        .labeled("plan.md");

        let screen = screen(&draw(&app, 14, 6));

        let rows: Vec<&str> = screen.lines().skip(1).take(3).collect();
        assert_eq!(rows, [" ┃1│aaaa bbbb", "   │cccc dddd", "  2│x"]);
    }

    #[test]
    fn one_change_is_counted_in_the_singular() {
        let app = App::open(
            View {
                changes: vec![change(2, 2, ChangeKind::Changed)],
                ..showing(numbered(3), &[])
            },
            "scott",
            (W, 8),
        )
        .labeled("plan.md");

        let screen = screen(&draw(&app, W, 8));

        assert!(screen.contains("L1/3  1 change  enter:"), "{screen}");
    }

    #[test]
    fn change_glyphs_on_a_terminal_too_narrow_for_them_do_not_panic() {
        for width in [1, 2] {
            let app = App::open(
                View {
                    changes: vec![change(1, 1, ChangeKind::Changed)],
                    ..showing(
                        numbered(3),
                        &[view("c1", (1, 1), Status::Open, State::Current)],
                    )
                },
                "scott",
                (width, 6),
            );

            let screen = screen(&draw(&app, width, 6));

            assert_eq!(
                screen.lines().nth(1),
                Some(if width == 1 { "●" } else { "●┃" })
            );
        }
    }

    #[test]
    fn without_changes_the_footer_has_no_change_count() {
        let app = app_with(numbered(3), &[], 8);

        let screen = screen(&draw(&app, W, 8));

        assert!(!screen.contains("change"), "{screen}");
    }

    #[test]
    fn gutter_glyph_and_style_follow_the_marker() {
        let threads = [
            said_by(view("c1", (1, 1), Status::Open, State::Current), &["scott"]),
            said_by(
                view("c2", (2, 2), Status::Open, State::Current),
                &["claude"],
            ),
            said_by(view("c3", (3, 3), resolved(), State::Current), &["claude"]),
        ];
        let app = app_with(numbered(4), &threads, 8);

        let terminal = draw(&app, W, 8);

        let buffer = terminal.backend().buffer();
        // Line `n` (0-based) is row `n + 1`, below the banner.
        let cell = |n: u16| &buffer[(0, n + 1)];
        assert_eq!(cell(0).symbol(), "●");
        assert_eq!(cell(1).symbol(), "◆");
        assert_eq!(cell(1).fg, Color::Yellow);
        assert_eq!(cell(2).symbol(), "○");
        assert!(cell(2).modifier.contains(Modifier::DIM));
        assert_eq!(cell(3).symbol(), " ");
    }

    #[test]
    fn input_mode_draws_a_titled_box_over_the_footer_with_the_file_above() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "jj");
        send(&mut app, [press(KeyCode::Enter)]);
        typed(&mut app, "why?");

        let screen = screen(&draw(&app, W, 8));

        let title = "Comment on line 3 as scott";
        let top = format!("┌{title}{}┐", "─".repeat(usize::from(W) - 2 - title.len()));
        let bottom = format!(
            "└{INPUT_HINT}{}┘",
            "─".repeat(usize::from(W) - 2 - INPUT_HINT.len())
        );
        assert_eq!(
            screen,
            format!(
                "{}\n   1│line 1\n   2│line 2\n   3│line 3\n   4│line 4\n{top}\n│why?{}│\n{bottom}",
                banner_row(W),
                " ".repeat(usize::from(W) - 2 - 4)
            )
        );
    }

    #[test]
    fn input_cursor_is_a_reversed_cell_mid_draft_and_at_the_end() {
        let mut app = app_with(numbered(3), &[], 8);
        send(&mut app, [press(KeyCode::Enter)]);
        typed(&mut app, "abc");
        let reversed = |terminal: &Terminal<TestBackend>, x: u16| {
            // 8 rows: the box fills rows 5..8, its text row is 6.
            terminal.backend().buffer()[(x, 6)]
                .modifier
                .contains(Modifier::REVERSED)
        };

        let at_end = draw(&app, W, 8);
        assert!(reversed(&at_end, 4), "the space after `abc`");
        assert!(!reversed(&at_end, 3));

        send(&mut app, [press(KeyCode::Left)]);
        send(&mut app, [press(KeyCode::Left)]);
        let mid = draw(&app, W, 8);
        assert!(reversed(&mid, 2), "on `b`");
        assert!(!reversed(&mid, 4));
        assert_eq!(mid.backend().buffer()[(2, 6)].symbol(), "b");
    }

    /// The last `n` rows of `terminal`'s screen: the comment box.
    fn box_rows(terminal: &Terminal<TestBackend>, n: usize) -> Vec<String> {
        let screen = screen(terminal);
        let rows: Vec<String> = screen.lines().map(str::to_string).collect();
        rows[rows.len() - n..].to_vec()
    }

    /// Whether cell (`x`, `y`) of `terminal` is drawn reversed.
    fn reversed_at(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> bool {
        terminal.backend().buffer()[(x, y)]
            .modifier
            .contains(Modifier::REVERSED)
    }

    /// A box on line 1 of a `cols` x `rows` terminal holding `text`, each
    /// `\n` typed as `enter`.
    fn box_with(text: &str, (cols, rows): (u16, u16)) -> App {
        let mut app = narrow(&["one", "two"], &[], (cols, rows));
        send(&mut app, [press(KeyCode::Enter)]);
        send(
            &mut app,
            text.chars().map(|c| {
                if c == '\n' {
                    press(KeyCode::Enter)
                } else {
                    ch(c)
                }
            }),
        );
        app
    }

    #[test]
    fn a_long_draft_wraps_inside_the_box_with_the_cursor_after_it() {
        let app = box_with("the quick brown fox jumps over", (20, 10));

        let terminal = draw(&app, 20, 10);

        assert_eq!(
            box_rows(&terminal, 4)[1..3],
            ["│the quick brown   │", "│fox jumps over    │"]
        );
        assert!(reversed_at(&terminal, 15, 8), "the cell after `over`");
        assert!(!reversed_at(&terminal, 14, 8));
    }

    #[test]
    fn a_full_row_still_has_a_cell_for_the_cursor() {
        let app = box_with("abcdefghijklmnopq", (20, 10));

        let terminal = draw(&app, 20, 10);

        assert_eq!(box_rows(&terminal, 3)[1], "│abcdefghijklmnopq │");
        assert!(reversed_at(&terminal, 18, 8));
    }

    #[test]
    fn a_pasted_tab_draws_as_four_spaces_with_the_cursor_after_the_text() {
        let mut app = box_with("", (20, 10));
        send(&mut app, [paste("\tab")]);

        let terminal = draw(&app, 20, 10);

        assert_eq!(box_rows(&terminal, 3)[1], "│    ab            │");
        assert!(reversed_at(&terminal, 7, 8), "the cell after `b`");
        assert!(!reversed_at(&terminal, 6, 8));
    }

    #[test]
    fn a_combining_mark_after_a_tab_draws_on_it_with_the_cursor_after_the_text() {
        let mut app = box_with("", (20, 10));
        send(&mut app, [paste("\t\u{301}x")]);

        let terminal = draw(&app, 20, 10);

        assert_eq!(
            box_rows(&terminal, 3)[1],
            "│   \u{20}\u{301}x             │"
        );
        assert!(reversed_at(&terminal, 6, 8), "the cell after `x`");
    }

    #[test]
    fn a_tab_indented_line_that_wraps_continues_at_the_left_edge() {
        let mut app = box_with("", (20, 10));
        send(&mut app, [paste("\taaaa bbbbbbbbbbb")]);

        let terminal = draw(&app, 20, 10);

        assert_eq!(
            box_rows(&terminal, 4)[1..3],
            ["│    aaaa          │", "│bbbbbbbbbbb       │"]
        );
    }

    #[test]
    fn blank_lines_in_the_draft_draw_as_blank_rows() {
        let app = box_with("one\n\ntwo", (20, 10));

        let terminal = draw(&app, 20, 10);

        assert_eq!(
            box_rows(&terminal, 5)[1..4],
            [
                "│one               │",
                "│                  │",
                "│two               │"
            ]
        );
    }

    #[test]
    fn a_draft_past_the_cap_shows_the_rows_around_the_cursor() {
        let text: Vec<String> = (1..=8).map(|i| format!("row {i}")).collect();
        // 16 rows: at most 16 / 2 - 2 = 6 text rows.
        let mut app = box_with(&text.join("\n"), (20, 16));

        let bottom = draw(&app, 20, 16);
        assert_eq!(
            box_rows(&bottom, 8)[1..7],
            text[2..8]
                .iter()
                .map(|r| format!("│{r:<18}│"))
                .collect::<Vec<_>>()
        );
        assert!(reversed_at(&bottom, 6, 14), "after `row 8`");

        for _ in 0..7 {
            send(&mut app, [press(KeyCode::Up)]);
        }
        let top = draw(&app, 20, 16);
        assert_eq!(box_rows(&top, 8)[1], "│row 1             │");
        assert!(
            reversed_at(&top, 6, 9),
            "after `row 1`, on the first text row"
        );
    }

    #[test]
    fn the_bottom_border_says_how_to_save_and_titles_are_unchanged() {
        let app = box_with("hi", (W, 8));

        let terminal = draw(&app, W, 8);

        let rows = box_rows(&terminal, 3);
        assert!(
            rows[0].starts_with("┌Comment on line 1 as scott─"),
            "{}",
            rows[0]
        );
        assert!(
            rows[2].starts_with("└ctrl+s:save enter:newline esc:cancel─"),
            "{}",
            rows[2]
        );
    }

    #[test]
    fn a_narrow_box_keeps_a_short_hint_that_still_says_save_and_cancel() {
        let app = box_with("hi", (30, 8));

        let rows = box_rows(&draw(&app, 30, 8), 3);

        assert_eq!(
            rows[2],
            format!("└ctrl+s:save esc:cancel{}┘", "─".repeat(6))
        );
    }

    #[test]
    fn the_border_asks_before_esc_discards_a_draft() {
        let mut app = box_with("hi", (W, 8));
        send(&mut app, [press(KeyCode::Esc)]);

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert!(rows[1].starts_with("│hi"), "the draft is still shown");
        assert!(
            rows[2].starts_with("└esc:discard the draft  any other key:keep it─"),
            "{}",
            rows[2]
        );
    }

    #[test]
    fn a_narrow_box_asks_to_discard_in_a_short_hint() {
        let mut app = box_with("hi", (30, 8));
        send(&mut app, [press(KeyCode::Esc)]);

        let rows = box_rows(&draw(&app, 30, 8), 3);

        assert_eq!(
            rows[2],
            format!("└esc:discard other:keep{}┘", "─".repeat(6))
        );
    }

    #[test]
    fn a_very_narrow_box_still_warns_before_discarding() {
        let mut app = box_with("hi", (20, 8));
        send(&mut app, [press(KeyCode::Esc)]);

        let rows = box_rows(&draw(&app, 20, 8), 3);

        assert_eq!(rows[2], format!("└esc:discard?{}┘", "─".repeat(6)));
    }

    const LOCKED: &str = "review is locked by another process";

    /// The error message's cells on the last row are red and bold, and the
    /// corner before it and the border after it are not. A wide character's
    /// trailing cell is skipped: ratatui resets it and the terminal never
    /// paints it.
    fn assert_error_styled(terminal: &Terminal<TestBackend>, message: &str) {
        let buffer = terminal.backend().buffer();
        let y = buffer.area.height - 1;
        let plain = |x: u16| {
            let cell = &buffer[(x, y)];
            cell.fg == Color::Reset && !cell.modifier.contains(Modifier::BOLD)
        };
        let cells = u16::try_from(wrap::width(message)).unwrap();
        let mut x = 1;
        while x <= cells {
            let cell = &buffer[(x, y)];
            assert_eq!(cell.fg, Color::Red, "x {x}");
            assert!(cell.modifier.contains(Modifier::BOLD), "x {x}");
            x += u16::try_from(wrap::width(cell.symbol()).max(1)).unwrap();
        }
        assert!(plain(0), "the corner");
        assert!(plain(cells + 1), "the border after the message");
    }

    #[test]
    fn a_failed_save_shows_its_error_on_the_bottom_border_with_the_draft() {
        let mut app = box_with("hi", (W, 8));
        update(&mut app, Event::SaveFailed(LOCKED.into()));

        let terminal = draw(&app, W, 8);

        let rows = box_rows(&terminal, 3);
        assert!(
            rows[0].starts_with("┌Comment on line 1 as scott─"),
            "{}",
            rows[0]
        );
        assert_eq!(rows[1], boxed("hi"));
        assert_eq!(
            rows[2],
            format!(
                "└{LOCKED}{}┘",
                "─".repeat(usize::from(W) - 2 - LOCKED.len())
            )
        );
        assert_error_styled(&terminal, LOCKED);
    }

    #[test]
    fn a_failed_reply_shows_its_error_under_the_reply_title() {
        let threads = [talk(
            "c1",
            (1, 1),
            "line 1",
            Status::Open,
            State::Current,
            &[("claude", "Why?")],
        )];
        let mut app = app_with(numbered(3), &threads, 8);
        send(&mut app, [press(KeyCode::Enter)]);
        send(&mut app, [ch('r')]);
        typed(&mut app, "ok");
        update(&mut app, Event::SaveFailed(LOCKED.into()));

        let terminal = draw(&app, W, 8);

        let rows = box_rows(&terminal, 3);
        assert!(rows[0].starts_with("┌Reply to c1 as scott─"), "{}", rows[0]);
        assert_eq!(rows[1], boxed("ok"));
        assert!(rows[2].starts_with(&format!("└{LOCKED}─")), "{}", rows[2]);
        assert_error_styled(&terminal, LOCKED);
    }

    #[test]
    fn a_failed_range_comment_shows_its_error_under_the_range_title() {
        let mut app = app_with(numbered(3), &[], 8);
        typed(&mut app, "vj");
        send(&mut app, [press(KeyCode::Enter)]);
        typed(&mut app, "ok");
        update(&mut app, Event::SaveFailed(LOCKED.into()));

        let terminal = draw(&app, W, 8);

        let rows = box_rows(&terminal, 3);
        assert!(
            rows[0].starts_with("┌Comment on lines 1-2 as scott─"),
            "{}",
            rows[0]
        );
        assert_eq!(rows[1], boxed("ok"));
        assert!(rows[2].starts_with(&format!("└{LOCKED}─")), "{}", rows[2]);
        assert_error_styled(&terminal, LOCKED);
    }

    #[test]
    fn a_long_error_is_cut_with_an_ellipsis_at_the_narrow_box_s_inner_width() {
        let mut app = box_with("hi", (20, 8));
        update(&mut app, Event::SaveFailed(LOCKED.into()));

        let terminal = draw(&app, 20, 8);

        let cut = format!("{}…", &LOCKED[..17]);
        assert_eq!(box_rows(&terminal, 3)[2], format!("└{cut}┘"));
        assert_error_styled(&terminal, &cut);
    }

    #[test]
    fn a_box_with_no_room_between_its_corners_shows_no_error() {
        let mut app = box_with("hi", (2, 8));
        update(&mut app, Event::SaveFailed(LOCKED.into()));

        let (text, _) = box_border(&app, false, &Target::NewThread, 2);

        assert_eq!(text, "");
    }

    #[test]
    fn a_hint_too_wide_even_in_its_shortest_form_ends_in_an_ellipsis() {
        let app = box_with("hi", (20, 8));
        let rows = box_rows(&draw(&app, 20, 8), 3);
        assert_eq!(rows[2], "└ctrl+s:save esc:c…┘");

        let mut app = box_with("hi", (10, 8));
        send(&mut app, [press(KeyCode::Esc)]);
        let rows = box_rows(&draw(&app, 10, 8), 3);
        assert_eq!(rows[2], "└esc:dis…┘");
    }

    /// The last row of `terminal` as text, skipping the blank cells that
    /// follow a wide character.
    fn last_row(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let y = buffer.area.height - 1;
        let mut text = String::new();
        let mut skip = 0;
        for x in 0..buffer.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            skip = wrap::width(symbol).saturating_sub(1);
            text.push_str(symbol);
        }
        text
    }

    #[test]
    fn a_wide_character_error_is_styled_by_cell_and_cut_whole() {
        // 24 cells: eight wide characters and a space, then eight more.
        let message = "鎖定されています レビューが使用中";
        let mut app = box_with("hi", (W, 8));
        update(&mut app, Event::SaveFailed(message.into()));
        let wide = draw(&app, W, 8);
        assert!(
            last_row(&wide).starts_with(&format!("└{message}─")),
            "{}",
            last_row(&wide)
        );
        assert_error_styled(&wide, message);

        // 18 inner cells: the first eight characters and the space fill 17;
        // the next wide character would straddle the cut, so the ellipsis
        // takes its place, and the corner stays.
        let mut app = box_with("hi", (20, 8));
        update(&mut app, Event::SaveFailed(message.into()));
        let narrow = draw(&app, 20, 8);
        let row = last_row(&narrow);
        assert_eq!(row, "└鎖定されています …┘");
        assert_eq!(wrap::width(&row), 20, "{row}");
        assert_error_styled(&narrow, "鎖定されています …");
    }

    #[test]
    fn an_error_s_newlines_and_tabs_show_as_spaces_on_one_border_row() {
        let mut app = box_with("hi", (W, 8));
        update(
            &mut app,
            Event::SaveFailed("cannot write\nx.json:\tdenied".into()),
        );

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert!(
            rows[2].starts_with("└cannot write x.json: denied─"),
            "{}",
            rows[2]
        );
        assert!(rows[2].ends_with('┘'), "{}", rows[2]);
        assert_error_styled(&draw(&app, W, 8), "cannot write x.json: denied");
    }

    #[test]
    fn an_error_s_runs_of_blank_and_control_characters_are_one_space() {
        let mut app = box_with("hi", (W, 8));
        update(
            &mut app,
            Event::SaveFailed("\r\nfailed:\r\n\r\ncaused by:  denied\n".into()),
        );

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert!(
            rows[2].starts_with("└failed: caused by: denied─"),
            "{}",
            rows[2]
        );
    }

    #[test]
    fn an_error_with_no_text_leaves_the_hint_on_the_border() {
        let mut app = box_with("hi", (W, 8));
        update(&mut app, Event::SaveFailed(" \n\t".into()));

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert!(
            rows[2].starts_with(&format!("└{INPUT_HINT}─")),
            "{}",
            rows[2]
        );
    }

    #[test]
    fn the_next_key_clears_the_error_and_brings_the_hint_back() {
        let mut app = box_with("hi", (W, 8));
        update(&mut app, Event::SaveFailed(LOCKED.into()));
        send(&mut app, [ch('!')]);

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert_eq!(rows[1], boxed("hi!"));
        assert!(
            rows[2].starts_with(&format!("└{INPUT_HINT}─")),
            "{}",
            rows[2]
        );
    }

    #[test]
    fn a_resize_narrower_rewraps_the_draft() {
        let mut app = box_with("aaaa bbbb cccc", (30, 10));
        assert_eq!(
            box_rows(&draw(&app, 30, 10), 3)[1].trim_end(),
            "│aaaa bbbb cccc              │"
        );

        update(&mut app, Event::Resize(13, 10));

        assert_eq!(
            box_rows(&draw(&app, 13, 10), 4)[1..3],
            ["│aaaa bbbb  │", "│cccc       │"]
        );
    }

    #[test]
    fn a_scrolled_view_starts_at_top() {
        let mut app = app_with(numbered(30), &[], 9);
        send(&mut app, [ch('G')]);

        let screen = screen(&draw(&app, 120, 9));

        let first = screen.lines().nth(1).unwrap();
        assert_eq!(first, "  24│line 24");
        assert!(
            screen
                .trim_end()
                .ends_with("L30/30  enter:thread/comment ]/[:jump q:quit v:select /:search ?:help")
        );
    }

    /// An app on a terminal `cols` wide, so wrapping matches the drawing.
    fn narrow(lines: &[&str], threads: &[ThreadView], (cols, rows): (u16, u16)) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(showing(lines, threads), "scott", (cols, rows)).labeled("plan.md")
    }

    const LONG: &str = "a\tbravo charlie delta echo foxtrot";

    #[test]
    fn long_lines_wrap_with_the_number_on_the_first_row_only() {
        let app = narrow(&[LONG, "short"], &[], (20, 6));

        let screen = screen(&draw(&app, 20, 6));

        assert_eq!(
            screen,
            "your turn
  1│a    bravo
   │charlie delta
   │echo foxtrot
  2│short
plan.md  L1/2  ente…"
        );
    }

    #[test]
    fn the_cursor_highlight_covers_every_row_of_a_wrapped_line() {
        let mut app = narrow(&["one", LONG, "three"], &[], (20, 7));
        send(&mut app, [ch('j')]);

        let terminal = draw(&app, 20, 7);

        let buffer = terminal.backend().buffer();
        let bg = |y: u16| buffer[(5, TOP + y)].bg;
        for y in 1..=3 {
            assert_eq!(bg(y), Color::DarkGray, "row {y}");
        }
        assert_eq!(bg(0), Color::Reset);
        assert_eq!(bg(4), Color::Reset);
    }

    #[test]
    fn a_thread_glyph_shows_on_a_wrapped_lines_first_row_only() {
        let threads = [view("c1", (1, 1), Status::Open, State::Current)];
        let app = narrow(&[LONG, "short"], &threads, (20, 6));

        let terminal = draw(&app, 20, 6);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, TOP)].symbol(), "●");
        assert_eq!(buffer[(0, TOP + 1)].symbol(), " ");
        assert_eq!(buffer[(0, TOP + 2)].symbol(), " ");
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
        let app = narrow(&TABLE, &[], (40, 7));

        let screen = screen(&draw(&app, 40, 7));

        let file: Vec<&str> = screen.lines().skip(1).take(5).collect();
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
        let threads = [view("c1", (4, 4), Status::Open, State::Current)];
        let mut app = narrow(&TABLE, &threads, (40, 7));
        typed(&mut app, "jj");

        let terminal = draw(&app, 40, 7);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, TOP + 3)].symbol(), "●");
        for y in [0, 1, 2, 4] {
            assert_eq!(buffer[(0, TOP + y)].symbol(), " ", "row {y}");
        }
        let bg = |y: u16| buffer[(5, TOP + y)].bg;
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
        let app = narrow(&WIDE_TABLE, &[], (30, 9));

        let screen = screen(&draw(&app, 30, 9));

        assert_eq!(
            screen,
            "your turn
  1│before
  2│ Q │ Decision
  3│───┼──────────────────────
  4│ 1 │ use the water
   │   │ filling rule for
   │   │ widths
  5│after
plan.md  L1/5  enter:thread/c…"
        );
    }

    #[test]
    fn a_wrapped_table_lines_glyph_is_on_its_first_row_and_its_highlight_on_all() {
        let threads = [view("c1", (4, 4), Status::Open, State::Current)];
        let mut app = narrow(&WIDE_TABLE, &threads, (30, 9));
        for _ in 0..3 {
            send(&mut app, [ch('j')]);
        }

        let terminal = draw(&app, 30, 9);

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, TOP + 3)].symbol(), "●");
        for y in [0, 1, 2, 4, 5, 6] {
            assert_eq!(buffer[(0, TOP + y)].symbol(), " ", "row {y}");
        }
        let bg = |y: u16| buffer[(5, TOP + y)].bg;
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
                let mut app = app_with(
                    numbered(5),
                    &[view("c1", (1, 2), Status::Open, State::Current)],
                    rows,
                );
                draw(&app, width, rows);
                send(&mut app, [press(KeyCode::Enter)]);
                draw(&app, width, rows);
                send(&mut app, [ch('a')]);
                typed(&mut app, "hello");
                draw(&app, width, rows);
                draw(&app_with(Vec::new(), &[], rows), width, rows);
            }
        }
    }

    #[test]
    fn an_empty_file_says_so() {
        let app = app_with(Vec::new(), &[], 4);

        let screen = screen(&draw(&app, W, 4));

        assert_eq!(
            screen,
            format!(
                "{}\n(empty file)\n\nplan.md  L0/0  enter:thread/comment ]/[:jump q:quit v:select /:search ?:help",
                banner_row(W)
            )
        );
    }

    #[test]
    fn an_empty_file_in_a_narrow_view_says_so_with_an_ellipsis() {
        let app = app_with(Vec::new(), &[], 4);

        let first = |width| {
            screen(&draw(&app, width, 4))
                .lines()
                .nth(1)
                .unwrap()
                .to_string()
        };

        assert_eq!(first(6), "(empt…");
        assert_eq!(first(12), "(empty file)");
    }

    #[test]
    fn only_the_cursor_line_is_highlighted() {
        let mut app = app_with(numbered(5), &[], 8);
        send(&mut app, [ch('j')]);

        let terminal = draw(&app, W, 8);

        let buffer = terminal.backend().buffer();
        let bg = |y: u16| buffer[(5, TOP + y)].bg;
        assert_eq!(bg(1), Color::DarkGray);
        for y in [0, 2, 3, 4] {
            assert_eq!(bg(y), Color::Reset, "row {y}");
        }
    }

    fn highlighted(lines: &[&str]) -> App {
        let lines = lines.iter().map(ToString::to_string).collect();
        App::open(showing(lines, &[]), "scott", (W, 6)).highlighted(true)
    }

    #[test]
    fn a_highlighted_heading_is_bold_magenta_in_its_text_and_not_its_gutter() {
        let app = highlighted(&["x", "# Title"]); // cursor on line 1

        let terminal = draw(&app, W, 6);

        let buffer = terminal.backend().buffer();
        assert_eq!(screen(&terminal).lines().nth(2), Some("  2│# Title"));
        let y = TOP + 1;
        for x in 4..11 {
            assert_eq!(buffer[(x, y)].fg, Color::Magenta, "x {x}");
            assert!(buffer[(x, y)].modifier.contains(Modifier::BOLD), "x {x}");
        }
        for x in 0..4 {
            assert_eq!(buffer[(x, y)].fg, Color::Reset, "gutter x {x}");
            assert!(
                !buffer[(x, y)].modifier.contains(Modifier::BOLD),
                "gutter x {x}"
            );
        }
    }

    #[test]
    fn the_cursor_highlight_shows_under_highlighted_text() {
        let app = highlighted(&["# Title", "x"]);

        let terminal = draw(&app, W, 6);

        let cell = &terminal.backend().buffer()[(4, TOP)];
        assert_eq!((cell.fg, cell.bg), (Color::Magenta, Color::DarkGray));
        assert_eq!(terminal.backend().buffer()[(20, TOP)].bg, Color::DarkGray);
    }

    #[test]
    fn a_fenced_block_has_dim_fences_and_a_cyan_body() {
        let app = highlighted(&["x", "```", "code", "```"]); // cursor on "x"

        let terminal = draw(&app, W, 6);

        let buffer = terminal.backend().buffer();
        for y in [1, 3] {
            assert!(
                buffer[(4, TOP + y)].modifier.contains(Modifier::DIM),
                "fence {y}"
            );
        }
        assert_eq!(buffer[(4, TOP + 2)].fg, Color::Cyan);
        assert_eq!(buffer[(7, TOP + 2)].fg, Color::Cyan);
    }

    #[test]
    fn nothing_on_the_cursor_row_is_dim_so_it_stays_legible() {
        let app = highlighted(&["```", "code", "```", "> quote"]);

        let terminal = draw(&app, W, 6);

        let buffer = terminal.backend().buffer();
        let cell = &buffer[(4, TOP)];
        assert!(!cell.modifier.contains(Modifier::DIM));
        assert_eq!(cell.bg, Color::DarkGray);
        assert!(
            buffer[(4, TOP + 2)].modifier.contains(Modifier::DIM),
            "off the cursor row"
        );
        assert!(
            buffer[(4, TOP + 3)].modifier.contains(Modifier::DIM),
            "quote marker"
        );
    }

    // --- visual selection ---

    /// Background of column 5 (file text) on each of the file view's first
    /// `n` rows.
    fn backgrounds(terminal: &Terminal<TestBackend>, n: u16) -> Vec<bool> {
        let buffer = terminal.backend().buffer();
        (TOP..TOP + n)
            .map(|y| buffer[(5, y)].bg == Color::DarkGray)
            .collect()
    }

    #[test]
    fn every_selected_line_is_highlighted_and_no_other() {
        let mut app = app_with(numbered(6), &[], 8);
        typed(&mut app, "jvjj");

        let terminal = draw(&app, W, 8);

        assert_eq!(
            backgrounds(&terminal, 6),
            [false, true, true, true, false, false]
        );
    }

    #[test]
    fn a_selected_wrapped_line_is_highlighted_on_all_its_rows() {
        let mut app = narrow(&["one", LONG, "three"], &[], (20, 7));
        typed(&mut app, "vj");

        let terminal = draw(&app, 20, 7);

        assert_eq!(backgrounds(&terminal, 5), [true, true, true, true, false]);
    }

    #[test]
    fn a_selected_wrapped_table_line_is_highlighted_on_all_its_rows() {
        let mut app = narrow(&WIDE_TABLE, &[], (30, 9));
        typed(&mut app, "jjvj");

        let terminal = draw(&app, 30, 9);

        assert_eq!(
            backgrounds(&terminal, 7),
            [false, false, true, true, true, true, false]
        );
    }

    #[test]
    fn after_a_resize_the_rewrapped_selection_is_still_exactly_highlighted() {
        let mut app = narrow(&["one", LONG, "three", "four"], &[], (60, 8));
        typed(&mut app, "jvk");
        assert_eq!(
            backgrounds(&draw(&app, 60, 8), 4),
            [true, true, false, false]
        );

        update(&mut app, Event::Resize(20, 8));

        assert_eq!(
            backgrounds(&draw(&app, 20, 8), 6),
            [true, true, true, true, false, false]
        );
    }

    #[test]
    fn the_visual_footer_shows_the_range_and_its_keys() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "jvjj");

        let screen = screen(&draw(&app, W, 8));

        assert!(
            screen.ends_with("plan.md  L4/12  visual 2-4  esc:cancel enter:comment j/k:extend"),
            "{screen}"
        );
    }

    #[test]
    fn the_visual_footer_orders_an_upward_range() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "jjjvkk");

        let screen = screen(&draw(&app, W, 8));

        assert!(screen.contains("L2/12  visual 2-4  "), "{screen}");
    }

    #[test]
    fn a_box_from_visual_names_the_range_and_keeps_it_highlighted() {
        let mut app = app_with(numbered(12), &[], 9);
        typed(&mut app, "jvjj");
        send(&mut app, [press(KeyCode::Enter)]);

        let terminal = draw(&app, W, 9);

        assert!(
            screen(&terminal).contains("┌Comment on lines 2-4 as scott─"),
            "{}",
            screen(&terminal)
        );
        assert_eq!(backgrounds(&terminal, 5), [false, true, true, true, false]);
    }

    #[test]
    fn quit_stays_in_the_normal_footer_on_a_70_column_terminal() {
        let threads = [view("c1", (1, 1), Status::Open, State::Outdated)];
        let app = app_with(numbered(12), &threads, 8);

        let screen = screen(&draw(&app, 70, 8));

        let footer = screen.lines().last().unwrap();
        assert!(footer.contains("1 outdated"), "{footer}");
        assert!(footer.contains("q:quit"), "{footer}");
    }

    #[test]
    fn cancel_stays_in_the_visual_footer_on_a_70_column_terminal() {
        let threads = [view("c1", (1, 1), Status::Open, State::Outdated)];
        let mut app = app_with(numbered(12), &threads, 8);
        typed(&mut app, "vjj");

        let screen = screen(&draw(&app, 70, 8));

        let footer = screen.lines().last().unwrap();
        assert!(footer.contains("visual 1-3  1 outdated"), "{footer}");
        assert!(footer.contains("esc:cancel"), "{footer}");
    }

    /// The last row of a `width`-column, 8-row terminal showing `app`.
    fn footer(app: &App, width: u16) -> String {
        screen(&draw(app, width, 8))
            .lines()
            .last()
            .unwrap()
            .to_string()
    }

    const NORMAL_FOOTER: &str =
        "plan.md  L1/12  enter:thread/comment ]/[:jump q:quit v:select /:search ?:help";

    /// A path wider than any footer it is drawn in.
    const LONG_PATH: &str = "/var/folders/xy/0123456789abcdef/T/.tmpAbC123/deep/q3/drafts/plan.md";

    #[test]
    fn a_long_path_is_cut_from_the_left_so_the_counts_show() {
        let threads = [view("c1", (1, 1), Status::Open, State::Outdated)];
        let app = app_with(numbered(12), &threads, 8).labeled(LONG_PATH);

        assert_eq!(
            footer(&app, 60),
            "…cdef/T/.tmpAbC123/deep/q3/drafts/plan.md  L1/12  1 outdated"
        );
    }

    #[test]
    fn the_path_shrinks_to_an_ellipsis_before_the_counts_are_cut() {
        let threads = [view("c1", (1, 1), Status::Open, State::Outdated)];
        let app = app_with(numbered(12), &threads, 8).labeled(LONG_PATH);

        assert_eq!(footer(&app, 20), "…  L1/12  1 outdated");
        assert_eq!(footer(&app, 15), "…  L1/12  1 ou…");
    }

    #[test]
    fn a_path_of_wide_characters_gives_way_by_whole_clusters() {
        let threads = [view("c1", (1, 1), Status::Open, State::Outdated)];
        let app = app_with(numbered(12), &threads, 8).labeled("/プロジェクト/計画/plan.md");

        // 21 cells for the path: a wide cluster would straddle the edge, so
        // the path takes 20 and the row is a cell short. The screen shows a
        // wide cluster's second cell as a space.
        assert_eq!(
            footer(&app, 40),
            "…ェ ク ト /計 画 /plan.md  L1/12  1 outdated"
        );
    }

    #[test]
    fn a_long_search_is_cut_before_the_path_gives_way() {
        let mut app = app_with(numbered(12), &[], 8);
        send(&mut app, [ch('/')]);
        typed(&mut app, &"x".repeat(70));
        send(&mut app, [press(KeyCode::Enter)]);

        assert_eq!(
            footer(&app, 30),
            format!("plan.md  L1/12  /{}…", "x".repeat(12))
        );
    }

    #[test]
    fn the_visual_and_panel_footers_shorten_the_path_too() {
        let threads = [view("c1", (1, 1), Status::Open, State::Outdated)];
        let mut app = app_with(numbered(12), &threads, 8).labeled(LONG_PATH);
        typed(&mut app, "vj");

        assert_eq!(
            footer(&app, 60),
            "…bC123/deep/q3/drafts/plan.md  L2/12  visual 1-2  1 outdated"
        );

        let app = opened_panel(&threads).labeled(LONG_PATH);
        let screen = screen(&draw(&app, 60, 16));
        assert_eq!(
            screen.lines().last().unwrap(),
            "…cdef/T/.tmpAbC123/deep/q3/drafts/plan.md  L1/12  1 outdated"
        );
    }

    #[test]
    fn a_footer_that_fits_exactly_is_drawn_unchanged() {
        let app = app_with(numbered(12), &[], 8);
        let width = u16::try_from(NORMAL_FOOTER.len()).unwrap();

        assert_eq!(footer(&app, width), NORMAL_FOOTER);
        assert_eq!(footer(&app, W), NORMAL_FOOTER);
    }

    #[test]
    fn a_normal_footer_too_wide_for_the_terminal_ends_in_an_ellipsis() {
        let app = app_with(numbered(12), &[], 8);

        assert_eq!(footer(&app, 30), "plan.md  L1/12  enter:thread/…");
    }

    #[test]
    fn a_visual_footer_too_wide_for_the_terminal_ends_in_an_ellipsis() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "vj");

        assert_eq!(footer(&app, 30), "plan.md  L2/12  visual 1-2  e…");
    }

    #[test]
    fn a_panel_footer_too_wide_for_the_terminal_ends_in_an_ellipsis() {
        let app = opened_panel(&[view("c1", (1, 1), Status::Open, State::Current)]);

        let screen = screen(&draw(&app, 30, 16));

        assert_eq!(
            screen.lines().last().unwrap(),
            "plan.md  L1/12  tab:next a:ad…"
        );
    }

    #[test]
    fn a_multi_line_error_in_the_footer_is_on_one_row() {
        let mut app = app_with(numbered(12), &[], 8);
        update(
            &mut app,
            Event::SaveFailed("cannot write\nx.json:\tdenied".into()),
        );

        assert!(
            footer(&app, W).starts_with("cannot write x.json: denied  L1/12  "),
            "{}",
            footer(&app, W)
        );
    }

    #[test]
    fn a_long_error_in_the_footer_ends_in_an_ellipsis() {
        let mut app = app_with(numbered(12), &[], 8);
        update(
            &mut app,
            Event::SaveFailed(
                "cannot write /home/u/.local/share/ptal/reviews/0123abcd.json: permission denied"
                    .into(),
            ),
        );

        let footer = footer(&app, 40);

        assert_eq!(footer, "cannot write /home/u/.local/share/ptal/…");
    }

    #[test]
    fn a_long_author_cuts_the_box_title_with_an_ellipsis_before_the_corner() {
        let author = "x".repeat(60);
        let mut app = App::open(showing(numbered(3), &[]), &author, (W, 8)).labeled("plan.md");
        send(&mut app, [press(KeyCode::Enter)]);

        let rows = box_rows(&draw(&app, W, 8), 3);

        let title = format!("Comment on line 1 as {}…", "x".repeat(56));
        assert_eq!(rows[0], format!("┌{title}┐"));
    }

    #[test]
    fn box_titles_too_wide_for_a_narrow_box_end_in_an_ellipsis() {
        let app = box_with("hi", (20, 8));
        assert_eq!(box_rows(&draw(&app, 20, 8), 3)[0], "┌Comment on line 1…┐");

        let mut app = narrow(&["one", "two"], &[], (20, 8));
        typed(&mut app, "vj");
        send(&mut app, [press(KeyCode::Enter)]);
        assert_eq!(box_rows(&draw(&app, 20, 8), 3)[0], "┌Comment on lines …┐");

        let threads = [view("c1", (1, 1), Status::Open, State::Current)];
        let mut app = narrow(&["one", "two"], &threads, (20, 16));
        send(&mut app, [press(KeyCode::Enter)]);
        send(&mut app, [ch('r')]);
        let screen = screen(&draw(&app, 20, 16));
        assert!(screen.contains("┌Reply to c1 as sc…┐"), "{screen}");
    }

    #[test]
    fn a_panel_title_too_wide_for_a_narrow_panel_ends_in_an_ellipsis() {
        let threads = [view("c1", (1, 1), Status::Open, State::Current)];
        let mut app = narrow(&["one", "two"], &threads, (15, 16));
        send(&mut app, [press(KeyCode::Enter)]);

        let screen = screen(&draw(&app, 15, 16));

        assert!(screen.contains("┌Threads on l…┐"), "{screen}");
    }

    #[test]
    fn a_one_line_selection_still_says_line() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "jjv");
        send(&mut app, [press(KeyCode::Enter)]);

        let screen = screen(&draw(&app, W, 8));

        assert!(screen.contains("┌Comment on line 3 as scott─"), "{screen}");
    }

    fn reversed_cells(terminal: &Terminal<TestBackend>) -> usize {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .count()
    }

    #[test]
    fn the_search_prompt_replaces_the_footer_with_the_query_and_a_note() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "/line 9");
        assert_eq!(footer(&app, W), "/line 9");
        typed(&mut app, "9");
        assert_eq!(footer(&app, W), "/line 99 [no match]");
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "G/line 2");
        assert_eq!(footer(&app, W), "/line 2 [wrapped]");
    }

    #[test]
    fn a_query_wider_than_the_row_shows_its_tail_and_drops_the_note() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "/abcdefghij");
        assert_eq!(footer(&app, 8), "…defghij");
        assert_eq!(footer(&app, 6), "…fghij");
        assert_eq!(footer(&app, 3), "…ij");
        for width in 1..20 {
            assert!(wrap::width(&footer(&app, width)) <= usize::from(width));
        }
    }

    #[test]
    fn a_wide_character_query_is_cut_whole() {
        let mut app = app_with(numbered(12), &[], 8);
        typed(&mut app, "/日本語");
        // The screen helper shows a wide cell's spacer as a blank.
        assert_eq!(footer(&app, 6).replace(' ', ""), "…本語");
        assert_eq!(footer(&app, 4).replace(' ', ""), "…語");
    }

    #[test]
    fn the_normal_footer_shows_the_active_search_and_the_hint() {
        let mut app = app_with(numbered(12), &[], 8);
        assert!(footer(&app, 200).ends_with("/:search ?:help"));
        typed(&mut app, "/line 3");
        send(&mut app, [press(KeyCode::Enter)]);
        assert!(footer(&app, 200).contains("L3/12  /line 3  enter"));
    }

    #[test]
    fn matches_are_drawn_reversed_and_only_while_a_search_is_active() {
        let mut app = app_with(numbered(12), &[], 8);
        assert_eq!(reversed_cells(&draw(&app, W, 8)), 0);
        typed(&mut app, "/line 2");
        assert_eq!(reversed_cells(&draw(&app, W, 8)), "line 2".len());
        send(&mut app, [press(KeyCode::Enter)]);
        assert_eq!(reversed_cells(&draw(&app, W, 8)), "line 2".len());
        send(&mut app, [press(KeyCode::Esc)]);
        assert_eq!(reversed_cells(&draw(&app, W, 8)), 0);
    }

    #[test]
    fn a_tiny_terminal_does_not_panic_in_the_prompt() {
        for (w, h) in [(20, 4), (1, 1), (80, 24)] {
            let mut app = app_with(numbered(12), &[], h);
            typed(&mut app, "/line 1");
            draw(&app, w, h);
        }
    }

    // --- help ---

    #[test]
    fn help_replaces_the_file_with_the_listing_at_80_by_24() {
        let mut app = app_with(numbered(12), &[], 24);
        send(&mut app, [ch('?')]);

        let screen = screen(&draw(&app, 80, 24));

        let rows: Vec<_> = screen.lines().collect();
        assert_eq!(rows[0], "Help");
        assert!(rows.contains(&"Normal"), "{screen}");
        assert!(
            rows.iter()
                .any(|r| r.starts_with("  j/k, ↓/↑") && r.contains("move the cursor"))
        );
        assert_eq!(rows[23], "j/k g/G:scroll  ?/esc:close");
        assert!(!screen.contains("line 1\n"), "{screen}");
    }

    #[test]
    fn help_scrolls_through_a_listing_taller_than_the_window() {
        let mut app = app_with(numbered(12), &[], 10);
        send(&mut app, [ch('?')]);
        let top = screen(&draw(&app, 80, 10));
        assert!(!top.contains("Thread panel"), "{top}");

        send(&mut app, [ch('j')]);
        assert_ne!(screen(&draw(&app, 80, 10)), top);

        send(&mut app, [ch('G')]);
        let end = screen(&draw(&app, 80, 10));
        assert!(end.contains("close the panel"), "{end}");
        assert!(end.lines().nth(8).is_some_and(|r| !r.is_empty()), "{end}");

        send(&mut app, [ch('g')]);
        assert_eq!(screen(&draw(&app, 80, 10)), top);
    }

    #[test]
    fn help_in_narrow_windows_fits_and_loses_no_words() {
        for width in [40, 24] {
            let rows = keys::rows(usize::from(width));
            assert!(
                rows.iter()
                    .all(|r| wrap::width(&r.text) <= usize::from(width))
            );
            let shown = rows
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            for section in keys::HELP {
                for entry in section.entries {
                    for word in entry
                        .keys
                        .split_whitespace()
                        .chain(entry.what.split_whitespace())
                    {
                        assert!(shown.contains(word), "{width}: {word} is missing");
                    }
                }
            }
            let mut app = app_with(numbered(12), &[], 24);
            send(&mut app, [ch('?')]);
            assert!(
                screen(&draw(&app, width, 24))
                    .lines()
                    .all(|r| wrap::width(r) <= usize::from(width))
            );
        }
    }

    #[test]
    fn help_on_tiny_terminals_does_not_panic() {
        for width in 0..=3 {
            for rows in 0..=3 {
                let mut app = app_with(numbered(5), &[], rows);
                send(&mut app, [ch('?')]);
                draw(&app, width, rows);
                send(&mut app, [ch('G')]);
                draw(&app, width, rows);
            }
        }
    }

    #[test]
    fn the_help_hint_is_the_first_to_go_in_a_narrow_footer() {
        let app = app_with(numbered(12), &[], 8);

        assert!(footer(&app, 70).contains("q:quit"));
        assert!(!footer(&app, 70).contains("?:help"));
        assert!(footer(&app, 80).ends_with("?:help"));
    }

    // --- the review banner ---

    fn banner(review: ReviewStatus, width: u16) -> String {
        let app = App::open(
            View {
                review,
                ..showing(numbered(3), &[])
            },
            "scott",
            (width, 6),
        )
        .labeled("plan.md");
        screen(&draw(&app, width, 6))
            .lines()
            .next()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_fresh_review_shows_your_turn_and_the_hand_off_keys_on_top() {
        let app = app_with(numbered(3), &[], 6);

        let screen = screen(&draw(&app, W, 6));

        let rows: Vec<&str> = screen.lines().collect();
        assert_eq!(rows[0], spread("your turn", "H:hand off C:close", 80));
        assert_eq!(rows[1], "  1│line 1");
    }

    #[test]
    fn during_the_agents_turn_the_banner_shows_the_latest_note() {
        let review = review_status(ReviewState::Turn(Side::Agent), Some("Over to you"));

        assert_eq!(
            banner(review, W),
            spread("agent's turn · scott: Over to you", "C:close", 80)
        );
    }

    #[test]
    fn a_closed_review_shows_its_verdict_and_the_reopen_key() {
        let review = review_status(ReviewState::Closed(Verdict::Approved), None);

        assert_eq!(
            banner(review, W),
            spread("closed: approved", "O:reopen", 80)
        );
    }

    #[test]
    fn a_long_or_multi_line_note_is_cut_on_one_row() {
        let multi = review_status(ReviewState::Turn(Side::Human), Some("First line\nsecond"));
        let long = review_status(ReviewState::Turn(Side::Human), Some(&"word ".repeat(30)));

        assert_eq!(
            banner(multi, W),
            spread("your turn · scott: First line…", "H:hand off C:close", 80)
        );
        let row = banner(long, 50);
        assert_eq!(row.chars().count(), 50, "{row:?}");
        assert!(row.starts_with("your turn · scott: word"), "{row:?}");
        assert!(row.ends_with("… H:hand off C:close"), "{row:?}");
    }

    #[test]
    fn a_banner_too_narrow_for_its_hint_drops_the_hint() {
        assert_eq!(banner(ReviewStatus::default(), 16), "your turn");
    }

    // --- hand-off and close boxes ---

    #[test]
    fn the_hand_off_box_says_what_saving_does() {
        let mut app = app_with(numbered(3), &[], 8);
        send(&mut app, [shift('H')]);

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert!(
            rows[0].starts_with("┌Hand off to the agent (note optional)─"),
            "{rows:?}"
        );
        assert!(
            rows[2].starts_with("└ctrl+s:hand off enter:newline esc:cancel─"),
            "{rows:?}"
        );
    }

    #[test]
    fn the_close_box_title_follows_the_verdict() {
        let mut app = app_with(numbered(3), &[], 8);
        send(&mut app, [shift('C')]);
        let title = |app: &App| box_rows(&draw(app, W, 8), 3)[0].clone();

        assert!(title(&app).starts_with("┌Close as ? (tab: approved / dropped)─"));
        assert!(
            box_rows(&draw(&app, W, 8), 3)[2].starts_with("└tab:verdict ctrl+s:close esc:cancel─")
        );
        send(&mut app, [press(KeyCode::Tab)]);
        assert!(title(&app).starts_with("┌Close as approved (tab: approved / dropped)─"));
        send(&mut app, [press(KeyCode::Tab)]);
        assert!(title(&app).starts_with("┌Close as dropped (tab: approved / dropped)─"));
    }

    #[test]
    fn approving_with_open_threads_asks_on_the_border() {
        let threads = [
            view("c1", (1, 1), Status::Open, State::Current),
            view("c2", (2, 2), Status::Open, State::Current),
        ];
        let mut app = app_with(numbered(3), &threads, 8);
        send(&mut app, [shift('C')]);
        send(&mut app, [press(KeyCode::Tab)]);
        send(&mut app, [ctrl('s')]);

        let rows = box_rows(&draw(&app, W, 8), 3);

        assert!(
            rows[2].starts_with("└2 open threads stay open: ctrl+s again to approve─"),
            "{rows:?}"
        );
    }
}
