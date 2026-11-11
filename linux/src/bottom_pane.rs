//! A pane below a note's text, which fits what it shows up to a share of the view until the owner
//! moves its divider.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib, graphene};

/// How far past a thin handle `GtkPaned` starts a drag, for a pointer and for a touch, matching
/// `HANDLE_EXTRA_SIZE` and `TOUCH_EXTRA_AREA_WIDTH` in its source.
const POINTER_REACH: f32 = 6.0;
const TOUCH_REACH: f32 = POINTER_REACH + 50.0;

/// The most of the view, in percent, that the panel takes while it fits what it shows.
const FITTED_SHARE: i32 = 20;

/// How far the unpin button sits from the start of the handle, lining it up with the panel's text.
const BUTTON_INSET: i32 = 12;

const CSS: &str = "
button.unpin {
  min-width: 0; min-height: 0; padding: 2px; -gtk-icon-size: 12px;
  background: none; box-shadow: none;
}
button.unpin:hover { color: @accent_color; }
";

type ResizedHandler = Rc<dyn Fn(Option<i32>)>;

/// Divides a note's text from the panel below it. The panel fits what it shows, up to a fifth of
/// the view, unless the owner has moved the handle to give the open note's panel a height of its
/// own. An unpin button on the handle fits the panel to what it shows again.
pub struct BottomPane {
    /// Set while a pointer or touch that went down on the handle stays down.
    dragging: Cell<bool>,
    /// Where the handle was, and the height the owner had given the panel, when the handle took
    /// keyboard focus, while it has it.
    focus_start: Cell<Option<(i32, Option<i32>)>>,
    handle: gtk::Widget,
    /// The height the owner gave the open note's panel, if they have.
    height: Cell<Option<i32>>,
    on_resized: RefCell<Option<ResizedHandler>>,
    panel: gtk::Box,
    paned: gtk::Paned,
    /// Set while the pane moves the handle itself, so the move isn't taken for the owner's.
    placing: Cell<bool>,
    /// Set once the owner moves the handle, until they let go of it.
    resized: Cell<bool>,
    unpin_button: gtk::Button,
    pub widget: gtk::Overlay,
}

impl BottomPane {
    /// Puts `panel` below `text`. The text keeps at least its own minimum height, however far the
    /// owner moves the handle.
    pub fn new(text: &impl IsA<gtk::Widget>, panel: &gtk::Box) -> Rc<Self> {
        style();
        let divided = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(text)
            .end_child(panel)
            .resize_end_child(false)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        // GtkPaned keeps its handle as an internal child between the two it shows.
        let handle = std::iter::successors(divided.first_child(), WidgetExt::next_sibling)
            .find(|child| {
                child != text.upcast_ref::<gtk::Widget>()
                    && child != panel.upcast_ref::<gtk::Widget>()
            })
            .expect("GtkPaned has a handle");
        let unpin_button = gtk::Button::builder()
            .icon_name("view-pin-symbolic")
            .tooltip_text("Unpin the height")
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .css_classes(["unpin"])
            .visible(false)
            .build();
        unpin_button.update_property(&[gtk::accessible::Property::Label("Unpin the height")]);
        let widget = gtk::Overlay::builder().child(&divided).build();
        widget.add_overlay(&unpin_button);
        let this = Rc::new(Self {
            dragging: Cell::new(false),
            focus_start: Cell::new(None),
            handle,
            height: Cell::new(None),
            on_resized: RefCell::default(),
            panel: panel.clone(),
            paned: divided,
            placing: Cell::new(false),
            resized: Cell::new(false),
            unpin_button,
            widget,
        });
        this.widget.connect_get_child_position(glib::clone!(
            #[weak]
            this,
            #[upgrade_or]
            None,
            move |_, child| this.place_button(child)
        ));
        this.unpin_button.connect_clicked(glib::clone!(
            #[weak]
            this,
            move |_| this.unpin()
        ));
        this.paned.connect_max_position_notify(glib::clone!(
            #[weak]
            this,
            move |_| this.update()
        ));
        this.paned.connect_position_notify(glib::clone!(
            #[weak]
            this,
            move |_| this.moved()
        ));
        this.paned.connect_has_focus_notify(glib::clone!(
            #[weak]
            this,
            move |paned| this.focus_changed(paned.has_focus())
        ));
        // GtkPaned doesn't say when the owner drags its handle, and it also moves the handle
        // itself as the window resizes.
        let pointer = gtk::EventControllerLegacy::new();
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        pointer.connect_event(glib::clone!(
            #[weak]
            this,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress => this.dragging.set(this.on_handle(event, false)),
                    gdk::EventType::TouchBegin => this.dragging.set(this.on_handle(event, true)),
                    gdk::EventType::ButtonRelease
                    | gdk::EventType::TouchEnd
                    | gdk::EventType::TouchCancel => this.released(),
                    _ => {}
                }
                glib::Propagation::Proceed
            }
        ));
        this.paned.add_controller(pointer);
        this
    }

    /// Calls `f` with the height the owner moves the panel to, once they let go of the handle, or
    /// with `None` once they fit the panel to what it shows again.
    pub fn connect_resized(&self, f: impl Fn(Option<i32>) + 'static) {
        *self.on_resized.borrow_mut() = Some(Rc::new(f));
    }

    /// Gives the panel the height the owner chose for the note that now shows, or fits the panel
    /// to what it shows if they haven't chosen one.
    pub fn show(&self, height: Option<i32>) {
        self.height.set(height);
        self.resized.set(false);
        self.update();
    }

    fn unpin(&self) {
        self.height.set(None);
        self.resized.set(false);
        self.update();
        let handler = self.on_resized.borrow().clone();
        if let Some(handler) = handler {
            handler(None);
        }
    }

    /// Places the handle again, after what the panel shows changes.
    pub fn update(&self) {
        self.unpin_button.set_visible(self.height.get().is_some() && self.panel.get_visible());
        if !self.ready() || self.dragging.get() || self.focus_start.get().is_some() {
            return;
        }
        let min = self.min();
        let height = self.height.get().unwrap_or_else(|| {
            let fitted = self.panel.measure(gtk::Orientation::Vertical, self.paned.width()).1;
            // The pane's height, less the handle, which is thin enough to leave out.
            let view = self.paned.max_position() + min;
            fitted.min(view * FITTED_SHARE / 100)
        });
        // GtkPaned measures from the start, and its maximum leaves the panel its least height.
        let position = self.paned.max_position() + min - height.max(min);
        if self.paned.is_position_set() && self.paned.position() == position {
            return;
        }
        self.placing.set(true);
        self.paned.set_position(position);
        self.placing.set(false);
    }

    fn moved(&self) {
        let by_owner = self.dragging.get() || self.focus_start.get().is_some();
        // GtkOverlay places the button only when it's allocated, which a move of the handle
        // doesn't cause. GtkPaned's own moves happen while the overlay allocates it, before the
        // overlay places the button, and queuing another allocation then would lay out twice.
        if by_owner || self.placing.get() {
            self.widget.queue_allocate();
        }
        if by_owner && !self.placing.get() && self.ready() {
            self.height.set(Some(self.paned.max_position() + self.min() - self.paned.position()));
            self.resized.set(true);
            self.unpin_button.set_visible(true);
        }
    }

    fn released(&self) {
        self.dragging.set(false);
        self.finish_resize();
    }

    /// `GtkPaned` moves its handle with the arrow keys while the pane itself has focus, which F8
    /// gives it, and Escape puts the handle back before focus returns to the text.
    fn focus_changed(&self, focused: bool) {
        if focused {
            self.focus_start.set(Some((self.paned.position(), self.height.get())));
            return;
        }
        let Some((position, height)) = self.focus_start.take() else {
            return;
        };
        if self.paned.position() == position {
            self.height.set(height);
            self.resized.set(false);
            self.unpin_button.set_visible(height.is_some());
        } else {
            self.finish_resize();
        }
    }

    fn finish_resize(&self) {
        let handler = self.on_resized.borrow().clone();
        if self.resized.take()
            && let Some(height) = self.height.get()
            && let Some(handler) = handler
        {
            handler(Some(height));
        }
    }

    /// Whether `event` went down where `GtkPaned` starts a drag of the handle.
    fn on_handle(&self, event: &gdk::Event, touch: bool) -> bool {
        let (Some((x, y)), Some(native)) = (event.position(), self.paned.native()) else {
            return false;
        };
        let (dx, dy) = native.surface_transform();
        #[allow(clippy::cast_possible_truncation)]
        let point = graphene::Point::new((x - dx) as f32, (y - dy) as f32);
        let (Some(point), Some(bounds)) =
            (native.compute_point(&self.paned, &point), self.handle.compute_bounds(&self.paned))
        else {
            return false;
        };
        let reach = if touch { TOUCH_REACH } else { POINTER_REACH };
        (bounds.y() - reach..=bounds.y() + bounds.height() + reach).contains(&point.y())
    }

    fn place_button(&self, button: &gtk::Widget) -> Option<gdk::Rectangle> {
        let bounds = self.handle.compute_bounds(&self.widget)?;
        let (_, width, _, _) = button.measure(gtk::Orientation::Horizontal, -1);
        let (_, height, _, _) = button.measure(gtk::Orientation::Vertical, width);
        let x = match self.widget.direction() {
            gtk::TextDirection::Rtl => self.widget.width() - width - BUTTON_INSET,
            _ => BUTTON_INSET,
        };
        #[allow(clippy::cast_possible_truncation)]
        let y = (bounds.y() + bounds.height() / 2.0).round() as i32 - height / 2;
        Some(gdk::Rectangle::new(x, y, width, height))
    }

    /// Whether the panel shows and the pane knows its own height. Until the pane's first
    /// allocation, its maximum is a placeholder.
    fn ready(&self) -> bool {
        self.panel.get_visible() && self.paned.max_position() < i32::MAX
    }

    fn min(&self) -> i32 {
        self.panel.measure(gtk::Orientation::Vertical, self.paned.width()).0
    }
}

/// Adds the unpin button's style to the display once.
fn style() {
    thread_local! {
        static ADDED: Cell<bool> = const { Cell::new(false) };
    }
    if ADDED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
