//! Status menus and Waiting text completion for projects and tasks.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use zib_core::Id;
use zib_core::work::{Status, Tree};

use crate::app::core;

use super::controls::append_picker_menu;
use super::{Window, panel};

pub(super) struct StatusMenu {
    pub model: gio::Menu,
    content: gtk::Box,
    slot: &'static str,
}

impl StatusMenu {
    pub fn attach(&self, popup: &gtk::PopoverMenu) {
        assert!(popup.add_child(&self.content, self.slot));
    }
}

impl Window {
    pub(super) fn work_status_button(&self, space: Id, note: Id, tree: &Tree) -> gtk::MenuButton {
        let status = tree.resolved_status(note);
        let label = status.as_ref().map_or_else(|| "Status".into(), Status::label);
        let text = gtk::Label::builder()
            .label(&label)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(18)
            .build();
        let button = gtk::MenuButton::builder().child(&text).build();
        button.add_css_class("flat");
        let hint = status.filter(|status| status.source != note).map_or_else(
            || label.clone(),
            |status| format!("{label} · Inherited from {}", tree.items[&status.source].title),
        );
        button.set_tooltip_text(Some(&format!("Work status · {hint}")));
        button.update_property(&[gtk::accessible::Property::Label("Work status")]);
        let w = self.clone();
        button.set_create_popup_func(move |button| {
            if button.popover().is_some() {
                return;
            }
            let popup = gtk::PopoverMenu::from_model(None::<&gio::Menu>);
            let status = w.work_status_menu(&popup, space, note, &w.inner().work.tree.borrow());
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
        let model = gio::Menu::new();
        let inherited = tree.inherited_status(note);
        let explicit = tree.resolved_status(note).filter(|status| status.source == note);
        let actions = gio::SimpleActionGroup::new();
        for (label, value) in
            [("Inherit", "inherit"), ("Maybe", "maybe"), ("None", "none"), ("Someday", "someday")]
        {
            if (value == "inherit" && inherited.is_none() && explicit.is_none())
                || explicit.as_ref().is_some_and(|status| status.value == value)
            {
                continue;
            }
            let action = gio::SimpleAction::new(value, None);
            let (w, popover) = (self.clone(), popup.downgrade());
            action.connect_activate(move |_, _| {
                let status = (value != "inherit").then(|| value.to_owned());
                w.save_work_status(popover.upgrade(), space, note, status, String::new());
            });
            actions.add_action(&action);
            model.append(Some(label), Some(&format!("work-status.{value}")));
        }
        popup.insert_action_group("work-status", Some(&actions));
        append_picker_menu(&model, "Waiting…", "waiting-value");
        let reason = tree.status(note).and_then(|status| status.waiting_for).unwrap_or_default();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (w, popover) = (self.clone(), popup.downgrade());
        content.connect_map(move |page| {
            if page.first_child().is_none() {
                let Some(popup) = popover.upgrade() else { return };
                page.append(&w.waiting_content(&popup, space, note, &reason));
            }
            let focus =
                page.first_child().and_then(|content| content.first_child()).map(|w| w.downgrade());
            glib::idle_add_local_once(move || {
                if let Some(focus) = focus.and_then(|w| w.upgrade()).filter(gtk::Widget::is_mapped)
                {
                    focus.grab_focus();
                }
            });
        });
        StatusMenu { model, content, slot: "waiting-value" }
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
        let apply = gtk::Button::with_label("Apply");
        apply.add_css_class("suggested-action");
        content.append(&apply);
        let (w, popover, input) = (self.clone(), popup.downgrade(), entry.downgrade());
        let save = std::rc::Rc::new(move || {
            if let Some(entry) = input.upgrade() {
                w.save_work_status(
                    popover.upgrade(),
                    space,
                    note,
                    Some("waiting".into()),
                    entry.text().into(),
                );
            }
        });
        let saving = save.clone();
        apply.connect_clicked(move |_| saving());
        entry.connect_activate(move |_| save());
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
                    .margin_start(6)
                    .margin_end(6)
                    .margin_top(4)
                    .margin_bottom(4)
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
