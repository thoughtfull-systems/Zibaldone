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
use typst::visualize::{ExchangeFormat, RasterImage};
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::{self, FontStore};
use typst_layout::PagedDocument;

use crate::body::Alignment;
use crate::markup::{self, Block, Inline, Item};
use crate::render::{math_scale, math_svg, mermaid_svg};
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
            concat!(
                "#set page(paper: {}, numbering: \"1\", ",
                "margin: (bottom: {}mm, left: {}mm, right: {}mm, top: {}mm))"
            ),
            string(&page.paper),
            page.margin_bottom,
            page.margin_left,
            page.margin_right,
            page.margin_top
        );
        let _ = writeln!(self.out, "#set text(font: \"Libertinus Serif\", size: {TEXT_SIZE}pt)");
        self.out.push_str(concat!(
            "#set par(justify: true)\n",
            // Image descent can use the paragraph's existing leading instead of adding another
            // gap. Tall formulas still reserve the space beyond that allowance and never clip.
            "#let inline-math(path, w, h) = context {\n",
            "  let slack = par.leading.to-absolute() / 2\n",
            "  let image-depth = h * 0.25\n",
            "  let rise = calc.max(measure([H]).height, h - image-depth - slack)\n",
            "  let depth = calc.max(0pt, image-depth - slack)\n",
            "  box(width: w, height: rise + depth, baseline: (at: bottom, shift: depth),\n",
            "    place(top + left, dy: rise + image-depth - h,\n",
            "      image(path, width: w, height: h, fit: \"contain\")))\n",
            "}\n",
            "#show raw: set text(font: \"DejaVu Sans Mono\", size: 0.9em)\n",
            "#show heading: set block(above: 1.4em, below: 0.8em)\n",
            // The colors the Linux editor gives headings on a light background.
            "#show heading.where(level: 1): set text(fill: rgb(\"#813d9c\"))\n",
            "#show heading.where(level: 2): set text(fill: rgb(\"#1a5fb4\"))\n",
            "#show heading.where(level: 3): set text(fill: rgb(\"#26a269\"))\n",
            "#show heading.where(level: 4): set text(fill: rgb(\"#c64600\"))\n",
            "#show heading.where(level: 5): set text(fill: rgb(\"#c64600\"))\n",
            "#show heading.where(level: 6): set text(fill: rgb(\"#c64600\"))\n",
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
            Block::Math(source) => self.math(source, true),
            Block::Diagram(source) => match mermaid_svg(source) {
                Ok(svg) => {
                    let path = self.file("svg", svg.into_bytes());
                    let _ = write!(self.out, "#block(image({}))", string(&path));
                }
                Err(_) => self.code_text(source),
            },
            Block::Table { header_rows, header_columns, alignments, rows } => {
                self.table(*header_rows, *header_columns, alignments, rows);
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

    fn table(
        &mut self,
        header_rows: usize,
        header_columns: usize,
        alignments: &[Alignment],
        rows: &[Vec<Vec<Block>>],
    ) {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        // The Linux editor's table style: borders and header shading in the text's color at a
        // quarter and 6% opacity on white, and padding of 6 and 10 pixels, at 0.75pt a pixel.
        let _ = write!(
            self.out,
            concat!(
                "#table(columns: {columns}, stroke: 0.75pt + luma(191), ",
                "inset: (x: 7.5pt, y: 4.5pt), ",
                "fill: (x, y) => if y < {header_rows} or x < {header_columns} {{ luma(240) }}, ",
                "align: ("
            ),
            columns = columns,
            header_rows = header_rows,
            header_columns = header_columns
        );
        for column in 0..columns {
            self.out.push_str(match alignments.get(column) {
                Some(Alignment::Center) => "center, ",
                Some(Alignment::Right) => "right, ",
                Some(Alignment::None | Alignment::Left) | None => "left, ",
            });
        }
        self.out.push(')');
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
        let rendered = math_svg(source, display).and_then(|svg| {
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
                .map_err(|error| Error::Render(error.to_string()))?;
            Ok((svg, tree.size()))
        });
        let Ok((svg, size)) = rendered else {
            if display {
                self.code_text(source);
            } else {
                let _ = write!(self.out, "#raw({})", string(source));
            }
            return;
        };
        let scale = math_scale(TEXT_SIZE);
        let width = f64::from(size.width()) * scale;
        let height = f64::from(size.height()) * scale;
        let path = self.file("svg", svg.into_bytes());
        if display {
            let _ = write!(self.out, "#block(image({}, width: {width:.4}pt))", string(&path));
        } else {
            let _ =
                write!(self.out, "#inline-math({}, {width:.4}pt, {height:.4}pt)", string(&path));
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
            let width = image_width(data, media);
            let path = self.file(extension, data.clone());
            // Inline at the size the editor shows, sitting on the baseline, and no wider than
            // the line.
            let width = width.map_or("auto".into(), |w| format!("calc.min({w:.2}pt, size.width)"));
            let _ =
                write!(self.out, "#box(layout(size => image({}, width: {width})))", string(&path));
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

/// The width of an image note's file, in points, as the editor shows it: a pixel for each of its
/// pixels, whatever resolution the file records, at three quarters of a point.
fn image_width(data: &[u8], media: &str) -> Option<f64> {
    let pixels = if media == "image/svg+xml" {
        let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
        f64::from(tree.size().width())
    } else {
        let format = ExchangeFormat::detect(data)?;
        f64::from(RasterImage::plain(Bytes::new(data.to_vec()), format).ok()?.width())
    };
    Some(pixels * 0.75)
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
    use typst::layout::{Frame, FrameItem};

    /// Records text baselines and image geometry from Typst's laid-out page, before PDF encoding.
    #[derive(Default)]
    struct Geometry {
        baselines: Vec<f64>,
        clipped_images: usize,
        images: Vec<(f64, f64, f64)>,
    }

    impl Geometry {
        fn visit(&mut self, frame: &Frame, top: f64, clipped: bool) {
            for (point, item) in frame.items() {
                let y = top + point.y.to_pt();
                match item {
                    FrameItem::Group(group) => {
                        self.visit(
                            &group.frame,
                            y + group.transform.ty.to_pt(),
                            clipped || group.clip.is_some(),
                        );
                    }
                    FrameItem::Image(_, size, _) => {
                        self.clipped_images += usize::from(clipped);
                        self.images.push((y, size.x.to_pt(), size.y.to_pt()));
                    }
                    FrameItem::Text(text) if text.text.starts_with("Line") => {
                        self.baselines.push(y)
                    }
                    _ => {}
                }
            }
        }
    }

    fn geometry(body: &str) -> Geometry {
        let refs = Refs::default();
        let mut writer = Writer { refs: &refs, files: Vec::new(), out: String::new() };
        writer.preamble(&Page::default());
        writer.blocks(&markup::parse(body));
        let world = Document::new(writer.out, writer.files);
        let document = typst::compile::<PagedDocument>(&world).output.unwrap();
        let mut geometry = Geometry::default();
        for page in document.pages() {
            geometry.visit(&page.frame, 0.0, false);
        }
        geometry
    }

    #[test]
    fn inline_math_keeps_normal_line_spacing_and_matches_block_and_editor_sizes() {
        let formula = "m=a*t(h)";
        let body = format!(
            concat!(
                "Line one above.  \nLine two above.  \nLine three ${formula}$ here.  \n",
                "Line four below.  \nLine five below.\n\n$${formula}$$\n\n```math\n{formula}\n```"
            ),
            formula = formula
        );
        let geometry = geometry(&body);
        assert_eq!(geometry.clipped_images, 0);
        assert_eq!(geometry.baselines.len(), 5);
        let normal = geometry.baselines[1] - geometry.baselines[0];
        for pair in geometry.baselines.windows(2) {
            assert!((pair[1] - pair[0] - normal).abs() < 0.01, "{:?}", geometry.baselines);
        }
        assert_eq!(geometry.images.len(), 3);
        let (_, width, height) = geometry.images[0];
        for &(_, w, h) in &geometry.images[1..] {
            assert!((w - width).abs() < 0.01);
            assert!((h - height).abs() < 0.01);
        }
        let root = tempfile::tempdir().unwrap();
        let renderer = crate::render::Renderer::new(root.path());
        let screen = renderer.image(crate::render::Kind::InlineMath, formula).unwrap();
        let screen_scale = math_scale(16.0);
        // Raster images round to pixels; compare the same em size within one natural pixel.
        let tolerance = math_scale(1.0);
        assert!(
            (width / TEXT_SIZE - f64::from(screen.width) * screen_scale / 16.0).abs() < tolerance
        );
        assert!(
            (height / TEXT_SIZE - f64::from(screen.height) * screen_scale / 16.0).abs() < tolerance
        );
    }

    #[test]
    fn tall_inline_formulas_reserve_room_without_shrinking_or_overlapping_other_lines() {
        let body = concat!(
            "Line one above.  \nLine two above.  \n",
            "Line three $\\frac{\\frac{a^2}{b^2}}{\\frac{c^2}{d^2}}$ here.  \n",
            "Line four below.  \nLine five below."
        );
        let geometry = geometry(body);
        assert_eq!(geometry.clipped_images, 0);
        let (top, _, height) = geometry.images[0];
        let normal = geometry.baselines[1] - geometry.baselines[0];
        assert!(height > TEXT_SIZE);
        assert!(geometry.baselines[3] - geometry.baselines[2] > normal);
        assert!(top > geometry.baselines[1] + 1.0, "{top} {height} {:?}", geometry.baselines);
        assert!(
            top + height < geometry.baselines[3] - TEXT_SIZE * 0.65,
            "{top} {height} {:?}",
            geometry.baselines
        );
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a *b* #c [d] $e$ \\ f\ng"), "a \\*b\\* \\#c \\[d\\] \\$e\\$ \\\\ f g");
        assert_eq!(string("say \"hi\"\\\n"), "\"say \\\"hi\\\"\\\\\\n\"");
    }

    #[test]
    fn prints_notes_to_pdf() {
        let target = Id::generate();
        let body = [
            "# Title *with* emphasis\n\n## Second\n\n#### Fourth\n\n",
            "Some **strong** text, ==highlight==, `code`, and $x^2$.\n\n",
            &format!("See [[{target}]] and [[{target}|a label]].\n\n"),
            "- [x] done\n- [ ] todo\n\n",
            "1. one\n2. two\n\n",
            "> quoted #hash [brackets]\n\n",
            "```rust\nfn main() {}\n```\n\n",
            "```math\n\\frac{a}{b}\n```\n\n",
            "| a | b |\n|:-:|--:|\n| 1 | 2 |\n\n",
            "---\n",
        ]
        .concat();
        let image = Id::generate();
        let body = format!("{body}\nAn image ![[{image}]] in a line.\n");
        let mut refs = Refs::default();
        refs.titles.insert(target, "Other **note**".into());
        refs.images.insert(image, (png(40, 20), "image/png".into()));
        let pdf = pdf(&[body, "Second note".into()], &refs, &Page::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn images_print_at_the_size_the_editor_shows() {
        assert_eq!(image_width(&png(40, 20), "image/png"), Some(30.0));
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="10"/>"#;
        assert_eq!(image_width(svg, "image/svg+xml"), Some(60.0));
        assert_eq!(image_width(b"not an image", "image/png"), None);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
        pixmap.fill(resvg::tiny_skia::Color::BLACK);
        pixmap.encode_png().unwrap()
    }
}
