//! Wrap detail controls at their own widths, without sharing columns between rows.

use gtk::glib;
use gtk::prelude::*;

const GAP: i32 = 4;

glib::wrapper! {
    pub struct DetailsLayout(ObjectSubclass<imp::DetailsLayout>)
        @extends gtk::LayoutManager;
}

pub(super) fn container() -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.set_layout_manager(Some(glib::Object::new::<DetailsLayout>()));
    content.set_halign(gtk::Align::Start);
    content
}

struct Placement {
    height: i32,
    widget: gtk::Widget,
    width: i32,
    x: i32,
    y: i32,
}

fn children(widget: &gtk::Widget) -> impl Iterator<Item = gtk::Widget> {
    std::iter::successors(widget.first_child(), WidgetExt::next_sibling)
        .filter(WidgetExt::should_layout)
}

fn horizontal(widget: &gtk::Widget) -> (i32, i32) {
    let (mut minimum, mut natural, mut count) = (0, 0, 0);
    for child in children(widget) {
        let (min, nat, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
        minimum = minimum.max(min);
        natural += nat;
        count += 1;
    }
    (minimum, natural + GAP * (count - 1).max(0))
}

fn placements(widget: &gtk::Widget, width: i32) -> (Vec<Placement>, i32) {
    let mut placed = Vec::new();
    let (mut x, mut y, mut row_height, mut row_start) = (0, 0, 0, 0);
    for child in children(widget) {
        let (_, natural, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
        let child_width = natural.min(width).max(0);
        let (_, height, _, _) = child.measure(gtk::Orientation::Vertical, child_width);
        if x > 0 && x + child_width > width {
            center(&mut placed[row_start..], row_height);
            y += row_height;
            x = 0;
            row_height = 0;
            row_start = placed.len();
        }
        placed.push(Placement { height, widget: child, width: child_width, x, y });
        row_height = row_height.max(height);
        x += child_width + GAP;
    }
    center(&mut placed[row_start..], row_height);
    (placed, y + row_height)
}

fn center(row: &mut [Placement], height: i32) {
    for item in row {
        item.y += (height - item.height) / 2;
    }
}

mod imp {
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{glib, graphene, gsk};

    use super::{horizontal, placements};

    #[derive(Default)]
    pub struct DetailsLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for DetailsLayout {
        const NAME: &'static str = "ZibDetailsLayout";
        type Type = super::DetailsLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for DetailsLayout {}

    impl LayoutManagerImpl for DetailsLayout {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            width: i32,
        ) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                let (minimum, natural) = horizontal(widget);
                (minimum, natural, -1, -1)
            } else {
                let width = if width < 0 { horizontal(widget).1 } else { width };
                let (_, height) = placements(widget, width);
                (height, height, -1, -1)
            }
        }

        // GTK transforms use floating-point coordinates for integer-pixel allocations.
        #[allow(clippy::cast_precision_loss)]
        fn allocate(&self, widget: &gtk::Widget, width: i32, _: i32, _: i32) {
            let (items, _) = placements(widget, width);
            let rtl = widget.direction() == gtk::TextDirection::Rtl;
            for item in items {
                let x = if rtl { width - item.x - item.width } else { item.x };
                let point = graphene::Point::new(x as f32, item.y as f32);
                item.widget.allocate(
                    item.width,
                    item.height,
                    -1,
                    Some(gsk::Transform::new().translate(&point)),
                );
            }
        }
    }
}
