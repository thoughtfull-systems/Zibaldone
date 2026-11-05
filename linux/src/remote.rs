//! Other machines' cursors and selections in the note open for editing, as the sync specification's
//! live editing defines them. Each machine has a color: its cursor is a caret with its name above,
//! and its selection a highlight.
//!
//! Marks hold each cursor and selection, so they move with the owner's typing. Each caret is a
//! child widget that the text view places at buffer coordinates.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use zib_core::Id;
use zib_core::sync::Selection;

/// How long a machine's cursor stays without a new `cursor` message.
pub const EXPIRY: Duration = zib_core::sync::CURSOR_TIMEOUT;
/// How often a machine sends its cursor again while nothing moves.
pub const RESEND: Duration = zib_core::sync::CURSOR_REPEAT;

/// The colors of machines' cursors, as red, green, and blue, picked by machine identifier.
const COLORS: [(u8, u8, u8); 6] = [
    (0xe0, 0x1b, 0x24),
    (0x1a, 0x5f, 0xb4),
    (0x26, 0xa2, 0x69),
    (0x91, 0x41, 0xac),
    (0xc6, 0x46, 0x00),
    (0x1c, 0x71, 0xd8),
];
/// The width of a caret, in pixels.
const CARET_WIDTH: i32 = 2;

/// The name of the tag that highlights selections in color `i`.
fn tag_name(i: usize) -> String {
    format!("remote-{i}")
}

/// The tags that highlight other machines' selections, one for each color.
pub fn tags() -> Vec<gtk::TextTag> {
    COLORS
        .iter()
        .enumerate()
        .map(|(i, &(r, g, b))| {
            let rgba = gdk::RGBA::new(
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                0.25,
            );
            gtk::TextTag::builder().name(tag_name(i)).background_rgba(&rgba).build()
        })
        .collect()
}

/// A machine's color, the same on every run.
fn color_of(machine: Id) -> usize {
    let sum: usize = machine.as_bytes().iter().map(|b| usize::from(*b)).sum();
    sum % COLORS.len()
}

struct Peer {
    name: String,
    /// The selections as they arrived, to place again after the text is replaced.
    selections: Vec<Selection>,
    /// Each selection's anchor and head in the buffer.
    marks: Vec<(gtk::TextMark, gtk::TextMark)>,
    seen: Instant,
}

/// A caret with a name tag, which the text view holds as an overlay.
struct Caret {
    widget: gtk::Box,
    label: gtk::Label,
    bar: gtk::DrawingArea,
    color: Rc<Cell<usize>>,
}

pub struct Remote {
    view: glib::WeakRef<gtk::TextView>,
    peers: RefCell<HashMap<Id, Peer>>,
    /// The machines with a live session on the note, whether or not their cursor arrived.
    sessions: RefCell<HashSet<Id>>,
    /// Carets, shown or hidden. GTK offers no way to take an overlay out of a text view, so they
    /// stay to use again.
    carets: RefCell<Vec<Caret>>,
    placing: Cell<bool>,
    me: Weak<Self>,
}

impl Remote {
    pub fn new(view: &gtk::TextView) -> Rc<Self> {
        let remote = Rc::new_cyclic(|me| Self {
            view: view.downgrade(),
            peers: RefCell::default(),
            sessions: RefCell::default(),
            carets: RefCell::default(),
            placing: Cell::new(false),
            me: me.clone(),
        });
        // Typing, wrapping, and lines that GTK measures as they scroll into view all move the text
        // under the carets.
        let watch = {
            let me = remote.me.clone();
            move |view: &gtk::TextView| {
                let Some(adjustment) = view.vadjustment() else { return };
                let me = me.clone();
                adjustment.connect_changed(move |_| {
                    if let Some(remote) = me.upgrade() {
                        remote.schedule_place();
                    }
                });
            }
        };
        watch(view);
        view.connect_vadjustment_notify(watch);
        let me = remote.me.clone();
        view.buffer().connect_changed(move |_| {
            if let Some(remote) = me.upgrade() {
                remote.schedule_place();
            }
        });
        remote
    }

    /// Records that a live session with `machine` started.
    pub fn started(&self, machine: Id) {
        self.sessions.borrow_mut().insert(machine);
    }

    /// Forgets a machine whose live session ended, and its cursors.
    pub fn ended(&self, machine: Id) {
        self.sessions.borrow_mut().remove(&machine);
        self.forget(machine);
    }

    /// Whether any other machine edits the note live.
    pub fn live(&self) -> bool {
        !self.sessions.borrow().is_empty()
    }

    /// Records the selections `machine` sent, which still need placing.
    pub fn received(&self, machine: Id, name: String, selections: Vec<Selection>) {
        // A cursor finishes arriving after its session may have ended, and then it's stale.
        if !self.sessions.borrow().contains(&machine) {
            return;
        }
        let mut peers = self.peers.borrow_mut();
        let peer = peers.entry(machine).or_insert_with(|| Peer {
            name: String::new(),
            selections: Vec::new(),
            marks: Vec::new(),
            seen: Instant::now(),
        });
        peer.name = name;
        peer.selections = selections;
        peer.seen = Instant::now();
    }

    /// The selections each machine sent, to turn into positions in the buffer.
    pub fn selections(&self) -> Vec<(Id, Vec<Selection>)> {
        self.peers.borrow().iter().map(|(id, p)| (*id, p.selections.clone())).collect()
    }

    /// Places a machine's selections at character offsets in the buffer, as anchor and head.
    pub fn place_at(&self, machine: Id, offsets: &[(i32, i32)]) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        let mut peers = self.peers.borrow_mut();
        let Some(peer) = peers.get_mut(&machine) else { return };
        for (anchor, head) in peer.marks.drain(..) {
            buffer.delete_mark(&anchor);
            buffer.delete_mark(&head);
        }
        for &(anchor, head) in offsets {
            let mark = |offset: i32| buffer.create_mark(None, &buffer.iter_at_offset(offset), true);
            peer.marks.push((mark(anchor), mark(head)));
        }
        drop(peers);
        self.schedule_place();
    }

    /// Forgets every machine's cursors that went quiet for longer than `EXPIRY`.
    pub fn expire(&self) {
        let quiet: Vec<Id> = self
            .peers
            .borrow()
            .iter()
            .filter(|(_, p)| p.seen.elapsed() > EXPIRY)
            .map(|(id, _)| *id)
            .collect();
        for machine in quiet {
            self.forget(machine);
        }
    }

    /// Forgets every machine, as when the owner opens another note.
    pub fn clear(&self) {
        let machines: Vec<Id> = self.peers.borrow().keys().copied().collect();
        for machine in machines {
            self.forget(machine);
        }
        self.sessions.borrow_mut().clear();
    }

    fn forget(&self, machine: Id) {
        let Some(peer) = self.peers.borrow_mut().remove(&machine) else { return };
        if let Some(view) = self.view.upgrade() {
            let buffer = view.buffer();
            for (anchor, head) in peer.marks {
                buffer.delete_mark(&anchor);
                buffer.delete_mark(&head);
            }
        }
        self.schedule_place();
    }

    fn schedule_place(&self) {
        if self.placing.replace(true) {
            return;
        }
        let me = self.me.clone();
        glib::idle_add_local_once(move || {
            if let Some(remote) = me.upgrade() {
                remote.placing.set(false);
                remote.place();
            }
        });
    }

    /// Highlights each selection and puts a caret at each cursor.
    fn place(&self) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        for i in 0..COLORS.len() {
            buffer.remove_tag_by_name(&tag_name(i), &start, &end);
        }
        let mut used = 0;
        for (machine, peer) in self.peers.borrow().iter() {
            let color = color_of(*machine);
            for (anchor, head) in &peer.marks {
                let (anchor, head) = (buffer.iter_at_mark(anchor), buffer.iter_at_mark(head));
                if anchor != head {
                    buffer.apply_tag_by_name(&tag_name(color), &anchor, &head);
                }
                let caret = self.caret(&view, used);
                used += 1;
                caret.color.set(color);
                let (r, g, b) = COLORS[color];
                caret.label.set_markup(&format!(
                    "<span background=\"#{r:02x}{g:02x}{b:02x}\" foreground=\"white\"> {} </span>",
                    glib::markup_escape_text(&peer.name)
                ));
                let rect = view.iter_location(&head);
                caret.bar.set_content_height(rect.height());
                caret.bar.queue_draw();
                let (_, label_height, _, _) = caret.label.measure(gtk::Orientation::Vertical, -1);
                caret.widget.set_visible(true);
                view.move_overlay(
                    &caret.widget,
                    rect.x() - CARET_WIDTH / 2,
                    rect.y() - label_height,
                );
            }
        }
        for caret in self.carets.borrow().iter().skip(used) {
            caret.widget.set_visible(false);
        }
    }

    /// The caret at `index`, made and added to the view if there aren't that many yet.
    fn caret(&self, view: &gtk::TextView, index: usize) -> std::cell::Ref<'_, Caret> {
        while self.carets.borrow().len() <= index {
            let color = Rc::new(Cell::new(0));
            let bar = gtk::DrawingArea::builder()
                .content_width(CARET_WIDTH)
                .halign(gtk::Align::Start)
                .build();
            let shade = color.clone();
            bar.set_draw_func(move |_, context, width, height| {
                let (r, g, b) = COLORS[shade.get()];
                context.set_source_rgb(
                    f64::from(r) / 255.0,
                    f64::from(g) / 255.0,
                    f64::from(b) / 255.0,
                );
                context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
                let _ = context.fill();
            });
            let label = gtk::Label::builder().halign(gtk::Align::Start).build();
            let attributes = pango::AttrList::new();
            attributes.insert(pango::AttrFloat::new_scale(pango::SCALE_X_SMALL));
            label.set_attributes(Some(&attributes));
            let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
            widget.append(&label);
            widget.append(&bar);
            // Clicks go through to the text under the caret.
            widget.set_can_target(false);
            widget.set_visible(false);
            view.add_overlay(&widget, 0, 0);
            self.carets.borrow_mut().push(Caret { widget, label, bar, color });
        }
        std::cell::Ref::map(self.carets.borrow(), |carets| &carets[index])
    }
}
