//! The editor's text view: `GtkSourceView`'s, drawing paragraph backgrounds on empty lines too,
//! drawing bullets and task boxes over the list syntax that a tag hides, and drawing a bar beside
//! block quotes for each level they nest. GTK draws a tag's paragraph background on every line
//! but an empty one, so a blank line in a code block would show as a gap in the block's shade.

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

mod imp {
    use std::cell::Cell;

    use adw::prelude::*;
    use gtk::glib;
    use gtk::subclass::prelude::*;
    use sourceview5::subclass::prelude::*;

    use super::{draw_over, draw_quote_bars, shade_empty_lines};

    #[derive(Default)]
    pub struct View {
        /// The name of the tag whose paragraph background the view draws on empty lines.
        pub shaded: Cell<&'static str>,
        /// The name of the tag on list markers and task boxes that the view draws over.
        pub drawn_over: Cell<&'static str>,
        /// The names of the tags on the lines of block quotes, one for each depth, outermost first.
        pub quote_margins: Cell<&'static [&'static str]>,
        /// The names of the tags that make room before quotes in list items, one for each depth.
        pub quote_rooms: Cell<&'static [&'static str]>,
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
            match layer {
                gtk::TextViewLayer::BelowText => {
                    shade_empty_lines(self.obj().upcast_ref(), self.shaded.get(), &snapshot);
                    let (margins, rooms) = (self.quote_margins.get(), self.quote_rooms.get());
                    draw_quote_bars(self.obj().upcast_ref(), margins, rooms, &snapshot);
                }
                gtk::TextViewLayer::AboveText => {
                    draw_over(self.obj().upcast_ref(), self.drawn_over.get(), &snapshot);
                }
                _ => {}
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
/// empty lines as well as on the rest, and draws a bullet or a task box over each run of text
/// with the tag named `drawn_over`, which should leave that text unseen. Beside each line with one
/// of the tags named in `quote_margins`, which move the lines of block quotes right by
/// `QUOTE_INDENT` for each depth, it draws a bar for each quote the line lies in, and likewise in
/// the room that the tags named in `quote_rooms` make on the character before a quote.
pub fn new(
    buffer: &sourceview5::Buffer,
    shaded: &'static str,
    drawn_over: &'static str,
    quote_margins: &'static [&'static str],
    quote_rooms: &'static [&'static str],
) -> sourceview5::View {
    let view: View = glib::Object::builder().property("buffer", buffer).build();
    view.imp().shaded.set(shaded);
    view.imp().drawn_over.set(drawn_over);
    view.imp().quote_margins.set(quote_margins);
    view.imp().quote_rooms.set(quote_rooms);
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

/// How far each level of block quote moves its lines right, past the editor's left margin.
pub const QUOTE_INDENT: i32 = 18;
/// How far into its level's room a quote's bar sits, and how wide it is.
const BAR_OFFSET: i32 = 4;
const BAR_WIDTH: i32 = 3;

/// Draws a bar beside each visible line of a block quote for each quote it lies in, so bars of
/// lines in a row join up, and nested quotes show nested bars: from the left margin in, on a line
/// that a quote's margin moves right, or in the room before a quote in a list item.
fn draw_quote_bars(
    view: &gtk::TextView,
    quote_margins: &[&str],
    quote_rooms: &[&str],
    snapshot: &gtk::Snapshot,
) {
    let table = view.buffer().tag_table();
    let tags = |names: &[&str]| -> Vec<gtk::TextTag> {
        names.iter().filter_map(|name| table.lookup(name)).collect()
    };
    let (margins, rooms) = (tags(quote_margins), tags(quote_rooms));
    if margins.is_empty() && rooms.is_empty() {
        return;
    }
    let color = view.color();
    let color = gtk::gdk::RGBA::new(color.red(), color.green(), color.blue(), 0.3);
    let visible = view.visible_rect();
    let (mut line, _) = view.line_at_y(visible.y());
    loop {
        let (y, height) = view.line_yrange(&line);
        if y > visible.y() + visible.height() {
            break;
        }
        if let Some((depth, left, height)) = quote_room(view, &line, &margins, &rooms, height) {
            for level in 0..depth {
                let x = left + level * QUOTE_INDENT + BAR_OFFSET;
                #[allow(clippy::cast_precision_loss)]
                let rect = graphene::Rect::new(x as f32, y as f32, BAR_WIDTH as f32, height as f32);
                snapshot.append_color(&color, &rect);
            }
        }
        if !line.forward_line() {
            break;
        }
    }
}

/// How many quotes the line starting at `line` lies in, where the room for their bars starts, in
/// buffer coordinates, and how tall the bars are, for a line `height` tall. Bars in the room
/// before a quote in a list item reach down only as far as the line's first row of text, as the
/// rows it wraps onto start at the margin.
fn quote_room(
    view: &gtk::TextView,
    line: &gtk::TextIter,
    margins: &[gtk::TextTag],
    rooms: &[gtk::TextTag],
    height: i32,
) -> Option<(i32, i32, i32)> {
    let depth = |index: usize| i32::try_from(index + 1).unwrap_or(0);
    if let Some(index) = margins.iter().position(|tag| line.has_tag(tag)) {
        return Some((depth(index), view.visible_rect().x() + view.left_margin(), height));
    }
    rooms.iter().enumerate().find_map(|(index, tag)| {
        let mut room = *line;
        if !room.has_tag(tag) && (!room.forward_to_tag_toggle(Some(tag)) || !room.has_tag(tag)) {
            return None;
        }
        if room.line() != line.line() {
            return None;
        }
        // The room follows the character's own width, a column of the monospace font that list
        // prefixes use. Pango leaves letter spacing out at the end of a line, as on a blank `>`
        // line whose `>` hides, so the room's own width can't place the bars.
        let cell = view.iter_location(&room);
        let mut end = *line;
        end.forward_to_line_end();
        let wraps = view.iter_location(&end).y() > cell.y();
        let (top, _) = view.line_yrange(line);
        let height = if wraps { cell.y() + cell.height() - top } else { height };
        Some((depth(index), cell.x() + monospace_width(view), height))
    })
}

/// Draws a bullet over each list marker with the tag named `drawn_over` in the visible lines, and
/// a box over each task box with it: a plain one, or a filled and checked one.
fn draw_over(view: &gtk::TextView, drawn_over: &str, snapshot: &gtk::Snapshot) {
    let Some(tag) = view.buffer().tag_table().lookup(drawn_over) else { return };
    let visible = view.visible_rect();
    let (mut at, _) = view.line_at_y(visible.y());
    let color = view.color();
    loop {
        if !at.has_tag(&tag) && !at.forward_to_tag_toggle(Some(&tag)) {
            break;
        }
        let start = at;
        at.forward_to_tag_toggle(Some(&tag));
        if view.iter_location(&start).y() > visible.y() + visible.height() {
            break;
        }
        if let Some((_, checked)) = task_box(&start, &at) {
            draw_box(snapshot, &box_rect(view, &start, &at), checked, &color);
            continue;
        }
        let mut marker = start;
        while marker < at {
            draw_bullet(view, snapshot, &marker, &color);
            marker.forward_char();
        }
    }
}

/// Draws the bullet for the list marker at `marker` over its cell, as wide as the inside of a task
/// box on its line, so the shapes look alike in size whatever font draws them, and line up with
/// boxes.
fn draw_bullet(
    view: &gtk::TextView,
    snapshot: &gtk::Snapshot,
    marker: &gtk::TextIter,
    color: &gtk::gdk::RGBA,
) {
    // The glyph and its weight.
    let (glyph, weight) = match marker.char() {
        '*' => ("•", gtk::pango::Weight::Normal),
        '-' => ("⁃", gtk::pango::Weight::Normal),
        '+' => ("＋", gtk::pango::Weight::Bold),
        _ => return,
    };
    let cell = view.iter_location(marker);
    let layout = view.create_pango_layout(Some(glyph));
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_weight(weight));
    layout.set_attributes(Some(&attributes));
    let (ink, _) = layout.extents();
    if ink.width() <= 0 || ink.height() <= 0 {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let (cell, ink) = (
        graphene::Rect::new(
            cell.x() as f32,
            cell.y() as f32,
            cell.width() as f32,
            cell.height() as f32,
        ),
        graphene::Rect::new(
            ink.x() as f32,
            ink.y() as f32,
            ink.width() as f32,
            ink.height() as f32,
        )
        .scale(1.0 / gtk::pango::SCALE as f32, 1.0 / gtk::pango::SCALE as f32),
    );
    let side = box_side(cell.height());
    let scale = (side - 2.0 * box_border(side)) / ink.width();
    snapshot.save();
    snapshot.translate(&cell.center());
    snapshot.scale(scale, scale);
    snapshot.translate(&graphene::Point::new(-ink.center().x(), -ink.center().y()));
    snapshot.append_layout(&layout, color);
    snapshot.restore();
}

/// The `[` of the task box that a run of drawn-over text from `start` to `end` stands for, and
/// whether it's checked: the box itself, or the marker of its item, which the hidden box follows.
fn task_box(start: &gtk::TextIter, end: &gtk::TextIter) -> Option<(gtk::TextIter, bool)> {
    let open = [*start, *end].into_iter().find(|at| at.char() == '[')?;
    let mut mark = open;
    mark.forward_char();
    Some((open, mark.char() != ' '))
}

/// The cells of the text from `start` to `end`, on one line.
fn run_rect(view: &gtk::TextView, start: &gtk::TextIter, end: &gtk::TextIter) -> graphene::Rect {
    let first = view.iter_location(start);
    let mut last = *end;
    last.backward_char();
    let last = view.iter_location(&last);
    #[allow(clippy::cast_precision_loss)]
    graphene::Rect::new(
        first.x() as f32,
        first.y() as f32,
        (last.x() + last.width() - first.x()) as f32,
        first.height() as f32,
    )
}

/// Where the view draws the task box whose drawn-over text runs from `start` to `end`: a square a
/// little smaller than a line, centered on the box's source, or in place of a marker, on the
/// marker, where its siblings' bullets are.
fn box_rect(view: &gtk::TextView, start: &gtk::TextIter, end: &gtk::TextIter) -> graphene::Rect {
    let run = run_rect(view, start, end);
    let side = box_side(run.height());
    #[allow(clippy::cast_precision_loss)]
    let center = if start.char() == '[' {
        run.x() + run.width() / 2.0
    } else {
        run.x() + view.iter_location(start).width() as f32 / 2.0
    };
    graphene::Rect::new(
        (center - side / 2.0).round(),
        (run.y() + (run.height() - side) / 2.0).round(),
        side,
        side,
    )
}

/// Draws a task box in `rect`: an outline in the text's color, or when `checked`, filled with the
/// accent color and checked.
fn draw_box(
    snapshot: &gtk::Snapshot,
    rect: &graphene::Rect,
    checked: bool,
    color: &gtk::gdk::RGBA,
) {
    let side = rect.width();
    let rounded = gsk::RoundedRect::from_rect(*rect, side * 0.2);
    if !checked {
        let outline = gtk::gdk::RGBA::new(color.red(), color.green(), color.blue(), 0.55);
        let width = box_border(side);
        snapshot.append_border(&rounded, &[width; 4], &[outline; 4]);
        return;
    }
    let style = adw::StyleManager::default();
    snapshot.push_rounded_clip(&rounded);
    snapshot.append_color(&style.accent_color_rgba(), rect);
    snapshot.pop();
    let (x, y) = (rect.x(), rect.y());
    let check = gsk::PathBuilder::new();
    check.move_to(x + side * 0.26, y + side * 0.52);
    check.line_to(x + side * 0.43, y + side * 0.69);
    check.line_to(x + side * 0.75, y + side * 0.33);
    let stroke = gsk::Stroke::new(side * 0.13);
    stroke.set_line_cap(gsk::LineCap::Round);
    stroke.set_line_join(gsk::LineJoin::Round);
    snapshot.append_stroke(&check.to_path(), &stroke, &gtk::gdk::RGBA::WHITE);
}

/// The `[` of the task box that the view draws under `x` and `y`, in buffer coordinates, or over
/// whose source they lie. A box can reach left past its source, over the character before it.
pub fn box_at(view: &gtk::TextView, x: i32, y: i32) -> Option<gtk::TextIter> {
    let tag =
        view.buffer().tag_table().lookup(view.downcast_ref::<View>()?.imp().drawn_over.get())?;
    let under = view.iter_at_location(x, y)?;
    let mut next = under;
    next.forward_char();
    #[allow(clippy::cast_precision_loss)]
    let point = graphene::Point::new(x as f32, y as f32);
    [under, next].into_iter().filter(|at| at.has_tag(&tag)).find_map(|mut start| {
        if !start.starts_tag(Some(&tag)) {
            start.backward_to_tag_toggle(Some(&tag));
        }
        let mut end = start;
        end.forward_to_tag_toggle(Some(&tag));
        let (open, _) = task_box(&start, &end)?;
        let area = run_rect(view, &start, &end).union(&box_rect(view, &start, &end));
        area.contains_point(&point).then_some(open)
    })
}

/// The side of the task box on a line of `height`, a little smaller than the line.
fn box_side(height: f32) -> f32 {
    (height * 0.8).round()
}

/// The width of the border of a task box whose side is `side`.
fn box_border(side: f32) -> f32 {
    (side * 0.1).max(1.5)
}

/// The width of a character of the monospace font that list prefixes use, in pixels.
pub fn monospace_width(view: &gtk::TextView) -> i32 {
    let layout = view.create_pango_layout(Some("0"));
    let mut font = view.pango_context().font_description().unwrap_or_default();
    font.set_family("monospace");
    layout.set_font_description(Some(&font));
    layout.pixel_size().0
}
