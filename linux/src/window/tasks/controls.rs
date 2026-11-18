//! Shared calendar popovers and searchable project-parent pickers for projects and task rows.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use zib_core::Id;
use zib_core::schema::{self, Value};
use zib_core::work::{Item, Tree};

use super::{Window, panel};

struct DatePicker {
    explicit: bool,
    field: &'static str,
    hint: String,
    note: Id,
    value: Option<String>,
}

impl Window {
    pub(super) fn work_controls(&self, parent: &gtk::Box, space: Id, item: &Item, tree: &Tree) {
        style();
        let task = item.note.type_name == schema::TASK;
        // Show availability before the deadline, following the task's lifecycle.
        let dates: Vec<_> = ["scheduled-on", "due-on"]
            .into_iter()
            .filter(|field| !task || tree.date(item.note.id, field).is_some())
            .collect();
        let columns = u32::try_from(dates.len() + 1).expect("at most three controls");
        let controls = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(4)
            .row_spacing(0)
            .min_children_per_line(if self.inner().project_pane.is_project() { columns } else { 1 })
            .max_children_per_line(columns)
            .valign(gtk::Align::Center)
            .build();
        if task {
            controls.add_css_class("zib-task-controls");
        }
        for field in dates {
            insert_control(&controls, &self.date_control(space, item, tree, field));
        }
        if task {
            insert_control(&controls, &self.task_actions(space, item, tree));
        } else {
            insert_control(&controls, &self.parent_picker(space, item, tree));
        }
        parent.append(&controls);
    }

    fn date_control(&self, space: Id, item: &Item, tree: &Tree, field: &'static str) -> gtk::Box {
        let date = tree.date(item.note.id, field);
        let name = if field == "due-on" { "Due" } else { "Scheduled" };
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let heading = gtk::Label::builder().label(name).xalign(0.0).build();
        heading.add_css_class("caption");
        heading.add_css_class("dim-label");
        labels.append(&heading);
        let value = date.as_ref().map_or("No date", |d| d.value.as_str());
        let label = gtk::Label::builder().label(value).xalign(0.0).build();
        label.add_css_class("caption");
        if field == "due-on" && !item.completed() {
            let (urgency, class) = zib_core::work::urgency(date.as_ref().map(|d| d.value.as_str()));
            label.add_css_class(class);
            if matches!(class, "error" | "warning") {
                heading.set_text(urgency.split('·').next().unwrap().trim());
            }
        }
        labels.append(&label);
        let button = gtk::MenuButton::builder()
            .icon_name("x-office-calendar-symbolic")
            .valign(gtk::Align::Center)
            .build();
        button.add_css_class("flat");
        button.add_css_class("zib-work-quiet");
        button.set_tooltip_text(Some(&format!("{name} date · {value}")));
        button.update_property(&[gtk::accessible::Property::Label(&format!("{name} date"))]);
        let picker = date_picker(item, tree, field);
        let w = self.clone();
        button.set_create_popup_func(move |button| {
            if button.popover().is_none() {
                button.set_popover(Some(&w.date_popover(space, &picker)));
            }
        });
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.append(&labels);
        row.append(&button);
        row
    }

    fn date_popover(&self, space: Id, picker: &DatePicker) -> gtk::Popover {
        let popup = gtk::Popover::new();
        self.track_work_popover(&popup);
        popup.set_child(Some(&self.date_content(space, picker, &popup)));
        popup
    }

    fn date_content(&self, space: Id, picker: &DatePicker, popup: &gtk::Popover) -> gtk::Box {
        let (field, note) = (picker.field, picker.note);
        let content = panel();
        let entry = gtk::Entry::builder().placeholder_text("YYYY-MM-DD").width_chars(12).build();
        entry.update_property(&[gtk::accessible::Property::Label("Date")]);
        if let Some(date) = &picker.value {
            entry.set_text(date);
        }
        content.append(&entry);
        let calendar = gtk::Calendar::new();
        select_date(&calendar, entry.text().as_str());
        content.append(&calendar);
        let hint = gtk::Label::builder()
            .label(&picker.hint)
            .wrap(true)
            .max_width_chars(32)
            .xalign(0.0)
            .build();
        hint.add_css_class("caption");
        hint.add_css_class("dim-label");
        content.append(&hint);
        let error = gtk::Label::builder()
            .label("Use a valid date in YYYY-MM-DD format.")
            .wrap(true)
            .visible(false)
            .xalign(0.0)
            .build();
        error.add_css_class("error");
        content.append(&error);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let apply = gtk::Button::with_label("Apply");
        apply.add_css_class("suggested-action");
        apply.set_sensitive(schema::is_date(entry.text().as_str()));
        let clear = gtk::Button::with_label("Clear date");
        clear.set_sensitive(picker.explicit);
        clear.set_tooltip_text(Some("Remove the override and inherit from the parent"));
        actions.append(&clear);
        actions.append(&apply);
        content.append(&actions);
        let changing = Rc::new(Cell::new(false));
        let (calendar_ref, apply_ref, error_ref) =
            (calendar.downgrade(), apply.downgrade(), error.downgrade());
        let syncing = changing.clone();
        entry.connect_changed(move |entry| {
            let valid = schema::is_date(entry.text().trim());
            if let Some(apply) = apply_ref.upgrade() {
                apply.set_sensitive(valid);
            }
            if let Some(error) = error_ref.upgrade() {
                error.set_visible(!valid && !entry.text().is_empty());
            }
            if valid && !syncing.replace(true) {
                if let Some(calendar) = calendar_ref.upgrade() {
                    select_date(&calendar, entry.text().trim());
                }
                syncing.set(false);
            }
        });
        let input = entry.downgrade();
        calendar.connect_day_selected(move |calendar| {
            if changing.get() {
                return;
            }
            if let Some(entry) = input.upgrade() {
                changing.set(true);
                entry.set_text(&calendar.date().format("%Y-%m-%d").unwrap());
                changing.set(false);
            }
        });
        self.connect_date_save(popup, &entry, &apply, space, note, field);
        let (w, popover, id) = (self.clone(), popup.downgrade(), note);
        clear.connect_clicked(move |_| {
            if let Some(popup) = popover.upgrade() {
                popup.popdown();
            }
            w.save_work_field(space, id, field, None);
        });
        content
    }

    fn connect_date_save(
        &self,
        popup: &gtk::Popover,
        entry: &gtk::Entry,
        apply: &gtk::Button,
        space: Id,
        note: Id,
        field: &'static str,
    ) {
        let (w, popup, input) = (self.clone(), popup.downgrade(), entry.downgrade());
        let save = Rc::new(move || {
            let Some(entry) = input.upgrade() else { return };
            let date = entry.text().trim().to_owned();
            if !schema::is_date(&date) {
                entry.add_css_class("error");
                return;
            }
            if let Some(popup) = popup.upgrade() {
                popup.popdown();
            }
            w.save_work_field(space, note, field, Some(Value::Date(date)));
        });
        let saving = save.clone();
        apply.connect_clicked(move |_| saving());
        entry.connect_activate(move |_| save());
    }

    fn parent_picker(&self, space: Id, item: &Item, tree: &Tree) -> gtk::MenuButton {
        let button = gtk::MenuButton::builder().icon_name("folder-symbolic").build();
        button.add_css_class("flat");
        button.add_css_class("zib-work-quiet");
        button.add_css_class("zib-work-parent");
        let parent = item.parent.map_or("No parent".into(), |id| tree.items[&id].title.clone());
        button.set_tooltip_text(Some(&format!("Change parent · {parent}")));
        button.update_property(&[gtk::accessible::Property::Label("Change parent")]);
        let (w, note) = (self.clone(), item.note.id);
        button.set_create_popup_func(move |button| {
            if button.popover().is_none() {
                button.set_popover(Some(&w.parent_popover(space, note)));
            }
        });
        button
    }

    fn parent_popover(&self, space: Id, note: Id) -> gtk::Popover {
        let popup = gtk::Popover::new();
        self.track_work_popover(&popup);
        popup.set_child(Some(&self.parent_content(space, note, &popup)));
        popup
    }

    fn parent_content(&self, space: Id, note: Id, popup: &gtk::Popover) -> gtk::Box {
        let candidates = project_candidates(&self.inner().work.tree.borrow(), note);
        let content = panel();
        let search =
            gtk::SearchEntry::builder().placeholder_text("Search projects").search_delay(0).build();
        search.update_property(&[gtk::accessible::Property::Label("Search projects")]);
        content.append(&search);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        for (_, title) in &candidates {
            let label = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(36)
                .margin_start(8)
                .margin_end(8)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            list.append(&gtk::ListBoxRow::builder().child(&label).activatable(true).build());
        }
        list.set_placeholder(Some(&gtk::Label::new(Some("No matching projects"))));
        content.append(
            &gtk::ScrolledWindow::builder()
                .child(&list)
                .min_content_width(280)
                .max_content_height(280)
                .propagate_natural_height(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        );
        let detach = gtk::Button::with_label("No parent");
        content.append(&detach);
        let (rows, titles) = (
            list.downgrade(),
            candidates.iter().map(|(_, title)| title.to_lowercase()).collect::<Vec<_>>(),
        );
        search.connect_search_changed(move |search| {
            let words = search.text().to_lowercase();
            if let Some(list) = rows.upgrade() {
                for (index, title) in (0..).zip(&titles) {
                    if let Some(row) = list.row_at_index(index) {
                        row.set_visible(words.split_whitespace().all(|word| title.contains(word)));
                    }
                }
            }
        });
        let (w, popover, id) = (self.clone(), popup.downgrade(), note);
        list.connect_row_activated(move |_, row| {
            if let Some((parent, _)) =
                candidates.get(usize::try_from(row.index()).unwrap_or(usize::MAX))
            {
                if let Some(popup) = popover.upgrade() {
                    popup.popdown();
                }
                w.save_work_field(space, id, "parent-id", Some(Value::Link(*parent)));
            }
        });
        let (w, popover, id) = (self.clone(), popup.downgrade(), note);
        detach.connect_clicked(move |_| {
            if let Some(popup) = popover.upgrade() {
                popup.popdown();
            }
            w.save_work_field(space, id, "parent-id", None);
        });
        content
    }

    fn task_actions(&self, space: Id, item: &Item, tree: &Tree) -> gtk::MenuButton {
        let note = item.note.id;
        let button = gtk::MenuButton::builder().icon_name("view-more-symbolic").build();
        button.add_css_class("flat");
        button.set_tooltip_text(Some("Task actions"));
        let menu = gio::Menu::new();
        let dates: Vec<_> = [("Add due date", "due-on"), ("Add schedule date", "scheduled-on")]
            .into_iter()
            .filter(|(_, field)| tree.date(note, field).is_none())
            .collect();
        for (label, field) in &dates {
            append_picker_menu(&menu, label, field);
        }
        append_picker_menu(&menu, "Choose parent…", "parent-id");
        menu.append(Some("Delete task…"), Some("task.delete"));
        let popup = gtk::PopoverMenu::from_model(Some(&menu));
        self.track_work_popover(popup.upcast_ref());
        popup.connect_show(|popup| popup.set_visible_submenu(Some("main")));
        for field in dates.into_iter().map(|(_, field)| field).chain(["parent-id"]) {
            let page = self.task_picker_page(&popup, space, note, field);
            assert!(popup.add_child(&page, field));
        }
        let actions = gio::SimpleActionGroup::new();
        let delete = gio::SimpleAction::new("delete", None);
        let (w, popover) = (self.clone(), popup.downgrade());
        delete.connect_activate(move |_, _| {
            if let Some(popup) = popover.upgrade() {
                popup.popdown();
            }
            w.delete_note_at(space, note);
        });
        actions.add_action(&delete);
        popup.insert_action_group("task", Some(&actions));
        button.set_popover(Some(&popup));
        button
    }

    fn task_picker_page(
        &self,
        popup: &gtk::PopoverMenu,
        space: Id,
        note: Id,
        field: &'static str,
    ) -> gtk::Box {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (w, popover) = (self.clone(), popup.downgrade());
        // Build each picker only when its submenu opens, preserving its input on return visits.
        page.connect_map(move |page| {
            if page.first_child().is_none() {
                let Some(popup) = popover.upgrade() else { return };
                let content = if field == "parent-id" {
                    w.parent_content(space, note, popup.upcast_ref())
                } else {
                    let picker = {
                        let tree = w.inner().work.tree.borrow();
                        let Some(item) = tree.items.get(&note) else { return };
                        date_picker(item, &tree, field)
                    };
                    w.date_content(space, &picker, popup.upcast_ref())
                };
                page.append(&content);
            }
            if let Some(focus) = page.first_child().and_then(|content| content.first_child()) {
                let focus = focus.downgrade();
                // Let the native menu finish focusing its navigation row before focusing the input.
                glib::idle_add_local_once(move || {
                    if let Some(focus) = focus.upgrade().filter(gtk::Widget::is_mapped) {
                        focus.grab_focus();
                    }
                });
            }
        });
        page
    }

    fn track_work_popover(&self, popup: &gtk::Popover) {
        let counted = Rc::new(Cell::new(false));
        let (w, shown) = (self.clone(), counted.clone());
        popup.connect_show(move |_| {
            if !shown.replace(true) {
                w.inner().work.popovers.set(w.inner().work.popovers.get() + 1);
            }
        });
        let w = self.clone();
        popup.connect_closed(move |_| {
            let work = &w.inner().work;
            if counted.replace(false) {
                work.popovers.set(work.popovers.get().saturating_sub(1));
            }
            if work.popovers.get() == 0 && work.refresh_pending.replace(false) {
                w.refresh_work();
            }
        });
    }
}

fn date_picker(item: &Item, tree: &Tree, field: &'static str) -> DatePicker {
    let date = tree.date(item.note.id, field);
    DatePicker {
        explicit: item.explicit_date(field).is_some(),
        field,
        hint: date.as_ref().filter(|date| date.source != item.note.id).map_or(
            "Clearing an override resumes inheritance.".into(),
            |date| {
                format!(
                    "Inherited from {}. Clearing an override resumes inheritance.",
                    tree.items[&date.source].title
                )
            },
        ),
        note: item.note.id,
        value: date.map(|date| date.value),
    }
}

fn append_picker_menu(menu: &gio::Menu, label: &str, name: &str) {
    let submenu = gio::Menu::new();
    let item = gio::MenuItem::new(None, None);
    item.set_attribute_value("custom", Some(&name.to_variant()));
    submenu.append_item(&item);
    menu.append_submenu(Some(label), &submenu);
}

fn insert_control(controls: &gtk::FlowBox, widget: &impl IsA<gtk::Widget>) {
    controls.insert(widget, -1);
    if let Some(child) = widget.as_ref().parent() {
        child.add_css_class("zib-work-control");
    }
}

const CSS: &str = "
flowboxchild.zib-work-control:hover,
menubutton.zib-work-quiet > button:hover {
    background-color: transparent;
    background-image: none;
    box-shadow: none;
}
menubutton.zib-work-parent > button image {
    -gtk-icon-transform: scaleX(-1);
}
button.zib-subproject-link {
    padding-left: 0;
    padding-right: 0;
}
button.zib-task-expander {
    min-width: 16px;
    padding-left: 0;
    padding-right: 0;
}
checkbutton.zib-task-completion {
    padding-left: 0;
    padding-right: 0;
}
flowbox.zib-task-controls > flowboxchild {
    padding: 0;
}
box.zib-task-after {
    box-shadow: inset 0 -2px @accent_color;
}
box.zib-task-before {
    box-shadow: inset 0 2px @accent_color;
}
box.zib-task-subtask {
    background-color: alpha(@accent_bg_color, 0.15);
    border-radius: 4px;
    box-shadow: inset 0 0 0 1px @accent_color;
}
";

fn style() {
    thread_local! {
        static ADDED: Cell<bool> = const { Cell::new(false) };
    }
    if ADDED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn project_candidates(tree: &Tree, note: Id) -> Vec<(Id, String)> {
    let mut candidates: Vec<_> = tree
        .items
        .iter()
        .filter(|(id, item)| {
            **id != note
                && item.note.type_name == schema::PROJECT
                && !tree.ancestors(**id).contains(&note)
        })
        .map(|(id, item)| (*id, item.title.clone()))
        .collect();
    candidates
        .sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    candidates
}

fn select_date(calendar: &gtk::Calendar, date: &str) {
    if let Ok(date) = glib::DateTime::from_iso8601(&format!("{date}T12:00:00Z"), None) {
        calendar.select_day(&date);
    }
}
