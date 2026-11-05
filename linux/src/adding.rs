//! The Add machine screen, as the pairing specification describes it: this machine's address,
//! nearby machines, pairing by address with a code to compare, and invites as links and QR codes.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, glib};
use tokio::sync::oneshot;
use zib_core::Id;
use zib_core::address::{Address, MachineKey};
use zib_core::net::NearbyEvent;
use zib_core::pairing::Party;

use crate::app::core;
use crate::state::State;
use crate::window::Window;

/// Shows the Add machine screen. While it's open, this machine answers pairing requests and
/// announces itself nearby.
// The screen's widgets read best built in one place.
#[allow(clippy::too_many_lines)]
pub fn show(window: &Window, state: Arc<State>) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();

    let address = state.address();
    let own = adw::PreferencesGroup::builder()
        .title("This machine")
        .description("Another machine can pair with this one by typing this address.")
        .build();
    let address_label = gtk::Label::builder()
        .label(address.key.to_display())
        .selectable(true)
        .wrap(true)
        .xalign(0.0)
        .build();
    address_label.add_css_class("monospace");
    own.add(&address_label);
    content.append(&own);

    let nearby = adw::PreferencesGroup::builder()
        .title("Nearby")
        .description("Machines on this network with their Add machine screen open.")
        .build();
    let nearby_list = gtk::ListBox::new();
    nearby_list.add_css_class("boxed-list");
    nearby.add(&nearby_list);
    content.append(&nearby);

    let by_address = adw::PreferencesGroup::builder().title("Pair by address").build();
    let address_entry = adw::EntryRow::builder().title("The other machine's address").build();
    let pair_button = gtk::Button::builder().label("Pair").valign(gtk::Align::Center).build();
    address_entry.add_suffix(&pair_button);
    by_address.add(&address_entry);
    content.append(&by_address);

    let invites = adw::PreferencesGroup::builder()
        .title("Invite")
        .description("A link that works once, for 15 minutes, for a machine on another network.")
        .build();
    let invite_button = gtk::Button::builder()
        .label("Make an invite to this space")
        .halign(gtk::Align::Start)
        .build();
    let invite_link = gtk::Label::builder()
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::Char)
        .xalign(0.0)
        .build();
    invite_link.add_css_class("monospace");
    let invite_qr = gtk::Picture::new();
    invite_qr.set_size_request(220, 220);
    invite_qr.set_visible(false);
    invites.add(&invite_button);
    invites.add(&invite_link);
    invites.add(&invite_qr);
    content.append(&invites);

    let join = adw::PreferencesGroup::builder().title("Join by invite").build();
    let join_entry = adw::EntryRow::builder().title("An invite link from another machine").build();
    let join_button = gtk::Button::builder().label("Join").valign(gtk::Align::Center).build();
    join_entry.add_suffix(&join_button);
    join.add(&join_entry);
    content.append(&join);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar
        .set_content(Some(&gtk::ScrolledWindow::builder().child(&content).vexpand(true).build()));
    let dialog =
        adw::Dialog::builder().title("Add machine").content_width(560).content_height(720).build();
    dialog.set_child(Some(&toolbar));

    // Nearby machines, as their announcements come and go.
    let found: Rc<RefCell<BTreeMap<MachineKey, (Address, gtk::ListBoxRow)>>> = Rc::default();
    {
        let state = state.clone();
        let found = found.clone();
        let list = nearby_list.clone();
        let window = window.clone();
        glib::spawn_future_local(async move {
            let receiver = core({
                let state = state.clone();
                async move { state.set_adding(true).await }
            })
            .await;
            let Some(mut receiver) = receiver else { return };
            while let Some(event) = receiver.recv().await {
                match event {
                    NearbyEvent::Found(machine) => {
                        if found.borrow().contains_key(&machine.address.key) {
                            continue;
                        }
                        let row = adw::ActionRow::builder()
                            .title(glib::markup_escape_text(if machine.name.is_empty() {
                                "A machine"
                            } else {
                                &machine.name
                            }))
                            .subtitle(machine.address.key.to_display())
                            .build();
                        let button =
                            gtk::Button::builder().label("Pair").valign(gtk::Align::Center).build();
                        let (state, window, address) =
                            (state.clone(), window.clone(), machine.address.clone());
                        button.connect_clicked(move |_| {
                            pair_with(&window, state.clone(), address.clone());
                        });
                        row.add_suffix(&button);
                        list.append(&row);
                        found
                            .borrow_mut()
                            .insert(machine.address.key, (machine.address, row.upcast()));
                    }
                    NearbyEvent::Lost(key) => {
                        if let Some((_, row)) = found.borrow_mut().remove(&key) {
                            list.remove(&row);
                        }
                    }
                }
            }
        });
    }

    {
        let (state, window, entry) = (state.clone(), window.clone(), address_entry.clone());
        pair_button.connect_clicked(move |_| match Address::parse(&entry.text()) {
            Ok(address) => pair_with(&window, state.clone(), address),
            Err(_) => window.toast("That isn't a machine address"),
        });
    }
    {
        let (state, window) = (state.clone(), window.clone());
        invite_button.connect_clicked(move |_| {
            let Some(space) = window_space(&window) else {
                window.toast("Pick a space first");
                return;
            };
            match state.invite(space) {
                Ok(invite) => {
                    let uri = invite.uri();
                    invite_link.set_label(&uri);
                    if let Some(texture) = qr_texture(&uri.to_uppercase()) {
                        invite_qr.set_paintable(Some(&texture));
                        invite_qr.set_visible(true);
                    }
                }
                Err(error) => window.toast(&format!("Couldn't make an invite: {error}")),
            }
        });
    }
    {
        let (state, window, entry) = (state.clone(), window.clone(), join_entry.clone());
        join_button.connect_clicked(move |_| {
            let uri = entry.text().trim().to_owned();
            let (state, window) = (state.clone(), window.clone());
            glib::spawn_future_local(async move {
                match core(async move { state.join(&uri).await }).await {
                    Ok(space) => {
                        window.toast("Joined the space");
                        window.space_changed(space);
                    }
                    Err(error) => window.toast(&format!("Couldn't join: {error}")),
                }
            });
        });
    }
    dialog.connect_closed(move |_| {
        let state = state.clone();
        glib::spawn_future_local(async move {
            core(async move { state.set_adding(false).await }).await;
        });
    });
    dialog.present(Some(&window.gtk_window()));
}

fn window_space(window: &Window) -> Option<Id> {
    window.current_space_id()
}

/// Pairs with a machine this one picked: asks which spaces to share, shows the code, and waits for
/// the owner to compare it with the other screen.
fn pair_with(window: &Window, state: Arc<State>, address: Address) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let spaces = core({
            let state = state.clone();
            async move { state.spaces().await }
        })
        .await;
        let Some(share) = choose_spaces(
            &window,
            "Pair with this machine?",
            "Pick the spaces to share with it.",
            &spaces,
        )
        .await
        else {
            return;
        };
        let (code_sender, code) = oneshot::channel();
        let (confirm, confirmed) = oneshot::channel();
        let pairing = core({
            let state = state.clone();
            async move { state.pair(address, code_sender, confirmed, share).await }
        });
        let compare = {
            let window = window.clone();
            async move {
                let Ok(code) = code.await else { return };
                let dialog = adw::AlertDialog::new(
                    Some(&format!("Code {code}")),
                    Some("Does the other machine show the same code? Confirm there too."),
                );
                dialog.add_responses(&[("cancel", "They differ"), ("ok", "They match")]);
                dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
                let matched = dialog.choose_future(Some(&window.gtk_window())).await == "ok";
                let _ = confirm.send(matched);
            }
        };
        glib::spawn_future_local(compare);
        match pairing.await {
            Ok(()) => window.space_changed(Id::from_bytes([0; 16])),
            Err(error) => window.toast(&format!("Pairing didn't finish: {error}")),
        }
    });
}

/// Answers another machine's request: shows its name and the code, and asks which spaces to share.
pub fn confirm_request(
    window: Option<&Window>,
    state: Arc<State>,
    peer: Party,
    code: String,
    reply: oneshot::Sender<Option<Vec<(Id, String)>>>,
) {
    let window = window.cloned();
    glib::spawn_future_local(async move {
        let spaces = core(async move { state.spaces().await }).await;
        let answer = match &window {
            Some(window) => {
                let heading = format!("Pair with {}?", peer.name);
                let body =
                    format!("Code {code}. Confirm only if the other machine shows the same code.");
                choose_spaces(window, &heading, &body, &spaces).await
            }
            None => None,
        };
        let _ = reply.send(answer);
    });
}

/// Asks the owner to confirm, with a checkbox for each space to share, and returns the spaces
/// chosen with their pet names, which the other machine suggests to its owner.
async fn choose_spaces(
    window: &Window,
    heading: &str,
    body: &str,
    spaces: &[(Id, String)],
) -> Option<Vec<(Id, String)>> {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let checks: Vec<(Id, String, gtk::CheckButton)> = spaces
        .iter()
        .map(|(id, name)| {
            let check = gtk::CheckButton::with_label(name);
            list.append(&check);
            (*id, name.clone(), check)
        })
        .collect();
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", "Pair")]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_extra_child(Some(&list));
    let response = dialog.choose_future(Some(&window.gtk_window())).await;
    (response == "ok").then(|| {
        checks
            .iter()
            .filter(|(_, _, c)| c.is_active())
            .map(|(id, name, _)| (*id, name.clone()))
            .collect()
    })
}

/// A QR code as a texture, with a quiet zone of four modules and eight pixels a module.
fn qr_texture(text: &str) -> Option<gdk::Texture> {
    let (width, colors) = zib_core::pairing::qr_modules(text)?;
    let scale = 8;
    let size = (width + 8) * scale;
    let mut pixels = vec![255u8; size * size * 3];
    for (i, color) in colors.iter().enumerate() {
        if !*color {
            continue;
        }
        let (x, y) = ((i % width + 4) * scale, (i / width + 4) * scale);
        for dy in 0..scale {
            for dx in 0..scale {
                let at = ((y + dy) * size + x + dx) * 3;
                pixels[at..at + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    let edge = i32::try_from(size).ok()?;
    let texture = gdk::MemoryTexture::new(
        edge,
        edge,
        gdk::MemoryFormat::R8g8b8,
        &glib::Bytes::from(&pixels),
        size * 3,
    );
    Some(texture.upcast())
}
