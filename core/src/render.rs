//! Math and diagrams drawn as images, as decision ZIB0009 describes: `RaTeX` for math, merman for
//! Mermaid diagrams, and resvg to turn their SVG into PNG for the apps. Results stay in the cache
//! directory, keyed by a hash of their source.

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
            // Diagrams drawn before their labels became SVG text lack labels, so they cache anew.
            Self::Mermaid => "mermaid-2",
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

/// The scale at which images are drawn.
const SCALE: f32 = 2.0;

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
        font_size: 20.0,
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
    // first, so a diagram that has some keeps Mermaid's defaults.
    let plain = "%%{init: {\"htmlLabels\": false, \"flowchart\": {\"htmlLabels\": false}}}%%\n";
    let source = if source.trim_start().starts_with("---") {
        source.to_owned()
    } else {
        format!("{plain}{source}")
    };
    let output = Renderer::new()
        .render(RenderRequest::svg(&source, OperationControl::new(), SvgRequest::default()))
        .map_err(|e| Error::Render(e.to_string()))?;
    match output {
        RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_owned()),
        _ => Err(Error::Render("no Mermaid diagram found".into())),
    }
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
}
