//! Math and diagrams drawn as images, as decision ZIB0009 describes: `RaTeX` for math, merman for
//! Mermaid diagrams, and resvg to turn their SVG into PNG for the apps. Results stay in the cache
//! directory, keyed by a hash of their source.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::store::write_atomic;
use crate::{Error, Result};

/// What to draw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    InlineMath,
    DisplayMath,
    Mermaid,
}

impl Kind {
    const fn tag(self) -> &'static str {
        match self {
            Self::InlineMath => "inline-math",
            Self::DisplayMath => "display-math",
            // Diagrams drawn before their labels became SVG text lack labels, and those with front
            // matter drew their labels unwrapped, so they cache anew.
            Self::Mermaid => "mermaid-4",
        }
    }
}

/// A drawn image, as PNG at twice its natural size, for sharp text on high-density screens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub png: Vec<u8>,
    /// The natural width and height in pixels, half the PNG's.
    pub width: u32,
    pub height: u32,
}

/// The font size of math SVGs, whose dimensions are in points.
const MATH_FONT_SIZE: f64 = 20.0;
/// The scale at which images are drawn.
const SCALE: f32 = 2.0;

/// Scales the renderer's natural pixel dimensions to a surrounding text size, in pixels or points.
/// SVG points become pixels at 96 pixels per inch, so undo that conversion before scaling the em.
pub fn math_scale(text_size: f64) -> f64 {
    text_size * 0.75 / MATH_FONT_SIZE
}

/// Draws math and diagrams and keeps the results in `dir`.
#[derive(Clone, Debug)]
pub struct Renderer {
    dir: PathBuf,
}

impl Renderer {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The image for `source`, from the cache when it's there.
    pub fn image(&self, kind: Kind, source: &str) -> Result<Image> {
        let key = blake3::hash(format!("{}\n{source}", kind.tag()).as_bytes()).to_hex().to_string();
        let path = self.dir.join(format!("{key}.png"));
        if let Ok(png) = fs::read(&path)
            && let Some((width, height)) = png_size(&png)
        {
            return Ok(Image { png, width: width.div_ceil(2), height: height.div_ceil(2) });
        }
        let svg = match kind {
            Kind::InlineMath => math_svg(source, false)?,
            Kind::DisplayMath => math_svg(source, true)?,
            Kind::Mermaid => mermaid_svg(source)?,
        };
        let image = rasterize(&svg)?;
        crate::store::create_private_dir(&self.dir)?;
        write_atomic(&path, &image.png)?;
        Ok(image)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Math in the LaTeX syntax that `KaTeX` accepts, as SVG with its glyphs drawn as paths.
pub fn math_svg(source: &str, display: bool) -> Result<String> {
    use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};
    use ratex_types::math_style::MathStyle;
    let ast = ratex_parser::parser::parse(source).map_err(|e| Error::Render(e.to_string()))?;
    let style = if display { MathStyle::Display } else { MathStyle::Text };
    let laid_out = layout(&ast, &LayoutOptions::default().with_style(style));
    validate_layout(&laid_out).map_err(Error::Render)?;
    let list = to_display_list(&laid_out);
    let options = ratex_svg::SvgOptions {
        font_size: MATH_FONT_SIZE,
        padding: 2.0,
        embed_glyphs: true,
        ..ratex_svg::SvgOptions::default()
    };
    Ok(ratex_svg::render_to_svg(&list, &options))
}

/// A Mermaid diagram as SVG.
pub fn mermaid_svg(source: &str) -> Result<String> {
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
    // Labels as SVG text, not HTML, which neither resvg nor Typst draws. Front matter has to come
    // first, so the settings follow it. HTML labels would reach the drawing as one line each, as
    // `foreign_labels` gives them, without the wrapping their boxes were sized for.
    let plain = "%%{init: {\"htmlLabels\": false, \"flowchart\": {\"htmlLabels\": false}}}%%\n";
    let after_front_matter = front_matter_end(source);
    let (front, rest) = source.split_at(after_front_matter);
    let source = format!("{front}{plain}{rest}");
    let output = Renderer::new()
        .render(RenderRequest::svg(&source, OperationControl::new(), SvgRequest::default()))
        .map_err(|e| Error::Render(e.to_string()))?;
    match output {
        RenderOutput::Svg(Some(svg)) => Ok(foreign_labels(svg.svg())),
        _ => Err(Error::Render("no Mermaid diagram found".into())),
    }
}

/// The byte offset just past a diagram's front matter, the lines between a first line of `---` and
/// the next such line, or 0 when it has none.
fn front_matter_end(source: &str) -> usize {
    let start = source.len() - source.trim_start().len();
    let mut lines = source[start..].split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return 0;
    }
    let mut at = start + 4;
    for line in lines {
        at += line.len();
        if line.trim_end() == "---" {
            return at;
        }
    }
    0
}

/// Replaces the HTML labels that some diagrams, such as mind maps, draw whatever the settings say,
/// with SVG text centered where they were. Labels inside a `<switch>` already have SVG text to fall
/// back to, and stay.
fn foreign_labels(svg: &str) -> String {
    const OPEN: &str = "<foreignObject";
    const CLOSE: &str = "</foreignObject>";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find(OPEN) {
        let Some(end) = rest[start..].find(CLOSE).map(|i| start + i + CLOSE.len()) else { break };
        let before = &rest[..start];
        out.push_str(before);
        let element = &rest[start..end];
        if before.trim_end().ends_with("<switch>") {
            out.push_str(element);
        } else {
            out.push_str(&label_text(element));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// SVG text for the content of a `<foreignObject>`, a line for each paragraph or line break.
fn label_text(element: &str) -> String {
    let tag_end = element.find('>').map_or(element.len(), |i| i + 1);
    let number = |name: &str| attribute(&element[..tag_end], name).unwrap_or(0.0);
    let (x, y) = (number("x") + number("width") / 2.0, number("y") + number("height") / 2.0);
    let mut lines = vec![String::new()];
    let mut inside = &element[tag_end..];
    while !inside.is_empty() {
        if let Some(tag) = inside.strip_prefix('<') {
            let close = tag.find('>').map_or(tag.len(), |i| i + 1);
            let breaks = tag.starts_with("br") || tag.starts_with("/p>");
            if breaks && !lines.last().is_some_and(String::is_empty) {
                lines.push(String::new());
            }
            inside = &tag[close..];
        } else {
            let text_end = inside.find('<').unwrap_or(inside.len());
            if let Some(line) = lines.last_mut() {
                line.push_str(&inside[..text_end]);
            }
            inside = &inside[text_end..];
        }
    }
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    // Lines sit 1.5em apart, as Mermaid's labels do, centered on the label's middle.
    #[allow(clippy::cast_precision_loss)]
    let first = -0.75 * (lines.len() - 1) as f64;
    let mut text =
        format!("<text x=\"{x}\" y=\"{y}\" text-anchor=\"middle\" dominant-baseline=\"central\">");
    for (i, line) in lines.iter().enumerate() {
        let dy = if i == 0 { first } else { 1.5 };
        let _ = write!(text, "<tspan x=\"{x}\" dy=\"{dy}em\">{line}</tspan>");
    }
    text.push_str("</text>");
    text
}

/// The number in an attribute of an element's start tag.
fn attribute(start_tag: &str, name: &str) -> Option<f64> {
    let at = start_tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let value = &start_tag[at..];
    value[..value.find('"')?].parse().ok()
}

/// The system's fonts, loaded once, for the text in diagrams.
fn fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Draws SVG as PNG at `SCALE` times its size.
pub fn rasterize(svg: &str) -> Result<Image> {
    let options = resvg::usvg::Options { fontdb: fonts(), ..resvg::usvg::Options::default() };
    let tree =
        resvg::usvg::Tree::from_str(svg, &options).map_err(|e| Error::Render(e.to_string()))?;
    let size = tree.size();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (width, height) =
        ((size.width() * SCALE).ceil() as u32, (size.height() * SCALE).ceil() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width.max(1), height.max(1))
        .ok_or_else(|| Error::Render("an image with no size".into()))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(SCALE, SCALE),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png().map_err(|e| Error::Render(e.to_string()))?;
    Ok(Image { png, width: width.div_ceil(2), height: height.div_ceil(2) })
}

/// The width and height in a PNG's header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(16..24)?;
    let width = u32::from_be_bytes(header[..4].try_into().ok()?);
    let height = u32::from_be_bytes(header[4..].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_math_and_diagrams_and_caches_them() {
        let dir = tempfile::tempdir().unwrap();
        let renderer = Renderer::new(dir.path());
        let math = renderer.image(Kind::DisplayMath, r"\frac{a}{b} + \sqrt{x^2}").unwrap();
        assert!(math.width > 0 && math.height > 0);
        assert_eq!(&math.png[1..4], b"PNG");
        let again = renderer.image(Kind::DisplayMath, r"\frac{a}{b} + \sqrt{x^2}").unwrap();
        assert_eq!(again, math);
        let diagram =
            renderer.image(Kind::Mermaid, "flowchart LR\n  idea --> draft --> note").unwrap();
        assert!(diagram.width > 20);
        assert!(renderer.image(Kind::Mermaid, "not a diagram at all").is_err());
    }

    #[test]
    fn wraps_labels_after_front_matter() {
        let label = "    id1[This is longer text in the box]\n";
        for source in [
            format!("flowchart LR\n{label}"),
            format!("---\ntitle: Node with text\n---\nflowchart LR\n{label}"),
        ] {
            let svg = mermaid_svg(&source).unwrap();
            assert!(svg.contains(">This</tspan>") && svg.contains(">in</tspan>"), "{source:?}");
        }
        assert_eq!(front_matter_end("---\ntitle: T\n---\nflowchart"), 17);
        assert_eq!(front_matter_end("flowchart LR"), 0);
        assert_eq!(front_matter_end("---\nunclosed"), 0);
    }

    #[test]
    fn draws_html_labels_as_svg_text() {
        let svg = mermaid_svg("mindmap\n  root((Root))\n    Child one").unwrap();
        assert!(!svg.contains("<foreignObject"));
        assert!(svg.contains(">Root</tspan>") && svg.contains(">Child one</tspan>"));
        let label = label_text(concat!(
            "<foreignObject x=\"10\" width=\"40\" height=\"20\"><div><p>One &amp; two</p>",
            "<p>three<br/>four</p></div></foreignObject>"
        ));
        assert_eq!(label.matches("<tspan x=\"30\"").count(), 3);
        assert!(label.contains("y=\"10\"") && label.contains(">One &amp; two</tspan>"));
        let kept = "<switch><foreignObject width=\"1\"><div>a</div></foreignObject><text>a</text>";
        assert_eq!(foreign_labels(kept), kept);
    }
}
