//! Inline additions retain their input and focus across background hierarchy refreshes.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use zib_core::Id;
use zib_core::schema;

use crate::app::core;

use super::{Window, drag, task_expander};

pub(super) struct AddRow {
    busy: Cell<bool>,
    entry: gtk::Entry,
    row: gtk::Box,
}

impl Window {
    pub(super) fn work_addition(&self, space: Id, parent: Id, kind: &'static str) {
        let key = (space, parent, kind);
        let existing = self.inner().work.additions.borrow().get(&key).cloned();
        let add = existing.unwrap_or_else(|| {
            let name = if kind == schema::PROJECT { "New subproject" } else { "New task" };
            let entry = gtk::Entry::builder()
                .hexpand(true)
                .placeholder_text(name)
                .truncate_multiline(true)
                .build();
            entry.update_property(&[gtk::accessible::Property::Label(name)]);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            let project = kind == schema::PROJECT;
            let prefix = gtk::Box::new(gtk::Orientation::Horizontal, if project { 4 } else { 2 });
            prefix.append(&drag::handle());
            if project {
                prefix.append(&gtk::Label::new(Some("▧")));
            } else {
                prefix.append(&task_expander(false));
                let completion = gtk::CheckButton::new();
                completion.add_css_class("zib-task-completion");
                prefix.append(&completion);
            }
            // Invisible controls reserve the themed row width even when the list has no entries.
            prefix.set_opacity(0.0);
            prefix.set_sensitive(false);
            prefix.set_can_target(false);
            prefix.update_state(&[gtk::accessible::State::Hidden(true)]);
            let work = &self.inner().work;
            let alignment =
                if kind == schema::PROJECT { &work.project_prefix } else { &work.task_prefix };
            alignment.add_widget(&prefix);
            row.append(&prefix);
            row.append(&entry);
            let add = Rc::new(AddRow { busy: Cell::new(false), entry, row });
            let (w, weak) = (self.clone(), Rc::downgrade(&add));
            add.entry.connect_activate(move |_| {
                if let Some(add) = weak.upgrade() {
                    w.submit_work_addition(space, parent, kind, add);
                }
            });
            self.inner().work.additions.borrow_mut().insert(key, add.clone());
            add
        });
        if let Some(parent) = add.row.parent().and_downcast::<gtk::Box>() {
            parent.remove(&add.row);
        }
        self.inner().work.widget.append(&add.row);
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
