//! Inline additions retain their input and focus across background hierarchy refreshes.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use zib_core::Id;
use zib_core::schema;

use crate::app::core;

use super::{Window, drag, task_expander};

#[derive(Clone, Copy)]
pub(super) enum Section {
    Subprojects,
    Subtasks,
    Tasks,
}

impl Section {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Subprojects => "Subprojects",
            Self::Subtasks => "Subtasks",
            Self::Tasks => "Tasks",
        }
    }

    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Subprojects => ("Add a subproject", "New subproject"),
            Self::Subtasks => ("Add a subtask", "New subtask"),
            Self::Tasks => ("Add a task", "New task"),
        }
    }

    fn kind(self) -> &'static str {
        if matches!(self, Self::Subprojects) { schema::PROJECT } else { schema::TASK }
    }
}

pub(super) struct AddRow {
    busy: Cell<bool>,
    button: gtk::Button,
    entry: gtk::Entry,
    revealed: Cell<bool>,
    row: gtk::Grid,
}

impl Window {
    pub(super) fn work_addition_revealed(&self, space: Id, parent: Id, section: Section) -> bool {
        self.inner()
            .work
            .additions
            .borrow()
            .get(&(space, parent, section.kind()))
            .is_some_and(|add| add.revealed.get())
    }

    pub(super) fn work_addition(&self, space: Id, parent: Id, section: Section, empty: bool) {
        let kind = section.kind();
        let key = (space, parent, kind);
        let existing = self.inner().work.additions.borrow().get(&key).cloned();
        let add = existing.unwrap_or_else(|| {
            let (action, name) = section.labels();
            let button = gtk::Button::builder().label(action).halign(gtk::Align::Start).build();
            let entry = gtk::Entry::builder()
                .hexpand(true)
                .placeholder_text(name)
                .truncate_multiline(true)
                .build();
            entry.update_property(&[gtk::accessible::Property::Label(name)]);
            let row = gtk::Grid::builder().column_spacing(4).build();
            let project = kind == schema::PROJECT;
            let prefix = gtk::Box::new(gtk::Orientation::Horizontal, if project { 4 } else { 2 });
            prefix.append(&drag::handle());
            if !project {
                prefix.append(&task_expander(false));
                let completed = gtk::CheckButton::new();
                completed.add_css_class("zib-task-completion");
                prefix.append(&completed);
            }
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            let open = gtk::Button::from_icon_name("go-next-symbolic");
            let menu = gtk::MenuButton::builder().icon_name("view-more-symbolic").build();
            open.add_css_class("flat");
            menu.add_css_class("flat");
            actions.append(&open);
            actions.append(&menu);
            for spacer in [&prefix, &actions] {
                spacer.set_opacity(0.0);
                spacer.set_sensitive(false);
                spacer.set_can_target(false);
                spacer.update_state(&[gtk::accessible::State::Hidden(true)]);
            }
            let work = &self.inner().work;
            let alignment = if project { &work.project_prefix } else { &work.task_prefix };
            alignment.add_widget(&prefix);
            work.row_actions.add_widget(&actions);
            row.attach(&prefix, 0, 0, 1, 1);
            row.attach(&entry, 1, 0, 1, 1);
            row.attach(&actions, 2, 0, 1, 1);
            let add = Rc::new(AddRow {
                busy: Cell::new(false),
                button,
                entry,
                revealed: Cell::new(false),
                row,
            });
            let (w, weak) = (self.clone(), Rc::downgrade(&add));
            add.entry.connect_activate(move |_| {
                if let Some(add) = weak.upgrade() {
                    w.submit_work_addition(space, parent, kind, add);
                }
            });
            let (w, weak) = (self.clone(), Rc::downgrade(&add));
            add.button.connect_clicked(move |_| {
                if let Some(add) = weak.upgrade() {
                    add.revealed.set(true);
                    w.draw_work(space, parent);
                    add.entry.grab_focus_without_selecting();
                }
            });
            self.inner().work.additions.borrow_mut().insert(key, add.clone());
            add
        });
        if !empty {
            add.revealed.set(false);
        }
        let widget = if empty && !add.revealed.get() {
            add.button.upcast_ref::<gtk::Widget>()
        } else {
            add.row.upcast_ref()
        };
        if let Some(parent) = widget.parent().and_downcast::<gtk::Box>() {
            parent.remove(widget);
        }
        self.inner().work.widget.append(widget);
    }

    fn submit_work_addition(&self, space: Id, parent: Id, kind: &'static str, add: Rc<AddRow>) {
        let submitted = add.entry.text().to_string();
        if add.busy.get() || submitted.trim().is_empty() {
            return;
        }
        add.busy.set(true);
        add.entry.grab_focus_without_selecting();
        // The title input is plain text, so markdown punctuation must remain literal.
        let mut body = String::from("# ");
        for character in submitted.trim().chars() {
            if character.is_ascii_punctuation() {
                body.push('\\');
            }
            body.push(character);
        }
        body.push_str("\n\n");
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let result =
                core(async move { state.create_work(space, kind, &body, Some(parent)).await })
                    .await;
            add.busy.set(false);
            match result {
                Ok(_) => {
                    if add.entry.text() == submitted {
                        add.entry.set_text("");
                    }
                    w.refresh_work();
                    w.reload_notes();
                    w.reload_saved_searches();
                }
                Err(error) => w.toast(&format!("Couldn't create {kind}: {error}")),
            }
        });
    }

    pub(super) fn focused_work_addition(&self) -> Option<gtk::Entry> {
        let focus = gtk::prelude::RootExt::focus(&self.inner().window)?;
        self.inner()
            .work
            .additions
            .borrow()
            .values()
            .find(|add| focus == add.entry || focus.is_ancestor(&add.entry))
            .map(|add| add.entry.clone())
    }
}
