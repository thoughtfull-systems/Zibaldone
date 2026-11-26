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
mod recurrence;
mod status;
mod titles;

use quick_add::Section;

type AdditionKey = (Id, Id, &'static str);

pub(super) struct WorkPanel {
    additions: RefCell<HashMap<AdditionKey, Rc<quick_add::AddRow>>>,
    completed: Cell<bool>,
    dragging: Cell<bool>,
    expanded: RefCell<HashSet<Id>>,
    generation: Cell<u64>,
    pub header: gtk::Box,
    pub(super) popovers: Cell<usize>,
    project_waiting: RefCell<Option<Rc<status::ProjectWaiting>>>,
    refresh_pending: Cell<bool>,
    pub(super) results_pending: Cell<bool>,
    reveal: Cell<bool>,
    row_expanders: gtk::SizeGroup,
    row_indicators: gtk::SizeGroup,
    pub selected: Cell<Option<Id>>,
    title_open: RefCell<Option<glib::SourceId>>,
    titles: RefCell<HashMap<(Id, Id), Rc<titles::TitleEdit>>>,
    pub tree: RefCell<Tree>,
    pub widget: gtk::Box,
}

impl WorkPanel {
    pub fn new() -> Self {
        let header = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
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
            project_waiting: RefCell::default(),
            refresh_pending: Cell::new(false),
            results_pending: Cell::new(false),
            reveal: Cell::new(false),
            row_expanders: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            row_indicators: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            selected: Cell::new(None),
            title_open: RefCell::default(),
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
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build()
}

fn work_expander(expanded: bool, project: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(if expanded { "pan-down-symbolic" } else { "pan-end-symbolic" })
        .valign(gtk::Align::Center)
        .build();
    button.add_css_class("flat");
    button.add_css_class("zib-task-expander");
    button.set_tooltip_text(Some(if project {
        "Expand or collapse subprojects"
    } else {
        "Expand or collapse subtasks"
    }));
    button
}

pub(super) fn recurrence_indicator() -> gtk::Image {
    let icon = gtk::Image::from_icon_name("media-playlist-repeat-symbolic");
    icon.set_tooltip_text(Some("Recurring task"));
    icon.update_property(&[gtk::accessible::Property::Label("Recurring task")]);
    icon
}

fn display_title(title: &str) -> &str {
    if title.trim().is_empty() { "Untitled" } else { title }
}

fn clear(panel: &gtk::Box) {
    while let Some(child) = panel.first_child() {
        panel.remove(&child);
    }
}

impl Window {
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

    /// Opens an occurrence or reveals a task in its nearest work context.
    pub fn open_note(&self, space: Id, note: Id) {
        self.open_work_context(space, note, false);
    }

    pub(super) fn open_search_result(&self, space: Id, note: Id) {
        self.open_work_context(space, note, true);
    }

    fn open_work_context(&self, space: Id, note: Id, search_result: bool) {
        let opening = self.begin_open();
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let tree = core(async move { state.work_tree(space).await }).await;
            if w.inner().opening.get() != opening {
                return;
            }
            let mut context = note;
            if let Ok(tree) = tree {
                let open_series = search_result && tree.recurrence_rules.contains_key(&note);
                if !open_series {
                    context = tree.context(note);
                }
                if !open_series
                    && tree.items.get(&note).is_some_and(|item| {
                        item.note.type_name == schema::TASK && !item.note.is_task_occurrence()
                    })
                {
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

    fn open_task(&self, space: Id, task: Id) {
        self.inner().focus_body.set(true);
        self.open_note_direct(space, task);
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
        let focused = self
            .focused_task_title()
            .or_else(|| self.focused_work_addition())
            .or_else(|| self.focused_project_waiting());
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
        let (projects, tasks): (Vec<_>, Vec<_>) = tree
            .children(note)
            .into_iter()
            .partition(|id| tree.items[id].note.type_name == schema::PROJECT);
        work.header.append(&self.work_controls(space, item, &tree));
        if !projects.is_empty() || !tasks.is_empty() {
            let completed = self.show_completed_checkbox(space, note);
            completed.set_halign(gtk::Align::End);
            work.header.append(&completed);
        }
        let project = item.note.type_name == schema::PROJECT;
        if project {
            self.draw_work_section(space, note, Section::Subprojects, projects, &tree);
        }
        if tree.series.contains(&note) {
            self.draw_occurrence_history(space, note, &tree);
        }
        let section = if project { Section::Tasks } else { Section::Subtasks };
        self.draw_work_section(space, note, section, tasks, &tree);
        drop(tree);
        if let Some(entry) = focused.filter(|entry| {
            entry.is_ancestor(&work.widget)
                || (entry.get_visible() && entry.is_ancestor(&work.header))
        }) {
            if let Some(view) = entry.parent().and_downcast::<gtk::Stack>() {
                view.set_visible_child_name("entry");
            }
            entry.grab_focus_without_selecting();
        }
        if work.reveal.replace(false) {
            self.reveal_task_title();
        }
    }

    fn draw_work_section(
        &self,
        space: Id,
        note: Id,
        section: Section,
        items: Vec<Id>,
        tree: &Tree,
    ) {
        let empty = items.is_empty();
        if !empty || self.work_addition_revealed(space, note, section) {
            self.draw_work_list(space, section, items, tree);
        }
        self.work_addition(space, note, section, empty);
    }

    fn show_completed_checkbox(&self, space: Id, note: Id) -> gtk::CheckButton {
        let completed = gtk::CheckButton::with_label("Show completed");
        completed.set_active(self.inner().work.completed.get());
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
        completed
    }

    fn draw_work_list(&self, space: Id, section: Section, items: Vec<Id>, tree: &Tree) {
        let work = &self.inner().work;
        let heading = gtk::Label::builder().label(section.title()).xalign(0.0).build();
        heading.add_css_class("heading");
        work.widget.append(&heading);
        let show_completed = work.completed.get();
        let selected = work.selected.get();
        let visible = |id: &Id| {
            show_completed
                || selected == Some(*id)
                || !tree.items[id].completed()
                || tree.unfinished_below(*id)
        };
        let mut pending: Vec<_> =
            items.into_iter().rev().filter(&visible).map(|id| (id, 0)).collect();
        while let Some((id, depth)) = pending.pop() {
            let item = &tree.items[&id];
            let mut children = tree
                .children(id)
                .into_iter()
                .rev()
                .filter(|id| tree.items[id].note.type_name == section.kind())
                .filter(&visible)
                .peekable();
            let has_children = children.peek().is_some();
            let expanded = has_children && work.expanded.borrow().contains(&id);
            self.work_hierarchy_row(space, item, depth, tree, has_children, expanded);
            if expanded {
                pending.extend(children.map(|id| (id, depth + 1)));
            }
        }
    }

    fn work_hierarchy_row(
        &self,
        space: Id,
        item: &Item,
        depth: i32,
        tree: &Tree,
        has_children: bool,
        expanded: bool,
    ) {
        let id = item.note.id;
        let toggle = work_expander(expanded, item.note.type_name == schema::PROJECT);
        toggle.set_sensitive(has_children);
        let w = self.clone();
        toggle.connect_clicked(move |_| {
            let mut expanded = w.inner().work.expanded.borrow_mut();
            if !expanded.remove(&id) {
                expanded.insert(id);
            }
            drop(expanded);
            w.refresh_work();
        });
        let indicator = if tree.series.contains(&id) {
            recurrence_indicator().upcast()
        } else {
            self.completion(space, item).upcast()
        };
        let row = self.work_row(space, item, depth, tree, &toggle, &indicator);
        self.inner().work.widget.append(&row);
    }

    fn work_row(
        &self,
        space: Id,
        item: &Item,
        depth: i32,
        tree: &Tree,
        expander: &gtk::Button,
        indicator: &gtk::Widget,
    ) -> gtk::Box {
        let work = &self.inner().work;
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.add_css_class("zib-work-row");
        style_work_row(&row, item, tree);
        row.set_margin_start(depth.min(8) * 14);
        let grid = gtk::Grid::new();
        work.row_expanders.add_widget(expander);
        grid.attach(expander, 0, 0, 1, 1);
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        heading.add_css_class("zib-work-row-heading");
        indicator.add_css_class("zib-work-row-indicator");
        indicator.set_valign(gtk::Align::Center);
        work.row_indicators.add_widget(indicator);
        heading.append(indicator);
        let title = self.work_title_view(space, item);
        if let Some(parent) = title.parent().and_downcast::<gtk::Box>() {
            parent.remove(&title);
        }
        heading.append(&title);
        grid.attach(&heading, 1, 0, 1, 1);
        let details = self.work_details(space, item, tree);
        if details.is_visible() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            line.add_css_class("zib-work-column-content");
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            work.row_indicators.add_widget(&spacer);
            line.append(&spacer);
            line.append(&details);
            grid.attach(&line, 1, 1, 1, 1);
        }
        row.append(&grid);
        self.connect_work_context_menu(&row, space, item, tree);
        self.work_drop_target(&row, space, item);
        row
    }

    fn completion(&self, space: Id, item: &Item) -> gtk::CheckButton {
        let check = gtk::CheckButton::new();
        let label = format!("Complete {}", display_title(&item.title));
        check.update_property(&[gtk::accessible::Property::Label(&label)]);
        check.set_active(item.completed());
        check.set_tooltip_text(Some(&label));
        let (w, id) = (self.clone(), item.note.id);
        let occurrence = item.note.is_task_occurrence();
        check.connect_toggled(move |check| {
            if occurrence {
                use zib_core::recurrence::Outcome;
                let outcome = if check.is_active() { Outcome::Completed } else { Outcome::Open };
                w.save_occurrence(space, id, outcome, String::new());
                return;
            }
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

    pub(super) fn new_work(&self, kind: &'static str) {
        let Some(space) = self.current_space_id() else {
            self.toast("Make a space first");
            return;
        };
        self.use_space(space);
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            match core(async move { state.create_work(space, kind, "", None).await }).await {
                Ok(note) => {
                    w.reload_notes();
                    w.reload_saved_searches();
                    w.open_task(space, note);
                }
                Err(error) => w.toast(&format!("Couldn't create {kind}: {error}")),
            }
        });
    }
}
