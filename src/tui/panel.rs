//! The thread panel's content: a pure function from threads and a width to
//! wrapped, styled rows.
//!
//! [`layout`] is the single owner of wrapping, so `update` can clamp scroll
//! from `lines.len()` and `render` can draw a slice without either repeating
//! the arithmetic.

use crate::model::{State, Status};
use crate::review::ThreadView;

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
/// rows of at most `width` chars. `selected` gets a `▶` on its header.
///
/// Wrapping counts `char`s, not display cells; wide glyphs may overflow and
/// are clipped by the buffer edge, as in the file view.
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
        let mut push = |text: String| lines.push(PanelLine { text, dim });

        let marker = if index == selected {
            SELECTED
        } else {
            UNSELECTED
        };
        push(format!("{marker}{}", header(view)));
        for row in indented(&view.thread.anchor.quote, QUOTE_INDENT, width) {
            push(row);
        }
        for comment in &view.thread.comments {
            let who = format!(
                "{AUTHOR_INDENT}{} · {}",
                comment.author,
                time(&comment.created_at)
            );
            push(who);
            for row in indented(&comment.body, BODY_INDENT, width) {
                push(row);
            }
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
/// prefixed with `indent`.
fn indented(text: &str, indent: &str, width: usize) -> Vec<String> {
    let room = width.saturating_sub(indent.chars().count());
    wrap(text, room)
        .into_iter()
        .map(|row| format!("{indent}{row}"))
        .collect()
}

/// Greedy word wrap at `width` chars (at least 1). Words longer than a row
/// are hard-broken; blank lines are kept.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for source in text.split('\n') {
        let mut current = String::new();
        let mut len = 0;
        for word in source.split_whitespace() {
            let mut chars: Vec<char> = word.chars().collect();
            if len > 0 && len + 1 + chars.len() <= width {
                current.push(' ');
                current.extend(&chars);
                len += 1 + chars.len();
                continue;
            }
            if len > 0 {
                rows.push(std::mem::take(&mut current));
            }
            while chars.len() > width {
                let rest = chars.split_off(width);
                rows.push(chars.into_iter().collect());
                chars = rest;
            }
            len = chars.len();
            current = chars.into_iter().collect();
        }
        rows.push(current);
    }
    rows
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
            texts(&layout)[3..],
            ["    alpha beta", "    gamma", "    ", "    delta"]
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

        assert_eq!(texts(&layout)[3..], ["    abcdef", "    ghij"]);
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
