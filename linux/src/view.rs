//! The editor's text view: `GtkSourceView`'s, drawing paragraph backgrounds on empty lines too.
//! GTK draws a tag's paragraph background on every line but an empty one, so a blank line in a
//! code block would show as a gap in the block's shade.

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene};

mod imp {
    use std::cell::Cell;

    use adw::prelude::*;
    use gtk::glib;
    use gtk::subclass::prelude::*;
    use sourceview5::subclass::prelude::*;

    use super::shade_empty_lines;

    #[derive(Default)]
    pub struct View {
        /// The name of the tag whose paragraph background the view draws on empty lines.
        pub shaded: Cell<&'static str>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for View {
        const NAME: &'static str = "ZibView";
        type Type = super::View;
        type ParentType = sourceview5::View;
    }

    impl ObjectImpl for View {}
    impl WidgetImpl for View {}

    impl TextViewImpl for View {
        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            self.parent_snapshot_layer(layer, snapshot.clone());
            if layer == gtk::TextViewLayer::BelowText {
                shade_empty_lines(self.obj().upcast_ref(), self.shaded.get(), &snapshot);
            }
        }
    }

    impl ViewImpl for View {}
}

glib::wrapper! {
    pub struct View(ObjectSubclass<imp::View>)
        @extends sourceview5::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

/// Creates a view of `buffer` that draws the paragraph background of the tag named `shaded` on
/// empty lines as well as on the rest.
pub fn new(buffer: &sourceview5::Buffer, shaded: &'static str) -> sourceview5::View {
    let view: View = glib::Object::builder().property("buffer", buffer).build();
    view.imp().shaded.set(shaded);
    view.upcast()
}

/// Draws the paragraph background of the tag named `shaded` under the visible empty lines that
/// have it, where GTK draws none, across the width it draws on the lines with text.
fn shade_empty_lines(view: &gtk::TextView, shaded: &str, snapshot: &gtk::Snapshot) {
    let Some(tag) = view.buffer().tag_table().lookup(shaded) else { return };
    let Some(color) = tag.paragraph_background_rgba() else { return };
    let visible = view.visible_rect();
    let (mut line, _) = view.line_at_y(visible.y());
    loop {
        let (y, height) = view.line_yrange(&line);
        if y > visible.y() + visible.height() {
            break;
        }
        if line.ends_line() && height > 0 && line.has_tag(&tag) {
            // GTK draws paragraph backgrounds between the view's margins.
            let width = visible.width() - view.left_margin() - view.right_margin();
            #[allow(clippy::cast_precision_loss)]
            let rect = graphene::Rect::new(
                (visible.x() + view.left_margin()) as f32,
                y as f32,
                width as f32,
                height as f32,
            );
            snapshot.append_color(&color, &rect);
        }
        if !line.forward_line() {
            break;
        }
    }
}
