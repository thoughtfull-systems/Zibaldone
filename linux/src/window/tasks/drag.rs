//! Drag handles reorder projects and reorder or nest tasks within their lists.

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::Id;
use zib_core::schema;
use zib_core::work::{Item, Tree};

use crate::app::core;

use super::Window;

#[derive(Clone, glib::Boxed)]
#[boxed_type(name = "ZibWorkDrag")]
struct WorkDrag {
    note: Id,
    space: Id,
}

#[derive(Clone, Copy)]
enum Zone {
    After,
    Before,
    Subtask,
}

impl Zone {
    fn at(y: f64, height: i32, project: bool) -> Self {
        let fraction = y / f64::from(height.max(1));
        if project {
            if fraction < 0.5 { Self::Before } else { Self::After }
        } else if fraction < 0.25 {
            Self::Before
        } else if fraction > 0.75 {
            Self::After
        } else {
            Self::Subtask
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::After => "zib-task-after",
            Self::Before => "zib-task-before",
            Self::Subtask => "zib-task-subtask",
        }
    }
}

fn destination(tree: &Tree, note: Id, target: Id, zone: Zone) -> Option<(Option<Id>, Option<Id>)> {
    if note == target || tree.ancestors(target).contains(&note) {
        return None;
    }
    let moving = tree.items.get(&note)?;
    let target_item = tree.items.get(&target)?;
    if moving.note.type_name != target_item.note.type_name {
        return None;
    }
    if moving.note.type_name == schema::PROJECT && moving.parent != target_item.parent {
        return None;
    }
    match zone {
        Zone::Before => Some((target_item.parent, Some(target))),
        Zone::Subtask => (moving.note.type_name == schema::TASK).then_some((Some(target), None)),
        Zone::After => {
            let parent = target_item.parent?;
            let siblings: Vec<_> = tree
                .children(parent)
                .into_iter()
                .filter(|id| tree.items[id].note.type_name == moving.note.type_name)
                .collect();
            let at = siblings.iter().position(|id| *id == target)?;
            let next = siblings[at + 1..].iter().find(|id| **id != note).copied();
            Some((Some(parent), next))
        }
    }
}

fn clear_indicator(widget: &gtk::Widget) {
    for class in ["zib-task-after", "zib-task-before", "zib-task-subtask"] {
        widget.remove_css_class(class);
    }
}

pub(super) fn handle() -> gtk::Image {
    let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    handle.add_css_class("dim-label");
    handle.set_valign(gtk::Align::Center);
    handle
}

impl Window {
    pub(super) fn work_drag_handle(&self, row: &gtk::Box, space: Id, item: &Item) {
        let handle = handle();
        handle.set_cursor_from_name(Some("grab"));
        let tooltip = if item.note.type_name == schema::PROJECT {
            "Drag to reorder subprojects"
        } else {
            "Drag to reorder · Drop at a row's edge to reorder, or its middle to make a subtask"
        };
        handle.set_tooltip_text(Some(tooltip));
        let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        let payload = WorkDrag { note: item.note.id, space };
        source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&payload.to_value()))
        });
        let (w, title) = (self.clone(), item.title.clone());
        source.connect_drag_begin(move |_, drag| {
            w.inner().work.dragging.set(true);
            let label = gtk::Label::new(Some(&title));
            label.add_css_class("card");
            gtk::DragIcon::for_drag(drag).set_child(Some(&label));
        });
        let w = self.clone();
        source.connect_drag_end(move |_, _, _| {
            w.inner().work.dragging.set(false);
            if w.inner().work.refresh_pending.replace(false) {
                w.refresh_work();
            }
        });
        handle.add_controller(source);
        row.append(&handle);
    }

    pub(super) fn work_drop_target(&self, row: &gtk::Box, space: Id, item: &Item) {
        let note = item.note.id;
        let project = item.note.type_name == schema::PROJECT;
        let target = gtk::DropTarget::new(WorkDrag::static_type(), gdk::DragAction::MOVE);
        target.set_preload(true);
        let w = self.clone();
        target
            .connect_enter(move |target, _, y| w.work_drop_motion(target, space, note, y, project));
        let w = self.clone();
        target.connect_motion(move |target, _, y| {
            w.work_drop_motion(target, space, note, y, project)
        });
        target.connect_leave(|target| {
            if let Some(widget) = target.widget() {
                clear_indicator(&widget);
            }
        });
        let w = self.clone();
        target.connect_drop(move |target, value, _, y| {
            let Some(widget) = target.widget() else { return false };
            clear_indicator(&widget);
            let Ok(payload) = value.get::<WorkDrag>() else { return false };
            if payload.space != space {
                return false;
            }
            let zone = Zone::at(y, widget.height(), project);
            let location = destination(&w.inner().work.tree.borrow(), payload.note, note, zone);
            let Some((parent, before)) = location else { return false };
            w.move_work(space, payload.note, parent, before, project);
            true
        });
        row.add_controller(target);
    }

    fn work_drop_motion(
        &self,
        target: &gtk::DropTarget,
        space: Id,
        note: Id,
        y: f64,
        project: bool,
    ) -> gdk::DragAction {
        let Some(widget) = target.widget() else { return gdk::DragAction::empty() };
        clear_indicator(&widget);
        let Some(payload) = target.value().and_then(|v| v.get::<WorkDrag>().ok()) else {
            return gdk::DragAction::empty();
        };
        let zone = Zone::at(y, widget.height(), project);
        if payload.space == space
            && destination(&self.inner().work.tree.borrow(), payload.note, note, zone).is_some()
        {
            widget.add_css_class(zone.class());
            gdk::DragAction::MOVE
        } else {
            gdk::DragAction::empty()
        }
    }

    fn move_work(
        &self,
        space: Id,
        note: Id,
        parent: Option<Id>,
        before: Option<Id>,
        project: bool,
    ) {
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let saved = core(async move {
                if project {
                    state.reorder_project(space, note, before).await
                } else {
                    state.move_task(space, note, parent, before).await
                }
            })
            .await;
            match saved {
                Ok(()) => {
                    if !project {
                        if let Some(parent) = parent {
                            w.inner().work.expanded.borrow_mut().insert(parent);
                        }
                        w.inner().work.selected.set(Some(note));
                    }
                    w.refresh_work();
                    w.reload_notes();
                }
                Err(error) => w.toast(&format!("Couldn't move task or project: {error}")),
            }
        });
    }
}
