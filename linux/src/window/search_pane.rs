//! A resizable search sidebar whose width and visibility belong to this machine.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::store::SidebarLayout;

use crate::app::core;

use super::{Window, pane_event_point};

const COLLAPSE_WIDTH: i32 = 100;
const DEFAULT_WIDTH: u32 = 320;
const SAVE_PAUSE: Duration = Duration::from_millis(500);

pub(super) struct SearchPane {
    busy: Cell<bool>,
    changing: Cell<bool>,
    dragging: Cell<bool>,
    handle: gtk::Widget,
    interacted: Cell<bool>,
    layout: Cell<SidebarLayout>,
    pending_width: Cell<Option<u32>>,
    ready: Cell<bool>,
    saved: Cell<Option<SidebarLayout>>,
    sidebar: gtk::Box,
    timer: RefCell<Option<glib::SourceId>>,
    toggle: gtk::ToggleButton,
    pub widget: gtk::Paned,
}

impl SearchPane {
    pub fn new(
        sidebar: &gtk::Box,
        content: &impl IsA<gtk::Widget>,
        toggle: &gtk::ToggleButton,
    ) -> Self {
        let widget = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(sidebar)
            .end_child(content)
            .resize_start_child(false)
            .resize_end_child(true)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .position(i32::try_from(DEFAULT_WIDTH).unwrap())
            .wide_handle(true)
            .build();
        let handle = std::iter::successors(widget.first_child(), WidgetExt::next_sibling)
            .find(|child| {
                child != sidebar.upcast_ref::<gtk::Widget>()
                    && child != content.upcast_ref::<gtk::Widget>()
            })
            .expect("GtkPaned has a handle");
        Self {
            busy: Cell::new(false),
            changing: Cell::new(false),
            dragging: Cell::new(false),
            handle,
            interacted: Cell::new(false),
            layout: Cell::new(SidebarLayout { visible: true, width: DEFAULT_WIDTH }),
            pending_width: Cell::new(None),
            ready: Cell::new(false),
            saved: Cell::new(None),
            sidebar: sidebar.clone(),
            timer: RefCell::default(),
            toggle: toggle.clone(),
            widget,
        }
    }

    pub fn shows_sidebar(&self) -> bool {
        self.toggle.is_active()
    }

    pub fn set_show_sidebar(&self, visible: bool) {
        self.toggle.set_active(visible);
    }

    fn collapse(&self) {
        // Keep the width from before this drag rather than the last tiny intermediate step.
        self.dragging.set(false);
        self.pending_width.take();
        self.set_show_sidebar(false);
    }

    fn resize_event(&self, event: &gdk::Event) {
        match event.event_type() {
            gdk::EventType::ButtonPress | gdk::EventType::TouchBegin => {
                let primary = event
                    .downcast_ref::<gdk::ButtonEvent>()
                    .is_none_or(|event| event.button() == gdk::BUTTON_PRIMARY);
                self.dragging.set(
                    primary
                        && self.shows_sidebar()
                        && pane_event_point(&self.widget, event).is_some_and(|point| {
                            self.handle
                                .compute_bounds(&self.widget)
                                .is_some_and(|bounds| bounds.contains_point(&point))
                        }),
                );
            }
            gdk::EventType::MotionNotify | gdk::EventType::TouchUpdate if self.dragging.get() => {
                if let Some(point) = pane_event_point(&self.widget, event) {
                    let mut width = f64::from(point.x());
                    if self.widget.direction() == gtk::TextDirection::Rtl {
                        width = f64::from(self.widget.width()) - width;
                    }
                    if width <= f64::from(COLLAPSE_WIDTH) {
                        self.collapse();
                    }
                }
            }
            gdk::EventType::ButtonRelease
            | gdk::EventType::TouchEnd
            | gdk::EventType::TouchCancel => {
                self.dragging.set(false);
            }
            _ => {}
        }
    }

    fn apply(&self) {
        self.changing.set(true);
        let layout = self.layout.get();
        self.toggle.set_active(layout.visible);
        self.sidebar.set_visible(layout.visible);
        self.widget.set_position(i32::try_from(layout.width).unwrap_or(i32::MAX));
        self.changing.set(false);
    }
}

impl Window {
    pub(super) fn connect_search_pane(&self) {
        let w = self.clone();
        self.inner().sidebar_split.toggle.connect_toggled(move |toggle| {
            let pane = &w.inner().sidebar_split;
            if pane.changing.get() {
                return;
            }
            pane.interacted.set(true);
            let mut layout = pane.layout.get();
            layout.visible = toggle.is_active();
            pane.layout.set(layout);
            pane.apply();
            w.save_search_pane();
        });
        let w = self.clone();
        self.inner().sidebar_split.widget.connect_position_notify(move |widget| {
            let pane = &w.inner().sidebar_split;
            if pane.changing.get() || !pane.ready.get() || !pane.shows_sidebar() {
                return;
            }
            let width = widget.position();
            if width <= COLLAPSE_WIDTH {
                pane.collapse();
                return;
            }
            pane.pending_width.set(Some(u32::try_from(width).unwrap_or(DEFAULT_WIDTH)));
            if let Some(timer) = pane.timer.take() {
                timer.remove();
            }
            let saving = w.clone();
            *pane.timer.borrow_mut() = Some(glib::timeout_add_local_once(SAVE_PAUSE, move || {
                saving.inner().sidebar_split.timer.take();
                saving.save_search_pane();
            }));
        });
        // The divider stops at the content minimum. A continued drag can still hide the sidebar.
        let pointer = gtk::EventControllerLegacy::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = self.clone();
        pointer.connect_event(move |_, event| {
            w.inner().sidebar_split.resize_event(event);
            glib::Propagation::Proceed
        });
        self.inner().sidebar_split.widget.add_controller(pointer);
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let stored = core(async move { state.search_sidebar().await }).await;
            let pane = &w.inner().sidebar_split;
            let mut layout =
                stored.unwrap_or(SidebarLayout { visible: true, width: DEFAULT_WIDTH });
            if layout.width <= u32::try_from(COLLAPSE_WIDTH).unwrap()
                || layout.width > i32::MAX.cast_unsigned()
            {
                layout.width = DEFAULT_WIDTH;
            }
            pane.saved.set(Some(layout));
            if pane.interacted.get() {
                layout.visible = pane.layout.get().visible;
            }
            pane.layout.set(layout);
            pane.apply();
            pane.ready.set(true);
            w.save_search_pane();
        });
    }

    pub(super) fn save_search_pane(&self) {
        let pane = &self.inner().sidebar_split;
        if let Some(timer) = pane.timer.take() {
            timer.remove();
        }
        if let Some(width) = pane.pending_width.take() {
            let mut layout = pane.layout.get();
            layout.width = width;
            pane.layout.set(layout);
        }
        if !pane.ready.get()
            || pane.saved.get() == Some(pane.layout.get())
            || pane.busy.replace(true)
        {
            return;
        }
        let w = self.clone();
        glib::spawn_future_local(async move {
            let pane = &w.inner().sidebar_split;
            while pane.saved.get() != Some(pane.layout.get()) {
                let layout = pane.layout.get();
                let state = w.inner().state.clone();
                match core(async move { state.set_search_sidebar(layout).await }).await {
                    Ok(()) => pane.saved.set(Some(layout)),
                    Err(error) => {
                        w.toast(&format!("Couldn't save sidebar layout: {error}"));
                        break;
                    }
                }
            }
            pane.busy.set(false);
        });
    }

    pub(crate) async fn finish_search_pane(&self) {
        while !self.inner().sidebar_split.ready.get() {
            glib::timeout_future(Duration::from_millis(10)).await;
        }
        self.save_search_pane();
        while self.inner().sidebar_split.busy.get() {
            glib::timeout_future(Duration::from_millis(10)).await;
        }
    }
}
