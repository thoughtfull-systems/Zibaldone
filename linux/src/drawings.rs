//! Math, diagrams, and embedded images, drawn in the editor over their source while the cursor is
//! away from it. A block's drawing takes the place of its lines, and stays below them while the
//! cursor is in it. Inline math takes the place of its source in the line.
//!
//! Each drawing is a child widget that the text view places at buffer coordinates. A tag on the
//! text it covers makes room for it: space below a block's last line, or letter spacing on the last
//! character of inline math, which stays in the line, unseen.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use zib_core::editing::{Drawing, Drawn};

/// Draws a drawing as a widget at its natural size, and hands it over once it's ready, or nothing
/// when the source should show instead.
pub type Draw = Rc<dyn Fn(&Drawing, Box<dyn FnOnce(Option<gtk::Widget>)>)>;

/// Space above and below a block's drawing.
const PAD: i32 = 8;
/// The font size, in pixels, that the core draws math at, and how much larger than the text math
/// shows, as in `KaTeX`.
const MATH_PX: f64 = 20.0;
const MATH_SCALE: f64 = 1.21;
/// The tallest an embedded image grows.
const MAX_IMAGE_HEIGHT: i32 = 480;
/// A line's height for its font size, and roughly how wide the `$` is that holds inline math's room
/// before its letter spacing, both in ems.
const LINE_HEIGHT: f64 = 1.3;
const GLYPH_WIDTH: f64 = 0.55;

struct Overlay {
    id: u64,
    drawn: Drawn,
    start: gtk::TextMark,
    end: gtk::TextMark,
    /// Makes room for the drawing.
    tag: gtk::TextTag,
    revealed: bool,
    /// The drawing, with its natural size, once it's ready.
    widget: Option<(gtk::Widget, i32, i32)>,
    /// The size it shows at.
    shown: Option<(i32, i32)>,
    /// The overlay of the view that holds the drawing.
    holder: gtk::Box,
}

pub struct Drawings {
    view: glib::WeakRef<gtk::TextView>,
    draw: Draw,
    hidden: &'static str,
    overlays: RefCell<Vec<Overlay>>,
    /// Hidden overlays that hold no drawing, to use again. GTK offers no way to take an overlay out
    /// of a text view: `gtk_text_view_remove` only knows anchored children, and leaves overlays.
    spare: RefCell<Vec<gtk::Box>>,
    next_id: Cell<u64>,
    placing: Cell<bool>,
    me: Weak<Self>,
}

impl Drawings {
    /// Manages the drawings in `view`, hiding their source with the tag named `hidden`.
    pub fn new(view: &gtk::TextView, draw: Draw, hidden: &'static str) -> Rc<Self> {
        let drawings = Rc::new_cyclic(|me| Self {
            view: view.downgrade(),
            draw,
            hidden,
            overlays: RefCell::default(),
            spare: RefCell::default(),
            next_id: Cell::new(0),
            placing: Cell::new(false),
            me: me.clone(),
        });
        // The layout changes size when lines wrap anew or when lines scroll into view and GTK
        // measures them, and both move the drawings.
        let watch = {
            let me = drawings.me.clone();
            move |view: &gtk::TextView| {
                let Some(adjustment) = view.vadjustment() else { return };
                for signal in ["changed", "value-changed"] {
                    let me = me.clone();
                    adjustment.connect_local(signal, false, move |_| {
                        if let Some(drawings) = me.upgrade() {
                            drawings.schedule_place();
                        }
                        None
                    });
                }
            }
        };
        watch(view);
        view.connect_vadjustment_notify(watch);
        drawings
    }

    /// Matches the drawings to the text after it changed: keeps those that are still there,
    /// redraws those whose source changed, and adds and removes the rest. Each comes with its
    /// character range and whether the cursor touches it.
    pub fn update(&self, buffer: &gtk::TextBuffer, found: Vec<(Drawn, Range<usize>, bool)>) {
        let mut old = std::mem::take(&mut *self.overlays.borrow_mut());
        let mut overlays = Vec::with_capacity(found.len());
        let mut redraw = Vec::new();
        for (drawn, chars, revealed) in found {
            let same = old.iter().position(|o| o.drawn.drawing == drawn.drawing);
            let near = || {
                old.iter().position(|o| {
                    std::mem::discriminant(&o.drawn.drawing)
                        == std::mem::discriminant(&drawn.drawing)
                        && overlaps(&char_range(buffer, o), &chars)
                })
            };
            let mut overlay = match same.or_else(near) {
                Some(i) => old.swap_remove(i),
                None => self.new_overlay(buffer, &drawn),
            };
            if overlay.drawn.drawing != drawn.drawing {
                redraw.push((overlay.id, drawn.drawing.clone()));
            }
            buffer.move_mark(&overlay.start, &buffer.iter_at_offset(offset(chars.start)));
            buffer.move_mark(&overlay.end, &buffer.iter_at_offset(offset(chars.end)));
            overlay.drawn = drawn;
            overlay.revealed = revealed;
            overlays.push(overlay);
        }
        for overlay in old {
            self.remove(buffer, overlay);
        }
        for overlay in &overlays {
            self.apply(buffer, overlay);
        }
        *self.overlays.borrow_mut() = overlays;
        for (id, drawing) in redraw {
            self.request(id, &drawing);
        }
        self.schedule_place();
    }

    /// Shows or hides the source of the `index`th drawing as the cursor comes and goes.
    pub fn reveal(&self, buffer: &gtk::TextBuffer, index: usize, revealed: bool) {
        let mut overlays = self.overlays.borrow_mut();
        let Some(overlay) = overlays.get_mut(index) else { return };
        overlay.revealed = revealed;
        self.apply(buffer, overlay);
        drop(overlays);
        self.schedule_place();
    }

    fn new_overlay(&self, buffer: &gtk::TextBuffer, drawn: &Drawn) -> Overlay {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let tag = gtk::TextTag::new(None);
        buffer.tag_table().add(&tag);
        let start = buffer.create_mark(None, &buffer.start_iter(), true);
        let end = buffer.create_mark(None, &buffer.start_iter(), false);
        let holder = self.spare.borrow_mut().pop().unwrap_or_else(|| {
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            holder.set_visible(false);
            if let Some(view) = self.view.upgrade() {
                view.add_overlay(&holder, 0, 0);
            }
            holder
        });
        self.request(id, &drawn.drawing);
        Overlay {
            id,
            drawn: drawn.clone(),
            start,
            end,
            tag,
            revealed: false,
            widget: None,
            shown: None,
            holder,
        }
    }

    /// Takes a drawing out of the view, and keeps its overlay to use again.
    fn remove(&self, buffer: &gtk::TextBuffer, overlay: Overlay) {
        if let Some((widget, _, _)) = overlay.widget {
            overlay.holder.remove(&widget);
        }
        overlay.holder.set_visible(false);
        self.spare.borrow_mut().push(overlay.holder);
        buffer.tag_table().remove(&overlay.tag);
        buffer.delete_mark(&overlay.start);
        buffer.delete_mark(&overlay.end);
    }

    /// Asks for a drawing, which replaces the overlay's widget if the overlay still wants it.
    fn request(&self, id: u64, drawing: &Drawing) {
        let me = self.me.clone();
        let wanted = drawing.clone();
        (self.draw)(
            drawing,
            Box::new(move |widget| {
                let Some(drawings) = me.upgrade() else { return };
                drawings.arrived(id, &wanted, widget);
            }),
        );
    }

    fn arrived(&self, id: u64, drawing: &Drawing, widget: Option<gtk::Widget>) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        let mut overlays = self.overlays.borrow_mut();
        let Some(overlay) = overlays.iter_mut().find(|o| o.id == id) else { return };
        if overlay.drawn.drawing != *drawing {
            return;
        }
        if let Some((old, _, _)) = overlay.widget.take() {
            overlay.holder.remove(&old);
        }
        overlay.holder.set_visible(false);
        overlay.shown = None;
        overlay.widget = widget.map(|widget| {
            // A picture asks for the size it shows at, which its texture, drawn at twice the size,
            // would otherwise set.
            let (width, height) = match widget.size_request() {
                (width, height) if width > 0 && height > 0 => (width, height),
                _ => {
                    let (_, natural) = widget.preferred_size();
                    (natural.width(), natural.height())
                }
            };
            self.clickable(&widget, &overlay.start);
            overlay.holder.append(&widget);
            (widget, width, height)
        });
        self.apply(&buffer, overlay);
        drop(overlays);
        self.schedule_place();
    }

    /// Lets a click on a drawing put the cursor in its source, which then shows.
    fn clickable(&self, widget: &gtk::Widget, start: &gtk::TextMark) {
        let click = gtk::GestureClick::new();
        let (view, start) = (self.view.clone(), start.clone());
        click.connect_released(move |gesture, _, _, _| {
            let Some(view) = view.upgrade() else { return };
            if start.is_deleted() {
                return;
            }
            let buffer = view.buffer();
            let mut at = buffer.iter_at_mark(&start);
            at.forward_char();
            buffer.place_cursor(&at);
            view.grab_focus();
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        widget.add_controller(click);
        widget.set_cursor_from_name(Some("pointer"));
    }

    /// Hides the source that a drawing covers, and makes room for it.
    fn apply(&self, buffer: &gtk::TextBuffer, overlay: &Overlay) {
        let (start, end) = (buffer.iter_at_mark(&overlay.start), buffer.iter_at_mark(&overlay.end));
        buffer.remove_tag(&overlay.tag, &buffer.start_iter(), &buffer.end_iter());
        if overlay.revealed {
            buffer.remove_tag_by_name(self.hidden, &start, &end);
        }
        let Some(view) = self.view.upgrade() else { return };
        let Some((_, height)) = overlay.shown.or_else(|| Self::size(&view, overlay)) else {
            return;
        };
        if overlay.drawn.block {
            let mut last_line = end;
            last_line.set_line_offset(0);
            if !overlay.revealed {
                // A block that ends the text keeps its last line, which holds the room below it.
                let hide_to = if end.is_end() { last_line } else { end };
                buffer.apply_tag_by_name(self.hidden, &start, &hide_to);
            }
            overlay.tag.set_pixels_below_lines(height + 2 * PAD);
            buffer.apply_tag(&overlay.tag, &last_line, &end.max(last_line));
        } else if !overlay.revealed {
            let mut last = end;
            last.backward_char();
            buffer.apply_tag_by_name(self.hidden, &start, &last);
            // The editor hides math's closing `$` too, but this one holds the room.
            buffer.remove_tag_by_name(self.hidden, &last, &end);
            let (width, height) =
                overlay.shown.or_else(|| Self::size(&view, overlay)).unwrap_or((0, 0));
            // The character grows to make its line as tall as the drawing.
            let font = font_px(&view).max(f64::from(height) / LINE_HEIGHT);
            overlay.tag.set_size_points(font * 72.0 / 96.0);
            #[allow(clippy::cast_possible_truncation)]
            let glyph = (font * GLYPH_WIDTH).round() as i32;
            overlay.tag.set_letter_spacing((width - glyph).max(0) * pango::SCALE);
            // GTK draws a fully transparent color as black.
            overlay.tag.set_foreground_rgba(Some(&gdk::RGBA::new(0.5, 0.5, 0.5, 0.004)));
            buffer.apply_tag(&overlay.tag, &last, &end);
        }
    }

    /// The size a drawing shows at: math at the editor's font size, and nothing wider than the
    /// text.
    fn size(view: &gtk::TextView, overlay: &Overlay) -> Option<(i32, i32)> {
        let (_, width, height) = overlay.widget.as_ref()?;
        let (width, height) = (f64::from(*width), f64::from(*height));
        let scale = match overlay.drawn.drawing {
            Drawing::InlineMath(_) | Drawing::DisplayMath(_) => {
                font_px(view) * MATH_SCALE / MATH_PX
            }
            Drawing::Diagram(_) => 1.0,
            Drawing::Embed(_) => (f64::from(MAX_IMAGE_HEIGHT) / height).min(1.0),
        };
        let room = f64::from((text_width(view) - 2 * PAD).max(1));
        let scale = scale.min(room / width);
        #[allow(clippy::cast_possible_truncation)]
        Some(((width * scale).round() as i32, (height * scale).round() as i32))
    }

    fn schedule_place(&self) {
        if self.placing.replace(true) {
            return;
        }
        let me = self.me.clone();
        glib::idle_add_local_once(move || {
            if let Some(drawings) = me.upgrade() {
                drawings.placing.set(false);
                drawings.place();
            }
        });
    }

    /// Puts each drawing where its source lies.
    fn place(&self) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        let mut resized = false;
        for overlay in self.overlays.borrow_mut().iter_mut() {
            let Some((widget, _, _)) = overlay.widget.clone() else { continue };
            let Some((width, height)) = Self::size(&view, overlay) else { continue };
            if overlay.shown != Some((width, height)) {
                overlay.shown = Some((width, height));
                widget.set_size_request(width, height);
                self.apply(&buffer, overlay);
                resized = true;
            }
            overlay.holder.set_visible(overlay.drawn.block || !overlay.revealed);
            let end = buffer.iter_at_mark(&overlay.end);
            let (x, y) = if overlay.drawn.block {
                let mut last_line = end;
                last_line.set_line_offset(0);
                let (top, line_height) = view.line_yrange(&last_line);
                let left = view.left_margin();
                let x = match overlay.drawn.drawing {
                    Drawing::Embed(_) => left,
                    _ => left + (text_width(&view) - width).max(0) / 2,
                };
                (x, top + line_height - height - PAD)
            } else {
                // GTK places text after hidden text as if it showed, but places hidden text where
                // the next shown character is, which is the one that holds the room.
                let place = view.iter_location(&buffer.iter_at_mark(&overlay.start));
                (place.x() + (place.width() - width) / 2, place.y() + (place.height() - height) / 2)
            };
            view.move_overlay(&overlay.holder, x, y);
        }
        if resized {
            self.schedule_place();
        }
    }
}

fn char_range(buffer: &gtk::TextBuffer, overlay: &Overlay) -> Range<usize> {
    let at =
        |mark: &gtk::TextMark| usize::try_from(buffer.iter_at_mark(mark).offset()).unwrap_or(0);
    at(&overlay.start)..at(&overlay.end)
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// The width the text wraps at.
fn text_width(view: &gtk::TextView) -> i32 {
    view.width() - view.left_margin() - view.right_margin()
}

/// The size of the editor's font in pixels.
fn font_px(view: &gtk::TextView) -> f64 {
    let font = view.pango_context().font_description().unwrap_or_default();
    let size = f64::from(font.size()) / f64::from(pango::SCALE);
    if font.is_size_absolute() { size } else { size * 96.0 / 72.0 }
}

fn offset(chars: usize) -> i32 {
    i32::try_from(chars).unwrap_or(i32::MAX)
}
