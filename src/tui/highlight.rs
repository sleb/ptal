//! Markdown structure highlighting for the file view.
//!
//! [`highlight`] parses the file's lines as CommonMark (with GitHub
//! strikethrough and tables) using `pulldown-cmark`, and gives each line a
//! [`Styling`]: a whole-line style plus inline spans in byte offsets of the
//! raw line. A construct spanning lines (a strong run across a line break, a
//! block quote) is cut at each line. Pure: no I/O, no ratatui widgets.
//!
//! What is styled, all with modifiers or one of the 16 named ANSI colors,
//! which the terminal's theme picks for light or dark, and never a
//! background (the cursor owns that):
//!
//! | Element | Style |
//! |---|---|
//! | Heading (ATX or setext) | whole line bold magenta |
//! | `**strong**` / `*emphasis*` / `~~strike~~` | bold / italic / crossed out |
//! | `` `inline code` `` | cyan, backticks included |
//! | Fenced or indented code block | fence lines dim, body cyan, no markup inside |
//! | List marker (`-`, `*`, `+`, `1.`, `1)`) | bold green, the marker only |
//! | Block quote | `>` markers dim, quoted text italic |
//! | Link | text underlined, the `(target)` dim |
//! | Table header line | whole line bold |
//!
//! The file view drops `DIM` on the cursor row, where faint text on the
//! cursor background is hard to read.
//!
//! Highlighting never changes text: delimiters stay visible, and
//! [`Styling::apply`] yields exactly the raw line. Code inside fenced blocks
//! is not highlighted by language.

use std::ops::Range;
use std::path::Path;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const HEADING: Style = Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD);
const STRONG: Style = Style::new().add_modifier(Modifier::BOLD);
const EMPHASIS: Style = Style::new().add_modifier(Modifier::ITALIC);
const STRIKETHROUGH: Style = Style::new().add_modifier(Modifier::CROSSED_OUT);
const CODE: Style = Style::new().fg(Color::Cyan);
const FENCE: Style = Style::new().add_modifier(Modifier::DIM);
const LIST_MARKER: Style = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
const QUOTE_MARKER: Style = Style::new().add_modifier(Modifier::DIM);
const QUOTE: Style = Style::new().add_modifier(Modifier::ITALIC);
const LINK_TEXT: Style = Style::new().add_modifier(Modifier::UNDERLINED);
const LINK_TARGET: Style = Style::new().add_modifier(Modifier::DIM);
const TABLE_HEADER: Style = Style::new().add_modifier(Modifier::BOLD);

/// How one file line is styled: a whole-line style plus inline spans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Styling {
    /// Style of the whole line, under its spans.
    pub line: Style,
    /// Inline styles over byte ranges of the raw line, outermost first; a
    /// later span patches over an earlier one where they overlap. Ranges are
    /// non-empty and lie on `char` boundaries.
    pub spans: Vec<(Range<usize>, Style)>,
}

impl Styling {
    /// `line` (the raw line this styling was made for) as a [`Line`] with
    /// [`Styling::line`] as its style and one span per run of equal style.
    /// Its text is exactly `line`.
    pub fn apply(&self, line: &str) -> Line<'static> {
        let mut cuts: Vec<usize> = self
            .spans
            .iter()
            .flat_map(|(range, _)| [range.start, range.end])
            .chain([0, line.len()])
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let spans: Vec<Span<'static>> = cuts
            .windows(2)
            .map(|cut| {
                let piece = cut[0]..cut[1];
                let style = self
                    .spans
                    .iter()
                    .filter(|(range, _)| range.start <= piece.start && piece.end <= range.end)
                    .fold(Style::new(), |style, (_, over)| style.patch(*over));
                Span::styled(line[piece].to_owned(), style)
            })
            .collect();
        Line::from(spans).style(self.line)
    }
}

/// Whether `path` names a markdown file: its extension is `md`, `markdown`
/// or `mdx`, in any case. Only markdown files are highlighted; anything
/// else is shown plain.
pub fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        ["md", "markdown", "mdx"]
            .iter()
            .any(|md| ext.eq_ignore_ascii_case(md))
    })
}

/// One [`Styling`] per line of `lines` (a file split by [`str::lines`]),
/// parsed as one markdown document.
pub fn highlight(lines: &[String]) -> Vec<Styling> {
    // `str::lines` ends lines at `\n` only, but CommonMark also ends one
    // at a lone `\r`. A space keeps every offset and the line structure the
    // rest of ptal sees.
    let source = lines.join("\n").replace('\r', " ");
    let mut doc = Document::new(lines);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    // The open code block's whole range and its body text so far.
    let mut code_block: Option<(Range<usize>, Vec<Range<usize>>)> = None;
    // The open link (links never nest): the index in `doc.spans` its own
    // spans go in at, so they stay outside the spans inside it, and the
    // range of its text so far.
    let mut link: Option<(usize, Option<Range<usize>>)> = None;
    // The open outermost block quote: the index in `doc.spans` its spans go
    // in at, its range, how deep quotes are nested now, and the deepest seen.
    // A quote is styled once, at its outermost end, knowing its depth.
    let mut quote: Option<(usize, Range<usize>, usize, usize)> = None;
    for (event, range) in Parser::new_ext(&source, options).into_offset_iter() {
        if let Some((_, text)) = &mut link
            && !matches!(event, Event::End(TagEnd::Link))
        {
            *text = Some(match text.take() {
                Some(text) => text.start.min(range.start)..text.end.max(range.end),
                None => range.clone(),
            });
        }
        match event {
            Event::Start(Tag::Heading { .. }) => doc.style_lines(&range, HEADING),
            Event::Start(Tag::TableHead) => doc.style_lines(&range, TABLE_HEADER),
            Event::Start(Tag::Strong) => doc.span(range, STRONG),
            Event::Start(Tag::Emphasis) => doc.span(range, EMPHASIS),
            Event::Start(Tag::Strikethrough) => doc.span(range, STRIKETHROUGH),
            Event::Code(_) => doc.span(range, CODE),
            Event::Start(Tag::Item) => {
                let marker = marker_len(&source[range.clone()]);
                doc.span(range.start..range.start + marker, LIST_MARKER);
            }
            Event::Start(Tag::BlockQuote(_)) => match &mut quote {
                Some((_, _, depth, deepest)) => {
                    *depth += 1;
                    *deepest = (*deepest).max(*depth);
                }
                None => quote = Some((doc.spans.len(), range, 1, 1)),
            },
            Event::End(TagEnd::BlockQuote(_)) => match &mut quote {
                Some((_, _, depth, _)) if *depth > 1 => *depth -= 1,
                _ => {
                    if let Some((at, range, _, deepest)) = quote.take() {
                        let own = doc.quote(&source, &range, deepest);
                        doc.spans.splice(at..at, own);
                    }
                }
            },
            Event::Start(Tag::CodeBlock(_)) => code_block = Some((range, Vec::new())),
            Event::Text(_) if code_block.is_some() => {
                if let Some((_, body)) = &mut code_block {
                    body.push(range.clone());
                }
                doc.span(range, CODE);
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((block, body)) = code_block.take() {
                    doc.fences(&block, &body);
                }
            }
            Event::Start(Tag::Link { .. }) => link = Some((doc.spans.len(), None)),
            Event::End(TagEnd::Link) => {
                if let Some((at, text)) = link.take() {
                    // The text's own range ends inside its `]`; the rest is
                    // the target, empty for an autolink.
                    let text = text.unwrap_or(range.start + 1..range.start + 1);
                    let target = (text.end + 1).min(range.end)..range.end;
                    let own = [(text, LINK_TEXT), (target, LINK_TARGET)];
                    doc.spans
                        .splice(at..at, own.into_iter().filter(|(r, _)| !r.is_empty()));
                }
            }
            _ => {}
        }
    }
    doc.finish()
}

/// Bytes of the list marker that `item` (an item's source, from its
/// marker) starts with: `-`, `*` or `+`, or digits and `.` or `)`.
fn marker_len(item: &str) -> usize {
    let digits = item.bytes().take_while(u8::is_ascii_digit).count();
    match item.as_bytes().get(digits) {
        Some(b'-' | b'*' | b'+') if digits == 0 => 1,
        Some(b'.' | b')') if digits > 0 => digits + 1,
        _ => 0,
    }
}

/// Styles gathered in source offsets, then cut into lines by
/// [`Document::finish`].
struct Document {
    /// Byte offset in the joined source where each line starts.
    starts: Vec<usize>,
    lens: Vec<usize>,
    line_styles: Vec<Style>,
    spans: Vec<(Range<usize>, Style)>,
}

impl Document {
    fn new(lines: &[String]) -> Self {
        let mut starts = Vec::with_capacity(lines.len());
        let mut at = 0;
        for line in lines {
            starts.push(at);
            at += line.len() + 1;
        }
        Document {
            starts,
            lens: lines.iter().map(String::len).collect(),
            line_styles: vec![Style::new(); lines.len()],
            spans: Vec::new(),
        }
    }

    /// The 0-based line holding source offset `at`.
    fn line_of(&self, at: usize) -> usize {
        self.starts
            .partition_point(|&start| start <= at)
            .saturating_sub(1)
    }

    /// Every line with a byte in `range`, with the part of it in `range`
    /// (line-relative, never empty).
    fn cut(&self, range: &Range<usize>) -> Vec<(usize, Range<usize>)> {
        if range.is_empty() || self.starts.is_empty() {
            return Vec::new();
        }
        (self.line_of(range.start)..=self.line_of(range.end - 1))
            .filter_map(|line| {
                let start = range.start.saturating_sub(self.starts[line]);
                let end = (range.end - self.starts[line]).min(self.lens[line]);
                (start < end).then_some((line, start..end))
            })
            .collect()
    }

    fn style_lines(&mut self, range: &Range<usize>, style: Style) {
        if range.is_empty() || self.starts.is_empty() {
            return;
        }
        for line in self.line_of(range.start)..=self.line_of(range.end - 1) {
            self.line_styles[line] = self.line_styles[line].patch(style);
        }
    }

    fn span(&mut self, range: Range<usize>, style: Style) {
        if !range.is_empty() {
            self.spans.push((range, style));
        }
    }

    /// The spans of a block quote nested `levels` deep: on each line, the
    /// markers (up to one `>` and one blank after it per level, past any
    /// container indentation) dim and the rest italic. A lazy continuation
    /// line has no markers and is all text; a `>` past the markers is text.
    fn quote(
        &self,
        source: &str,
        range: &Range<usize>,
        levels: usize,
    ) -> Vec<(Range<usize>, Style)> {
        let mut spans = Vec::new();
        for (line, part) in self.cut(range) {
            let start = self.starts[line] + part.start;
            let text = &source.as_bytes()[start..self.starts[line] + part.end];
            let mut markers = 0;
            for _ in 0..levels {
                let blanks = text[markers..]
                    .iter()
                    .take_while(|b| matches!(b, b' ' | b'\t'))
                    .count();
                if text.get(markers + blanks) != Some(&b'>') {
                    break;
                }
                markers += blanks + 1;
                if matches!(text.get(markers), Some(b' ' | b'\t')) {
                    markers += 1;
                }
            }
            let marker = start..start + markers;
            let rest = marker.end..start + text.len();
            spans.extend(
                [(marker, QUOTE_MARKER), (rest, QUOTE)]
                    .into_iter()
                    .filter(|(r, _)| !r.is_empty()),
            );
        }
        spans
    }

    /// Dim the lines of a code `block` outside its `body` (in order, as
    /// parsed): its fences. Every line from the body's first to its last is
    /// body, blank ones too. An indented block has no fences.
    fn fences(&mut self, block: &Range<usize>, body: &[Range<usize>]) {
        let body_lines = match (body.first(), body.last()) {
            (Some(first), Some(last)) if last.end > first.start => {
                Some(self.line_of(first.start)..=self.line_of(last.end - 1))
            }
            _ => None,
        };
        for (line, part) in self.cut(block) {
            if !body_lines.as_ref().is_some_and(|body| body.contains(&line)) {
                let start = self.starts[line];
                self.span(start + part.start..start + part.end, FENCE);
            }
        }
    }

    fn finish(self) -> Vec<Styling> {
        let mut stylings: Vec<Styling> = self
            .line_styles
            .iter()
            .map(|&line| Styling {
                line,
                spans: Vec::new(),
            })
            .collect();
        for (range, style) in &self.spans {
            for (line, part) in self.cut(range) {
                stylings[line].spans.push((part, *style));
            }
        }
        stylings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn bold() -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    fn of(text: &str) -> (Vec<String>, Vec<Styling>) {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let stylings = highlight(&lines);
        (lines, stylings)
    }

    /// Each span of line `at` as the text it covers and its style.
    fn spans(text: &str, at: usize) -> Vec<(String, Style)> {
        let (lines, stylings) = of(text);
        stylings[at]
            .spans
            .iter()
            .map(|(range, style)| (lines[at][range.clone()].to_owned(), *style))
            .collect()
    }

    fn span(text: &str, style: Style) -> (String, Style) {
        (text.to_owned(), style)
    }

    #[test]
    fn a_heading_styles_its_whole_line() {
        let (_, stylings) = of("# Title\n\nSetext\n===\nplain");

        let heading = Style::new().add_modifier(Modifier::BOLD).fg(Color::Magenta);
        assert_eq!(stylings[0].line, heading);
        assert_eq!(stylings[1].line, Style::new());
        assert_eq!(stylings[2].line, heading);
        assert_eq!(stylings[3].line, heading);
        assert_eq!(stylings[4], Styling::default());
    }

    #[test]
    fn strong_emphasis_and_strikethrough_cover_their_delimiters() {
        assert_eq!(spans("**x** y", 0), [span("**x**", bold())]);
        let italic = Style::new().add_modifier(Modifier::ITALIC);
        assert_eq!(
            spans("*x* _y_", 0),
            [span("*x*", italic), span("_y_", italic)]
        );
        let crossed = Style::new().add_modifier(Modifier::CROSSED_OUT);
        assert_eq!(spans("a ~~x~~", 0), [span("~~x~~", crossed)]);
    }

    #[test]
    fn intraword_underscores_and_lone_stars_are_not_emphasis() {
        assert_eq!(spans("snake_case_name and 2 * 3 * 4", 0), []);
    }

    #[test]
    fn inline_code_is_cyan_with_its_backticks_and_hides_markup() {
        let code = Style::new().fg(Color::Cyan);
        assert_eq!(spans("see `a*b*c` here", 0), [span("`a*b*c`", code)]);
    }

    #[test]
    fn a_list_marker_is_styled_and_nothing_else_on_the_line() {
        let marker = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
        assert_eq!(spans("- item", 0), [span("-", marker)]);
        assert_eq!(spans("12. item", 0), [span("12.", marker)]);
        assert_eq!(spans("3) item", 0), [span("3)", marker)]);
        assert_eq!(spans("- a\n  * nested", 1), [span("*", marker)]);
    }

    #[test]
    fn a_block_quote_dims_its_markers_and_italicizes_its_text() {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let italic = Style::new().add_modifier(Modifier::ITALIC);
        assert_eq!(
            spans("> quote", 0),
            [span("> ", dim), span("quote", italic)]
        );
        let marker = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
        assert_eq!(
            spans("> - x", 0),
            [span("> ", dim), span("- x", italic), span("-", marker)]
        );
        assert_eq!(
            spans("> one\n> two", 1),
            [span("> ", dim), span("two", italic)]
        );
    }

    #[test]
    fn a_quote_marker_is_one_gt_and_one_space_per_nesting_level() {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let italic = Style::new().add_modifier(Modifier::ITALIC);
        let code = Style::new().fg(Color::Cyan);
        assert_eq!(
            spans("> ```\n> >>> x\n> ```", 1),
            [span("> ", dim), span(">>> x", italic), span(">>> x", code)]
        );
        assert_eq!(
            spans(">     code", 0),
            [
                span("> ", dim),
                span("    code", italic),
                span("code", code)
            ]
        );
        assert_eq!(
            spans("> > a\n> > b", 1),
            [span("> > ", dim), span("b", italic)]
        );
        assert_eq!(spans("> a\nlazy", 1), [span("lazy", italic)]);
    }

    #[test]
    fn a_link_underlines_its_text_and_dims_its_target() {
        let under = Style::new().add_modifier(Modifier::UNDERLINED);
        let dim = Style::new().add_modifier(Modifier::DIM);
        assert_eq!(
            spans("go [text](http://x) now", 0),
            [span("text", under), span("(http://x)", dim)]
        );
        assert_eq!(spans("<http://y>", 0), [span("http://y", under)]);
    }

    #[test]
    fn a_fenced_block_dims_its_fences_and_colors_its_body_without_markup() {
        let (lines, stylings) = of("```rs\n**not bold**\n```\nafter");

        let dim = Style::new().add_modifier(Modifier::DIM);
        let code = Style::new().fg(Color::Cyan);
        assert_eq!(stylings[0].spans, [(0..5, dim)]);
        assert_eq!(stylings[1].spans, [(0..lines[1].len(), code)]);
        assert_eq!(stylings[2].spans, [(0..3, dim)]);
        assert_eq!(stylings[3], Styling::default());
    }

    #[test]
    fn blank_lines_inside_a_fenced_block_are_body_not_fences() {
        let (_, stylings) = of("```\na\n\nb\n```");

        let dim = Style::new().add_modifier(Modifier::DIM);
        let fences: Vec<bool> = stylings
            .iter()
            .map(|s| s.spans.iter().any(|(_, style)| *style == dim))
            .collect();
        assert_eq!(fences, [true, false, false, false, true]);
    }

    #[test]
    fn a_long_fenced_block_highlights_quickly() {
        let mut lines = vec!["```".to_string()];
        lines.extend((0..40_000).map(|i| format!("log line {i}")));
        lines.push("```".to_string());

        let started = std::time::Instant::now();
        let stylings = highlight(&lines);

        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            stylings[40_001].spans,
            [(0..3, Style::new().add_modifier(Modifier::DIM))]
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end_of_the_file() {
        let code = Style::new().fg(Color::Cyan);
        assert_eq!(
            spans("```\n# not a heading\n**no**", 2),
            [span("**no**", code)]
        );
        assert_eq!(of("```\n# not a heading").1[1].line, Style::new());
    }

    #[test]
    fn an_indented_block_colors_its_body() {
        let code = Style::new().fg(Color::Cyan);
        assert_eq!(
            spans("para\n\n    code **x**", 2),
            [span("code **x**", code)]
        );
    }

    #[test]
    fn a_table_header_line_is_bold_and_its_body_is_not() {
        let (_, stylings) = of("a | b\n--|--\n**x** | y");

        assert_eq!(stylings[0].line, bold());
        assert_eq!(stylings[1].line, Style::new());
        assert_eq!(stylings[2].line, Style::new());
        assert_eq!(spans("a | b\n--|--\n**x** | y", 2), [span("**x**", bold())]);
    }

    #[test]
    fn emphasis_across_a_line_break_is_cut_at_the_line() {
        assert_eq!(spans("**two\nlines**", 0), [span("**two", bold())]);
        assert_eq!(spans("**two\nlines**", 1), [span("lines**", bold())]);
    }

    #[test]
    fn a_lone_carriage_return_does_not_end_a_line() {
        let fence = vec!["a\r```".to_string(), "**x**".to_string()];
        assert_eq!(highlight(&fence)[1].spans, [(0..5, bold())]);

        let heading = vec!["x\r# h".to_string()];
        assert_eq!(highlight(&heading)[0].line, Style::new());
    }

    #[test]
    fn nested_styles_patch_inner_over_outer() {
        let text = "# A **b** `c`";
        let (_, stylings) = of(text);

        assert_eq!(
            stylings[0].line,
            Style::new().add_modifier(Modifier::BOLD).fg(Color::Magenta)
        );
        let code = Style::new().fg(Color::Cyan);
        assert_eq!(spans(text, 0), [span("**b**", bold()), span("`c`", code)]);
        let line = stylings[0].apply(text);
        let styles: Vec<(String, Style)> = line
            .spans
            .iter()
            .map(|s| (s.content.to_string(), line.style.patch(s.style)))
            .collect();
        let heading = Style::new().add_modifier(Modifier::BOLD).fg(Color::Magenta);
        assert_eq!(
            styles,
            [
                span("# A ", heading),
                span("**b**", heading.patch(bold())),
                span(" ", heading),
                span("`c`", heading.patch(code)),
            ]
        );
    }

    #[test]
    fn markdown_is_an_md_markdown_or_mdx_extension_in_any_case() {
        for yes in ["plan.md", "PLAN.MD", "x.markdown", "dir/x.mdx", "x.Md"] {
            assert!(is_markdown(Path::new(yes)), "{yes}");
        }
        for no in ["x.txt", "Makefile", "x.md.bak", ".md", "md", "x.mdown"] {
            assert!(!is_markdown(Path::new(no)), "{no}");
        }
    }

    const CORPUS: &str = "\
# Plan

Some *prose* with **bold**, `code`, ~~gone~~ and [a link](http://x).
A **strong run
across lines** and snake_case_name.

- one
  - two `x`
1. first
12) twelfth

> quoted **text**
> - item in quote
lazy continuation

```rust
fn main() { let x = *y; }
```

    indented code

| a | **b** |
|---|---|
| 漢字 | e\u{301} |

Setext
---
lone\rcarriage **return\r** here
tab\there and 👨\u{200d}👩\u{200d}👧 <http://auto>
```
unclosed";

    #[test]
    fn styling_is_one_per_line_in_bounds_and_never_changes_text() {
        let (lines, stylings) = of(CORPUS);

        assert_eq!(stylings.len(), lines.len());
        for (line, styling) in lines.iter().zip(&stylings) {
            for (range, _) in &styling.spans {
                assert!(range.start < range.end, "empty {range:?} in {line:?}");
                assert!(range.end <= line.len(), "{range:?} past {line:?}");
                assert!(line.is_char_boundary(range.start) && line.is_char_boundary(range.end));
            }
            assert_eq!(styling.apply(line).to_string(), *line);
        }
        assert!(stylings.iter().any(|s| !s.spans.is_empty()));
    }
}
