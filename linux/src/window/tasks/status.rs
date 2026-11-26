//! Status menus and Waiting text completion for projects and tasks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use zib_core::Id;
use zib_core::work::{Status, Tree};

use crate::app::core;

use super::controls::{append_picker_menu, focus_picker};
use super::{Window, panel};

const WAITING_LABEL: &str = "Waiting for:";

pub(super) struct ProjectWaiting {
    entry: gtk::Entry,
    key: (Id, Id),
    on_leave: RefCell<Option<String>>,
    queued: RefCell<Option<String>>,
    saved: RefCell<Option<Status>>,
    saving: Cell<bool>,
}

impl ProjectWaiting {
    fn load(&self, status: Option<&Status>) {
        let waiting = status.filter(|status| status.value == "waiting");
        let reason = waiting.and_then(|status| status.waiting_for.as_deref()).unwrap_or_default();
        if self.saving.get()
            || (self.entry.get_visible()
                && self.saved.borrow().as_ref() == waiting
                && self.entry.text().trim() != reason)
        {
            return;
        }
        self.entry.set_visible(waiting.is_some());
        if self.entry.text() != reason {
            self.entry.set_text(reason);
        }
        *self.saved.borrow_mut() = waiting.cloned();
    }

    fn button(&self) -> Option<gtk::MenuButton> {
        self.entry.prev_sibling().and_downcast::<gtk::MenuButton>()
    }
}

pub(super) struct StatusMenu {
    pub model: gio::Menu,
    content: gtk::Box,
    slot: &'static str,
}

#[derive(Clone)]
struct StatusOptions {
    explicit: Option<Status>,
    inherited: Option<Status>,
    reason: String,
}

impl StatusOptions {
    fn read(note: Id, tree: &Tree) -> Self {
        Self {
            explicit: tree.resolved_status(note).filter(|status| status.source == note),
            inherited: tree.inherited_status(note),
            reason: tree.status(note).and_then(|status| status.waiting_for).unwrap_or_default(),
        }
    }
}

impl StatusMenu {
    pub fn attach(&self, popup: &gtk::PopoverMenu) {
        assert!(popup.add_child(&self.content, self.slot));
    }
}

impl Window {
    pub(super) fn project_status_control(&self, space: Id, note: Id, tree: &Tree) -> gtk::Box {
        let edit = self.project_waiting(space, note);
        let status = tree.status(note);
        edit.load(status.as_ref());
        let label = if edit.entry.get_visible() {
            WAITING_LABEL.to_owned()
        } else {
            status.as_ref().map_or_else(|| "No status".into(), Status::label)
        };
        let button = status_button(note, tree, false, &label, status.as_ref());
        button.add_css_class("zib-work-field-control");
        button.set_always_show_arrow(true);
        button.set_sensitive(!edit.saving.get());
        let (w, waiting) = (self.clone(), Rc::downgrade(&edit));
        button.connect_state_flags_changed(move |button, previous| {
            if previous.contains(gtk::StateFlags::ACTIVE)
                && !button.state_flags().contains(gtk::StateFlags::ACTIVE)
                && let Some(edit) = waiting.upgrade()
            {
                w.finish_waiting_focus_change(edit);
            }
        });
        let w = self.clone();
        let waiting = Rc::downgrade(&edit);
        let options = StatusOptions::read(note, tree);
        button.set_create_popup_func(move |button| {
            if button.popover().is_some() {
                return;
            }
            let popup = gtk::PopoverMenu::from_model(None::<&gio::Menu>);
            let select = gio::SimpleAction::new("waiting", None);
            let Some(edit) = waiting.upgrade() else { return };
            let (window, waiting, menu) = (w.clone(), Rc::downgrade(&edit), popup.downgrade());
            let label = button.child().and_downcast::<gtk::Label>().unwrap();
            select.connect_activate(move |_, _| {
                let Some(edit) = waiting.upgrade() else { return };
                edit.on_leave.take();
                label.set_label(WAITING_LABEL);
                edit.entry.set_visible(true);
                if let Some(menu) = menu.upgrade() {
                    menu.popdown();
                }
                let entry = edit.entry.clone();
                glib::idle_add_local_once(move || {
                    if entry.is_mapped() {
                        entry.grab_focus_without_selecting();
                    }
                });
                let reason = edit.entry.text().trim().to_owned();
                window.save_project_waiting(edit, reason);
            });
            let model = w.status_choices(&popup, space, note, &options, Some((&select, &edit)));
            popup.set_menu_model(Some(&model));
            w.track_work_popover(popup.upcast_ref());
            let (window, waiting) = (w.clone(), Rc::downgrade(&edit));
            popup.connect_closed(move |_| {
                if let Some(edit) = waiting.upgrade() {
                    window.finish_waiting_focus_change(edit);
                }
            });
            button.set_popover(Some(&popup));
        });
        let control = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        control.set_hexpand(true);
        if let Some(parent) = edit.entry.parent().and_downcast::<gtk::Box>() {
            parent.remove(&edit.entry);
        }
        control.append(&button);
        control.append(&edit.entry);
        control
    }

    fn project_waiting(&self, space: Id, note: Id) -> Rc<ProjectWaiting> {
        let mut current = self.inner().work.project_waiting.borrow_mut();
        if let Some(edit) = current.as_ref().filter(|edit| edit.key == (space, note)) {
            return edit.clone();
        }
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .width_chars(12)
            .truncate_multiline(true)
            .visible(false)
            .tooltip_text("Press Enter or leave the field to save")
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Waiting for")]);
        let edit = Rc::new(ProjectWaiting {
            entry,
            key: (space, note),
            on_leave: RefCell::default(),
            queued: RefCell::default(),
            saved: RefCell::default(),
            saving: Cell::new(false),
        });
        let (w, waiting) = (self.clone(), Rc::downgrade(&edit));
        edit.entry.connect_activate(move |entry| {
            if let Some(edit) = waiting.upgrade() {
                edit.on_leave.take();
                w.save_project_waiting(edit, entry.text().trim().to_owned());
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let (w, waiting) = (self.clone(), Rc::downgrade(&edit));
        focus.connect_leave(move |_| {
            let Some(edit) = waiting.upgrade() else { return };
            let reason = edit.entry.text().trim().to_owned();
            let unchanged =
                edit.saved.borrow().as_ref().is_some_and(|status| {
                    status.waiting_for.as_deref().unwrap_or_default() == reason
                });
            if edit.saving.get() || !unchanged {
                edit.on_leave.replace(Some(reason));
                w.finish_waiting_focus_change(edit);
            }
        });
        self.complete_project_waiting(&edit.entry, &focus);
        edit.entry.add_controller(focus);
        *current = Some(edit.clone());
        edit
    }

    pub(super) fn focused_project_waiting(&self) -> Option<gtk::Entry> {
        let focus = gtk::prelude::RootExt::focus(&self.inner().window)?;
        let current = self.inner().work.project_waiting.borrow();
        let entry = &current.as_ref()?.entry;
        (focus == *entry || focus.is_ancestor(entry)).then(|| entry.clone())
    }

    fn finish_waiting_focus_change(&self, edit: Rc<ProjectWaiting>) {
        if edit.on_leave.borrow().is_none() {
            return;
        }
        let w = self.clone();
        // A sidebar refresh can move the entry and restore its focus in the same turn.
        glib::idle_add_local_once(move || {
            // Let a click on the dropdown finish before saving a reason it might clear.
            if edit.button().is_some_and(|button| {
                button.state_flags().contains(gtk::StateFlags::ACTIVE)
                    || button.popover().is_some_and(|menu| menu.is_mapped())
            }) {
                return;
            }
            let Some(reason) = edit.on_leave.take() else { return };
            if edit.entry.get_visible() && w.focused_project_waiting().as_ref() != Some(&edit.entry)
            {
                w.save_project_waiting(edit, reason);
            }
        });
    }

    fn save_project_waiting(&self, edit: Rc<ProjectWaiting>, reason: String) {
        let (space, note) = edit.key;
        if edit.saving.get() {
            edit.queued.replace(Some(reason));
            return;
        }
        if edit.saved.borrow().as_ref().is_some_and(|status| {
            status.source == note && status.waiting_for.as_deref().unwrap_or_default() == reason
        }) {
            return;
        }
        edit.saving.set(true);
        if let Some(button) = edit.button() {
            button.set_sensitive(false);
        }
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let submitted = reason.clone();
            let result = core(async move {
                state.set_work_status(space, note, Some("waiting".into()), submitted).await
            })
            .await;
            edit.saving.set(false);
            if let Some(button) = edit.button() {
                button.set_sensitive(true);
            }
            let queued = edit.queued.take();
            match result {
                Ok(()) => {
                    *edit.saved.borrow_mut() = Some(Status {
                        source: note,
                        value: "waiting".into(),
                        waiting_for: (!reason.is_empty()).then_some(reason),
                    });
                    if let Some(reason) = queued {
                        w.save_project_waiting(edit, reason);
                    }
                    w.refresh_work();
                    w.reload_notes();
                }
                Err(error) => w.toast(&format!("Couldn't update status: {error}")),
            }
        });
    }

    fn complete_project_waiting(&self, entry: &gtk::Entry, focus: &gtk::EventControllerFocus) {
        let values = Rc::new(RefCell::new(Vec::<String>::new()));
        let keys = gtk::EventControllerKey::new();
        let candidates = values.clone();
        keys.connect_key_pressed(move |keys, key, _, modifiers| {
            if key != gdk::Key::Tab || !modifiers.is_empty() {
                return glib::Propagation::Proceed;
            }
            let Some(entry) = keys.widget().and_downcast::<gtk::Entry>() else {
                return glib::Propagation::Proceed;
            };
            let text = entry.text();
            let query = text.to_lowercase();
            if !query.is_empty()
                && let Some(value) =
                    candidates.borrow().iter().find(|value| value.to_lowercase().contains(&query))
                && value.as_str() != text
            {
                entry.set_text(value);
                entry.set_position(-1);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
        let started = Cell::new(false);
        let state = self.inner().state.clone();
        focus.connect_enter(move |_| {
            if !started.replace(true) {
                let (state, values) = (state.clone(), values.clone());
                glib::spawn_future_local(async move {
                    let loaded = core(async move { state.waiting_values().await }).await;
                    *values.borrow_mut() = loaded;
                });
            }
        });
    }

    pub(super) fn work_status_button(
        &self,
        space: Id,
        note: Id,
        tree: &Tree,
        compact: bool,
    ) -> gtk::MenuButton {
        let status = tree.status(note);
        let label = status.as_ref().map_or_else(|| "No status".into(), Status::label);
        let button = status_button(note, tree, compact, &label, status.as_ref());
        if tree.items[&note].note.is_task_occurrence() {
            button.set_sensitive(false);
            return button;
        }
        let w = self.clone();
        let options = StatusOptions::read(note, tree);
        button.set_create_popup_func(move |button| {
            if button.popover().is_some() {
                return;
            }
            let popup = gtk::PopoverMenu::from_model(None::<&gio::Menu>);
            let status = w.status_menu(&popup, space, note, &options);
            popup.set_menu_model(Some(&status.model));
            status.attach(&popup);
            w.track_work_popover(popup.upcast_ref());
            popup.connect_show(|popup| popup.set_visible_submenu(Some("main")));
            button.set_popover(Some(&popup));
        });
        button
    }

    pub(super) fn work_status_menu(
        &self,
        popup: &gtk::PopoverMenu,
        space: Id,
        note: Id,
        tree: &Tree,
    ) -> StatusMenu {
        self.status_menu(popup, space, note, &StatusOptions::read(note, tree))
    }

    fn status_menu(
        &self,
        popup: &gtk::PopoverMenu,
        space: Id,
        note: Id,
        options: &StatusOptions,
    ) -> StatusMenu {
        let model = self.status_choices(popup, space, note, options, None);
        let reason = options.reason.clone();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (w, popover) = (self.clone(), popup.downgrade());
        content.connect_map(move |page| {
            if page.first_child().is_none() {
                let Some(popup) = popover.upgrade() else { return };
                page.append(&w.waiting_content(&popup, space, note, &reason));
            }
            focus_picker(page);
        });
        StatusMenu { model, content, slot: "waiting-value" }
    }

    fn status_choices(
        &self,
        popup: &gtk::PopoverMenu,
        space: Id,
        note: Id,
        options: &StatusOptions,
        inline_waiting: Option<(&gio::SimpleAction, &Rc<ProjectWaiting>)>,
    ) -> gio::Menu {
        let model = gio::Menu::new();
        let (explicit, inherited) = (&options.explicit, &options.inherited);
        let actions = gio::SimpleActionGroup::new();
        // Follow the owner's status-menu order, placing Waiting before Clear.
        for (label, value) in [
            ("Inherited", "inherit"),
            ("Someday", "someday"),
            ("Maybe", "maybe"),
            ("Waiting…", "waiting"),
            ("Clear", "none"),
        ] {
            if value == "waiting" {
                if let Some((action, _)) = inline_waiting {
                    actions.add_action(action);
                    model.append(Some("Waiting..."), Some("work-status.waiting"));
                } else {
                    append_picker_menu(&model, label, "waiting-value");
                }
                continue;
            }
            if (value == "inherit" && inherited.is_none() && explicit.is_none())
                || (value == "none"
                    && inherited.as_ref().is_none_or(|status| status.value == "none"))
                || explicit.as_ref().is_some_and(|status| status.value == value)
            {
                continue;
            }
            let action = gio::SimpleAction::new(value, None);
            let (w, popover) = (self.clone(), popup.downgrade());
            let waiting = inline_waiting.map(|(_, edit)| Rc::downgrade(edit));
            action.connect_activate(move |_, _| {
                if let Some(edit) = waiting.as_ref().and_then(std::rc::Weak::upgrade) {
                    edit.on_leave.take();
                }
                let status = (value != "inherit").then(|| value.to_owned());
                w.save_work_status(popover.upgrade(), space, note, status, String::new());
            });
            actions.add_action(&action);
            model.append(Some(label), Some(&format!("work-status.{value}")));
        }
        popup.insert_action_group("work-status", Some(&actions));
        model
    }

    fn waiting_content(
        &self,
        popup: &gtk::PopoverMenu,
        space: Id,
        note: Id,
        reason: &str,
    ) -> gtk::Box {
        let content = panel();
        let entry = gtk::Entry::builder()
            .placeholder_text("Waiting for…")
            .text(reason)
            .truncate_multiline(true)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Waiting for")]);
        content.append(&entry);
        let list = gtk::ListBox::new();
        list.add_css_class("zib-work-picker-list");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        let empty = gtk::Label::new(Some("No previous matches"));
        empty.add_css_class("dim-label");
        list.set_placeholder(Some(&empty));
        content.append(
            &gtk::ScrolledWindow::builder()
                .child(&list)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .min_content_width(260)
                .max_content_height(180)
                .propagate_natural_height(true)
                .build(),
        );
        self.waiting_completion(&entry, &list);
        entry.set_tooltip_text(Some("Press Enter to save"));
        let (w, popover) = (self.clone(), popup.downgrade());
        entry.connect_activate(move |entry| {
            w.save_work_status(
                popover.upgrade(),
                space,
                note,
                Some("waiting".into()),
                entry.text().into(),
            );
        });
        content
    }

    fn waiting_completion(&self, entry: &gtk::Entry, list: &gtk::ListBox) {
        let rows = list.downgrade();
        entry.connect_changed(move |entry| {
            if let Some(list) = rows.upgrade() {
                filter_suggestions(entry, &list);
            }
        });
        let input = entry.downgrade();
        list.connect_row_activated(move |_, row| {
            if let Some(entry) = input.upgrade() {
                fill_suggestion(&entry, row);
                entry.emit_by_name::<()>("activate", &[]);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let (input, rows) = (entry.downgrade(), list.downgrade());
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let handled = input
                .upgrade()
                .zip(rows.upgrade())
                .is_some_and(|(entry, list)| suggestion_key(&entry, &list, key, modifiers));
            handled.into()
        });
        entry.add_controller(keys);
        let (state, input, rows) =
            (self.inner().state.clone(), entry.downgrade(), list.downgrade());
        glib::spawn_future_local(async move {
            let values = core(async move { state.waiting_values().await }).await;
            let Some((entry, list)) = input.upgrade().zip(rows.upgrade()) else { return };
            for value in values {
                let label = gtk::Label::builder()
                    .label(&value)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .max_width_chars(32)
                    .margin_start(8)
                    .margin_end(8)
                    .margin_top(6)
                    .margin_bottom(6)
                    .build();
                label.set_tooltip_text(Some(&value));
                list.append(&gtk::ListBoxRow::builder().child(&label).build());
            }
            filter_suggestions(&entry, &list);
        });
    }

    fn save_work_status(
        &self,
        popup: Option<gtk::PopoverMenu>,
        space: Id,
        note: Id,
        status: Option<String>,
        reason: String,
    ) {
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            match core(async move { state.set_work_status(space, note, status, reason).await })
                .await
            {
                Ok(()) => {
                    if let Some(popup) = popup {
                        popup.popdown();
                    }
                    w.refresh_work();
                    w.reload_notes();
                }
                Err(error) => w.toast(&format!("Couldn't update status: {error}")),
            }
        });
    }
}

fn status_button(
    note: Id,
    tree: &Tree,
    compact: bool,
    label: &str,
    status: Option<&Status>,
) -> gtk::MenuButton {
    let has_reason = status.is_some_and(|status| status.waiting_for.is_some());
    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(if compact && has_reason { 32 } else { 18 })
        .build();
    let button = gtk::MenuButton::builder().child(&text).build();
    if compact {
        text.add_css_class("caption");
        button.add_css_class("flat");
    }
    let hint = status.filter(|status| status.source != note).map_or_else(
        || label.to_owned(),
        |status| format!("{label} · Inherited from {}", tree.items[&status.source].title),
    );
    button.set_tooltip_text(Some(&format!("Work status · {hint}")));
    button.update_property(&[gtk::accessible::Property::Label("Work status")]);
    button
}

fn rows(list: &gtk::ListBox) -> Vec<gtk::ListBoxRow> {
    let mut rows = Vec::new();
    let mut next = list.first_child();
    while let Some(widget) = next {
        next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            rows.push(row);
        }
    }
    rows
}

fn filter_suggestions(entry: &gtk::Entry, list: &gtk::ListBox) {
    list.unselect_all();
    let query = entry.text().to_lowercase();
    for row in rows(list) {
        let text = row.child().and_downcast::<gtk::Label>().unwrap().text();
        row.set_visible(text.to_lowercase().contains(&query));
    }
}

fn fill_suggestion(entry: &gtk::Entry, row: &gtk::ListBoxRow) {
    let label = row.child().and_downcast::<gtk::Label>().unwrap();
    entry.set_text(&label.text());
    entry.set_position(-1);
    entry.grab_focus_without_selecting();
}

fn suggestion_key(
    entry: &gtk::Entry,
    list: &gtk::ListBox,
    key: gdk::Key,
    modifiers: gdk::ModifierType,
) -> bool {
    let rows: Vec<_> = rows(list).into_iter().filter(gtk::prelude::WidgetExt::is_visible).collect();
    if rows.is_empty()
        || modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
    {
        return false;
    }
    let selected =
        list.selected_row().and_then(|selected| rows.iter().position(|row| *row == selected));
    match key {
        gdk::Key::Down | gdk::Key::Up => {
            let index = if key == gdk::Key::Down {
                selected.map_or(0, |i| (i + 1).min(rows.len() - 1))
            } else {
                selected.map_or(rows.len() - 1, |i| i.saturating_sub(1))
            };
            list.select_row(Some(&rows[index]));
            true
        }
        gdk::Key::Return if selected.is_some() => {
            fill_suggestion(entry, &rows[selected.unwrap()]);
            entry.emit_by_name::<()>("activate", &[]);
            true
        }
        gdk::Key::Tab => {
            let row = &rows[selected.unwrap_or(0)];
            let text = row.child().and_downcast::<gtk::Label>().unwrap().text();
            if text == entry.text() {
                false
            } else {
                fill_suggestion(entry, row);
                true
            }
        }
        _ => false,
    }
}
