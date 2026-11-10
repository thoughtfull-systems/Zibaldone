//! How the editor's text looks: which ranges show as headings, emphasis, code, and the rest, which
//! show as syntax, and which lines draw on a shade, as code blocks do. Apps turn each look into
//! their own fonts and colors.

use std::ops::Range;

mod drawings;

pub use drawings::{Drawing, Paragraph, Rendered};

use crate::editing::Drawing as SourceDrawing;
use crate::editing::{QuotedLine, Span, Style, list_pads, quoted_lines, spans};
use crate::markup::{self, Inline};
use crate::{Id, body};

/// How a range of the text looks. Several looks can cover the same text, as emphasis inside a link
/// does, and apps apply them in the order they come, so a later one wins where they disagree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Look {
    /// A heading of level 1 to 6.
    Heading(u8),
    Emphasis,
    Strong,
    Strikethrough,
    Underline,
    Highlight,
    /// A code span, or inline math, in a monospace font on a light shade.
    Code,
    /// Text in a monospace font: code blocks, display math, pipe tables, and the indentation,
    /// markers, and boxes that start list items, so they line up.
    Monospace,
    Link,
    /// A block quote's text.
    Quote,
    /// The text of a task item that's checked.
    Completed,
    /// Markdown syntax, such as `**`, `[[`, or a heading's `#`, which apps dim.
    Syntax,
    /// The literal source of a block that failed to render.
    Error,
}

/// A range of the text, in bytes, and how it looks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Styled {
    pub range: Range<usize>,
    pub look: Look,
}

/// A shape an app draws over text in place of its source, while the selection is away.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shape {
    /// A bulleted list item's marker, `*`, `-`, or `+`.
    Bullet(char),
    /// A task item's box, and whether it's checked. Over a list item's marker and the space after
    /// it, the box takes the marker's place, and goes where the marker is. Over the box's own
    /// source, `[ ]` or `[x]`, it goes in the middle of it.
    TaskBox(bool),
}

/// Text that an app draws a shape over. The text stays in the layout, unseen, to keep its room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Marker {
    pub range: Range<usize>,
    pub shape: Shape,
}

/// A title drawn in place of a link's source. Its styles use byte ranges in `text`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    pub range: Range<usize>,
    pub text: String,
    pub styles: Vec<Styled>,
}

/// An available image's natural size in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageSize {
    pub width: u32,
    pub height: u32,
}

/// An image that takes one place on the text's baseline, standing for its whole embed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub range: Range<usize>,
    pub target: Id,
    pub size: ImageSize,
    /// The source shows inline with the picture above it while the selection touches it.
    pub revealed: bool,
}

/// How a text looks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Display {
    /// Math and diagrams in source order, including pending and failed renders.
    pub drawings: Vec<Drawing>,
    /// The quoted lines, including blank lines and lazy paragraph continuations, in source order.
    pub quotes: Vec<QuotedLine>,
    /// Available embedded images, in source order. Revealing the source keeps the image in place.
    pub images: Vec<Image>,
    /// Titles for wikilinks without labels, in source order. These never enter the body.
    pub labels: Vec<Label>,
    /// The looks: ordinary styles first, syntax dimming next, and render errors last.
    pub styles: Vec<Styled>,
    /// The lines that draw on a shade across the editor's width, as code blocks, display math,
    /// and pipe tables do, each without its line break.
    pub shaded: Vec<Range<usize>>,
    /// The syntax that hides, because the selection doesn't touch what it belongs to, in order and
    /// without overlaps. None of it holds a line break.
    pub hidden: Vec<Range<usize>>,
    /// The bullets and task boxes to draw, in order.
    pub markers: Vec<Marker>,
    /// The hidden source of each task box that takes its list item's marker's place: `[ ]` or
    /// `[x]`, and the space after it. Moves treat it apart from other hidden syntax.
    pub task_boxes: Vec<Range<usize>>,
    /// Lines of numbered list items, with how many columns of the monospace font that list
    /// prefixes use to move them right, so a list's one-digit numbers line up on the right.
    pub pads: Vec<(Range<usize>, usize)>,
}

/// How `text` looks with `selection`, a byte range, selected. An element's syntax shows while the
/// selection touches it, the cursor at either edge included, and hides otherwise. In a pipe table
/// that the selection touches, all of it shows, so its columns line up.
pub fn display(text: &str, selection: &Range<usize>) -> Display {
    with_titles(text, selection, &mut |_| None)
}

/// How `text` looks, resolving the markdown title of each wikilink without a label.
pub fn with_titles(
    text: &str,
    selection: &Range<usize>,
    title: &mut dyn FnMut(Id) -> Option<String>,
) -> Display {
    with_images(text, selection, title, &mut |_| None)
}

/// How `text` looks with the titles and available images the app supplies. Missing images keep
/// their link text, and the source shows when the selection touches an embed.
pub fn with_images(
    text: &str,
    selection: &Range<usize>,
    title: &mut dyn FnMut(Id) -> Option<String>,
    image: &mut dyn FnMut(Id) -> Option<ImageSize>,
) -> Display {
    with_resources(text, selection, title, image, &mut |_| None)
}

/// How the source looks with the platform's titles, image sizes, and math and diagram renders.
pub fn with_resources(
    text: &str,
    selection: &Range<usize>,
    title: &mut dyn FnMut(Id) -> Option<String>,
    image: &mut dyn FnMut(Id) -> Option<ImageSize>,
    render: &mut dyn FnMut(&SourceDrawing) -> Option<Rendered>,
) -> Display {
    with_previews(text, selection, title, image, render, &mut |_, _| None)
}

/// Reuses each occurrence's settled preview only while its current source has no render result.
pub(super) fn with_previews(
    text: &str,
    selection: &Range<usize>,
    title: &mut dyn FnMut(Id) -> Option<String>,
    image: &mut dyn FnMut(Id) -> Option<ImageSize>,
    render: &mut dyn FnMut(&SourceDrawing) -> Option<Rendered>,
    preview: &mut dyn FnMut(&Range<usize>, &SourceDrawing) -> Option<SourceDrawing>,
) -> Display {
    let mut display = Display::default();
    let mut syntax = Vec::new();
    let found = spans(text);
    let tables: Vec<Range<usize>> = found
        .iter()
        .filter(|span| span.style == Style::Table && span.touches(selection))
        .map(|span| span.range.clone())
        .collect();
    for span in found {
        let shows = span.touches(selection)
            || tables.iter().any(|t| t.start <= span.range.start && span.range.end <= t.end);
        let Span { range, style, syntax: pieces } = span;
        let display_math = style == Style::Math && text[range.clone()].starts_with("$$");
        let looks = match style {
            Style::BlockQuote => vec![Look::Quote],
            Style::Checkbox(false) | Style::Strong => vec![Look::Strong],
            Style::Checkbox(true) => vec![Look::Strong, Look::Completed],
            Style::Code => vec![Look::Code],
            Style::CodeBlock | Style::ListMarker | Style::ListPrefix | Style::Table => {
                vec![Look::Monospace]
            }
            Style::Completed => vec![Look::Completed],
            Style::Emphasis => vec![Look::Emphasis],
            Style::Escape => Vec::new(),
            Style::Heading(level) => vec![Look::Heading(level)],
            Style::Highlight => vec![Look::Highlight],
            Style::Link => vec![Look::Link],
            Style::Math => vec![if display_math { Look::Monospace } else { Look::Code }],
            Style::Strikethrough => vec![Look::Strikethrough],
            Style::Underline => vec![Look::Underline],
        };
        display.styles.extend(looks.into_iter().map(|look| Styled { range: range.clone(), look }));
        if display_math || matches!(style, Style::CodeBlock | Style::Table) {
            display.shaded.extend(lines(text, &range));
        }
        // A bullet and a task box draw over their syntax, rather than dim and hide it. In a
        // bulleted list, a task's box takes its marker's place, and the box's source hides.
        match style {
            Style::ListMarker if !shows => {
                let bullet = text[range.clone()].chars().next().unwrap_or('-');
                display.markers.push(Marker { range: range.clone(), shape: Shape::Bullet(bullet) });
            }
            Style::Checkbox(checked) if !shows => {
                if let Some(first) = pieces.first() {
                    let shape = Shape::TaskBox(checked);
                    display.markers.push(Marker { range: first.clone(), shape });
                }
                if let Some(source) = pieces.get(1) {
                    display.hidden.push(source.clone());
                    display.task_boxes.push(source.clone());
                }
            }
            _ => {}
        }
        if !matches!(style, Style::Checkbox(_) | Style::ListMarker) {
            if !shows {
                display.hidden.extend(pieces.iter().filter(|p| !p.is_empty()).cloned());
            }
            syntax.extend(pieces.into_iter().map(|range| Styled { range, look: Look::Syntax }));
        }
    }
    display.styles.extend(syntax);
    display.shaded.sort_by_key(|line| line.start);
    display.shaded.dedup();
    display.hidden = merged(display.hidden);
    display.pads = list_pads(text);
    display.quotes = quoted_lines(text);
    for link in body::wikilinks(text) {
        let touches =
            |range: &Range<usize>| range.start <= selection.end && selection.start <= range.end;
        if tables.iter().any(|t| t.contains(&link.range.start)) {
            continue;
        }
        if link.embed
            && let Some(size) = image(link.target).filter(|s| s.width > 0 && s.height > 0)
        {
            display.images.push(Image {
                revealed: touches(&link.range),
                range: link.range,
                target: link.target,
                size,
            });
            continue;
        }
        if touches(&link.range) || link.label.is_some() {
            continue;
        }
        let markdown = title(link.target).unwrap_or_else(|| link.target.to_string());
        let mut label = Label { range: link.range, text: String::new(), styles: Vec::new() };
        label.append(&markup::inline_markdown(&markdown));
        if label.text.trim().is_empty() {
            "Untitled".clone_into(&mut label.text);
            label.styles.clear();
        }
        display.labels.push(label);
    }
    drawings::collect(&mut display, text, selection, render, preview);
    display
}

impl Label {
    /// Keeps a title's inline formatting without putting its markdown syntax on screen.
    fn append(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            let start = self.text.len();
            let (children, look) = match inline {
                Inline::Emphasis(children) => (children, Look::Emphasis),
                Inline::Strong(children) => (children, Look::Strong),
                Inline::Strikethrough(children) => (children, Look::Strikethrough),
                Inline::Underline(children) => (children, Look::Underline),
                Inline::Highlight(children) => (children, Look::Highlight),
                Inline::Link { content, .. } => {
                    self.append(content);
                    continue;
                }
                Inline::Text(text) => {
                    self.text.push_str(text);
                    continue;
                }
                Inline::Code(text) | Inline::Math { source: text, .. } => {
                    self.text.push_str(text);
                    self.styles.push(Styled { range: start..self.text.len(), look: Look::Code });
                    continue;
                }
                Inline::SoftBreak | Inline::HardBreak => {
                    self.text.push(' ');
                    continue;
                }
                _ => {
                    self.text.push_str(&markup::flatten_text(std::slice::from_ref(inline)));
                    continue;
                }
            };
            self.append(children);
            self.styles.push(Styled { range: start..self.text.len(), look });
        }
    }
}

/// `ranges` in order, with those that overlap or meet joined.
fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| range.start);
    let mut joined: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match joined.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    joined
}

/// The lines that `range` touches, from the start of its first line, without their line breaks.
fn lines(text: &str, range: &Range<usize>) -> Vec<Range<usize>> {
    let mut start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let mut found = Vec::new();
    loop {
        let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        found.push(start..end);
        if end >= range.end || end == text.len() {
            break;
        }
        start = end + 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Note bodies: Labels in display editors.
    #[test]
    fn draws_formatted_titles_only_away_from_unlabeled_links() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let link = format!("[[{id}#part]]");
        let text = format!("{link} [[{id}|chosen]] `[[{id}]]`\n\nend");
        let shown = with_titles(&text, &(text.len()..text.len()), &mut |_| {
            Some("**café** =under= ==bright== ~~gone~~ `code` $x$ &amp;".to_owned())
        });
        assert_eq!(shown.labels.len(), 1);
        let label = &shown.labels[0];
        assert_eq!(label.range, 0..link.len());
        assert_eq!(label.text, "café under bright gone code x &");
        let looks: Vec<_> =
            label.styles.iter().map(|s| (&label.text[s.range.clone()], s.look)).collect();
        assert_eq!(
            looks,
            [
                ("café", Look::Strong),
                ("under", Look::Underline),
                ("bright", Look::Highlight),
                ("gone", Look::Strikethrough),
                ("code", Look::Code),
                ("x", Look::Code),
            ]
        );
        for at in [0, 2, link.len()] {
            assert_eq!(
                with_titles(&text, &(at..at), &mut |_| Some("title".into())).labels,
                Vec::<Label>::new(),
            );
        }
    }

    #[test]
    fn missing_and_untitled_links_have_visible_labels() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("[[{id}]]\nend");
        assert_eq!(display(&text, &(text.len()..text.len())).labels[0].text, id.to_string());
        let shown = with_titles(&text, &(text.len()..text.len()), &mut |_| Some(String::new()));
        assert_eq!(shown.labels[0].text, "Untitled");
    }

    /// Built-in types: Embeds.
    #[test]
    fn images_replace_embeds_with_and_without_captions_but_never_links_or_code() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("![[{id}]] ![[{id}|caption]] [[{id}]] `![[{id}]]`\nend");
        let size = ImageSize { width: 80, height: 40 };
        let shown = with_images(
            &text,
            &(text.len()..text.len()),
            &mut |_| Some("Title".into()),
            &mut |_| Some(size),
        );
        assert_eq!(shown.images.len(), 2);
        assert_eq!(shown.images[0], Image { range: 0..31, target: id, size, revealed: false });
        assert_eq!(&text[shown.images[1].range.clone()], format!("![[{id}|caption]]"));
        assert_eq!(shown.labels.len(), 1);
        for at in [0, 5, 31] {
            let shown = with_images(&text, &(at..at), &mut |_| None, &mut |_| Some(size));
            assert_eq!(shown.images.len(), 2);
            assert!(shown.images[0].revealed);
            assert!(!shown.images[1].revealed);
        }
        let missing = with_images(
            &text,
            &(text.len()..text.len()),
            &mut |_| Some("Title".into()),
            &mut |_| None,
        );
        assert_eq!(missing.images, Vec::<Image>::new());
        assert_eq!(missing.labels.len(), 2);
        assert_eq!(
            with_images(&text, &(text.len()..text.len()), &mut |_| None, &mut |_| Some(
                ImageSize { width: 0, height: 40 }
            ))
            .images,
            Vec::<Image>::new()
        );
    }

    /// The looks of `text`, each with the text it covers.
    fn looks(text: &str) -> Vec<(&str, Look)> {
        display(text, &(0..0)).styles.into_iter().map(|s| (&text[s.range], s.look)).collect()
    }

    #[test]
    fn dims_syntax_last() {
        assert_eq!(
            looks("# Title **bold**"),
            [
                ("# Title **bold**", Look::Heading(1)),
                ("**bold**", Look::Strong),
                ("# ", Look::Syntax),
                ("**", Look::Syntax),
                ("**", Look::Syntax),
            ]
        );
    }

    #[test]
    fn hides_syntax_away_from_the_selection() {
        let text = "# Title\n\nSome **bold** and [a](http://x).\n\n| *a* | b |\n|---|---|\n\nend";
        let hidden = |at: usize| -> Vec<&str> {
            with_resources(text, &(at..at), &mut |_| None, &mut |_| None, &mut |d| {
                matches!(d, SourceDrawing::Table(_)).then_some(Rendered::Table)
            })
            .hidden
            .iter()
            .map(|r| &text[r.clone()])
            .collect()
        };
        let all = ["# ", "**", "**", "[", "](http://x)", "*", "*"];
        assert_eq!(hidden(text.len()), all);
        // The cursor at either edge of an element, or inside it, shows its syntax.
        let bold = text.find("**").unwrap();
        for at in [bold, bold + 4, bold + 8] {
            assert_eq!(hidden(at), ["# ", "[", "](http://x)", "*", "*"]);
        }
        // Anywhere in a pipe table shows all of its syntax.
        let table = text.find("| b").unwrap();
        assert_eq!(hidden(table), ["# ", "**", "**", "[", "](http://x)"]);
    }

    #[test]
    fn hides_punctuation_escapes_away_from_the_selection() {
        let text = r"\*Deals cost \$329. C:\tools and `\*` stay literal. \\*";
        let hidden = |at| display(text, &(at..at)).hidden;
        let price = text.find(r"\$").unwrap();
        let last = text.rfind(r"\\").unwrap();
        let code = text.find('`').unwrap();
        assert_eq!(
            hidden(text.len()),
            [0..1, price..price + 1, code..code + 1, code + 3..code + 4, last..last + 1],
        );
        assert_eq!(hidden(1)[0], price..price + 1);
    }

    #[test]
    fn draws_bullets_and_boxes_away_from_the_selection() {
        let text = "- one\n- [x] done\n1. [ ] first\n\nend";
        let away = display(text, &(text.len()..text.len()));
        let markers: Vec<(&str, Shape)> =
            away.markers.iter().map(|m| (&text[m.range.clone()], m.shape)).collect();
        assert_eq!(
            markers,
            [
                ("-", Shape::Bullet('-')),
                ("- ", Shape::TaskBox(true)),
                ("[ ]", Shape::TaskBox(false))
            ]
        );
        let boxes: Vec<&str> = away.task_boxes.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(boxes, ["[x] "]);
        assert!(away.hidden.contains(&away.task_boxes[0]));
        // On the item's marker or box, its source shows.
        let on = display(text, &(6..6));
        assert_eq!(on.markers.len(), 2);
        assert_eq!(on.task_boxes, Vec::<Range<usize>>::new());
    }

    #[test]
    fn shows_math_like_code() {
        assert_eq!(
            looks("a $x$ b"),
            [("$x$", Look::Code), ("$", Look::Syntax), ("$", Look::Syntax)]
        );
        let text = "a\n\n$$\nx\n$$\n";
        let shown = display(text, &(0..0));
        assert_eq!(shown.styles[0].look, Look::Monospace);
        let shaded: Vec<&str> = shown.shaded.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(shaded, ["$$", "x", "$$"]);
    }

    #[test]
    fn shades_code_blocks_and_pipe_tables() {
        let text =
            "- item\n\n  ```rust\n  fn main() {}\n  ```\n\n| a | b |\n|---|---|\n| c | d |\n";
        let shown = display(text, &(0..0));
        let shaded: Vec<&str> = shown.shaded.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(
            shaded,
            ["  ```rust", "  fn main() {}", "  ```", "| a | b |", "|---|---|", "| c | d |"]
        );
    }

    #[test]
    fn quote_decorations_keep_depth_and_start_through_blank_and_lazy_lines() {
        let text = "> outer\n>\n> > inner\nlazy\n\n- > listed\n  > more\n\nend";
        let shown = display(text, &(text.len()..text.len()));
        let quotes: Vec<_> = shown
            .quotes
            .iter()
            .map(|q| (&text[q.line.clone()], q.depth, q.start - q.line.start))
            .collect();
        assert_eq!(
            quotes,
            [
                ("> outer", 1, 0),
                (">", 1, 0),
                ("> > inner", 2, 0),
                ("lazy", 2, 0),
                ("- > listed", 1, 2),
                ("  > more", 1, 2),
            ]
        );
        assert_eq!(display(text, &(2..2)).quotes, shown.quotes);
        let code = "```\n> literal\n```\n\n    > indented code\n\nend";
        assert_eq!(display(code, &(0..0)).quotes, Vec::<QuotedLine>::new());
    }

    #[test]
    fn code_shading_includes_empty_lines_and_stops_at_the_closing_fence() {
        let text = "```rust\nlet x = 1;\n\n```\nafter\n\n> ```\n> code\n>\n> ```\n\nend";
        let shown = display(text, &(text.len()..text.len()));
        let shaded: Vec<_> = shown.shaded.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(shaded, ["```rust", "let x = 1;", "", "```", "> ```", "> code", ">", "> ```"]);
    }
}
