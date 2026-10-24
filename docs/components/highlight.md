# `highlight`

**Role:** make a markdown file's structure visible in the file view. It lives at `tui::highlight` and depends on nothing else in the crate, only `pulldown-cmark` and ratatui's `Style` and `Line`. See [tui.md](tui.md) for where it is used.

## Owns

### API
- `highlight(lines) -> Vec<Styling>` parses the lines, joined by `\n`, as one CommonMark document (strikethrough and tables on, nothing else).
- It returns one `Styling { line: Style, spans: Vec<(Range<usize>, Style)> }` per line: a whole-line style plus inline spans in byte offsets of the raw line, outermost first, cut at line boundaries.
- `Styling::apply(line) -> Line` builds the styled line.
- `is_markdown(path)` is true for a `md`, `markdown` or `mdx` extension in any case. Only those files are highlighted.

### Styles
- Heading lines bold magenta; strong bold, emphasis italic, strikethrough crossed out.
- Inline code cyan, backticks included.
- Fenced and indented code blocks: dim fence lines and a cyan body, no markup parsed inside.
- List markers bold green, the marker only.
- Block quotes: dim `>` markers and italic text.
- Links: underlined text and a dim `(target)`.
- A table's header line bold.
- The style constants sit together at the top of the module.

## Invariants
- Highlighting never changes text: delimiters stay visible, `apply` yields exactly the raw line, and spans are non-empty and on `char` boundaries.
- Only modifiers and the 16 named ANSI colors, which the terminal theme sets for light or dark. No RGB, no 256-color indexes, no backgrounds (the cursor owns `bg`).
- Off when `NO_COLOR` is set and non-empty, decided once in `cli`. The TUI's own chrome (cursor, gutter glyphs) keeps its colors.
- Styling is recomputed only when a re-read changes the lines; a resize only re-wraps.

## Contract
`highlight`, `Styling` and `is_markdown` are the whole surface; `tui` calls them and applies the result.

## Tests
Examples per element (nesting, emphasis across a line break, unclosed fences, quoted lists), plus a property over a fixed corpus: one styling per line, spans in bounds and on `char` boundaries, and the styled line's text is the raw line.
