//! Project context, compact task rows, and their inline title and date controls.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use zib_core::Id;
use zib_core::schema::{self, Value};
use zib_core::work::{Item, Tree};

use crate::app::core;

use super::Window;

mod controls;
mod drag;
mod flow;
mod quick_add;
mod status;
mod titles;

type AdditionKey = (Id, Id, &'static str);

pub(super) struct WorkPanel {
    additions: RefCell<HashMap<AdditionKey, Rc<quick_add::AddRow>>>,
    completed: Cell<bool>,
    dragging: Cell<bool>,
    expanded: RefCell<HashSet<Id>>,
    generation: Cell<u64>,
    pub header: gtk::Box,
    pub(super) popovers: Cell<usize>,
    project_prefix: gtk::SizeGroup,
    refresh_pending: Cell<bool>,
    pub(super) results_pending: Cell<bool>,
    reveal: Cell<bool>,
    pub selected: Cell<Option<Id>>,
    task_prefix: gtk::SizeGroup,
    titles: RefCell<HashMap<(Id, Id), Rc<titles::TitleEdit>>>,
    pub tree: RefCell<Tree>,
    pub widget: gtk::Box,
}

impl WorkPanel {
    pub fn new() -> Self {
        let header = panel();
        header.set_margin_top(2);
        header.set_margin_bottom(2);
        let widget = panel();
        widget.set_spacing(4);
        widget.set_margin_top(4);
        widget.set_margin_bottom(4);
        header.set_visible(false);
        widget.set_visible(false);
        Self {
            additions: RefCell::default(),
            completed: Cell::new(false),
            dragging: Cell::new(false),
            expanded: RefCell::default(),
            generation: Cell::new(0),
            header,
            popovers: Cell::new(0),
            project_prefix: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            refresh_pending: Cell::new(false),
            results_pending: Cell::new(false),
            reveal: Cell::new(false),
            selected: Cell::new(None),
            task_prefix: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            titles: RefCell::default(),
            tree: RefCell::default(),
            widget,
        }
    }
}

pub(super) fn style_work_row(row: &impl IsA<gtk::Widget>, item: &Item, tree: &Tree) {
    row.add_css_class("zib-work-emphasis");
    if item.completed() {
        row.add_css_class("dim-label");
    } else {
        let due = tree.date(item.note.id, "due-on");
        let (_, class) = zib_core::work::urgency(due.as_ref().map(|date| date.value.as_str()));
        match class {
            "error" => row.add_css_class("zib-work-overdue"),
            "warning" => row.add_css_class("zib-work-warning"),
            _ => {}
        }
    }
}

fn panel() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_start(20)
        .margin_end(20)
        .margin_top(10)
        .margin_bottom(10)
        .build()
}

fn task_expander(expanded: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(if expanded { "pan-down-symbolic" } else { "pan-end-symbolic" })
        .build();
    button.add_css_class("flat");
    button.add_css_class("zib-task-expander");
    button.set_tooltip_text(Some("Expand or collapse subtasks"));
    button
}

fn clear(panel: &gtk::Box) {
    while let Some(child) = panel.first_child() {
        panel.remove(&child);
    }
}

impl Window {
    pub(super) fn work_layout(&self, type_name: &str) {
        let inner = self.inner();
        let inline = type_name == schema::TASK;
        let panels = if inline { &inner.editor_content } else { &inner.project_pane.content };
        for panel in [&inner.work.header, &inner.work.widget] {
            if panel.parent().is_none_or(|parent| parent != *panels) {
                if let Some(parent) = panel.parent().and_downcast::<gtk::Box>() {
                    parent.remove(panel);
                }
                panels.append(panel);
            }
        }
        let nested = inner.widget.parent().is_some_and(|p| p == inner.editor_content);
        if inline != nested {
            if nested {
                inner.editor_content.remove(&inner.widget);
            } else if let Some(viewport) = inner.widget.parent().and_downcast::<gtk::Viewport>() {
                viewport.set_child(None::<&gtk::Widget>);
            }
            inner.editor_scroll.set_child(None::<&gtk::Widget>);
            if inline {
                inner.editor_content.insert_child_after(&inner.widget, Some(&inner.work.header));
                inner.editor_scroll.set_child(Some(&inner.editor_content));
            } else {
                inner.editor_scroll.set_child(Some(&inner.widget));
            }
        }
        if let Some(viewport) = inner.editor_scroll.child().and_downcast::<gtk::Viewport>() {
            viewport.set_scroll_to_focus(false);
        }
        inner.project_pane.show(type_name == schema::PROJECT);
    }

    pub(super) fn connect_work(&self) {
        let weak = std::rc::Rc::downgrade(&self.0);
        let mut date = zib_core::work::today();
        // Queries using today and urgency labels must refresh even when no note changes.
        glib::timeout_add_local(Duration::from_secs(30), move || {
            let Some(inner) = weak.upgrade() else { return glib::ControlFlow::Break };
            let today = zib_core::work::today();
            if date != today {
                date = today;
                let window = Window(inner);
                window.reload_notes();
                window.refresh_work();
            }
            glib::ControlFlow::Continue
        });
    }

    /// A task opens its nearest project, or the root task for a standalone tree.
    pub fn open_note(&self, space: Id, note: Id) {
        let opening = self.begin_open();
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let tree = core(async move { state.work_tree(space).await }).await;
            if w.inner().opening.get() != opening {
                return;
            }
            let mut context = note;
            if let Ok(tree) = tree {
                context = tree.context(note);
                if tree.items.get(&note).is_some_and(|i| i.note.type_name == schema::TASK) {
                    w.inner().work.selected.set(Some(note));
                    w.inner().work.reveal.set(true);
                    w.inner().work.expanded.borrow_mut().extend(tree.ancestors(note));
                } else {
                    w.inner().work.selected.set(None);
                }
            }
            let already_open = w
                .inner()
                .editor
                .borrow()
                .as_ref()
                .is_some_and(|e| e.space == space && e.note == context);
            if already_open {
                w.refresh_work();
            } else {
                w.open_note_direct(space, context);
            }
        });
    }

    pub(super) fn refresh_work(&self) {
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let work = &self.inner().work;
        if work.popovers.get() > 0 {
            work.refresh_pending.set(true);
            return;
        }
        let generation = work.generation.get().wrapping_add(1);
        work.generation.set(generation);
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let loaded = core(async move {
                let tree = state.work_tree(space).await?;
                let completed = state.show_completed(space, note).await?;
                Ok::<_, zib_core::Error>((tree, completed))
            })
            .await;
            let Ok((tree, completed)) = loaded else { return };
            if w.inner().work.generation.get() != generation
                || !w
                    .inner()
                    .editor
                    .borrow()
                    .as_ref()
                    .is_some_and(|e| e.space == space && e.note == note)
            {
                return;
            }
            if w.inner().work.popovers.get() > 0 || w.inner().work.dragging.get() {
                w.inner().work.refresh_pending.set(true);
                return;
            }
            let selected = w.inner().work.selected.get();
            if selected.is_some_and(|id| tree.context(id) != note) {
                w.inner().work.selected.set(None);
            }
            w.inner().work.completed.set(completed);
            *w.inner().work.tree.borrow_mut() = tree;
            w.draw_work(space, note);
        });
    }

    fn draw_work(&self, space: Id, note: Id) {
        let work = &self.inner().work;
        let focused = self.focused_task_title().or_else(|| self.focused_work_addition());
        clear(&work.header);
        clear(&work.widget);
        let tree = work.tree.borrow();
        let Some(item) = tree.items.get(&note) else {
            work.header.set_visible(false);
            work.widget.set_visible(false);
            return;
        };
        work.header.set_visible(true);
        work.widget.set_visible(true);
        self.work_header(space, item, &tree);
        if item.note.type_name == schema::PROJECT {
            let heading = gtk::Label::builder().label("Subprojects").xalign(0.0).build();
            heading.add_css_class("heading");
            work.widget.append(&heading);
        }
        for child in tree
            .children(note)
            .into_iter()
            .filter(|id| tree.items[id].note.type_name == schema::PROJECT)
        {
            let item = &tree.items[&child];
            let prefix = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            self.work_drag_handle(&prefix, space, item);
            prefix.append(&gtk::Label::new(Some("▧")));
            work.project_prefix.add_widget(&prefix);
            work.widget.append(&self.work_row(space, item, 0, &tree, &prefix));
        }
        if item.note.type_name == schema::PROJECT {
            self.work_addition(space, note, schema::PROJECT);
        }
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let label = gtk::Label::builder().label("Tasks").xalign(0.0).hexpand(true).build();
        label.add_css_class("heading");
        heading.append(&label);
        let completed = gtk::CheckButton::with_label("Show completed");
        completed.set_active(work.completed.get());
        let w = self.clone();
        completed.connect_toggled(move |button| {
            button.set_sensitive(false);
            let completed = button.is_active();
            let (w, state) = (w.clone(), w.inner().state.clone());
            glib::spawn_future_local(async move {
                let saved =
                    core(async move { state.set_show_completed(space, note, completed).await })
                        .await;
                if let Err(error) = saved {
                    w.toast(&format!("Couldn't save Show completed: {error}"));
                }
                w.refresh_work();
            });
        });
        heading.append(&completed);
        work.widget.append(&heading);
        let mut pending: Vec<_> = tree.children(note).into_iter().rev().map(|id| (id, 0)).collect();
        while let Some((id, depth)) = pending.pop() {
            let item = &tree.items[&id];
            if item.note.type_name != schema::TASK {
                continue;
            }
            if item.completed()
                && !work.completed.get()
                && work.selected.get() != Some(id)
                && !tree.unfinished_below(id)
            {
                continue;
            }
            self.task_row(space, item, depth, &tree);
            if work.expanded.borrow().contains(&id) {
                pending.extend(tree.children(id).into_iter().rev().map(|id| (id, depth + 1)));
            }
        }
        self.work_addition(space, note, schema::TASK);
        drop(tree);
        if let Some(entry) = focused {
            entry.grab_focus_without_selecting();
        }
        if work.reveal.replace(false) {
            self.reveal_task_title();
        }
    }

    fn work_header(&self, space: Id, item: &Item, tree: &Tree) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(parent) = item.parent {
            let title = format!("← {}", tree.items[&parent].title);
            let label = gtk::Label::builder()
                .label(&title)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .xalign(0.0)
                .build();
            let button = gtk::Button::builder()
                .child(&label)
                .hexpand(true)
                .halign(gtk::Align::Start)
                .valign(gtk::Align::Center)
                .build();
            button.set_tooltip_text(Some(&title));
            button.add_css_class("flat");
            let w = self.clone();
            button.connect_clicked(move |_| w.open_note(space, parent));
            row.append(&button);
        }
        self.work_controls(&row, space, item, tree);
        if item.note.type_name == schema::TASK {
            row.append(&self.completion(space, item));
        }
        self.inner().work.header.append(&row);
    }

    fn task_row(&self, space: Id, item: &Item, depth: i32, tree: &Tree) {
        let work = &self.inner().work;
        let id = item.note.id;
        let handles = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        self.work_drag_handle(&handles, space, item);
        let children = tree.children(id);
        let expanded = work.expanded.borrow().contains(&id);
        let toggle = task_expander(expanded);
        toggle.set_sensitive(!children.is_empty());
        let w = self.clone();
        toggle.connect_clicked(move |_| {
            let mut expanded = w.inner().work.expanded.borrow_mut();
            if !expanded.remove(&id) {
                expanded.insert(id);
            }
            drop(expanded);
            w.refresh_work();
        });
        handles.append(&toggle);
        let completion = self.completion(space, item);
        completion.add_css_class("zib-task-completion");
        handles.append(&completion);
        work.task_prefix.add_widget(&handles);
        work.widget.append(&self.work_row(space, item, depth, tree, &handles));
    }

    fn work_row(
        &self,
        space: Id,
        item: &Item,
        depth: i32,
        tree: &Tree,
        prefix: &gtk::Box,
    ) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.add_css_class("zib-work-row");
        style_work_row(&row, item, tree);
        row.set_margin_start(depth.min(8) * 14);
        let grid = gtk::Grid::builder().column_spacing(4).row_spacing(0).build();
        prefix.set_valign(gtk::Align::Center);
        grid.attach(prefix, 0, 0, 1, 1);
        let title = self.work_title_view(space, item);
        if let Some(parent) = title.parent().and_downcast::<gtk::Grid>() {
            parent.remove(&title);
        }
        grid.attach(&title, 1, 0, 1, 1);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        actions.set_halign(gtk::Align::End);
        if item.note.type_name == schema::PROJECT {
            let open = gtk::Button::from_icon_name("go-next-symbolic");
            open.add_css_class("flat");
            open.set_tooltip_text(Some("Open subproject"));
            let (w, id) = (self.clone(), item.note.id);
            open.connect_clicked(move |_| w.open_note(space, id));
            actions.append(&open);
        }
        actions.append(&self.task_actions(space, item, tree));
        grid.attach(&actions, 2, 0, 1, 1);
        grid.attach(&self.work_details(space, item, tree), 1, 1, 2, 1);
        row.append(&grid);
        self.work_drop_target(&row, space, item);
        row
    }

    fn completion(&self, space: Id, item: &Item) -> gtk::CheckButton {
        let check = gtk::CheckButton::new();
        check.update_property(&[gtk::accessible::Property::Label(&format!(
            "Complete {}",
            item.title
        ))]);
        check.set_active(item.completed());
        check.set_tooltip_text(Some(&format!("Complete {}", item.title)));
        let (w, id) = (self.clone(), item.note.id);
        check.connect_toggled(move |check| {
            let date = check.is_active().then(|| Value::Date(zib_core::work::today()));
            w.save_work_field(space, id, "completed-on", date);
        });
        check
    }

    pub(super) fn forget_deleted_notes(&self, space: Id, deleted: &[Id]) {
        for id in deleted {
            self.discard_task_title(space, *id);
            self.inner().work.expanded.borrow_mut().remove(id);
        }
        self.inner()
            .work
            .additions
            .borrow_mut()
            .retain(|(s, parent, _), _| *s != space || !deleted.contains(parent));
        if self.inner().work.selected.get().is_some_and(|id| deleted.contains(&id)) {
            self.inner().work.selected.set(None);
        }
        for history in [&self.inner().back, &self.inner().forward] {
            history.borrow_mut().retain(|(s, id)| *s != space || !deleted.contains(id));
        }
        self.update_history();
        let open = self
            .inner()
            .editor
            .borrow()
            .as_ref()
            .is_some_and(|editor| editor.space == space && deleted.contains(&editor.note));
        if open {
            self.close_editor();
            self.inner().stack.set_visible_child_name("empty");
        }
        self.refresh_work();
        self.reload_notes();
    }

    fn save_work_field(&self, space: Id, note: Id, field: &'static str, value: Option<Value>) {
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            match core(async move { state.set_work_field(space, note, field, value).await }).await {
                Ok(()) => {
                    if field == "parent-id" {
                        w.open_note(space, note);
                    } else {
                        w.refresh_work();
                    }
                    w.reload_notes();
                    w.reload_saved_searches();
                }
                Err(error) => w.toast(&format!("Couldn't update task or project: {error}")),
            }
        });
    }

    pub(super) fn new_work(&self, kind: &'static str, parent: Option<Id>) {
        let space = if parent.is_some() {
            self.inner().editor.borrow().as_ref().map(|e| e.space)
        } else {
            self.current_space_id()
        };
        let Some(space) = space else {
            self.toast("Make a space first");
            return;
        };
        let w = self.clone();
        glib::spawn_future_local(async move {
            let entry =
                gtk::Entry::builder().placeholder_text("Title").activates_default(true).build();
            let title = if kind == schema::PROJECT { "New project" } else { "New task" };
            let dialog = adw::AlertDialog::new(Some(title), None);
            dialog.set_extra_child(Some(&entry));
            dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
            dialog.set_default_response(Some("create"));
            dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
            dialog.set_response_enabled("create", false);
            let creating = dialog.downgrade();
            entry.connect_changed(move |entry| {
                if let Some(dialog) = creating.upgrade() {
                    dialog.set_response_enabled("create", !entry.text().trim().is_empty());
                }
            });
            if dialog.choose_future(Some(&w.inner().window)).await != "create" {
                return;
            }
            let title = entry.text().trim().to_owned();
            if title.is_empty() {
                w.toast("Enter a title");
                return;
            }
            let state = w.inner().state.clone();
            match core(async move {
                state.create_work(space, kind, &format!("# {title}\n\n"), parent).await
            })
            .await
            {
                Ok(note) => {
                    w.reload_notes();
                    w.reload_saved_searches();
                    w.open_note(space, note);
                }
                Err(error) => w.toast(&format!("Couldn't create {kind}: {error}")),
            }
        });
    }
}
