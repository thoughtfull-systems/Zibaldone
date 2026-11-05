//! The tree that every app renders a note's body from, as the note bodies specification and
//! decision ZIB0009 define it.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use crate::Id;
use crate::body::{self, Mark, MarkedSpan, WikiLink};

/// A block of a note's body.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Code {
        language: String,
        text: String,
    },
    /// Display math, from a `math` block.
    Math(String),
    /// A Mermaid diagram.
    Diagram(String),
    /// A pipe table or a list table. Each cell holds blocks.
    Table {
        header_rows: usize,
        header_columns: usize,
        rows: Vec<Vec<Vec<Block>>>,
    },
    Rule,
    /// Raw HTML, which apps show as plain text.
    Html(String),
}

/// An item of a list, with its task box if it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// Text and its span formatting.
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Math {
        source: String,
        display: bool,
    },
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Underline(Vec<Inline>),
    Highlight(Vec<Inline>),
    Link {
        url: String,
        content: Vec<Inline>,
    },
    Image {
        url: String,
        alt: String,
    },
    /// A wikilink; without a label, apps show the target's title.
    WikiLink {
        target: Id,
        label: Option<Vec<Inline>>,
    },
    Embed {
        target: Id,
        label: Option<String>,
    },
    SoftBreak,
    HardBreak,
}

/// The tree of a note's body.
pub fn parse(body: &str) -> Vec<Block> {
    let links = body::wikilinks(body);
    // A wikilink's label is parsed on its own, marks and all.
    let marks: Vec<MarkedSpan> = body::marks(body)
        .into_iter()
        .filter(|mark| !links.iter().any(|link| link.range.contains(&mark.open.start)))
        .collect();
    let mut builder = Builder {
        source: body,
        links: &links,
        marks: &marks,
        stack: vec![Frame::root()],
        skip_until: 0,
    };
    for (event, range) in Parser::new_ext(body, body::options()).into_offset_iter() {
        builder.event(event, range);
    }
    builder.finish()
}

/// A container under construction.
struct Frame {
    kind: FrameKind,
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
}

enum FrameKind {
    Root,
    Paragraph,
    Heading {
        level: u8,
        setext: Option<String>,
    },
    Quote,
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Item {
        checked: Option<bool>,
    },
    Span(SpanKind),
    Link {
        url: String,
    },
    Image {
        url: String,
    },
    /// An underline or highlight, whose `=` runs open and close it inside text.
    Mark(Mark),
    Code {
        language: String,
        text: String,
    },
    Table {
        rows: Vec<Vec<Vec<Block>>>,
        header_rows: usize,
    },
    Row,
    Cell,
    Html,
}

#[derive(Clone, Copy)]
enum SpanKind {
    Emphasis,
    Strong,
    Strikethrough,
    /// A construct the dialect doesn't give a meaning, whose text shows as it is.
    Plain,
}

impl Frame {
    fn root() -> Self {
        Self::new(FrameKind::Root)
    }

    fn new(kind: FrameKind) -> Self {
        Self { kind, blocks: Vec::new(), inlines: Vec::new() }
    }
}

struct Builder<'a> {
    source: &'a str,
    links: &'a [WikiLink],
    marks: &'a [MarkedSpan],
    stack: Vec<Frame>,
    /// Text before this byte belongs to a wikilink already emitted.
    skip_until: usize,
}

impl Builder<'_> {
    fn top(&mut self) -> &mut Frame {
        self.stack.last_mut().expect("the root frame")
    }

    fn push_inline(&mut self, inline: Inline) {
        self.top().inlines.push(inline);
    }

    fn push_block(&mut self, block: Block) {
        self.top().blocks.push(block);
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, &range),
            Event::End(end) => self.end(end),
            Event::Text(text) => self.text(&text, range),
            Event::Code(code) => {
                if range.start >= self.skip_until {
                    self.push_inline(Inline::Code(code.into_string()));
                }
            }
            Event::InlineMath(math) => {
                self.push_inline(Inline::Math { source: math.into_string(), display: false });
            }
            Event::DisplayMath(math) => {
                self.push_inline(Inline::Math { source: math.into_string(), display: true });
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                self.push_inline(Inline::Text(html.into_string()));
            }
            Event::SoftBreak => self.push_inline(Inline::SoftBreak),
            Event::HardBreak => self.push_inline(Inline::HardBreak),
            Event::Rule => self.push_block(Block::Rule),
            Event::TaskListMarker(checked) => {
                // In a loose list, the box comes inside the item's first paragraph.
                let item =
                    self.stack.iter_mut().rev().take(2).find_map(|frame| match &mut frame.kind {
                        FrameKind::Item { checked } => Some(checked),
                        _ => None,
                    });
                if let Some(slot) = item {
                    *slot = Some(checked);
                }
            }
            Event::FootnoteReference(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: &Range<usize>) {
        let kind = match tag {
            Tag::Paragraph => FrameKind::Paragraph,
            Tag::Heading { level, .. } => {
                let text = &self.source[range.clone()];
                let setext = (!text.trim_start().starts_with('#'))
                    .then(|| text.trim_end().lines().last().unwrap_or("").trim().to_owned());
                FrameKind::Heading { level: level as u8, setext }
            }
            Tag::BlockQuote(_) => FrameKind::Quote,
            Tag::List(start) => FrameKind::List { start, items: Vec::new() },
            Tag::Item => FrameKind::Item { checked: None },
            Tag::Emphasis => FrameKind::Span(SpanKind::Emphasis),
            Tag::Strong => FrameKind::Span(SpanKind::Strong),
            Tag::Strikethrough => FrameKind::Span(SpanKind::Strikethrough),
            Tag::Link { dest_url, .. } => FrameKind::Link { url: dest_url.into_string() },
            Tag::Image { dest_url, .. } => FrameKind::Image { url: dest_url.into_string() },
            Tag::CodeBlock(kind) => FrameKind::Code {
                language: match kind {
                    CodeBlockKind::Fenced(info) => info.into_string(),
                    CodeBlockKind::Indented => String::new(),
                },
                text: String::new(),
            },
            Tag::Table(_) => FrameKind::Table { rows: Vec::new(), header_rows: 0 },
            Tag::TableHead | Tag::TableRow => FrameKind::Row,
            Tag::TableCell => FrameKind::Cell,
            Tag::HtmlBlock => FrameKind::Html,
            _ => FrameKind::Span(SpanKind::Plain),
        };
        self.stack.push(Frame::new(kind));
    }

    fn end(&mut self, end: TagEnd) {
        let frame = self.stack.pop().expect("a frame for each end");
        let inlines = frame.inlines;
        match frame.kind {
            FrameKind::Root => unreachable!("the root never ends"),
            FrameKind::Paragraph | FrameKind::Cell => self.push_block(Block::Paragraph(inlines)),
            FrameKind::Heading { level, setext: None } => {
                self.push_block(Block::Heading { level, content: inlines });
            }
            FrameKind::Heading { setext: Some(underline), .. } => {
                // Zib has no setext headings: the text stays a paragraph, and the underline reads
                // as it would without them.
                let dashes = underline.chars().filter(|c| *c == '-').count();
                if underline.starts_with('-') && dashes >= 3 {
                    self.push_block(Block::Paragraph(inlines));
                    self.push_block(Block::Rule);
                } else {
                    let mut content = inlines;
                    content.push(Inline::SoftBreak);
                    content.push(Inline::Text(underline));
                    self.push_block(Block::Paragraph(content));
                }
            }
            FrameKind::Quote => self.push_block(Block::Quote(frame.blocks)),
            FrameKind::List { start, items } => self.push_block(Block::List { start, items }),
            FrameKind::Item { checked } => {
                let mut blocks = frame.blocks;
                if !inlines.is_empty() {
                    blocks.insert(0, Block::Paragraph(inlines));
                }
                if let FrameKind::List { items, .. } = &mut self.top().kind {
                    items.push(Item { checked, blocks });
                }
            }
            // A mark's closing run ends it before its element ends.
            FrameKind::Span(SpanKind::Plain) | FrameKind::Mark(_) => {
                for inline in inlines {
                    self.push_inline(inline);
                }
            }
            FrameKind::Span(kind) => {
                let inline = match kind {
                    SpanKind::Emphasis => Inline::Emphasis(inlines),
                    SpanKind::Strong => Inline::Strong(inlines),
                    SpanKind::Strikethrough | SpanKind::Plain => Inline::Strikethrough(inlines),
                };
                self.push_inline(inline);
            }
            FrameKind::Link { url } => self.push_inline(Inline::Link { url, content: inlines }),
            FrameKind::Image { url } => {
                let alt = flatten_text(&inlines);
                self.push_inline(Inline::Image { url, alt });
            }
            FrameKind::Code { language, text } => {
                let first = language.split_whitespace().next().unwrap_or("");
                let block = match first {
                    "math" => Block::Math(text),
                    "mermaid" => Block::Diagram(text),
                    "table" => match body::list_table(&language, &text) {
                        Some(table) => Block::Table {
                            header_rows: table.header_rows,
                            header_columns: table.header_columns,
                            rows: table
                                .rows
                                .iter()
                                .map(|row| row.iter().map(|cell| parse(cell)).collect())
                                .collect(),
                        },
                        None => Block::Code { language, text },
                    },
                    _ => Block::Code { language, text },
                };
                self.push_block(block);
            }
            FrameKind::Table { rows, header_rows } => {
                self.push_block(Block::Table { header_rows, header_columns: 0, rows });
            }
            FrameKind::Row => {
                let cells = frame.blocks.into_iter().map(|cell| match cell {
                    Block::Paragraph(content) if content.is_empty() => Vec::new(),
                    other => vec![other],
                });
                let row: Vec<Vec<Block>> = cells.collect();
                if let FrameKind::Table { rows, header_rows } = &mut self.top().kind {
                    if matches!(end, TagEnd::TableHead) {
                        *header_rows += 1;
                    }
                    rows.push(row);
                }
            }
            FrameKind::Html => {
                let text = flatten_text(&inlines);
                self.push_block(Block::Html(text));
            }
        }
    }

    fn text(&mut self, text: &str, range: Range<usize>) {
        if let FrameKind::Code { text: code, .. } = &mut self.top().kind {
            code.push_str(text);
            return;
        }
        let mut runs: Vec<(Range<usize>, Option<Mark>)> = self
            .marks
            .iter()
            .flat_map(|mark| [(mark.open.clone(), Some(mark.mark)), (mark.close.clone(), None)])
            .filter(|(run, _)| range.start <= run.start && run.end <= range.end)
            .collect();
        if runs.is_empty() {
            self.unmarked_text(text, range);
            return;
        }
        runs.sort_by_key(|(run, _)| run.start);
        let source = self.source;
        let mut at = range.start;
        for (run, mark) in runs {
            if at < run.start {
                self.unmarked_text(&source[at..run.start], at..run.start);
            }
            match mark {
                Some(mark) => self.stack.push(Frame::new(FrameKind::Mark(mark))),
                None => self.close_mark(&source[run.clone()]),
            }
            at = run.end;
        }
        if at < range.end {
            self.unmarked_text(&source[at..range.end], at..range.end);
        }
    }

    /// Ends the underline or highlight that the innermost frame holds, at its closing `run`.
    fn close_mark(&mut self, run: &str) {
        let FrameKind::Mark(mark) = self.top().kind else {
            self.push_inline(Inline::Text(run.to_owned()));
            return;
        };
        let inlines = self.stack.pop().expect("a mark's frame").inlines;
        self.push_inline(match mark {
            Mark::Underline => Inline::Underline(inlines),
            Mark::Highlight => Inline::Highlight(inlines),
            Mark::Both => Inline::Underline(vec![Inline::Highlight(inlines)]),
        });
    }

    fn unmarked_text(&mut self, text: &str, range: Range<usize>) {
        // Wikilinks span several text events, so they come from their ranges in the source.
        let mut at = range.start;
        let mut out = Vec::new();
        for link in
            self.links.iter().filter(|l| l.range.start < range.end && l.range.end > range.start)
        {
            if link.range.start > at && at >= self.skip_until {
                out.extend(autolinks(&self.source[at..link.range.start.min(range.end)]));
            }
            if link.range.start >= self.skip_until {
                out.push(if link.embed {
                    Inline::Embed { target: link.target, label: link.label.clone() }
                } else {
                    Inline::WikiLink {
                        target: link.target,
                        label: link.label.as_deref().map(inline_markdown),
                    }
                });
                self.skip_until = link.range.end;
            }
            at = link.range.end.max(at);
        }
        if at < range.end && range.end > self.skip_until {
            let start = at.max(self.skip_until).max(range.start);
            let piece = if start == range.start {
                text.to_owned()
            } else {
                self.source[start..range.end].to_owned()
            };
            // An embed's `!` precedes its link in the source, but the parser may report it as text.
            out.extend(autolinks(&piece));
        }
        for inline in out {
            self.push_inline(inline);
        }
    }

    fn finish(mut self) -> Vec<Block> {
        while self.stack.len() > 1 {
            let frame = self.stack.pop().expect("checked");
            self.top().blocks.extend(frame.blocks);
        }
        self.stack.pop().expect("the root").blocks
    }
}

/// A label or other short text, parsed as inline markdown.
pub fn inline_markdown(text: &str) -> Vec<Inline> {
    match parse(text).into_iter().next() {
        Some(Block::Paragraph(inlines) | Block::Heading { content: inlines, .. }) => inlines,
        _ => vec![Inline::Text(text.to_owned())],
    }
}

/// Splits text at bare web addresses, following GFM's extended autolinks in their simple forms.
fn autolinks(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let found = ["https://", "http://", "www."]
            .iter()
            .filter_map(|prefix| rest.find(prefix).map(|at| (at, *prefix)))
            .filter(|(at, _)| *at == 0 || !rest[..*at].ends_with(|c: char| c.is_alphanumeric()))
            .min_by_key(|(at, _)| *at);
        let Some((at, prefix)) = found else { break };
        let end =
            rest[at..].find(|c: char| c.is_whitespace() || c == '<').map_or(rest.len(), |e| at + e);
        let mut url = &rest[at..end];
        url = url.trim_end_matches(['.', ',', ':', ';', '!', '?', '"', '\'', '*', '_', '~', ')']);
        if url.len() <= prefix.len() {
            out.push(Inline::Text(rest[..at + prefix.len()].to_owned()));
            rest = &rest[at + prefix.len()..];
            continue;
        }
        if at > 0 {
            out.push(Inline::Text(rest[..at].to_owned()));
        }
        let href = if prefix == "www." { format!("http://{url}") } else { url.to_owned() };
        out.push(Inline::Link { url: href, content: vec![Inline::Text(url.to_owned())] });
        rest = &rest[at + url.len()..];
    }
    if !rest.is_empty() {
        out.push(Inline::Text(rest.to_owned()));
    }
    out
}

/// The text of inlines, without formatting.
pub fn flatten_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::Math { source, .. } => out.push_str(source),
            Inline::Emphasis(c)
            | Inline::Strong(c)
            | Inline::Strikethrough(c)
            | Inline::Underline(c)
            | Inline::Highlight(c)
            | Inline::Link { content: c, .. } => out.push_str(&flatten_text(c)),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::WikiLink { label: Some(label), .. } => out.push_str(&flatten_text(label)),
            Inline::Embed { label: Some(label), .. } => out.push_str(label),
            Inline::WikiLink { target, .. } | Inline::Embed { target, .. } => {
                out.push_str(&target.to_string());
            }
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01k97krzh1fcx9rk9fhsgt1de7";

    #[test]
    fn finds_boxes_in_loose_lists() {
        let tree = parse("- [ ] one\n\n- [x] two\n");
        let Block::List { items, .. } = &tree[0] else { panic!("{tree:?}") };
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(items[1].checked, Some(true));
    }

    #[test]
    fn parses_blocks_and_spans() {
        let tree = parse(
            "# Title\n\nSome *emphasis*, ==highlight== and =underline=.\n\n- [x] done\n- todo\n",
        );
        assert!(matches!(&tree[0], Block::Heading { level: 1, .. }));
        let Block::Paragraph(inlines) = &tree[1] else { panic!("{tree:?}") };
        assert!(inlines.iter().any(|i| matches!(i, Inline::Highlight(_))));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Underline(_))));
        let Block::List { items, .. } = &tree[2] else { panic!("{tree:?}") };
        assert_eq!(items[0].checked, Some(true));
    }

    #[test]
    fn marks_hold_other_spans() {
        let text = |t: &str| Inline::Text(t.into());
        assert_eq!(
            parse("==**this**== and =a ==b= c=="),
            [Block::Paragraph(vec![
                Inline::Highlight(vec![Inline::Strong(vec![text("this")])]),
                text(" and "),
                Inline::Underline(vec![text("a ==b")]),
                text(" c=="),
            ])]
        );
    }

    #[test]
    fn finds_wikilinks_embeds_and_bare_addresses() {
        let tree = parse(&format!("See [[{A}|the *label*]] and ![[{A}]] at https://example.com."));
        let Block::Paragraph(inlines) = &tree[0] else { panic!("{tree:?}") };
        assert!(inlines.iter().any(|i| matches!(i, Inline::WikiLink { label: Some(_), .. })));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Embed { .. })));
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Link { url, .. } if url == "https://example.com"))
        );
        assert!(!flatten_text(inlines).contains("!!"));
    }

    #[test]
    fn has_no_setext_headings() {
        let tree = parse("Not a heading\n===\n\nNor this\n---\n");
        assert!(matches!(&tree[0], Block::Paragraph(_)));
        assert_eq!(
            flatten_text(match &tree[0] {
                Block::Paragraph(i) => i,
                _ => unreachable!(),
            }),
            "Not a heading ==="
        );
        assert!(matches!(&tree[1], Block::Paragraph(_)));
        assert!(matches!(&tree[2], Block::Rule));
    }

    #[test]
    fn renders_math_diagrams_and_tables() {
        let body = [
            "Inline $x^2$.\n\n$$\\int f$$\n\n```math\na+b\n```\n\n",
            "```mermaid\nflowchart LR\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n",
            "```table header-columns=1\n- - H\n  - *x*\n```\n",
        ]
        .concat();
        let tree = parse(&body);
        let Block::Paragraph(first) = &tree[0] else { panic!("{tree:?}") };
        assert!(first.iter().any(|i| matches!(i, Inline::Math { display: false, .. })));
        assert!(tree.iter().any(|b| matches!(b, Block::Math(_))));
        assert!(tree.iter().any(|b| matches!(b, Block::Diagram(_))));
        let tables: Vec<_> = tree.iter().filter(|b| matches!(b, Block::Table { .. })).collect();
        assert_eq!(tables.len(), 2);
        assert!(matches!(tables[0], Block::Table { header_rows: 1, rows, .. } if rows.len() == 2));
        assert!(matches!(tables[1], Block::Table { header_columns: 1, .. }));
    }
}
