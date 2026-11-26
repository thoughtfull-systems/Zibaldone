//! Inline task titles keep drafts across row refreshes and edit only the first body line.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::schema;
use zib_core::work::{Item, raw_title};
use zib_core::{Error, Id};

use crate::app::core;

use super::{Window, display_title};

pub(super) struct TitleEdit {
    busy: Cell<bool>,
    changing: Cell<bool>,
    conflicted: Cell<bool>,
    entry: gtk::Entry,
    expected: RefCell<String>,
    finish: Cell<bool>,
    label: gtk::Label,
    saved: RefCell<String>,
    view: gtk::Stack,
    timer: RefCell<Option<glib::SourceId>>,
}

impl TitleEdit {
    fn new(window: &Window, space: Id, item: &Item) -> Rc<Self> {
        let title = entry_title(item);
        let entry = gtk::Entry::builder()
            .text(title)
            .placeholder_text("Title")
            .hexpand(true)
            .width_chars(8)
            .max_width_chars(40)
            .valign(gtk::Align::Center)
            .build();
        entry.add_css_class("zib-work-title-entry");
        entry.set_enable_undo(true);
        let name =
            if item.note.type_name == schema::PROJECT { "Project title" } else { "Task title" };
        entry.update_property(&[gtk::accessible::Property::Label(name)]);
        entry.set_tooltip_text(Some("Title · Enter or leave the field to save"));
        let label = gtk::Label::builder()
            .label(display_title(&item.title))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        label.add_css_class("zib-work-title-text");
        let button = gtk::Button::builder().child(&label).hexpand(true).build();
        button.add_css_class("flat");
        button.add_css_class("zib-work-title");
        let view =
            gtk::Stack::builder().hexpand(true).hhomogeneous(false).vhomogeneous(false).build();
        view.add_named(&button, Some("label"));
        view.add_named(&entry, Some("entry"));
        view.set_visible_child_name("label");
        let edit = Rc::new(TitleEdit {
            busy: Cell::new(false),
            changing: Cell::new(false),
            conflicted: Cell::new(false),
            entry: entry.clone(),
            expected: RefCell::new(raw_title(&item.note.body).into()),
            finish: Cell::new(false),
            label,
            view,
            saved: RefCell::new(title.into()),
            timer: RefCell::default(),
        });
        window.connect_work_title(&button, space, item);
        edit
    }

    fn start(&self) {
        self.finish.set(false);
        self.view.set_visible_child_name("entry");
        self.entry.grab_focus();
    }

    fn dirty(&self) -> bool {
        self.entry.text().trim() != *self.saved.borrow()
    }

    fn load(&self, item: &Item) {
        self.changing.set(true);
        raw_title(&item.note.body).clone_into(&mut self.expected.borrow_mut());
        let title = entry_title(item);
        title.clone_into(&mut self.saved.borrow_mut());
        if self.entry.text().as_str() != title {
            self.entry.set_text(title);
        }
        self.label.set_text(display_title(&item.title));
        self.entry.remove_css_class("error");
        self.entry.set_tooltip_text(Some("Title · Enter or leave the field to save"));
        self.conflicted.set(false);
        self.changing.set(false);
    }
    fn finish_if_saved(&self) {
        if self.finish.get() && !self.dirty() && !self.busy.get() && !self.conflicted.get() {
            self.view.set_visible_child_name("label");
            self.finish.set(false);
        }
    }
}

fn entry_title(item: &Item) -> &str {
    if raw_title(&item.note.body).is_empty() { "" } else { &item.title }
}

impl Window {
    fn connect_work_title(&self, button: &gtk::Button, space: Id, item: &Item) {
        let note = item.note.id;
        let project = item.note.type_name == schema::PROJECT;
        button.set_tooltip_text(Some("Open · Double-click to rename · Drag to reorder or nest"));
        let clicks = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .name("zib-work-title-click")
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let count = Rc::new(Cell::new(None));
        let (w, presses) = (self.clone(), count.clone());
        clicks.connect_pressed(move |_, number, _, _| {
            w.cancel_work_title_open();
            presses.set(Some(number));
        });
        let clear_count = |count: &Rc<Cell<Option<i32>>>| {
            let count = count.clone();
            let released = count.get();
            glib::idle_add_local_once(move || {
                if count.get() == released {
                    count.set(None);
                }
            });
        };
        let releases = count.clone();
        clicks.connect_released(move |_, _, _, _| clear_count(&releases));
        let cancelled = count.clone();
        clicks.connect_cancel(move |_, _| clear_count(&cancelled));
        button.add_controller(clicks);
        self.work_drag_source(button, space, note);

        let w = self.clone();
        button.connect_clicked(move |button| {
            let count = count.take();
            if w.inner().work.dragging.get() {
                return;
            }
            match count {
                Some(1) => w.delay_work_title_open(button, space, note, project),
                Some(2) => w.edit_work_title(space, note),
                Some(_) => (),
                // Keyboard and accessibility activation open the item immediately.
                None => w.open_work_title(space, note, project),
            }
        });
    }

    fn delay_work_title_open(&self, button: &gtk::Button, space: Id, note: Id, project: bool) {
        self.cancel_work_title_open();
        let interval = u64::try_from(button.settings().gtk_double_click_time()).unwrap_or(0);
        let button = button.downgrade();
        let w = self.clone();
        let open = glib::timeout_add_local_once(Duration::from_millis(interval), move || {
            w.inner().work.title_open.take();
            if button.upgrade().is_some_and(|button| button.is_mapped()) {
                w.open_work_title(space, note, project);
            }
        });
        self.inner().work.title_open.replace(Some(open));
    }

    fn open_work_title(&self, space: Id, note: Id, project: bool) {
        if project {
            self.open_note(space, note);
        } else {
            self.open_task(space, note);
        }
    }

    pub(in crate::window) fn cancel_work_title_open(&self) {
        if let Some(open) = self.inner().work.title_open.take() {
            open.remove();
        }
    }

    pub(super) fn work_title_view(&self, space: Id, item: &Item) -> gtk::Stack {
        self.task_title_entry(space, item);
        self.inner().work.titles.borrow()[&(space, item.note.id)].view.clone()
    }

    pub(super) fn edit_work_title(&self, space: Id, note: Id) {
        let edit = self.inner().work.titles.borrow().get(&(space, note)).cloned();
        if let Some(edit) = edit.filter(|edit| edit.view.is_mapped()) {
            edit.start();
        }
    }

    pub(super) fn task_title_entry(&self, space: Id, item: &Item) -> gtk::Entry {
        let id = item.note.id;
        if let Some(edit) = self.inner().work.titles.borrow().get(&(space, id)) {
            if !edit.dirty() && !edit.busy.get() {
                edit.load(item);
            }
            return edit.entry.clone();
        }
        let edit = TitleEdit::new(self, space, item);
        let entry = edit.entry.clone();
        self.inner().work.titles.borrow_mut().insert((space, id), edit.clone());
        let w = self.clone();
        let editing = Rc::downgrade(&edit);
        entry.connect_changed(move |_| {
            let Some(edit) = editing.upgrade().filter(|e| !e.changing.get()) else { return };
            edit.finish.set(false);
            if let Some(timer) = edit.timer.take() {
                timer.remove();
            }
            let next = w.clone();
            *edit.timer.borrow_mut() =
                Some(glib::timeout_add_local_once(Duration::from_millis(750), move || {
                    if let Some(edit) = next.inner().work.titles.borrow().get(&(space, id)) {
                        edit.timer.take();
                    }
                    next.save_task_title(space, id);
                }));
        });
        let (w, finishing) = (self.clone(), Rc::downgrade(&edit));
        entry.connect_activate(move |_| {
            if let Some(edit) = finishing.upgrade() {
                edit.finish.set(true);
            }
            w.save_task_title(space, id);
        });
        let focus = gtk::EventControllerFocus::new();
        let (w, opening) = (self.clone(), Rc::downgrade(&edit));
        let task = item.note.type_name == schema::TASK;
        focus.connect_enter(move |_| {
            if let Some(edit) = opening.upgrade() {
                edit.finish.set(false);
            }
            if task {
                w.inner().work.selected.set(Some(id));
            }
        });
        let (w, finishing) = (self.clone(), Rc::downgrade(&edit));
        focus.connect_leave(move |_| {
            if let Some(edit) = finishing.upgrade() {
                edit.finish.set(true);
            }
            w.save_task_title(space, id);
        });
        entry.add_controller(focus);
        let keys = gtk::EventControllerKey::new();
        let w = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            if let Some(edit) = w.inner().work.titles.borrow().get(&(space, id))
                && let Some(item) = w.inner().work.tree.borrow().items.get(&id)
            {
                edit.load(item);
                edit.finish.set(true);
                edit.finish_if_saved();
            }
            glib::Propagation::Stop
        });
        entry.add_controller(keys);
        entry
    }

    fn save_task_title(&self, space: Id, note: Id) {
        let Some(edit) = self.inner().work.titles.borrow().get(&(space, note)).cloned() else {
            return;
        };
        if let Some(timer) = edit.timer.take() {
            timer.remove();
        }
        if edit.busy.get() || edit.conflicted.get() || !edit.dirty() {
            edit.finish_if_saved();
            return;
        }
        let title = edit.entry.text().trim().to_owned();
        if title.is_empty() {
            edit.entry.add_css_class("error");
            edit.entry.set_tooltip_text(Some("Enter a task title, or press Escape to restore it"));
            return;
        }
        edit.busy.set(true);
        let (w, state) = (self.clone(), self.inner().state.clone());
        let (expected, writing) = (edit.expected.borrow().clone(), title.clone());
        // Start the core write now, including when flushing edits before the window closes.
        let saving = core(async move { state.rename_task(space, note, &expected, &writing).await });
        glib::spawn_future_local(async move {
            let result = saving.await;
            edit.busy.set(false);
            if !w.inner().work.titles.borrow().contains_key(&(space, note)) {
                return;
            }
            match result {
                Ok(line) => {
                    *edit.expected.borrow_mut() = line;
                    edit.label.set_text(&title);
                    *edit.saved.borrow_mut() = title;
                    edit.entry.remove_css_class("error");
                    edit.finish_if_saved();
                    if edit.dirty() {
                        w.save_task_title(space, note);
                    }
                }
                Err(error) => {
                    edit.finish.set(false);
                    edit.view.set_visible_child_name("entry");
                    edit.conflicted.set(true);
                    edit.entry.add_css_class("error");
                    let message = if matches!(error, Error::NoteChanged(_)) {
                        "Title changed elsewhere. Press Escape to reload it, then edit again."
                            .into()
                    } else {
                        format!("Couldn't save task title: {error}")
                    };
                    edit.entry.set_tooltip_text(Some(&message));
                    w.toast(&message);
                }
            }
            w.reload_notes();
            w.refresh_work();
        });
    }

    pub(in crate::window) fn save_task_titles(&self) {
        let keys: Vec<_> = self.inner().work.titles.borrow().keys().copied().collect();
        for (space, note) in keys {
            self.save_task_title(space, note);
        }
    }

    pub(crate) async fn finish_task_titles(&self) -> bool {
        loop {
            self.save_task_titles();
            let busy = self.inner().work.titles.borrow().values().any(|e| e.busy.get());
            if !busy {
                return self.inner().work.titles.borrow().values().all(|e| !e.dirty());
            }
            glib::timeout_future(Duration::from_millis(10)).await;
        }
    }

    pub(super) fn discard_task_title(&self, space: Id, note: Id) {
        if let Some(edit) = self.inner().work.titles.borrow_mut().remove(&(space, note))
            && let Some(timer) = edit.timer.take()
        {
            timer.remove();
        }
    }

    pub(super) fn focused_task_title(&self) -> Option<gtk::Entry> {
        let focus = gtk::prelude::RootExt::focus(&self.inner().window)?;
        self.inner()
            .work
            .titles
            .borrow()
            .values()
            .find(|edit| focus == edit.entry || focus.is_ancestor(&edit.entry))
            .map(|edit| edit.entry.clone())
    }

    pub(super) fn reveal_task_title(&self) {
        let work = &self.inner().work;
        let space = self.inner().editor.borrow().as_ref().map(|e| e.space);
        let Some(key) = space.zip(work.selected.get()) else { return };
        let Some(edit) = work.titles.borrow().get(&key).cloned() else { return };
        let w = self.clone();
        edit.view.add_tick_callback(move |view, _| {
            let pane = &w.inner().project_pane;
            if w.inner().work.selected.get() == Some(key.1)
                && let Some(bounds) = view.compute_bounds(&pane.content)
            {
                let adjustment = pane.scroll.vadjustment();
                let top = (f64::from(bounds.y()) - 30.0).max(0.0);
                if top < adjustment.value() || top > adjustment.value() + adjustment.page_size() {
                    adjustment
                        .set_value(top.min((adjustment.upper() - adjustment.page_size()).max(0.0)));
                }
            }
            glib::ControlFlow::Break
        });
    }
}
