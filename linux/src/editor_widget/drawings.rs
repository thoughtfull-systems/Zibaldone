//! Render resources and block previews for the editor. Positions are relative to a source line.

use std::collections::HashMap;
use std::ops::Range;

use adw::prelude::*;
use gtk::{gdk, graphene, pango};
use zib_core::editing::Drawing;
use zib_core::editor::display::{self, ImageSize};

use super::{Line, MARGIN_SIDES, pixels};

#[cfg(test)]
mod tests;

pub(super) type Resource = Result<(gdk::Texture, ImageSize), String>;
pub(super) type Resources = HashMap<Drawing, Resource>;

const PAD: f64 = 8.0;
const RED: (f32, f32, f32, f32) = (0.8, 0.15, 0.15, 1.0);

pub(super) enum Content {
    Error(pango::Layout),
    Image(gdk::Texture),
}

pub(super) struct Block {
    pub content: Content,
    pub cursor: usize,
    pub range: Range<usize>,
    pub rect: graphene::Rect,
}

impl Block {
    pub fn paint(&self, snapshot: &gtk::Snapshot, selected: &Range<usize>, color: &gdk::RGBA) {
        match &self.content {
            Content::Error(layout) => {
                snapshot.save();
                snapshot.translate(&graphene::Point::new(self.rect.x(), self.rect.y()));
                let (r, g, b, a) = RED;
                snapshot.append_layout(layout, &gdk::RGBA::new(r, g, b, a));
                snapshot.restore();
            }
            Content::Image(texture) => {
                snapshot.append_texture(texture, &self.rect);
                if selected.start <= self.range.start && self.range.end <= selected.end {
                    snapshot.append_color(color, &self.rect);
                }
            }
        }
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        match &self.content {
            Content::Error(_) => None,
            Content::Image(texture) => Some(texture.clone()),
        }
    }
}

/// A collapsed block keeps real text before it, such as a bullet or words before display math.
/// Indentation alone hides with its source, but still sets the drawing's left inset.
pub(super) fn collapse(line: &mut Line, drawing: &display::Drawing) {
    line.end = drawing.range.end;
    line.shaded = false;
    if line.layout.text().trim().is_empty() {
        line.layout.set_text("");
        line.pieces.clear();
        line.height = 0.0;
    }
}

/// Adds a preview below this line, centered in the width left after its original source's inset.
/// Errors use their own wrapped layout and never enter the editable source or its undo history.
pub(super) fn append(
    widget: &super::EditorWidget,
    line: &mut Line,
    drawing: &display::Drawing,
    resource: &Resource,
    origin: f64,
    width: i32,
) {
    let available = (f64::from(width) - MARGIN_SIDES - origin).max(1.0);
    let (content, w, h) = match resource {
        Ok((texture, size)) => {
            let math = matches!(drawing.source, Drawing::InlineMath(_) | Drawing::DisplayMath(_));
            let scale = if math { crate::drawings::math_scale(widget) } else { 1.0 };
            let w = f64::from(size.width) * scale;
            let h = f64::from(size.height) * scale;
            let fit = (available / w.max(1.0)).min(1.0);
            (Content::Image(texture.clone()), w * fit, h * fit)
        }
        Err(message) => {
            let layout = widget.create_pango_layout(Some(message));
            let style = pango::AttrList::new();
            style.insert(pango::AttrString::new_family("monospace"));
            layout.set_attributes(Some(&style));
            layout.set_width(super::units(available));
            layout.set_wrap(pango::WrapMode::WordChar);
            let (w, h) = layout.size();
            (Content::Error(layout), pixels(w), pixels(h))
        }
    };
    let x = origin - line.left + (available - w) / 2.0;
    #[allow(clippy::cast_possible_truncation)]
    let rect = graphene::Rect::new(x as f32, (line.height + PAD) as f32, w as f32, h as f32);
    line.blocks.push(Block { content, cursor: drawing.cursor, range: drawing.range.clone(), rect });
    line.height += h + 2.0 * PAD;
}
