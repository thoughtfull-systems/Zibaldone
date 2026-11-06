//! Drawn math and diagrams, their render results, and the source paragraphs they replace.

use std::ops::Range;

use crate::editing::{self, Drawing as Source, Style};

use super::{Display, ImageSize};

/// A render result supplied by the platform. Neither availability nor errors edit the body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Rendered {
    Image(ImageSize),
    Error(String),
    Table,
}

/// Math or a diagram, with the source and the platform's most recent render result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Drawing {
    pub block: bool,
    /// Clicking a block enters its second line, past quote markers and list indentation.
    pub cursor: usize,
    /// The source position that sets the drawing's left inset, before centering it.
    pub indent: usize,
    /// The source of the displayed result, which can precede the current source while rendering.
    pub preview: Option<Source>,
    pub range: Range<usize>,
    pub rendered: Option<Rendered>,
    pub revealed: bool,
    pub source: Source,
}

impl Drawing {
    pub fn collapsed(&self) -> bool {
        !self.revealed && matches!(self.rendered, Some(Rendered::Image(_) | Rendered::Table))
    }
}

/// A source paragraph, or a drawn block with any preceding text kept on its first line. Ranges
/// omit line breaks. A block's range ends where its source begins, and its index picks the drawing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paragraph {
    pub drawing: Option<usize>,
    pub range: Range<usize>,
}

impl Display {
    /// Source order matters: a block can split a paragraph into text before and after its drawing.
    pub fn paragraphs(&self, text: &str) -> Vec<Paragraph> {
        let mut blocks = self
            .drawings
            .iter()
            .enumerate()
            .filter(|(_, drawing)| drawing.block && drawing.collapsed())
            .peekable();
        let mut paragraphs = Vec::new();
        let mut start = 0;
        loop {
            let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            if let Some(&(index, drawing)) = blocks.peek()
                && drawing.range.start <= end
            {
                paragraphs
                    .push(Paragraph { drawing: Some(index), range: start..drawing.range.start });
                start = drawing.range.end;
                blocks.next();
                if start == text.len() {
                    break;
                }
                if text[start..].starts_with('\n') {
                    start += 1;
                }
            } else {
                paragraphs.push(Paragraph { drawing: None, range: start..end });
                if end == text.len() {
                    break;
                }
                start = end + 1;
            }
        }
        paragraphs
    }
}

pub(super) fn collect(
    display: &mut Display,
    text: &str,
    selection: &Range<usize>,
    render: &mut dyn FnMut(&Source) -> Option<Rendered>,
    preview: &mut dyn FnMut(&Range<usize>, &Source) -> Option<Source>,
) {
    let prefixes: Vec<_> = editing::spans(text)
        .into_iter()
        .filter(|span| span.style == Style::ListPrefix)
        .map(|span| span.range)
        .collect();
    for drawn in editing::drawings(text) {
        if !matches!(
            drawn.drawing,
            Source::InlineMath(_) | Source::DisplayMath(_) | Source::Diagram(_) | Source::Table(_)
        ) {
            continue;
        }
        let start = text[..drawn.range.start].rfind('\n').map_or(0, |i| i + 1);
        let indent = prefixes
            .iter()
            .filter(|p| start <= p.start && p.end <= drawn.range.start)
            .map(|p| p.end)
            .chain(display.quotes.iter().filter(|q| q.line.start == start).map(|q| q.start))
            .max()
            .unwrap_or(start);
        let cursor = if drawn.block {
            text[drawn.range.clone()].find('\n').map_or(drawn.range.start, |newline| {
                let start = drawn.range.start + newline + 1;
                let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
                let mut at = start;
                if let Some(quote) = display.quotes.iter().find(|q| q.line.start == start) {
                    at = quote.start;
                    for _ in 0..quote.depth {
                        if !text[at..end].starts_with('>') {
                            break;
                        }
                        at += 1;
                        at += text[at..end].len() - text[at..end].trim_start_matches(' ').len();
                    }
                }
                at + text[at..end].len() - text[at..end].trim_start_matches([' ', '\t']).len()
            })
        } else {
            drawn.range.start
        };
        let usable = |result: &Rendered| match result {
            Rendered::Error(_) | Rendered::Table => true,
            Rendered::Image(size) => size.width > 0 && size.height > 0,
        };
        let (key, rendered) = if let Some(result) = render(&drawn.drawing).filter(usable) {
            (Some(drawn.drawing.clone()), Some(result))
        } else {
            let key = preview(&drawn.range, &drawn.drawing);
            let result = key.as_ref().and_then(&mut *render).filter(usable);
            (key, result)
        };
        if drawn.block && matches!(rendered, Some(Rendered::Error(_))) {
            display
                .styles
                .push(super::Styled { range: drawn.range.clone(), look: super::Look::Error });
        }
        let revealed = drawn.touches(selection);
        // Pending and failed math retain all their source, including the dollar delimiters.
        if !matches!(rendered, Some(Rendered::Image(_) | Rendered::Table)) {
            display.hidden.retain(|hidden| {
                hidden.end <= drawn.range.start || hidden.start >= drawn.range.end
            });
        }
        display.drawings.push(Drawing {
            block: drawn.block,
            cursor,
            indent,
            preview: key,
            range: drawn.range,
            rendered,
            revealed,
            source: drawn.drawing,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::display::with_resources;

    fn shown(text: &str, selection: Range<usize>) -> Display {
        with_resources(text, &selection, &mut |_| None, &mut |_| None, &mut |_| {
            Some(Rendered::Image(ImageSize { width: 100, height: 50 }))
        })
    }

    #[test]
    fn classifies_math_and_diagrams_without_rendering_literals_or_table_contents() {
        let text = concat!(
            "$x$ and $$y$$\n\n$$\nz\n$$\n\n```math\nw\n```\n\n",
            "```mermaid\nflowchart LR\na-->b\n```\n\n`$literal$`\n\n",
            "| a | b |\n|---|---|\n| $cell$ | x |\n\nend"
        );
        let display = shown(text, text.len()..text.len());
        let sources: Vec<_> = display
            .drawings
            .iter()
            .filter(|d| !matches!(d.source, Source::Table(_)))
            .map(|d| (&d.source, d.block))
            .collect();
        assert_eq!(
            display.drawings.iter().filter(|d| matches!(d.source, Source::Table(_))).count(),
            1
        );
        assert_eq!(
            sources,
            [
                (&Source::InlineMath("x".into()), false),
                (&Source::DisplayMath("y".into()), false),
                (&Source::DisplayMath("\nz\n".into()), true),
                (&Source::DisplayMath("w\n".into()), true),
                (&Source::Diagram("flowchart LR\na-->b\n".into()), true),
            ]
        );
        for drawing in &display.drawings {
            for at in [drawing.range.start, drawing.range.end] {
                let display = shown(text, at..at);
                assert!(
                    display.drawings.iter().find(|d| d.range == drawing.range).unwrap().revealed
                );
            }
        }
    }

    #[test]
    fn collapsed_blocks_keep_prefixes_suffixes_blank_lines_and_the_final_newline() {
        for text in [
            "before $$\nx\n$$ after\nend",
            "- ```mermaid\n  a-->b\n  ```\n\nend\n",
            "$$x$$",
            "$$x$$\n",
        ] {
            // A selection outside the body leaves every block collapsed, including at EOF.
            let display = shown(text, text.len() + 1..text.len() + 1);
            let parts: Vec<_> = display
                .paragraphs(text)
                .into_iter()
                .map(|p| (p.drawing.is_some(), text[p.range].to_owned()))
                .collect();
            let expected: Vec<_> = match text {
                "before $$\nx\n$$ after\nend" => {
                    vec![(true, "before "), (false, " after"), (false, "end")]
                }
                "- ```mermaid\n  a-->b\n  ```\n\nend\n" => {
                    vec![(true, "- "), (false, ""), (false, "end"), (false, "")]
                }
                "$$x$$" => vec![(true, "")],
                _ => vec![(true, ""), (false, "")],
            };
            assert_eq!(
                parts,
                expected.into_iter().map(|(b, s)| (b, s.to_owned())).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn multiline_single_dollar_source_stays_literal() {
        let text = "before $x\n+ y$ after\nend";
        let display = shown(text, text.len()..text.len());
        assert_eq!(display.drawings, Vec::<Drawing>::new());
        let paragraphs = display.paragraphs(text);
        assert_eq!(&text[paragraphs[0].range.clone()], "before $x");
        assert_eq!(&text[paragraphs[1].range.clone()], "+ y$ after");
        assert_eq!(&text[paragraphs[2].range.clone()], "end");
    }

    #[test]
    fn block_entries_skip_structural_prefixes_but_keep_math_relation_symbols() {
        for (text, expected, indent) in [
            ("```math\n  > x\n```\n\nend", "> x", 0),
            ("- ```math\n  x\n  ```\n\nend", "x", 2),
            ("> ```math\n> x\n> ```\n\nend", "x", 0),
        ] {
            let display = shown(text, text.len()..text.len());
            let block = &display.drawings[0];
            assert!(text[block.cursor..].starts_with(expected), "{text:?}");
            assert_eq!(block.indent, indent);
        }
    }

    #[test]
    fn pending_and_failed_drawings_keep_literal_source_and_errors_stay_out_of_the_body() {
        let text = "$x$\n\n```mermaid\nbroken\n```\nend";
        let pending = with_resources(
            text,
            &(text.len()..text.len()),
            &mut |_| None,
            &mut |_| None,
            &mut |_| None,
        );
        assert!(pending.drawings.iter().all(|d| !d.collapsed()));
        assert!(!pending.hidden.iter().any(|r| r.contains(&0)));
        let failed = with_resources(
            text,
            &(text.len()..text.len()),
            &mut |_| None,
            &mut |_| None,
            &mut |_| Some(Rendered::Error("bad syntax".into())),
        );
        assert!(failed.drawings.iter().all(|d| !d.collapsed()));
        assert_eq!(failed.paragraphs(text), pending.paragraphs(text));
        assert!(failed.styles.iter().any(|s| s.look == super::super::Look::Error));
    }
}
