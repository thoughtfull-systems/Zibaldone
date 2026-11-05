//! Sharing spaces with the machines this one paired with, and moving notes between spaces, as
//! decision ZIB0016 describes. To stop sharing with a machine, the owner moves the notes to a new
//! space and shares that one with the machines to keep.

use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use zib_core::Id;

use crate::app::core;
use crate::state::State;
use crate::window::Window;

/// What the owner reads when a machine didn't answer an offer, which it hears again at the next
/// sync.
const WAITING: &str = "The space waits for the other machine, which didn't answer";

/// A drop-down of the machine's spaces, with `current` picked.
fn space_picker(spaces: &[(Id, String)], current: Option<Id>) -> gtk::DropDown {
    let names: Vec<&str> = spaces.iter().map(|(_, name)| name.as_str()).collect();
    let picker = gtk::DropDown::from_strings(&names);
    let index = spaces.iter().position(|(id, _)| Some(*id) == current).unwrap_or(0);
    picker.set_selected(u32::try_from(index).unwrap_or(0));
    picker
}

fn picked(picker: &gtk::DropDown, spaces: &[(Id, String)]) -> Option<Id> {
    spaces.get(usize::try_from(picker.selected()).ok()?).map(|(id, _)| *id)
}

/// Shows the machines this one paired with for a space, and shares the space with any of them.
pub fn share(window: &Window, state: Arc<State>) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let spaces = core({
            let state = state.clone();
            async move { state.spaces().await }
        })
        .await;
        if spaces.is_empty() {
            return;
        }
        let picker = space_picker(&spaces, window.current_space_id());
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("boxed-list");
        let fields = gtk::Box::new(gtk::Orientation::Vertical, 12);
        fields.append(&picker);
        fields.append(&list);
        let fill = {
            let (list, state, window, spaces) =
                (list.clone(), state.clone(), window.clone(), spaces.clone());
            move |picker: &gtk::DropDown| {
                let Some(space) = picked(picker, &spaces) else { return };
                fill_machines(&list, &window, state.clone(), space);
            }
        };
        fill(&picker);
        picker.connect_selected_notify(fill);
        let dialog = adw::AlertDialog::new(
            Some("Share space"),
            Some("Machines you paired with receive the space once you share it."),
        );
        dialog.add_response("close", "Close");
        dialog.set_extra_child(Some(&fields));
        dialog.choose_future(Some(&window.gtk_window())).await;
    });
}

/// Lists the paired machines for `space`, each with a button to share it, or a note that it does.
fn fill_machines(list: &gtk::ListBox, window: &Window, state: Arc<State>, space: Id) {
    let (list, window) = (list.clone(), window.clone());
    glib::spawn_future_local(async move {
        let machines = core({
            let state = state.clone();
            async move { state.sharing(space).await.unwrap_or_default() }
        })
        .await;
        list.remove_all();
        if machines.is_empty() {
            let row = adw::ActionRow::builder()
                .title("No paired machines")
                .subtitle("Pair with a machine through Add machine first.")
                .build();
            list.append(&row);
        }
        for (party, member) in machines {
            let row =
                adw::ActionRow::builder().title(glib::markup_escape_text(&party.name)).build();
            row.set_subtitle(&party.key.to_display());
            if member {
                row.add_suffix(&gtk::Label::new(Some("Shared")));
            } else {
                let button =
                    gtk::Button::builder().label("Share").valign(gtk::Align::Center).build();
                let (state, window, list) = (state.clone(), window.clone(), list.clone());
                let machine = party.machine;
                button.connect_clicked(move |button| {
                    button.set_sensitive(false);
                    let (state, window, list) = (state.clone(), window.clone(), list.clone());
                    glib::spawn_future_local(async move {
                        let shared = core({
                            let state = state.clone();
                            async move { state.share_space(space, machine).await }
                        })
                        .await;
                        if let Err(error) = shared {
                            window.toast(&format!("{WAITING}: {error}"));
                        }
                        fill_machines(&list, &window, state, space);
                    });
                });
                row.add_suffix(&button);
            }
            list.append(&row);
        }
    });
}

/// Moves every note of one space to another, after the owner picks both.
pub fn move_all(window: &Window, state: Arc<State>) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let spaces = core({
            let state = state.clone();
            async move { state.spaces().await }
        })
        .await;
        if spaces.len() < 2 {
            window.toast("Make another space to move the notes to first");
            return;
        }
        let current = window.current_space_id();
        let from = space_picker(&spaces, current);
        let other = spaces.iter().map(|(id, _)| *id).find(|id| Some(*id) != current);
        let to = space_picker(&spaces, other);
        let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
        fields.append(&gtk::Label::builder().label("From").halign(gtk::Align::Start).build());
        fields.append(&from);
        fields.append(&gtk::Label::builder().label("To").halign(gtk::Align::Start).build());
        fields.append(&to);
        let dialog = adw::AlertDialog::new(
            Some("Move notes"),
            Some("Every note moves with its files, on every machine that shares the space."),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("move", "Move")]);
        dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        dialog.set_extra_child(Some(&fields));
        if dialog.choose_future(Some(&window.gtk_window())).await != "move" {
            return;
        }
        let (Some(from), Some(to)) = (picked(&from, &spaces), picked(&to, &spaces)) else {
            return;
        };
        if from == to {
            window.toast("Pick two different spaces");
            return;
        }
        let moved = core(async move { state.move_notes(from, to).await }).await;
        match moved {
            Ok(notes) => {
                window.notes_moved(from, to, &notes);
                window.toast(&format!("Moved {} notes", notes.len()));
            }
            Err(error) => window.toast(&format!("Couldn't move every note: {error}")),
        }
    });
}

/// Moves one note to another space that the owner picks.
pub fn move_note(window: &Window, state: Arc<State>, space: Id, note: Id) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let spaces = core({
            let state = state.clone();
            async move { state.spaces().await }
        })
        .await;
        let spaces: Vec<(Id, String)> = spaces.into_iter().filter(|(id, _)| *id != space).collect();
        if spaces.is_empty() {
            window.toast("Make another space to move the note to first");
            return;
        }
        let to = space_picker(&spaces, None);
        let dialog = adw::AlertDialog::new(
            Some("Move note"),
            Some("The note moves with its files, on every machine that shares the space."),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("move", "Move")]);
        dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        dialog.set_extra_child(Some(&to));
        if dialog.choose_future(Some(&window.gtk_window())).await != "move" {
            return;
        }
        let Some(to) = picked(&to, &spaces) else { return };
        match core(async move { state.move_note(space, to, note).await }).await {
            Ok(()) => window.notes_moved(space, to, &[note]),
            Err(error) => window.toast(&format!("Couldn't move the note: {error}")),
        }
    });
}
