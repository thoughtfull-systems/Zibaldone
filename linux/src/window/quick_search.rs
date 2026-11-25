//! The keyboard-driven popup for opening notes by title.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use zib_core::Id;

use crate::app::core;

use super::{PickerNote, Window, arrow_keys, picker_notes, picker_scroll};

const LIMIT: usize = 20;

impl Window {
    pub(super) fn open_quick_search(&self) {
        let inner = self.inner();
        if inner.quick_search.parent().is_none() {
            inner.quick_search.set_parent(&inner.title);
        }
        inner.quick_search.popup();
    }

    pub(super) fn connect_quick_search(&self) {
        let entry = gtk::SearchEntry::builder().placeholder_text("Search note titles").build();
        entry.update_property(&[gtk::accessible::Property::Label("Search note titles")]);
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let status = gtk::Label::builder().css_classes(["dim-label"]).build();
        list.set_placeholder(Some(&status));
        let scroll = picker_scroll(&list);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&entry);
        content.append(&scroll);
        let popover = self.inner().quick_search.clone();
        popover.set_child(Some(&content));

        let generation = Rc::new(Cell::new(0u64));
        let targets = Rc::new(RefCell::new(Vec::new()));
        let fill = {
            let (generation, targets, list) = (generation.clone(), targets.clone(), list.clone());
            let weak = Rc::downgrade(&self.0);
            move |query: String| {
                let Some(window) = weak.upgrade().map(Window) else {
                    return;
                };
                let state = window.inner().state.clone();
                let names: HashMap<_, _> = window.shown_spaces().into_iter().collect();
                let spaces: Vec<_> = names.keys().copied().collect();
                let empty = query.trim().is_empty();
                let current = generation.get().wrapping_add(1);
                generation.set(current);
                targets.borrow_mut().clear();
                list.remove_all();
                status.set_label("Searching…");
                let (generation, targets, list, status) =
                    (generation.clone(), targets.clone(), list.clone(), status.clone());
                glib::spawn_future_local(async move {
                    let result = core(async move {
                        let hits = state.search_titles(&spaces, &query, LIMIT)?;
                        Ok::<_, zib_core::Error>(picker_notes(&state, hits).await)
                    })
                    .await;
                    if generation.get() != current {
                        return;
                    }
                    let matches = match result {
                        Ok(matches) => matches,
                        Err(error) => {
                            status.set_label(&format!("Couldn't search notes: {error}"));
                            return;
                        }
                    };
                    status.set_label(if empty { "No notes" } else { "No matching titles" });
                    *targets.borrow_mut() = append_results(&list, matches, &names);
                    list.select_row(list.row_at_index(0).as_ref());
                });
            }
        };
        let update = fill.clone();
        entry.connect_changed(move |entry| update(entry.text().to_string()));
        let input = entry.clone();
        popover.connect_map(move |_| {
            if input.text().is_empty() {
                fill(String::new());
            } else {
                input.set_text("");
            }
            input.grab_focus();
        });
        popover.connect_unmap(move |_| generation.set(generation.get().wrapping_add(1)));

        let weak = Rc::downgrade(&self.0);
        list.connect_row_activated(glib::clone!(
            #[weak]
            popover,
            move |_, row| {
                let target = usize::try_from(row.index())
                    .ok()
                    .and_then(|index| targets.borrow().get(index).copied());
                if popover.is_mapped()
                    && let (Some(window), Some((space, note))) = (weak.upgrade(), target)
                {
                    popover.popdown();
                    Window(window).open_search_result(space, note);
                }
            }
        ));
        let results = list.clone();
        entry.connect_activate(move |_| {
            if let Some(row) = results.selected_row() {
                results.emit_by_name::<()>("row-activated", &[&row]);
            }
        });
        entry.connect_stop_search(glib::clone!(
            #[weak]
            popover,
            move |_| popover.popdown()
        ));
        entry.add_controller(arrow_keys(&list, &scroll));
    }
}

fn append_results(
    list: &gtk::ListBox,
    matches: Vec<PickerNote>,
    names: &HashMap<Id, String>,
) -> Vec<(Id, Id)> {
    let mut targets = Vec::with_capacity(matches.len());
    for note in matches {
        list.append(&note.row(names));
        targets.push((note.space, note.hit.id));
    }
    targets
}
