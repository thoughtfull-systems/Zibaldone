//! How the editor's text looks: which ranges show as headings, emphasis, code, and the rest, which
//! show as syntax, and which lines draw on a shade, as code blocks do. Apps turn each look into
//! their own fonts and colors.

use std::ops::Range;

use crate::editing::{Span, Style, spans};

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
}

/// A range of the text, in bytes, and how it looks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Styled {
    pub range: Range<usize>,
    pub look: Look,
}

/// How a text looks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Display {
    /// The looks, with every piece of syntax last, so its dimming wins over the color of what
    /// holds it.
    pub styles: Vec<Styled>,
    /// The lines that draw on a shade across the editor's width, as code blocks, display math,
    /// and pipe tables do, each without its line break.
    pub shaded: Vec<Range<usize>>,
}

/// How `text` looks, with all of its syntax showing.
pub fn display(text: &str) -> Display {
    let mut display = Display::default();
    let mut syntax = Vec::new();
    for Span { range, style, syntax: pieces } in spans(text) {
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
        // A task box and a list marker are drawn over, not dimmed, from the next step on.
        if !matches!(style, Style::Checkbox(_) | Style::ListMarker) {
            syntax.extend(pieces.into_iter().map(|range| Styled { range, look: Look::Syntax }));
        }
    }
    display.styles.extend(syntax);
    display.shaded.sort_by_key(|line| line.start);
    display.shaded.dedup();
    display
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

    /// The looks of `text`, each with the text it covers.
    fn looks(text: &str) -> Vec<(&str, Look)> {
        display(text).styles.into_iter().map(|s| (&text[s.range], s.look)).collect()
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
    fn shows_math_like_code() {
        assert_eq!(
            looks("a $x$ b"),
            [("$x$", Look::Code), ("$", Look::Syntax), ("$", Look::Syntax)]
        );
        let text = "a\n\n$$\nx\n$$\n";
        let shown = display(text);
        assert_eq!(shown.styles[0].look, Look::Monospace);
        let shaded: Vec<&str> = shown.shaded.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(shaded, ["$$", "x", "$$"]);
    }

    #[test]
    fn shades_code_blocks_and_pipe_tables() {
        let text =
            "- item\n\n  ```rust\n  fn main() {}\n  ```\n\n| a | b |\n|---|---|\n| c | d |\n";
        let shown = display(text);
        let shaded: Vec<&str> = shown.shaded.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(
            shaded,
            ["  ```rust", "  fn main() {}", "  ```", "| a | b |", "|---|---|", "| c | d |"]
        );
    }
}
