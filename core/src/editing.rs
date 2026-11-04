//! What an editor needs to show markdown styled as it's written: the styled elements of a body,
//! with the syntax to hide while the cursor is away, and the edit that toggles a task box.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::Id;
use crate::body::{self, Mark};
use crate::lists::Item as ListItem;

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
    let mut in_code_block = false;
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
            Event::Text(_) if !in_code_block => marks(body, &range, &mut spans),
            Event::Start(tag) => {
                let style = match tag {
                    Tag::BlockQuote(_) => Some(Style::BlockQuote),
                    Tag::CodeBlock(_) => {
                        in_code_block = true;
                        Some(Style::CodeBlock)
                    }
                    Tag::Emphasis => Some(Style::Emphasis),
                    Tag::Heading { level, .. } => Some(Style::Heading(level as u8)),
                    Tag::Link { .. } => Some(Style::Link),
                    Tag::Strikethrough => Some(Style::Strikethrough),
                    Tag::Strong => Some(Style::Strong),
                    _ => None,
                };
                stack.push(Frame { inner: None, range, style });
            }
            Event::End(end) => {
                if end == TagEnd::CodeBlock {
                    in_code_block = false;
                }
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

/// The underlines and highlights in a run of text, which the note bodies specification marks
/// with `=`.
fn marks(body: &str, range: &Range<usize>, spans: &mut Vec<Span>) {
    for marked in body::marked_spans(&body[range.clone()]) {
        let open = range.start + marked.open.start..range.start + marked.open.end;
        let close = range.start + marked.close.start..range.start + marked.close.end;
        let whole = open.start..close.end;
        let syntax = vec![open, close];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hidden(body: &str) -> Vec<(Style, Vec<&str>)> {
        spans(body)
            .into_iter()
            .map(|span| (span.style, span.syntax.iter().map(|r| &body[r.clone()]).collect()))
            .collect()
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
