//! The saved-search picker, definition editor, and controls above the results.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use adw::prelude::*;
use gtk::glib;
use zib_core::Id;
use zib_core::search::saved::Definition;

use crate::app::core;
use crate::state::searches::SearchEntry;

use super::Window;

type SearchKey = (Id, Id);

#[derive(Clone)]
struct Draft {
    definition: Definition,
    expected: Option<SearchEntry>,
}

pub(super) struct SearchSidebar {
    pub widget: gtk::Box,
    pub picker: gtk::DropDown,
    pub scope: gtk::DropDown,
    pub save: gtk::Button,
    pub copy: gtk::Button,
    status: gtk::Label,
    entries: RefCell<Vec<SearchEntry>>,
    selected: RefCell<Option<SearchEntry>>,
    scopes: RefCell<Vec<Option<Id>>>,
    drafts: RefCell<HashMap<Option<SearchKey>, Draft>>,
    changing: Cell<bool>,
    busy: Cell<bool>,
    valid: Cell<bool>,
    loading: Cell<u64>,
}

impl SearchSidebar {
    pub fn new(search: &gtk::TextView, sort: &gtk::MenuButton) -> Self {
        let literal =
            gtk::TextTag::builder().name("literal-search-query").insert_hyphens(false).build();
        search.buffer().tag_table().add(&literal);
        search.buffer().connect_changed(move |buffer| {
            buffer.apply_tag(&literal, &buffer.start_iter(), &buffer.end_iter());
        });
        let picker = gtk::DropDown::from_strings(&["New search…"]);
        picker.update_property(&[gtk::accessible::Property::Label("Saved search")]);
        let scope = gtk::DropDown::from_strings(&["All spaces"]);
        scope.update_property(&[gtk::accessible::Property::Label("Search spaces")]);
        let save = gtk::Button::with_label("Save");
        let copy = gtk::Button::with_label("Save copy…");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.append(&save);
        actions.append(&copy);
        let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
        status.add_css_class("dim-label");
        let query = gtk::ScrolledWindow::builder()
            .child(search)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(100)
            .max_content_height(220)
            .propagate_natural_height(true)
            .build();
        query.add_css_class("frame");
        let options = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        scope.set_hexpand(true);
        options.append(&scope);
        options.append(sort);
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(10)
            .margin_end(10)
            .margin_top(10)
            .margin_bottom(10)
            .build();
        widget.append(&picker);
        widget.append(&gtk::Label::builder().label("Search definition").xalign(0.0).build());
        widget.append(&query);
        widget.append(&actions);
        widget.append(&options);
        widget.append(&status);
        Self {
            widget,
            picker,
            scope,
            save,
            copy,
            status,
            entries: RefCell::default(),
            selected: RefCell::default(),
            scopes: RefCell::new(vec![None]),
            drafts: RefCell::default(),
            changing: Cell::new(false),
            busy: Cell::new(false),
            valid: Cell::new(true),
            loading: Cell::new(0),
        }
    }

    fn key(&self) -> Option<SearchKey> {
        self.selected.borrow().as_ref().map(|entry| (entry.space, entry.search.id))
    }

    fn scope_id(&self) -> Option<Id> {
        self.scopes.borrow().get(self.scope.selected() as usize).copied().flatten()
    }
}

impl Window {
    pub(super) fn toggle_search(&self) {
        let split = &self.inner().sidebar_split;
        split.set_show_sidebar(!split.shows_sidebar());
    }

    pub(super) fn search_definition(&self) -> Definition {
        let inner = self.inner();
        let buffer = inner.search.buffer();
        Definition {
            query: buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string(),
            scope: inner.search_sidebar.scope_id(),
            sort: inner.sort.get(),
        }
    }

    pub(super) fn search_spaces(&self) -> Vec<(Id, String)> {
        let scope = self.inner().search_sidebar.scope_id();
        self.inner()
            .spaces
            .borrow()
            .iter()
            .filter(|(id, _, _)| scope.is_none_or(|s| s == *id))
            .map(|(id, name, _)| (*id, name.clone()))
            .collect()
    }

    pub(super) fn connect_search_sidebar(&self) {
        let sidebar = &self.inner().search_sidebar;
        let w = self.clone();
        self.inner().search.buffer().connect_changed(move |_| w.search_edited());
        let w = self.clone();
        sidebar.scope.connect_selected_notify(move |_| w.search_edited());
        let w = self.clone();
        sidebar.picker.connect_selected_notify(move |_| w.select_saved_search());
        let w = self.clone();
        sidebar.save.connect_clicked(move |_| w.save_search(false));
        let w = self.clone();
        sidebar.copy.connect_clicked(move |_| w.save_search(true));
    }

    pub(super) fn search_edited(&self) {
        if self.inner().search_sidebar.changing.get() {
            return;
        }
        self.refresh_search_completion();
        self.inner().search_sidebar.valid.set(false);
        self.update_search_actions();
        self.reload_notes();
    }

    fn search_dirty(&self) -> bool {
        self.inner()
            .search_sidebar
            .selected
            .borrow()
            .as_ref()
            .is_none_or(|entry| entry.search.definition != self.search_definition())
    }

    pub(super) fn update_search_actions(&self) {
        let sidebar = &self.inner().search_sidebar;
        let dirty = self.search_dirty();
        let available = !self.inner().spaces.borrow().is_empty();
        sidebar
            .save
            .set_sensitive(dirty && available && !sidebar.busy.get() && sidebar.valid.get());
        sidebar.save.set_css_classes(if dirty { &["suggested-action"] } else { &[] });
        sidebar.copy.set_visible(sidebar.selected.borrow().is_some());
        sidebar.copy.set_sensitive(available && !sidebar.busy.get() && sidebar.valid.get());
        sidebar.status.set_label(if sidebar.busy.get() {
            "Saving…"
        } else if sidebar.scope_id().is_some() && self.search_spaces().is_empty() {
            "This machine doesn't hold the searched space"
        } else if dirty {
            "Unsaved changes"
        } else {
            ""
        });
    }

    pub(super) fn search_validated(&self, valid: bool) {
        self.inner().search_sidebar.valid.set(valid);
        self.update_search_actions();
    }

    fn apply_search(&self, definition: &Definition) {
        let sidebar = &self.inner().search_sidebar;
        sidebar.changing.set(true);
        if self.search_definition().query != definition.query {
            self.inner().search.buffer().set_text(&definition.query);
        }
        self.inner().sort.set(definition.sort);
        let sort = definition.sort.name().to_variant();
        let _ = gtk::prelude::WidgetExt::activate_action(
            &self.inner().window,
            "win.sort-notes",
            Some(&sort),
        );
        self.fill_search_scopes(definition.scope);
        sidebar.changing.set(false);
        self.search_edited();
    }

    fn fill_search_scopes(&self, selected: Option<Id>) {
        let sidebar = &self.inner().search_sidebar;
        let mut labels = vec!["All spaces".to_owned()];
        let mut ids = vec![None];
        for (id, name, _) in self.inner().spaces.borrow().iter() {
            labels.push(name.clone());
            ids.push(Some(*id));
        }
        if selected.is_some() && !ids.contains(&selected) {
            labels.push("Unavailable space".into());
            ids.push(selected);
        }
        let at = ids.iter().position(|id| *id == selected).unwrap_or(0);
        *sidebar.scopes.borrow_mut() = ids;
        let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
        sidebar.scope.set_model(Some(&model));
        sidebar.scope.set_selected(u32::try_from(at).unwrap_or(0));
    }

    fn select_saved_search(&self) {
        let sidebar = &self.inner().search_sidebar;
        if sidebar.changing.get() {
            return;
        }
        let current = Draft {
            definition: self.search_definition(),
            expected: sidebar.selected.borrow().clone(),
        };
        if self.search_dirty() {
            sidebar.drafts.borrow_mut().insert(sidebar.key(), current);
        } else {
            sidebar.drafts.borrow_mut().remove(&sidebar.key());
        }
        let entry = sidebar
            .picker
            .selected()
            .checked_sub(1)
            .and_then(|i| sidebar.entries.borrow().get(i as usize).cloned());
        let key = entry.as_ref().map(|entry| (entry.space, entry.search.id));
        let draft = sidebar.drafts.borrow().get(&key).cloned();
        let (definition, expected) = if let Some(draft) = draft {
            (draft.definition, draft.expected)
        } else {
            (entry.as_ref().map(|entry| entry.search.definition.clone()).unwrap_or_default(), entry)
        };
        *sidebar.selected.borrow_mut() = expected;
        self.apply_search(&definition);
    }

    pub(super) fn reload_saved_searches(&self) {
        let sidebar = &self.inner().search_sidebar;
        let generation = sidebar.loading.get().wrapping_add(1);
        sidebar.loading.set(generation);
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let entries = core(async move { state.saved_searches().await }).await;
            let sidebar = &w.inner().search_sidebar;
            if generation != sidebar.loading.get() {
                return;
            }
            let definition = w.search_definition();
            let key = sidebar.key();
            let dirty = w.search_dirty();
            sidebar.changing.set(true);
            let names = w
                .inner()
                .spaces
                .borrow()
                .iter()
                .map(|(id, n, _)| (*id, n.clone()))
                .collect::<HashMap<_, _>>();
            let mut labels = vec!["New search…".to_owned()];
            for entry in &entries {
                let name = &entry.search.name;
                labels.push(if names.len() > 1 {
                    format!("{name} · {}", names.get(&entry.space).cloned().unwrap_or_default())
                } else {
                    name.clone()
                });
            }
            let at = entries.iter().position(|e| Some((e.space, e.search.id)) == key);
            if !dirty && let Some(at) = at {
                *sidebar.selected.borrow_mut() = Some(entries[at].clone());
            }
            *sidebar.entries.borrow_mut() = entries;
            let labels =
                gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
            sidebar.picker.set_model(Some(&labels));
            sidebar.picker.set_selected(at.map_or(0, |i| u32::try_from(i + 1).unwrap_or(0)));
            sidebar.changing.set(false);
            if dirty {
                w.apply_search(&definition);
            } else {
                let definition = sidebar
                    .selected
                    .borrow()
                    .as_ref()
                    .map(|e| e.search.definition.clone())
                    .unwrap_or(definition);
                w.apply_search(&definition);
            }
        });
    }

    fn save_search(&self, copy: bool) {
        let existing = self.inner().search_sidebar.selected.borrow().clone();
        if !copy && let Some(existing) = existing {
            self.write_search(existing.space, Some(existing.search), String::new());
            return;
        }
        self.name_search();
    }

    fn name_search(&self) {
        let spaces = self
            .inner()
            .spaces
            .borrow()
            .iter()
            .map(|(id, n, _)| (*id, n.clone()))
            .collect::<Vec<_>>();
        if spaces.is_empty() {
            return;
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let name = gtk::Entry::builder().placeholder_text("Search name").build();
        if let Some(entry) = self.inner().search_sidebar.selected.borrow().as_ref() {
            name.set_text(&format!("{} copy", entry.search.name));
        }
        let destination = gtk::DropDown::from_strings(
            &spaces.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
        );
        let default = self.search_definition().scope.or(self.current_space_id());
        destination.set_selected(
            u32::try_from(spaces.iter().position(|(id, _)| Some(*id) == default).unwrap_or(0))
                .unwrap_or(0),
        );
        content.append(&name);
        content.append(&gtk::Label::builder().label("Store in space").xalign(0.0).build());
        content.append(&destination);
        let dialog =
            adw::AlertDialog::builder().heading("Save search").extra_child(&content).build();
        dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("save", !name.text().trim().is_empty());
        let d = dialog.clone();
        name.connect_changed(move |name| {
            d.set_response_enabled("save", !name.text().trim().is_empty());
        });
        let w = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "save"
                && let Some((space, _)) = spaces.get(destination.selected() as usize)
            {
                w.write_search(*space, None, name.text().to_string());
            }
        });
        dialog.present(Some(&self.inner().window));
    }

    fn write_search(
        &self,
        space: Id,
        expected: Option<zib_core::search::saved::SavedSearch>,
        name: String,
    ) {
        let sidebar = &self.inner().search_sidebar;
        if sidebar.busy.replace(true) {
            return;
        }
        let original_key = sidebar.key();
        let updating = expected.is_some();
        let definition = self.search_definition();
        sidebar.drafts.borrow_mut().insert(
            original_key,
            Draft { definition: definition.clone(), expected: sidebar.selected.borrow().clone() },
        );
        self.update_search_actions();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let submitted = definition.clone();
            let result =
                core(async move { state.save_search(space, expected, name, submitted).await })
                    .await;
            let sidebar = &w.inner().search_sidebar;
            sidebar.busy.set(false);
            match result {
                Ok(entry) => {
                    if updating
                        && let Some(draft) = sidebar.drafts.borrow_mut().get_mut(&original_key)
                    {
                        draft.expected = Some(entry.clone());
                    }
                    if sidebar.key() == original_key
                        && (updating || w.search_definition() == definition)
                    {
                        *sidebar.selected.borrow_mut() = Some(entry);
                    }
                    w.reload_saved_searches();
                }
                Err(error) => w.toast(&error.to_string()),
            }
            w.update_search_actions();
        });
    }
}
