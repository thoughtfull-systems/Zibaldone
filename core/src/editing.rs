//! What an editor needs to show markdown styled as it's written: the styled elements of a body,
//! with the syntax to hide while the cursor is away, and the edit that toggles a task box.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::Id;
use crate::body::{self, Mark};
use crate::lists::{Edit, Item as ListItem};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Style {
    BlockQuote,
    /// A task item's box, `[ ]` or `[x]`, and whether it's checked.
    Checkbox(bool),
    Code,
    CodeBlock,
    /// The text of a task item that's checked.
    Completed,
    Emphasis,
    /// A heading of level 1 to 6.
    Heading(u8),
    Highlight,
    /// A markdown link, wikilink, or embed.
    Link,
    /// The indentation, marker, and box that start a list item's line, or the indentation of a
    /// line that continues one. Editors draw these in a monospace font, so that continuation lines
    /// and nested items line up with the text above them.
    ListPrefix,
    Math,
    Strikethrough,
    Strong,
    Underline,
}

/// A styled element of a note's body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    /// Byte range of the whole element, including its syntax.
    pub range: Range<usize>,
    pub style: Style,
    /// Byte ranges of the syntax to hide while the cursor is away from the element. None of them
    /// contain a line break.
    pub syntax: Vec<Range<usize>>,
}

impl Span {
    /// Whether the cursor or selection `range` touches the element, so its syntax should show.
    pub fn touches(&self, range: &Range<usize>) -> bool {
        self.range.start <= range.end && range.start <= self.range.end
    }
}

/// Finds the styled elements of `body`, ordered by where they start.
///
/// A heading without text, such as `##` while it's being typed, stays plain text until text
/// follows its space, and setext headings stay paragraphs, as the dialect has none.
pub fn spans(body: &str) -> Vec<Span> {
    struct Frame {
        /// Bytes covered by the element's children: its content, without its syntax.
        inner: Option<Range<usize>>,
        range: Range<usize>,
        style: Option<Style>,
    }

    let mut spans = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    for (event, range) in Parser::new_ext(body, body::options()).into_offset_iter() {
        if !matches!(event, Event::End(_))
            && let Some(parent) = stack.last_mut()
        {
            parent.inner = Some(match parent.inner.take() {
                Some(inner) => inner.start.min(range.start)..inner.end.max(range.end),
                None => range.clone(),
            });
        }
        match event {
            Event::Start(tag) => {
                let style = match tag {
                    Tag::BlockQuote(_) => Some(Style::BlockQuote),
                    Tag::CodeBlock(_) => Some(Style::CodeBlock),
                    Tag::Emphasis => Some(Style::Emphasis),
                    Tag::Heading { level, .. } => Some(Style::Heading(level as u8)),
                    Tag::Link { .. } => Some(Style::Link),
                    Tag::Strikethrough => Some(Style::Strikethrough),
                    Tag::Strong => Some(Style::Strong),
                    _ => None,
                };
                stack.push(Frame { inner: None, range, style });
            }
            Event::End(_) => {
                let Some(Frame { inner, range, style: Some(style) }) = stack.pop() else {
                    continue;
                };
                // pulldown-cmark can report a child outside its parent: in a task item that turns
                // into a setext heading, the box comes before the heading's start.
                let inner = inner
                    .map(|inner| inner.start.max(range.start)..inner.end.min(range.end))
                    .filter(|inner| inner.start < inner.end);
                if matches!(style, Style::Heading(_))
                    && (inner.is_none() || !body[range.clone()].trim_start().starts_with('#'))
                {
                    continue;
                }
                let syntax = match (style, inner) {
                    (Style::BlockQuote, _) => quote_markers(body, &range),
                    (Style::CodeBlock, _) | (_, None) => Vec::new(),
                    (_, Some(inner)) => vec![range.start..inner.start, inner.end..range.end],
                };
                spans.push(span(body, range, style, syntax));
            }
            Event::Code(_) => {
                spans.push(delimited(body, range, b'`', Style::Code));
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                spans.push(delimited(body, range, b'$', Style::Math));
            }
            Event::TaskListMarker(checked) => {
                if checked {
                    let line_end =
                        body[range.end..].find('\n').map_or(body.len(), |i| range.end + i);
                    let rest = &body[range.end..line_end];
                    let text_start = line_end - rest.trim_start().len();
                    spans.push(span(body, text_start..line_end, Style::Completed, Vec::new()));
                }
                spans.push(span(body, range, Style::Checkbox(checked), Vec::new()));
            }
            _ => {}
        }
    }
    marks(body, &mut spans);
    spans.extend(list_prefixes(body));
    for link in body::wikilinks(body) {
        let Range { start, end } = link.range;
        let open = start + if link.embed { 3 } else { 2 };
        let separator = label_separator(&body[open..end - 2]).map(|i| open + i);
        let label_start = separator.map_or(open, |i| i + 1);
        // A wikilink ends at the first unescaped `]]`, so every backslash inside starts an escape.
        let mut escapes = Vec::new();
        let mut i = label_start;
        while i < end - 2 {
            if body.as_bytes()[i] == b'\\' {
                escapes.push(i..i + 1);
                i += 2;
            } else {
                i += 1;
            }
        }
        let syntax: Vec<Range<usize>> = std::iter::once(start..label_start)
            .chain(escapes)
            .chain(std::iter::once(end - 2..end))
            .collect();
        spans.push(span(body, start..end, Style::Link, syntax));
    }
    spans.sort_by_key(|span| span.range.start);
    spans
}

/// What an editor draws in place of an element's source while the cursor is away from it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Drawing {
    InlineMath(String),
    DisplayMath(String),
    Diagram(String),
    /// A list table, whose cells `markup::parse` reads.
    Table(body::ListTable),
    Embed(Id),
}

/// An element that an editor draws.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Drawn {
    /// Byte range of the element's source, which ends before any line break that follows it.
    pub range: Range<usize>,
    pub drawing: Drawing,
    /// Whether the drawing takes the place of the lines its source spans, rather than sitting
    /// inside a line of text: the source fills its lines, or it's display math across lines.
    pub block: bool,
}

impl Drawn {
    /// Whether the cursor or selection `range` touches the element, so its source should show.
    pub fn touches(&self, range: &Range<usize>) -> bool {
        self.range.start <= range.end && range.start <= self.range.end
    }
}

/// Finds the math, diagrams, and embeds of `body` that an editor draws, ordered by where they
/// start.
pub fn drawings(body: &str) -> Vec<Drawn> {
    let fills_lines = |range: &Range<usize>| {
        let line_start = body[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = body[range.end..].find('\n').map_or(body.len(), |i| range.end + i);
        body[line_start..range.start].trim().is_empty()
            && body[range.end..line_end].trim().is_empty()
    };
    let mut drawn = Vec::new();
    let mut code: Option<(Range<usize>, String, String)> = None;
    for (event, range) in Parser::new_ext(body, body::options()).into_offset_iter() {
        match event {
            Event::InlineMath(source) => drawn.push(Drawn {
                range,
                drawing: Drawing::InlineMath(source.into_string()),
                block: false,
            }),
            Event::DisplayMath(source) => {
                // Display math across lines takes their place too, between any text before and
                // after it on its first and last lines.
                let block = fills_lines(&range) || body[range.clone()].contains('\n');
                drawn.push(Drawn {
                    range,
                    drawing: Drawing::DisplayMath(source.into_string()),
                    block,
                });
            }
            Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(language))) => {
                code = Some((range, language.into_string(), String::new()));
            }
            Event::Text(text) => {
                if let Some((_, _, source)) = code.as_mut() {
                    source.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let Some((range, info, source)) = code.take() else { continue };
                let end = range.start + body[range.clone()].trim_end_matches(['\n', '\r']).len();
                let drawing = match info.split_whitespace().next().unwrap_or("") {
                    "math" => Drawing::DisplayMath(source),
                    "mermaid" => Drawing::Diagram(source),
                    "table" => match body::list_table(&info, &source) {
                        Some(table) => Drawing::Table(table),
                        None => continue,
                    },
                    _ => continue,
                };
                drawn.push(Drawn { range: range.start..end, drawing, block: true });
            }
            _ => {}
        }
    }
    for link in body::wikilinks(body).into_iter().filter(|link| link.embed) {
        let block = fills_lines(&link.range);
        drawn.push(Drawn { range: link.range, drawing: Drawing::Embed(link.target), block });
    }
    drawn.sort_by_key(|drawn| drawn.range.start);
    drawn
}

/// The source of a cell's text in the list table at `block` of `body`, the range of a drawn table,
/// as a byte range of `body`. `row`, `column`, and `nested` pick the cell, as for
/// `body::table_cell_source`.
pub fn table_cell_in_body(
    body: &str,
    block: Range<usize>,
    row: usize,
    column: usize,
    nested: &[(usize, usize, usize)],
) -> Option<Range<usize>> {
    let fenced = &body[block.clone()];
    let start = block.start + fenced.find('\n')? + 1;
    let last_line = block.start + fenced.rfind('\n').map_or(0, |i| i + 1);
    let closing = body[last_line..block.end].trim_start();
    let closed = last_line > start && (closing.starts_with("```") || closing.starts_with("~~~"));
    let end = if closed { last_line - 1 } else { block.end };
    // A table nested in a list item has the item's indentation, which its content drops.
    let line_start = body[..block.start].rfind('\n').map_or(0, |i| i + 1);
    let indent = body[line_start..block.start].chars().count();
    let lines = crate::lists::Dedented::new(&body[start..end.max(start)], indent);
    let cell = body::table_cell_source(&lines.text, row, column, nested)?;
    Some(start + lines.original(cell.start)..start + lines.original(cell.end))
}

/// Where a link points.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkTarget {
    Note(Id),
    Web(String),
}

/// The target of the wikilink, embed, or markdown link that byte `offset` of `body` falls in.
pub fn link_at(body: &str, offset: usize) -> Option<LinkTarget> {
    if let Some(link) = body::wikilinks(body).into_iter().find(|l| l.range.contains(&offset)) {
        return Some(LinkTarget::Note(link.target));
    }
    Parser::new_ext(body, body::options()).into_offset_iter().find_map(|(event, range)| match event
    {
        Event::Start(Tag::Link { dest_url, .. }) if range.contains(&offset) => {
            Some(LinkTarget::Web(dest_url.into_string()))
        }
        _ => None,
    })
}

/// The byte offset of the first unescaped `|` in the inside of a wikilink.
fn label_separator(inner: &str) -> Option<usize> {
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'|' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// A code span or math, whose syntax is the run of `delimiter` at each end.
fn delimited(body: &str, range: Range<usize>, delimiter: u8, style: Style) -> Span {
    let count = body[range.clone()].bytes().take_while(|&b| b == delimiter).count();
    let count = count.min(range.len() / 2);
    let syntax = vec![range.start..range.start + count, range.end - count..range.end];
    span(body, range, style, syntax)
}

/// The underlines and highlights of `body`, which the note bodies specification marks with `=`.
fn marks(body: &str, spans: &mut Vec<Span>) {
    for marked in body::marks(body) {
        let whole = marked.open.start..marked.close.end;
        let syntax = vec![marked.open, marked.close];
        match marked.mark {
            Mark::Underline => spans.push(span(body, whole, Style::Underline, syntax)),
            Mark::Highlight => spans.push(span(body, whole, Style::Highlight, syntax)),
            Mark::Both => {
                spans.push(span(body, whole.clone(), Style::Highlight, syntax));
                spans.push(span(body, whole, Style::Underline, Vec::new()));
            }
        }
    }
}

/// Finds the innermost list item that byte `offset` falls in, and returns the edit that toggles
/// its box: the byte range to replace, and the text to replace it with. An item without a box gets
/// an unchecked one. An item covers its lines from their start, including indentation, up to the
/// end of its text, but not its nested items.
pub fn toggle_checkbox(body: &str, offset: usize) -> Option<(Range<usize>, &'static str)> {
    struct OpenItem {
        /// The box's range and whether it's checked, once the parser reports it.
        checkbox: Option<(Range<usize>, bool)>,
        range: Range<usize>,
    }

    let mut open: Vec<OpenItem> = Vec::new();
    let mut items = Vec::new();
    for (event, range) in Parser::new_ext(body, body::options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Item) => open.push(OpenItem { checkbox: None, range }),
            Event::TaskListMarker(checked) => {
                if let Some(item) = open.last_mut() {
                    item.checkbox = Some((range, checked));
                }
            }
            Event::End(TagEnd::Item) => {
                let Some(OpenItem { checkbox, range: item }) = open.pop() else { continue };
                let line_start = body[..item.start].rfind('\n').map_or(0, |i| i + 1);
                let start = if body[line_start..item.start].trim().is_empty() {
                    line_start
                } else {
                    item.start
                };
                let text_end = item.start + body[item.clone()].trim_end().len();
                let trailing = body[text_end..].bytes().take_while(|b| matches!(b, b' ' | b'\t'));
                let end = text_end + trailing.count();
                let edit = if let Some((mark, checked)) = checkbox {
                    (mark.start + 1..mark.end - 1, if checked { " " } else { "x" })
                } else {
                    let line_end =
                        body[line_start..].find('\n').map_or(body.len(), |i| line_start + i);
                    let line = &body[line_start..line_end];
                    let Some(parsed) = ListItem::parse(line) else {
                        continue;
                    };
                    let at = line_start + parsed.content_start;
                    // A box only counts as one when text follows it, but an item whose only
                    // text is a box should toggle it, not get a second box.
                    let lone_box = parsed.checkbox.then(|| line[..parsed.content_start].rfind('['));
                    if let Some(open) = lone_box.flatten() {
                        let mark = line_start + open + 1..line_start + open + 2;
                        let checked = body[mark.clone()] != *" ";
                        (mark, if checked { " " } else { "x" })
                    } else {
                        let spaced = body[..at].ends_with(' ');
                        (at..at, if spaced { "[ ] " } else { " [ ] " })
                    }
                };
                items.push((start..end, edit));
            }
            _ => {}
        }
    }
    items
        .into_iter()
        .filter(|(item, _)| item.start <= offset && offset <= item.end)
        .min_by_key(|(item, _)| item.len())
        .map(|(_, edit)| edit)
}

fn list_prefixes(body: &str) -> Vec<Span> {
    let mut prefixes = Vec::new();
    let mut depth = 0;
    let mut list_start = 0;
    for (event, range) in Parser::new_ext(body, body::options()).into_offset_iter() {
        match event {
            Event::Start(Tag::List(_)) => {
                if depth == 0 {
                    list_start = range.start;
                }
                depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                depth -= 1;
                if depth > 0 {
                    continue;
                }
                let mut line_start = body[..list_start].rfind('\n').map_or(0, |i| i + 1);
                while line_start < range.end {
                    let line_end =
                        body[line_start..].find('\n').map_or(body.len(), |i| line_start + i);
                    let line = &body[line_start..line_end];
                    let prefix = ListItem::parse(line).map_or_else(
                        || line.len() - line.trim_start_matches(' ').len(),
                        |item| item.content_start,
                    );
                    if prefix > 0 {
                        let range = line_start..line_start + prefix;
                        prefixes.push(span(body, range, Style::ListPrefix, Vec::new()));
                    }
                    line_start = line_end + 1;
                }
            }
            _ => {}
        }
    }
    prefixes
}

/// Finds the `>` that starts each line of a block quote, with the space after it.
fn quote_markers(body: &str, range: &Range<usize>) -> Vec<Range<usize>> {
    let mut markers = Vec::new();
    let mut line_start = range.start;
    for line in body[range.clone()].split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if line[indent..].starts_with('>') {
            let space = usize::from(line[indent + 1..].starts_with(' '));
            markers.push(line_start..line_start + indent + 1 + space);
        }
        line_start += line.len();
    }
    markers
}

/// Builds a span whose range ends before any trailing line break, so a cursor on the next line
/// doesn't touch it, and whose syntax never hides a line break.
fn span(body: &str, mut range: Range<usize>, style: Style, syntax: Vec<Range<usize>>) -> Span {
    while range.end > range.start && matches!(body.as_bytes()[range.end - 1], b'\n' | b'\r') {
        range.end -= 1;
    }
    let syntax = syntax
        .into_iter()
        .filter(|range| range.start < range.end)
        .flat_map(|range| {
            let mut pieces = Vec::new();
            let mut piece_start = range.start;
            for (i, byte) in body[range.clone()].bytes().enumerate() {
                if matches!(byte, b'\n' | b'\r') {
                    pieces.push(piece_start..range.start + i);
                    piece_start = range.start + i + 1;
                }
            }
            pieces.push(piece_start..range.end);
            pieces
        })
        .filter(|piece| !piece.is_empty())
        .collect();
    Span { range, style, syntax }
}

/// The edit that typing `typed` at byte `cursor` makes when it completes the opening of a block
/// at the start of a line: a third backtick opens a code block, with the cursor after the fence
/// for its language, and a second `$` opens display math, with the cursor inside. The closing line
/// follows at the opening's indentation, so a block in a list item stays in it.
pub fn complete_block_opening(body: &str, cursor: usize, typed: char) -> Option<Edit> {
    let run = match typed {
        '`' => "``",
        '$' => "$",
        _ => return None,
    };
    let line_end = body[cursor..].find('\n').map_or(body.len(), |i| cursor + i);
    if !body[..cursor].ends_with(run) || !body[cursor..line_end].trim().is_empty() {
        return None;
    }
    let start = cursor - run.len();
    if body[..start].ends_with(typed) || in_code_block(body, start) {
        return None;
    }
    let column = block_column(body, start)?;
    let indent = " ".repeat(column);
    let (text, into) = match typed {
        '`' => (format!("`\n{indent}```"), 1),
        _ => (format!("$\n{indent}\n{indent}$$"), 2 + indent.len()),
    };
    Some(Edit { cursor: cursor + into, range: cursor..line_end, text })
}

/// The edit that typing `typed` makes with the byte range `selection` selected, when the inline
/// form of a block wraps it and fills the rest of its lines: a third backtick around a code span
/// makes a code block, and a second `$` around inline math makes display math. The selection
/// becomes the block's content and stays selected. Returns the edit and the selection's range
/// after it.
pub fn fence_selection(
    body: &str,
    selection: Range<usize>,
    typed: char,
) -> Option<(Edit, Range<usize>)> {
    let (wrap, fence) = match typed {
        '`' => ("``", "```"),
        '$' => ("$", "$$"),
        _ => return None,
    };
    let open = selection.start.checked_sub(wrap.len())?;
    let close = selection.end + wrap.len();
    if body.get(open..selection.start)? != wrap || body.get(selection.end..close)? != wrap {
        return None;
    }
    let line_end = body[close..].find('\n').map_or(body.len(), |i| close + i);
    if body[..open].ends_with(typed)
        || body[close..].starts_with(typed)
        || !body[close..line_end].trim().is_empty()
        || in_code_block(body, open)
    {
        return None;
    }
    let column = block_column(body, open)?;
    let indent = " ".repeat(column);
    let selected = &body[selection];
    let text = format!("{fence}\n{indent}{selected}\n{indent}{fence}");
    let start = open + fence.len() + 1 + indent.len();
    let selected = start..start + selected.len();
    Some((Edit { cursor: selected.end, range: open..line_end, text }, selected))
}

/// Whether byte `at` falls inside a code block or another construct whose text is literal, past
/// its start.
fn in_code_block(body: &str, at: usize) -> bool {
    body::code_ranges(body).iter().any(|range| range.start < at && at < range.end)
}

/// The column of byte `at` in its line, when only indentation, or a list item's marker and box,
/// come before it there, so a block can start at it.
fn block_column(body: &str, at: usize) -> Option<usize> {
    let line_start = body[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
    let before = &body[line_start..at];
    let after_marker = ListItem::parse(&body[line_start..line_end])
        .is_some_and(|item| item.content_start == before.len());
    (before.trim().is_empty() || after_marker).then(|| before.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::ListTable;

    fn hidden(body: &str) -> Vec<(Style, Vec<&str>)> {
        spans(body)
            .into_iter()
            .map(|span| (span.style, span.syntax.iter().map(|r| &body[r.clone()]).collect()))
            .collect()
    }

    #[test]
    fn finds_drawings() {
        let target = Id::generate();
        let body = [
            "Inline $x^2$ here.\n\n$$\na + b\n$$\n\n",
            "```mermaid\nflowchart LR\n  a --> b\n```\n\n```rust\nfn main() {}\n```\n\n",
            // A list table draws, and a `table` block that isn't one stays code.
            "```table header-rows=0\n- - a\n  - b\n```\n\n```table\njust text\n```\n\n",
            &format!("![[{target}]]\n\nSee ![[{target}]] inline."),
        ]
        .concat();
        let found: Vec<(&str, Drawing, bool)> =
            drawings(&body).into_iter().map(|d| (&body[d.range], d.drawing, d.block)).collect();
        assert_eq!(
            found,
            [
                ("$x^2$", Drawing::InlineMath("x^2".into()), false),
                ("$$\na + b\n$$", Drawing::DisplayMath("\na + b\n".into()), true),
                (
                    "```mermaid\nflowchart LR\n  a --> b\n```",
                    Drawing::Diagram("flowchart LR\n  a --> b\n".into()),
                    true
                ),
                (
                    "```table header-rows=0\n- - a\n  - b\n```",
                    Drawing::Table(ListTable {
                        header_rows: 0,
                        header_columns: 0,
                        rows: vec![vec!["a".into(), "b".into()]],
                    }),
                    true
                ),
                (&format!("![[{target}]]"), Drawing::Embed(target), true),
                (&format!("![[{target}]]"), Drawing::Embed(target), false),
            ]
        );
        // Display math across lines is a block, even with text beside it, and on one line with
        // text it's inline.
        let blocks = |body: &str| drawings(body).iter().map(|d| d.block).collect::<Vec<_>>();
        assert_eq!(blocks("this \n$$\ng=q^{2}\n$$this"), [true]);
        assert_eq!(blocks("this$$g=q^{2}$$"), [false]);
    }

    #[test]
    fn finds_table_cells_in_the_body() {
        let table = "```table\n- - a\n  - b\n```";
        let nested = "- item\n\n  ```table\n  - - a\n    - b\n  ```\n";
        for body in [format!("Intro\n\n{table}\n"), nested.to_owned()] {
            let block = drawings(&body).into_iter().next().unwrap().range;
            let cell = table_cell_in_body(&body, block, 0, 1, &[]).unwrap();
            assert_eq!(&body[cell], "b", "{body:?}");
        }
    }

    #[test]
    fn finds_inline_syntax() {
        assert_eq!(
            hidden("Some *em*, **strong**, ~~gone~~, `code`, $x$, and [label](https://x.example)."),
            [
                (Style::Emphasis, vec!["*", "*"]),
                (Style::Strong, vec!["**", "**"]),
                (Style::Strikethrough, vec!["~~", "~~"]),
                (Style::Code, vec!["`", "`"]),
                (Style::Math, vec!["$", "$"]),
                (Style::Link, vec!["[", "](https://x.example)"]),
            ]
        );
    }

    #[test]
    fn finds_marks() {
        assert_eq!(
            hidden("an =under= and ==high== line"),
            [(Style::Underline, vec!["=", "="]), (Style::Highlight, vec!["==", "=="])]
        );
        // A mark can hold other spans, and a span can hold a mark.
        assert_eq!(
            hidden("==**this**== and **==that==**"),
            [
                (Style::Highlight, vec!["==", "=="]),
                (Style::Strong, vec!["**", "**"]),
                (Style::Strong, vec!["**", "**"]),
                (Style::Highlight, vec!["==", "=="]),
            ]
        );
    }

    #[test]
    fn hides_wikilink_targets() {
        let id = Id::generate();
        let body = format!("See [[{id}|Sam \\| Co]] and ![[{id}]].");
        let found = hidden(&body);
        let target = format!("[[{id}|");
        assert_eq!(
            found,
            [(Style::Link, vec![target.as_str(), "\\", "]]"]), (Style::Link, vec!["![[", "]]"]),]
        );
    }

    #[test]
    fn styles_headings_quotes_and_tasks() {
        assert_eq!(hidden("## Title"), [(Style::Heading(2), vec!["## "])]);
        assert_eq!(hidden("##"), []);
        assert_eq!(hidden("Title\n====="), []);
        assert_eq!(hidden("> one\n> two"), [(Style::BlockQuote, vec!["> ", "> "])]);
        assert_eq!(
            hidden("- [x] done"),
            [
                (Style::ListPrefix, vec![]),
                (Style::Checkbox(true), vec![]),
                (Style::Completed, vec![])
            ]
        );
    }

    /// Types `typed` at the `|` in `before`, and returns the result with `|` at the new cursor.
    fn type_at(before: &str, typed: char) -> Option<String> {
        let cursor = before.find('|').unwrap();
        let body = before.replacen('|', "", 1);
        let edit = complete_block_opening(&body, cursor, typed)?;
        let mut after = body;
        after.replace_range(edit.range, &edit.text);
        after.insert(edit.cursor, '|');
        Some(after)
    }

    #[test]
    fn completes_block_openings() {
        assert_eq!(type_at("``|", '`').as_deref(), Some("```|\n```"));
        assert_eq!(type_at("text\n``|\nmore", '`').as_deref(), Some("text\n```|\n```\nmore"));
        let nested = "- something\n  - another\n    ``|";
        let fenced = "- something\n  - another\n    ```|\n    ```";
        assert_eq!(type_at(nested, '`').as_deref(), Some(fenced));
        assert_eq!(type_at("- [ ] ``|", '`').as_deref(), Some("- [ ] ```|\n      ```"));
        assert_eq!(type_at("$|", '$').as_deref(), Some("$$\n|\n$$"));
        assert_eq!(type_at("- a\n  $|", '$').as_deref(), Some("- a\n  $$\n  |\n  $$"));
        // Not after text, before text, past a longer run, or inside a code block.
        assert_eq!(type_at("a ``|", '`'), None);
        assert_eq!(type_at("``| a", '`'), None);
        assert_eq!(type_at("```|", '`'), None);
        assert_eq!(type_at("$$|", '$'), None);
        assert_eq!(type_at("```\n``|", '`'), None);
    }

    #[test]
    fn fences_selections() {
        let fence = |before: &str| {
            let start = before.find('[').unwrap();
            let end = before.find(']').unwrap() - 1;
            let body = before.replacen('[', "", 1).replacen(']', "", 1);
            let typed = if body[..start].ends_with('$') { '$' } else { '`' };
            let (edit, selected) = fence_selection(&body, start..end, typed)?;
            let mut after = body;
            after.replace_range(edit.range, &edit.text);
            after.insert(selected.end, ']');
            after.insert(selected.start, '[');
            Some(after)
        };
        let text = "this is an **example** for you";
        let fenced = format!("```\n[{text}]\n```");
        assert_eq!(fence(&format!("``[{text}]``")), Some(fenced));
        assert_eq!(fence("- a\n  ``[b]``").as_deref(), Some("- a\n  ```\n  [b]\n  ```"));
        assert_eq!(fence("- ``[b]``").as_deref(), Some("- ```\n  [b]\n  ```"));
        // A selection that shares its line with other text gets an inline code span instead.
        assert_eq!(fence("a ``[b]``"), None);
        assert_eq!(fence("``[b]`` c"), None);
        assert_eq!(fence("`[b]`"), None);
        assert_eq!(fence("$[f=19^{2}]$").as_deref(), Some("$$\n[f=19^{2}]\n$$"));
        assert_eq!(fence("- a\n  $[x]$").as_deref(), Some("- a\n  $$\n  [x]\n  $$"));
        assert_eq!(fence("$$[x]$$"), None);
        assert_eq!(fence("a $[x]$"), None);
    }

    #[test]
    fn toggles_boxes() {
        let toggle = |body: &str, offset| {
            let (range, text) = toggle_checkbox(body, offset)?;
            let mut out = body.to_owned();
            out.replace_range(range, text);
            Some(out)
        };
        assert_eq!(toggle("- [ ] a", 6).as_deref(), Some("- [x] a"));
        assert_eq!(toggle("- [x] a", 0).as_deref(), Some("- [ ] a"));
        assert_eq!(toggle("- a", 2).as_deref(), Some("- [ ] a"));
        assert_eq!(toggle("- a\n  - [ ] b", 12).as_deref(), Some("- a\n  - [x] b"));
        assert_eq!(toggle("plain", 2), None);
    }
}
