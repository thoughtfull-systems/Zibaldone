//! Printing notes to PDF with Typst, as decision ZIB0010 describes: the core's tree of each body
//! becomes Typst markup, with math and diagrams as the SVG that the screen shows, and Typst
//! compiles it in memory.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::{self, FontStore};
use typst_layout::PagedDocument;

use crate::markup::{self, Block, Inline, Item};
use crate::render::{math_svg, mermaid_svg};
pub use crate::store::Page;
use crate::{Error, Id, Result};

/// What the notes refer to: the markdown title of each note they link to or embed, and the file of
/// each image note they embed, with its media type.
#[derive(Clone, Debug, Default)]
pub struct Refs {
    pub titles: HashMap<Id, String>,
    pub images: HashMap<Id, (Vec<u8>, String)>,
}

/// Task boxes, drawn rather than taken from a font, which might lack them or draw them as emoji.
const UNCHECKED: &str =
    "#box(width: 0.75em, height: 0.75em, stroke: 0.6pt, radius: 1.5pt, baseline: 0.05em)#h(0.35em)";
const CHECKED: &str = concat!(
    "#box(width: 0.75em, height: 0.75em, stroke: 0.6pt, radius: 1.5pt, baseline: 0.05em, ",
    "fill: luma(70), align(center + horizon, text(fill: white, size: 0.7em, weight: \"bold\", ",
    "font: \"DejaVu Sans Mono\")[✓]))#h(0.35em)"
);
/// The frame of a code block, which takes the code as its last argument.
const CODE_BLOCK: &str = "#block(fill: luma(243), inset: 8pt, radius: 3pt, width: 100%, ";
/// The body font's size, in points.
const TEXT_SIZE: f64 = 11.0;
/// The size the core draws math at, in SVG units, which Typst reads as points.
const MATH_SIZE: f64 = 20.0;

/// Prints the notes with `bodies` as one PDF, each note starting on a new page.
pub fn pdf(bodies: &[String], refs: &Refs, page: &Page) -> Result<Vec<u8>> {
    let mut writer = Writer { refs, files: Vec::new(), out: String::new() };
    writer.preamble(page);
    for (i, body) in bodies.iter().enumerate() {
        if i > 0 {
            writer.out.push_str("#pagebreak()\n\n");
        }
        writer.blocks(&markup::parse(body));
    }
    let world = Document::new(writer.out, writer.files);
    let document = typst::compile::<PagedDocument>(&world).output.map_err(|errors| {
        let messages: Vec<String> = errors.iter().map(|e| e.message.to_string()).collect();
        Error::Render(messages.join("; "))
    })?;
    typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()).map_err(|errors| {
        let messages: Vec<String> = errors.iter().map(|e| e.message.to_string()).collect();
        Error::Render(messages.join("; "))
    })
}

/// Builds the Typst markup of notes, and the files it refers to.
struct Writer<'a> {
    refs: &'a Refs,
    files: Vec<(String, Vec<u8>)>,
    out: String,
}

impl Writer<'_> {
    fn preamble(&mut self, page: &Page) {
        let _ = writeln!(
            self.out,
            "#set page(paper: {}, margin: {}mm, numbering: \"1\")",
            string(&page.paper),
            page.margin
        );
        let _ = writeln!(self.out, "#set text(font: \"Libertinus Serif\", size: {TEXT_SIZE}pt)");
        self.out.push_str(concat!(
            "#set par(justify: true)\n",
            "#show raw: set text(font: \"DejaVu Sans Mono\", size: 0.9em)\n",
            "#show heading: set block(above: 1.4em, below: 0.8em)\n",
            "#show link: set text(fill: rgb(\"#1a5fb4\"))\n\n",
        ));
    }

    /// Adds a file the markup refers to, and returns its path.
    fn file(&mut self, extension: &str, data: Vec<u8>) -> String {
        let path = format!("/{}.{extension}", self.files.len());
        self.files.push((path.clone(), data));
        path
    }

    fn blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            self.block(block);
            self.out.push_str("\n\n");
        }
    }

    /// The markup of blocks, inside a content block.
    fn content(&mut self, blocks: &[Block]) {
        self.out.push('[');
        self.blocks(blocks);
        self.out.push(']');
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Heading { level, content } => {
                self.out.push_str(&"=".repeat(usize::from(*level)));
                self.out.push(' ');
                self.inlines(content);
            }
            Block::Paragraph(content) => self.inlines(content),
            Block::Quote(blocks) => {
                self.out.push_str(
                    "#block(inset: (left: 10pt, y: 2pt), stroke: (left: 2pt + luma(180)))",
                );
                self.content(blocks);
            }
            Block::List { start, items } => self.list(*start, items),
            Block::Code { language, text } => {
                let language = language.split_whitespace().next().unwrap_or("");
                let _ = write!(
                    self.out,
                    "{CODE_BLOCK}raw(block: true, lang: {}, {}))",
                    string(language),
                    string(text.trim_end_matches('\n'))
                );
            }
            Block::Math(source) => match math_svg(source, true) {
                Ok(svg) => {
                    let width = svg_width(&svg) * TEXT_SIZE / MATH_SIZE;
                    let path = self.file("svg", svg.into_bytes());
                    let _ = write!(
                        self.out,
                        "#align(center, image({}, width: {width:.2}pt))",
                        string(&path)
                    );
                }
                Err(_) => self.code_text(source),
            },
            Block::Diagram(source) => match mermaid_svg(source) {
                Ok(svg) => {
                    let path = self.file("svg", svg.into_bytes());
                    let _ = write!(self.out, "#align(center, image({}))", string(&path));
                }
                Err(_) => self.code_text(source),
            },
            Block::Table { header_rows, header_columns, rows } => {
                self.table(*header_rows, *header_columns, rows);
            }
            Block::Rule => self.out.push_str("#line(length: 100%, stroke: 0.5pt + luma(160))"),
            Block::Html(text) => self.out.push_str(&escape(text)),
        }
    }

    /// Source that didn't render, shown as code.
    fn code_text(&mut self, source: &str) {
        let _ = write!(self.out, "#raw(block: true, {})", string(source));
    }

    fn list(&mut self, start: Option<u64>, items: &[Item]) {
        let tasks = items.iter().any(|item| item.checked.is_some());
        match start {
            Some(first) if !tasks => {
                let _ = write!(self.out, "#enum(start: {first}, tight: true");
            }
            // A list of tasks shows each item's box in place of a marker.
            _ if tasks => self.out.push_str("#list(tight: true, marker: none"),
            _ => self.out.push_str("#list(tight: true"),
        }
        for item in items {
            self.out.push_str(", [");
            match item.checked {
                Some(true) => self.out.push_str(CHECKED),
                Some(false) => self.out.push_str(UNCHECKED),
                None if tasks => self.out.push_str("• "),
                None => {}
            }
            self.blocks(&item.blocks);
            self.out.push(']');
        }
        self.out.push(')');
    }

    fn table(&mut self, header_rows: usize, header_columns: usize, rows: &[Vec<Vec<Block>>]) {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let _ = write!(self.out, "#table(columns: {columns}, align: left");
        for (y, row) in rows.iter().enumerate() {
            if y == 0 && header_rows > 0 {
                self.out.push_str(", table.header(repeat: true");
            }
            for x in 0..columns {
                self.out.push_str(", ");
                let header = y < header_rows || x < header_columns;
                if header {
                    self.out.push_str("strong");
                }
                match row.get(x) {
                    Some(cell) => self.content(cell),
                    None => self.out.push_str("[]"),
                }
            }
            if y + 1 == header_rows {
                self.out.push(')');
            }
        }
        if rows.len() < header_rows {
            self.out.push(')');
        }
        self.out.push(')');
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    /// Wraps inlines in a Typst function, such as `emph`.
    fn wrapped(&mut self, function: &str, inlines: &[Inline]) {
        let _ = write!(self.out, "#{function}[");
        self.inlines(inlines);
        self.out.push(']');
    }

    fn inline(&mut self, inline: &Inline) {
        match inline {
            Inline::Text(text) => self.out.push_str(&escape(text)),
            Inline::Code(code) => {
                let _ = write!(self.out, "#raw({})", string(code));
            }
            Inline::Math { source, display } => self.math(source, *display),
            Inline::Emphasis(inner) => self.wrapped("emph", inner),
            Inline::Strong(inner) => self.wrapped("strong", inner),
            Inline::Strikethrough(inner) => self.wrapped("strike", inner),
            Inline::Underline(inner) => self.wrapped("underline", inner),
            Inline::Highlight(inner) => self.wrapped("highlight", inner),
            Inline::Link { url, content } => {
                let _ = write!(self.out, "#link({})[", string(url));
                self.inlines(content);
                self.out.push(']');
            }
            Inline::Image { alt, .. } => {
                let _ = write!(self.out, "#emph[{}]", escape(alt));
            }
            Inline::WikiLink { target, label } => match label {
                Some(label) => self.inlines(label),
                None => self.title(*target),
            },
            Inline::Embed { target, label } => self.embed(*target, label.as_deref()),
            Inline::SoftBreak => self.out.push(' '),
            Inline::HardBreak => self.out.push_str("\\\n"),
        }
    }

    /// A note's title, as a wikilink without a label shows it.
    fn title(&mut self, target: Id) {
        match self.refs.titles.get(&target) {
            Some(title) if !title.trim().is_empty() => {
                let title = markup::inline_markdown(title);
                self.inlines(&title);
            }
            Some(_) => self.out.push_str("Untitled"),
            None => self.out.push_str("#strike[Missing note]"),
        }
    }

    fn math(&mut self, source: &str, display: bool) {
        let Ok(svg) = math_svg(source, display) else {
            let _ = write!(self.out, "#raw({})", string(source));
            return;
        };
        let width = svg_width(&svg) * TEXT_SIZE / MATH_SIZE;
        let path = self.file("svg", svg.into_bytes());
        if display {
            let _ =
                write!(self.out, "#align(center, image({}, width: {width:.2}pt))", string(&path));
        } else {
            let _ = write!(
                self.out,
                "#box(baseline: 25%, image({}, width: {width:.2}pt))",
                string(&path)
            );
        }
    }

    /// An image note's file, or else the note's title in a frame.
    fn embed(&mut self, target: Id, label: Option<&str>) {
        if let Some((data, media)) = self.refs.images.get(&target) {
            let extension = match media.as_str() {
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "image/svg+xml" => "svg",
                "image/webp" => "webp",
                _ => "png",
            };
            let path = self.file(extension, data.clone());
            let _ = write!(self.out, "#align(center, image({}))", string(&path));
            return;
        }
        self.out.push_str("#box(stroke: 0.5pt + luma(160), inset: 4pt, radius: 2pt)[");
        match label {
            Some(label) => self.out.push_str(&escape(label)),
            None => self.title(target),
        }
        self.out.push(']');
    }
}

/// Escapes text for Typst markup, where most punctuation means something.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' => out.push(' '),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// A Typst string literal.
fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The width of an SVG image, in points. usvg measures in pixels, three quarters of a point.
fn svg_width(svg: &str) -> f64 {
    resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
        .map_or(MATH_SIZE, |tree| f64::from(tree.size().width()) * 0.75)
}

/// The fonts that print: Typst's own, which the build bundles, and then the system's, for scripts
/// those lack. Finding the system's takes a moment, so it happens once.
fn font_store() -> &'static FontStore {
    static FONTS: OnceLock<FontStore> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut store = FontStore::new();
        store.extend(fonts::embedded());
        store.extend(fonts::system());
        store
    })
}

fn library() -> &'static LazyHash<Library> {
    static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIBRARY.get_or_init(|| LazyHash::new(<Library as LibraryExt>::default()))
}

/// The one Typst source of a print, and the files it refers to.
struct Document {
    main: Source,
    files: HashMap<FileId, Bytes>,
}

impl Document {
    fn new(markup: String, files: Vec<(String, Vec<u8>)>) -> Self {
        let id = |path: &str| {
            let vpath = VirtualPath::new(path).expect("a valid path");
            RootedPath::new(VirtualRoot::Project, vpath).intern()
        };
        let main = Source::new(id("/main.typ"), markup);
        let files = files.into_iter().map(|(path, data)| (id(&path), Bytes::new(data))).collect();
        Self { main, files }
    }
}

impl World for Document {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        font_store().book()
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() { Ok(self.main.clone()) } else { Err(not_found(id)) }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.get(&id).cloned().ok_or_else(|| not_found(id))
    }

    fn font(&self, index: usize) -> Option<Font> {
        font_store().font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

fn not_found(id: FileId) -> FileError {
    FileError::NotFound(PathBuf::from(id.get().vpath().get_without_slash()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a *b* #c [d] $e$ \\ f\ng"), "a \\*b\\* \\#c \\[d\\] \\$e\\$ \\\\ f g");
        assert_eq!(string("say \"hi\"\\\n"), "\"say \\\"hi\\\"\\\\\\n\"");
    }

    #[test]
    fn prints_notes_to_pdf() {
        let target = Id::generate();
        let body = [
            "# Title *with* emphasis\n\n",
            "Some **strong** text, ==highlight==, `code`, and $x^2$.\n\n",
            &format!("See [[{target}]] and [[{target}|a label]].\n\n"),
            "- [x] done\n- [ ] todo\n\n",
            "1. one\n2. two\n\n",
            "> quoted #hash [brackets]\n\n",
            "```rust\nfn main() {}\n```\n\n",
            "```math\n\\frac{a}{b}\n```\n\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n\n",
            "---\n",
        ]
        .concat();
        let mut refs = Refs::default();
        refs.titles.insert(target, "Other **note**".into());
        let pdf = pdf(&[body, "Second note".into()], &refs, &Page::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }
}
