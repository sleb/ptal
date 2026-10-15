//! The thread panel's content: a pure function from threads and a width to
//! wrapped, styled rows.
//!
//! [`layout`] is the single owner of the panel's rows, so `update` can clamp
//! scroll from `lines.len()` and `render` can draw a slice without either
//! repeating the arithmetic. How text wraps is [`tui::wrap`](super::wrap)'s.

use crate::model::{State, Status};
use crate::review::ThreadView;
use crate::tui::wrap;

/// One screen row of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelLine {
    pub text: String,
    /// Resolved threads are drawn dimmed.
    pub dim: bool,
}

/// The panel's rows for a set of threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub lines: Vec<PanelLine>,
    /// Row of each thread's header, in the order the threads were given.
    pub thread_starts: Vec<usize>,
}

const SELECTED: &str = "▶ ";
const UNSELECTED: &str = "  ";
const QUOTE_INDENT: &str = "  > ";
const AUTHOR_INDENT: &str = "  ";
const BODY_INDENT: &str = "    ";

/// Stack `threads` (already filtered to the cursor line, in store order) as
/// rows of at most `width` cells. `selected` gets a `▶` on its header.
///
/// Every row is wrapped by [`wrap::plain_rows`], one source line at a time,
/// so text is measured as ratatui draws it and every newline is a row break.
/// Whitespace shows as written and hangs on the end of the row it breaks
/// (clipped when drawn, like the file view's). Quote and body rows sit under
/// their indent; header and author lines continue with no indent. The only
/// row wider than `width` is one holding a single cluster wider than the room
/// after its indent.
pub fn layout(threads: &[&ThreadView], selected: usize, width: usize) -> Layout {
    let mut lines = Vec::new();
    let mut thread_starts = Vec::with_capacity(threads.len());
    for (index, view) in threads.iter().enumerate() {
        if index > 0 {
            lines.push(PanelLine {
                text: String::new(),
                dim: false,
            });
        }
        thread_starts.push(lines.len());
        let dim = matches!(view.thread.status, Status::Resolved { .. });
        let mut push = |rows: Vec<String>| {
            lines.extend(rows.into_iter().map(|text| PanelLine { text, dim }));
        };

        let marker = if index == selected {
            SELECTED
        } else {
            UNSELECTED
        };
        push(indented(&format!("{marker}{}", header(view)), "", width));
        push(indented(&view.thread.anchor.quote, QUOTE_INDENT, width));
        for comment in &view.thread.comments {
            let who = format!(
                "{AUTHOR_INDENT}{} · {}",
                comment.author,
                time(&comment.created_at)
            );
            push(indented(&who, "", width));
            push(indented(&comment.body, BODY_INDENT, width));
        }
    }
    Layout {
        lines,
        thread_starts,
    }
}

/// `c3 open`, `c3 open · outdated`, `c3 resolved by claude`.
fn header(view: &ThreadView) -> String {
    let mut text = match &view.thread.status {
        Status::Open => format!("{} open", view.thread.id),
        Status::Resolved { resolved_by, .. } => {
            format!("{} resolved by {resolved_by}", view.thread.id)
        }
    };
    if view.state == State::Outdated {
        text.push_str(" · outdated");
    }
    text
}

/// `YYYY-MM-DD HH:MMZ` from an RFC 3339 UTC timestamp; anything not in that
/// shape is shown as stored. No timezone conversion.
fn time(created_at: &str) -> String {
    let bytes = created_at.as_bytes();
    let shaped = created_at.is_ascii()
        && bytes.len() >= 17
        && bytes[10] == b'T'
        && created_at.ends_with('Z');
    if shaped {
        format!("{} {}Z", &created_at[..10], &created_at[11..16])
    } else {
        created_at.to_string()
    }
}

/// `text` wrapped to what is left of `width` after `indent`, each row
/// prefixed with `indent`. Each source line wraps on its own, so every
/// newline is a row break and a blank line is an empty row.
fn indented(text: &str, indent: &str, width: usize) -> Vec<String> {
    let room = width.saturating_sub(wrap::width(indent));
    text.split('\n')
        .flat_map(|line| wrap::plain_rows(line, room))
        .map(|row| format!("{indent}{row}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Anchor, Comment, Thread};

    fn thread(id: &str, quote: &str, status: Status, state: State) -> ThreadView {
        ThreadView {
            thread: Thread {
                id: id.into(),
                anchor: Anchor {
                    start_line: 1,
                    end_line: 1,
                    quote: quote.into(),
                },
                status,
                comments: vec![],
            },
            state,
        }
    }

    fn comment(mut view: ThreadView, author: &str, at: &str, body: &str) -> ThreadView {
        let id = format!("{}-{}", view.thread.id, view.thread.comments.len());
        view.thread.comments.push(Comment {
            id,
            author: author.into(),
            body: body.into(),
            created_at: at.into(),
        });
        view
    }

    fn resolved() -> Status {
        Status::Resolved {
            resolved_by: "claude".into(),
            resolved_at: "2026-10-12T11:00:00Z".into(),
        }
    }

    fn texts(layout: &Layout) -> Vec<&str> {
        layout.lines.iter().map(|l| l.text.as_str()).collect()
    }

    /// The last `n` rows: a body's, when the author line above it wraps.
    fn last(layout: &Layout, n: usize) -> Vec<&str> {
        let texts = texts(layout);
        texts[texts.len() - n..].to_vec()
    }

    fn one() -> ThreadView {
        let view = thread("c1", "ship it", Status::Open, State::Current);
        let view = comment(view, "scott", "2026-10-12T09:30:15Z", "Why now?");
        comment(
            view,
            "claude",
            "2026-10-12T09:45:00Z",
            "Because it is ready.",
        )
    }

    #[test]
    fn a_thread_shows_header_quote_and_each_comment() {
        let view = one();

        let layout = layout(&[&view], 0, 40);

        assert_eq!(
            texts(&layout),
            [
                "▶ c1 open",
                "  > ship it",
                "  scott · 2026-10-12 09:30Z",
                "    Why now?",
                "  claude · 2026-10-12 09:45Z",
                "    Because it is ready.",
            ]
        );
        assert_eq!(layout.thread_starts, [0]);
        assert!(layout.lines.iter().all(|l| !l.dim));
    }

    #[test]
    fn a_multi_line_quote_gets_a_marker_on_every_line() {
        let view = thread("c1", "one\ntwo", Status::Open, State::Current);

        let layout = layout(&[&view], 0, 40);

        assert_eq!(texts(&layout), ["▶ c1 open", "  > one", "  > two"]);
    }

    #[test]
    fn a_resolved_thread_says_who_resolved_it_and_is_dim() {
        let view = comment(
            thread("c2", "q", resolved(), State::Current),
            "scott",
            "2026-10-12T09:00:00Z",
            "hi",
        );

        let layout = layout(&[&view], 0, 40);

        assert_eq!(layout.lines[0].text, "▶ c2 resolved by claude");
        assert!(layout.lines.iter().all(|l| l.dim));
    }

    #[test]
    fn an_outdated_thread_is_labeled() {
        let open = thread("c1", "q", Status::Open, State::Outdated);
        let done = thread("c2", "q", resolved(), State::Outdated);

        let layout = layout(&[&open, &done], 0, 40);

        assert_eq!(layout.lines[0].text, "▶ c1 open · outdated");
        assert_eq!(
            layout.lines[layout.thread_starts[1]].text,
            "  c2 resolved by claude · outdated"
        );
    }

    #[test]
    fn threads_stack_with_a_blank_row_and_the_selection_marks_one_header() {
        let (a, b) = (one(), one());

        let layout = layout(&[&a, &b], 1, 40);

        assert_eq!(layout.thread_starts, [0, 7]);
        assert_eq!(layout.lines[0].text, "  c1 open");
        assert_eq!(layout.lines[6].text, "");
        assert_eq!(layout.lines[7].text, "▶ c1 open");
    }

    #[test]
    fn bodies_wrap_at_the_width_and_keep_blank_lines() {
        let view = comment(
            thread("c1", "q", Status::Open, State::Current),
            "claude",
            "2026-10-12T09:00:00Z",
            "alpha beta gamma\n\ndelta",
        );

        let layout = layout(&[&view], 0, 14); // 10 chars after the indent

        assert_eq!(
            last(&layout, 4),
            ["    alpha beta ", "    gamma", "    ", "    delta"],
            "the space a row breaks on hangs on its end"
        );
    }

    #[test]
    fn a_word_longer_than_the_row_is_hard_broken() {
        let view = comment(
            thread("c1", "q", Status::Open, State::Current),
            "claude",
            "2026-10-12T09:00:00Z",
            "abcdefghij",
        );

        let layout = layout(&[&view], 0, 10); // 6 chars after the indent

        assert_eq!(last(&layout, 2), ["    abcdef", "    ghij"]);
    }

    #[test]
    fn unrecognised_timestamps_are_shown_raw() {
        let view = comment(
            thread("c1", "q", Status::Open, State::Current),
            "x",
            "yesterday",
            "b",
        );

        let layout = layout(&[&view], 0, 40);

        assert_eq!(layout.lines[2].text, "  x · yesterday");
    }

    fn body(text: &str) -> ThreadView {
        comment(
            thread("c1", "q", Status::Open, State::Current),
            "claude",
            "2026-10-12T09:00:00Z",
            text,
        )
    }

    #[test]
    fn wide_characters_fill_rows_by_cells() {
        let view = body("一二三四五六七八九十一二");

        let layout = layout(&[&view], 0, 14); // 10 cells after the indent

        assert_eq!(
            last(&layout, 3),
            ["    一二三四五", "    六七八九十", "    一二"]
        );
    }

    #[test]
    fn a_grapheme_cluster_is_never_split_across_rows() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let view = body(&family.repeat(3));

        let layout = layout(&[&view], 0, 6); // 2 cells after the indent

        let row = format!("    {family}");
        assert_eq!(last(&layout, 3), [row.as_str(); 3]);
    }

    #[test]
    fn whitespace_in_a_body_is_kept_as_written() {
        let view = body("  - item\n  - item  two");

        let layout = layout(&[&view], 0, 40);

        assert_eq!(texts(&layout)[3..], ["      - item", "      - item  two"]);
    }

    #[test]
    fn long_header_and_author_lines_wrap() {
        let status = Status::Resolved {
            resolved_by: "a-very-long-resolver-name".into(),
            resolved_at: "2026-10-12T11:00:00Z".into(),
        };
        let view = comment(
            thread("c12", "q", status, State::Outdated),
            "山田太郎山田太郎",
            "2026-10-12T09:00:00Z",
            "b",
        );

        let layout = layout(&[&view], 0, 20);

        assert_eq!(
            texts(&layout),
            [
                "▶ c12 resolved by ",
                "a-very-long-resolver",
                "-name · outdated",
                "  > q",
                "  山田太郎山田太郎 · ",
                "2026-10-12 09:00Z",
                "    b",
            ]
        );
    }

    #[test]
    fn a_newline_in_a_name_is_a_row_break() {
        let status = Status::Resolved {
            resolved_by: "ops\nbot".into(),
            resolved_at: "2026-10-12T11:00:00Z".into(),
        };
        let view = comment(
            thread("c1", "q", status, State::Current),
            "a\nb",
            "2026-10-12T09:00:00Z",
            "x",
        );

        let layout = layout(&[&view], 0, 40);

        assert_eq!(
            texts(&layout),
            [
                "▶ c1 resolved by ops",
                "bot",
                "  > q",
                "  a",
                "b · 2026-10-12 09:00Z",
                "    x",
            ]
        );
    }

    const CORPUS: &[&str] = &[
        "plain ascii words that run on past the edge",
        "漢字かなカナ混じりの文、そして English words too",
        "emoji 👨\u{200d}👩\u{200d}👧👨\u{200d}👩\u{200d}👧 and 🎉🎉🎉",
        "e\u{301}e\u{301}e\u{301} combining marks e\u{301}",
        "\tTabbed\tcolumns",
        "first\n\nthird after a blank\n",
        "",
    ];

    #[test]
    fn every_panel_row_fits_the_panel() {
        for text in CORPUS {
            let view = comment(
                thread("c1", text, Status::Open, State::Outdated),
                text,
                "2026-10-12T09:00:00Z",
                text,
            );
            for width in 5..=20 {
                for line in layout(&[&view], 0, width).lines {
                    let shown = line.text.trim_end();
                    // A cluster wider than the room after the indent still
                    // gets a row of its own.
                    let content = [QUOTE_INDENT, BODY_INDENT]
                        .iter()
                        .find_map(|indent| shown.strip_prefix(indent))
                        .unwrap_or(shown);
                    let one_cluster = ratatui::text::Span::raw(content)
                        .styled_graphemes(ratatui::style::Style::new())
                        .count()
                        == 1;
                    assert!(
                        wrap::width(shown) <= width || one_cluster,
                        "{:?} overflows {width} for {text:?}",
                        line.text
                    );
                }
            }
        }
    }

    #[test]
    fn tiny_widths_do_not_panic() {
        let view = one();
        for width in 0..=5 {
            let layout = layout(&[&view], 0, width);
            assert!(!layout.lines.is_empty());
        }
        assert!(layout(&[], 0, 0).lines.is_empty());
    }
}
