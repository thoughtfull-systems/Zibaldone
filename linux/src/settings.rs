//! The Settings screen starts with agent preferences and can grow other preference pages.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use zib_core::agent::Settings;
use zib_core::{Id, agent};

use crate::app::core;
use crate::state::State;
use crate::window::Window;

/// An editable model selector accepts account models and manually entered identifiers.
#[derive(Clone)]
pub(crate) struct ModelPicker {
    pub widget: gtk::Box,
    entry: gtk::Entry,
    list: gtk::ListBox,
}

impl ModelPicker {
    pub fn connect_changed(&self, f: impl Fn(&Self) + 'static) {
        let picker = self.clone();
        self.entry.connect_changed(move |_| f(&picker));
    }

    pub fn has_focus(&self) -> bool {
        self.entry.has_focus()
    }
}

pub(crate) fn model_picker() -> ModelPicker {
    let entry = gtk::Entry::builder().placeholder_text("Model identifier").hexpand(true).build();
    let list = gtk::ListBox::new();
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .min_content_width(260)
        .max_content_height(320)
        .propagate_natural_height(true)
        .build();
    let popover = gtk::Popover::builder().child(&scroll).build();
    let menu = gtk::MenuButton::builder()
        .icon_name("pan-down-symbolic")
        .tooltip_text("Available models; use Load models to refresh")
        .popover(&popover)
        .build();
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    widget.add_css_class("linked");
    widget.append(&entry);
    widget.append(&menu);
    let selected = entry.clone();
    list.connect_row_activated(move |_, row| {
        if let Some(label) = row.child().and_downcast::<gtk::Label>() {
            selected.set_text(&label.text());
            popover.popdown();
        }
    });
    ModelPicker { widget, entry, list }
}

pub(crate) fn set_model(picker: &ModelPicker, model: &str) {
    picker.entry.set_text(model);
}

pub(crate) fn selected_model(picker: &ModelPicker) -> String {
    picker.entry.text().trim().to_owned()
}

pub(crate) async fn load_models(picker: &ModelPicker, key: String) -> Result<(), String> {
    let models = core(async move { agent::models(&key).await }).await.map_err(|e| e.to_string())?;
    picker.list.remove_all();
    for model in models {
        picker.list.append(
            &gtk::Label::builder()
                .label(&model)
                .xalign(0.0)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(8)
                .margin_end(8)
                .build(),
        );
    }
    Ok(())
}

// Build the preference controls and their handlers together, as in the main window.
#[allow(clippy::too_many_lines)]
pub fn show(window: &Window, state: Arc<State>) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let settings = core({
            let state = state.clone();
            async move { state.agent_settings().await }
        })
        .await;
        let dialog = adw::PreferencesDialog::builder().title("Settings").build();
        let page = adw::PreferencesPage::builder()
            .icon_name("user-available-symbolic")
            .title("Agent")
            .build();
        let group = adw::PreferencesGroup::builder()
            .title("Agent")
            .description("These settings stay on this machine. Prompt changes apply to a new chat.")
            .build();
        let key = adw::PasswordEntryRow::builder().title("OpenAI API key").build();
        key.set_text(&settings.api_key);
        group.add(&key);

        let model = model_picker();
        set_model(&model, &settings.model);
        let model_row = adw::ActionRow::builder().title("Model").build();
        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Load models from OpenAI")
            .valign(gtk::Align::Center)
            .build();
        model_row.add_suffix(&model.widget);
        model_row.add_suffix(&refresh);
        group.add(&model_row);
        let (loading_model, loading_key, w) = (model.clone(), key.clone(), window.clone());
        refresh.connect_clicked(move |button| {
            button.set_sensitive(false);
            let (model, key, w, button) =
                (loading_model.clone(), loading_key.text().to_string(), w.clone(), button.clone());
            glib::spawn_future_local(async move {
                if let Err(error) = load_models(&model, key).await {
                    w.toast(&error);
                }
                button.set_sensitive(true);
            });
        });

        let prompt = adw::EntryRow::builder().title("System prompt note ID (optional)").build();
        prompt.set_text(&settings.system_prompt_note.map(|id| id.to_string()).unwrap_or_default());
        let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
        let open = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open the prompt note")
            .valign(gtk::Align::Center)
            .build();
        prompt.add_suffix(&choose);
        prompt.add_suffix(&open);
        group.add(&prompt);
        let (w, s, entry) = (window.clone(), state.clone(), prompt.clone());
        choose.connect_clicked(move |_| {
            let (w, s, entry) = (w.clone(), s.clone(), entry.clone());
            glib::spawn_future_local(async move {
                if let Some(id) = choose_prompt(&w, s).await {
                    entry.set_text(&id.to_string());
                }
            });
        });
        let (w, closing, entry) = (window.clone(), dialog.clone(), prompt.clone());
        open.connect_clicked(move |_| {
            if let Ok(id) = Id::parse(entry.text().trim()) {
                closing.close();
                w.open_uri(&format!("zib:note/{id}"));
            } else {
                w.toast("Choose a prompt note first");
            }
        });
        let save =
            gtk::Button::builder().label("Save").halign(gtk::Align::End).margin_top(12).build();
        save.add_css_class("suggested-action");
        let saving = Rc::new(Cell::new(false));
        let (w, closing) = (window.clone(), dialog.clone());
        save.connect_clicked(move |_| {
            if saving.replace(true) {
                return;
            }
            let note = prompt.text();
            let prompt_note = if note.trim().is_empty() {
                None
            } else if let Ok(id) = Id::parse(note.trim()) {
                Some(id)
            } else {
                w.toast("The system prompt needs a valid note ID");
                saving.set(false);
                return;
            };
            let settings = Settings {
                api_key: key.text().trim().into(),
                model: selected_model(&model),
                system_prompt_note: prompt_note,
            };
            let (state, w, closing, saving) =
                (state.clone(), w.clone(), closing.clone(), saving.clone());
            glib::spawn_future_local(async move {
                match core(async move { state.set_agent_settings(settings).await }).await {
                    Ok(()) => {
                        closing.close();
                    }
                    Err(error) => w.toast(&format!("Couldn't save Settings: {error}")),
                }
                saving.set(false);
            });
        });
        group.add(&save);
        page.add(&group);
        dialog.add(&page);
        dialog.present(Some(&window.gtk_window()));
    });
}

async fn choose_prompt(window: &Window, state: Arc<State>) -> Option<Id> {
    let notes = core(async move {
        let mut notes = Vec::new();
        for (space, name) in state.spaces().await {
            for note in state.notes(space).await.unwrap_or_default() {
                notes.push((note.id, format!("{} — {name}", note.plain)));
            }
        }
        notes.sort_by_key(|(_, label)| label.to_lowercase());
        notes
    })
    .await;
    let names: Vec<&str> = notes.iter().map(|(_, name)| name.as_str()).collect();
    let picker = gtk::DropDown::from_strings(&names);
    picker.set_enable_search(true);
    let dialog = adw::AlertDialog::new(Some("System prompt note"), None);
    dialog.add_responses(&[("cancel", "Cancel"), ("choose", "Choose")]);
    dialog.set_response_enabled("choose", !notes.is_empty());
    dialog.set_extra_child(Some(&picker));
    if dialog.choose_future(Some(&window.gtk_window())).await == "choose" {
        notes.get(usize::try_from(picker.selected()).ok()?).map(|(id, _)| *id)
    } else {
        None
    }
}
