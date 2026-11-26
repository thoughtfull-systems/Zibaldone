//! The shared indicator for Markdown task boxes and GTK check buttons.

use adw::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

pub(crate) const CORNER_RADIUS: f32 = 0.2;
pub(crate) const OUTLINE_OPACITY: f32 = 0.55;

pub(crate) fn side(line_height: f32) -> f32 {
    (line_height * 0.8).round()
}

pub(crate) fn border(side: f32) -> f32 {
    (side * 0.1).max(1.5)
}

pub(crate) fn draw(
    snapshot: &gtk::Snapshot,
    rect: graphene::Rect,
    color: &gdk::RGBA,
    checked: bool,
) {
    let side = rect.width();
    let rounded = gsk::RoundedRect::from_rect(rect, side * CORNER_RADIUS);
    if !checked {
        let outline = color.with_alpha(OUTLINE_OPACITY);
        snapshot.append_border(&rounded, &[border(side); 4], &[outline; 4]);
        return;
    }
    let accent = adw::StyleManager::default().accent_color_rgba();
    snapshot.push_rounded_clip(&rounded);
    snapshot.append_color(&accent, &rect);
    snapshot.pop();
    draw_check(snapshot, &rect);
}

fn draw_check(snapshot: &gtk::Snapshot, rect: &graphene::Rect) {
    let side = rect.width();
    let (x, y) = (rect.x(), rect.y());
    let check = gsk::PathBuilder::new();
    check.move_to(x + side * 0.26, y + side * 0.52);
    check.line_to(x + side * 0.43, y + side * 0.69);
    check.line_to(x + side * 0.75, y + side * 0.33);
    let stroke = gsk::Stroke::new(side * 0.13);
    stroke.set_line_cap(gsk::LineCap::Round);
    stroke.set_line_join(gsk::LineJoin::Round);
    snapshot.append_stroke(&check.to_path(), &stroke, &gdk::RGBA::WHITE);
}

/// Render the editor's check mark at a resolution that works on high-density displays.
pub(crate) fn check_icon() -> glib::GString {
    let bounds = graphene::Rect::new(0.0, 0.0, 96.0, 96.0);
    let snapshot = gtk::Snapshot::new();
    draw_check(&snapshot, &bounds);
    let node = snapshot.to_node().expect("the snapshot contains the check mark");
    let renderer = gsk::CairoRenderer::new();
    renderer.realize(None::<&gdk::Surface>).expect("Cairo renders the icon without a surface");
    let image = renderer.render_texture(node, Some(&bounds));
    renderer.unrealize();
    glib::base64_encode(&image.save_to_png_bytes())
}
