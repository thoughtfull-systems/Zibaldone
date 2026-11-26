//! Work notes beside independently scrolling details, with a machine-local divider width.

use std::cell::Cell;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib, graphene};

use crate::app::core;

use super::Window;

const MIN_NOTE_WIDTH: i32 = 280;
const MIN_SIDEBAR_WIDTH: i32 = 600;

pub(super) struct ProjectPane {
    active: Cell<bool>,
    busy: Cell<bool>,
    changing: Cell<bool>,
    pub content: gtk::Box,
    dragging: Cell<bool>,
    focus_start: Cell<Option<(i32, Option<u32>)>>,
    handle: gtk::Widget,
    interacted: Cell<bool>,
    loaded: Cell<bool>,
    note: gtk::Widget,
    pending: Cell<bool>,
    saved: Cell<Option<u32>>,
    pub scroll: gtk::ScrolledWindow,
    width: Cell<Option<u32>>,
    pub widget: gtk::Paned,
}

impl ProjectPane {
    pub fn new(note: &impl IsA<gtk::Widget>) -> Self {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_bottom(20)
            .margin_end(20)
            .margin_start(20)
            .margin_top(20)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::External)
            .min_content_width(MIN_SIDEBAR_WIDTH)
            .width_request(MIN_SIDEBAR_WIDTH)
            .visible(false)
            .build();
        let widget = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(note)
            .end_child(&scroll)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .wide_handle(true)
            .build();
        let handle = std::iter::successors(widget.first_child(), WidgetExt::next_sibling)
            .find(|child| {
                child != note.upcast_ref::<gtk::Widget>()
                    && child != scroll.upcast_ref::<gtk::Widget>()
            })
            .expect("GtkPaned has a handle");
        Self {
            active: Cell::new(false),
            busy: Cell::new(false),
            changing: Cell::new(false),
            content,
            dragging: Cell::new(false),
            focus_start: Cell::new(None),
            handle,
            interacted: Cell::new(false),
            loaded: Cell::new(false),
            note: note.as_ref().clone(),
            pending: Cell::new(false),
            saved: Cell::new(None),
            scroll,
            width: Cell::new(None),
            widget,
        }
    }

    pub fn show(&self, visible: bool) {
        self.active.set(visible);
        self.note.set_width_request(if visible { MIN_NOTE_WIDTH } else { -1 });
        self.scroll.set_visible(visible);
        self.apply();
    }

    fn bounds(&self) -> Option<(i32, i32, i32)> {
        if !self.active.get() || !self.loaded.get() || self.widget.width() == 0 {
            return None;
        }
        let min = self.scroll.measure(gtk::Orientation::Horizontal, -1).0.max(MIN_SIDEBAR_WIDTH);
        let available = self.widget.max_position().checked_add(min)?;
        let note_min = self.widget.min_position().max(MIN_NOTE_WIDTH);
        if available < min + note_min {
            return None;
        }
        // Bound the sidebar to 70% while leaving the note at least its minimum width.
        let max = (available / 10 * 7).max(min).min(available - note_min);
        Some((available, min, max))
    }

    fn apply(&self) {
        if self.changing.get() {
            return;
        }
        let Some((available, min, max)) = self.bounds() else { return };
        let width = self.width.get().and_then(|w| i32::try_from(w).ok()).unwrap_or(available / 2);
        let position = available - width.clamp(min, max);
        self.changing.set(true);
        self.widget.set_position(position);
        self.changing.set(false);
    }

    fn moved(&self) {
        if self.changing.get() {
            return;
        }
        if (self.dragging.get() || self.widget.has_focus())
            && let Some((available, min, max)) = self.bounds()
        {
            let width = (available - self.widget.position()).clamp(min, max);
            self.width.set(u32::try_from(width).ok());
            self.interacted.set(true);
            self.pending.set(true);
        }
        self.apply();
    }

    fn on_handle(&self, event: &gdk::Event, touch: bool) -> bool {
        let (Some((x, y)), Some(native)) = (event.position(), self.widget.native()) else {
            return false;
        };
        let (dx, dy) = native.surface_transform();
        #[allow(clippy::cast_possible_truncation)]
        let point = graphene::Point::new((x - dx) as f32, (y - dy) as f32);
        let (Some(point), Some(bounds)) =
            (native.compute_point(&self.widget, &point), self.handle.compute_bounds(&self.widget))
        else {
            return false;
        };
        let reach = if touch { 56.0 } else { 6.0 };
        self.active.get()
            && point.x() >= bounds.x() - reach
            && point.x() <= bounds.x() + bounds.width() + reach
            && point.y() >= bounds.y()
            && point.y() <= bounds.y() + bounds.height()
    }
}

impl Window {
    pub(super) fn connect_project_pane(&self) {
        let w = self.clone();
        self.inner().project_pane.widget.connect_max_position_notify(move |_| {
            w.inner().project_pane.apply();
        });
        let w = self.clone();
        self.inner().project_pane.widget.connect_position_notify(move |_| {
            w.inner().project_pane.moved();
        });
        let w = self.clone();
        self.inner().project_pane.scroll.connect_map(move |scroll| {
            let w = w.clone();
            scroll.add_tick_callback(move |_, _| {
                w.inner().project_pane.apply();
                glib::ControlFlow::Break
            });
        });
        let w = self.clone();
        self.inner().project_pane.widget.connect_has_focus_notify(move |widget| {
            let pane = &w.inner().project_pane;
            if widget.has_focus() {
                pane.focus_start.set(Some((widget.position(), pane.width.get())));
            } else if let Some((position, width)) = pane.focus_start.take() {
                if widget.position() == position {
                    pane.width.set(width);
                    pane.pending.set(false);
                    pane.apply();
                }
                w.save_project_pane();
            }
        });
        let pointer = gtk::EventControllerLegacy::new();
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = self.clone();
        pointer.connect_event(move |_, event| {
            let pane = &w.inner().project_pane;
            match event.event_type() {
                gdk::EventType::ButtonPress => pane.dragging.set(pane.on_handle(event, false)),
                gdk::EventType::TouchBegin => pane.dragging.set(pane.on_handle(event, true)),
                gdk::EventType::ButtonRelease
                | gdk::EventType::TouchCancel
                | gdk::EventType::TouchEnd => {
                    pane.dragging.set(false);
                    w.save_project_pane();
                }
                _ => {}
            }
            glib::Propagation::Proceed
        });
        self.inner().project_pane.widget.add_controller(pointer);
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let stored = core(async move { state.project_sidebar_width().await }).await;
            let pane = &w.inner().project_pane;
            pane.saved.set(stored);
            if !pane.interacted.get() {
                pane.width.set(stored.filter(|w| *w > 0 && i32::try_from(*w).is_ok()));
            }
            pane.loaded.set(true);
            pane.apply();
            w.save_project_pane();
        });
    }

    pub(super) fn save_project_pane(&self) {
        let pane = &self.inner().project_pane;
        if !pane.loaded.get()
            || !pane.pending.get()
            || pane.saved.get() == pane.width.get()
            || pane.busy.replace(true)
        {
            return;
        }
        let w = self.clone();
        glib::spawn_future_local(async move {
            let pane = &w.inner().project_pane;
            while pane.saved.get() != pane.width.get() {
                let Some(width) = pane.width.get() else { break };
                let state = w.inner().state.clone();
                match core(async move { state.set_project_sidebar_width(width).await }).await {
                    Ok(()) => pane.saved.set(Some(width)),
                    Err(error) => {
                        w.toast(&format!("Couldn't save project sidebar width: {error}"));
                        break;
                    }
                }
            }
            pane.busy.set(false);
            pane.pending.set(pane.saved.get() != pane.width.get());
        });
    }

    pub(crate) async fn finish_project_pane(&self) {
        while !self.inner().project_pane.loaded.get() {
            glib::timeout_future(Duration::from_millis(10)).await;
        }
        self.save_project_pane();
        while self.inner().project_pane.busy.get() {
            glib::timeout_future(Duration::from_millis(10)).await;
        }
    }
}
