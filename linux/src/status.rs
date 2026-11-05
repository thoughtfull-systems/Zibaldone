//! The sync details that the status icon opens, as the sync specification's sync status
//! describes: each space's state, and when each other member last synced.

use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use zib_core::sync::SyncState;

use crate::app::core;
use crate::state::{SpaceSync, State};

/// The popover of sync details, which fills in again each time it opens.
pub fn popover(state: Arc<State>) -> gtk::Popover {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
    list.set_size_request(340, -1);
    let scroller = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(480)
        .build();
    let popover = gtk::Popover::builder().child(&scroller).build();
    popover.connect_show(move |_| {
        let (list, state) = (list.clone(), state.clone());
        glib::spawn_future_local(async move {
            let details = core(async move { state.sync_details().await }).await;
            fill(&list, &details);
        });
    });
    popover
}

fn fill(list: &gtk::Box, details: &[SpaceSync]) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    if details.is_empty() {
        list.append(&gtk::Label::new(Some("No spaces yet")));
    }
    let now = zib_core::time::now();
    for space in details {
        let group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(&space.name))
            .description(describe(space.state))
            .build();
        for member in &space.members {
            let synced = member.last.map_or_else(
                || "Never synced".to_owned(),
                |last| format!("Synced {}", ago(now - last)),
            );
            let holds = if member.lacks { "lacks changes made here" } else { "has every change" };
            let address = member.key.to_display();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&member.name))
                .subtitle(format!("{synced}, {holds}\n{}…", &address[..14]))
                .build();
            let icon =
                if member.lacks { "emblem-synchronizing-symbolic" } else { "emblem-ok-symbolic" };
            row.add_prefix(&gtk::Image::from_icon_name(icon));
            group.add(&row);
        }
        list.append(&group);
    }
}

fn describe(state: Option<SyncState>) -> &'static str {
    match state {
        None => "Not shared with another machine",
        Some(SyncState::Synced) => "Every machine has every change",
        Some(SyncState::PartlySynced) => "Some machines lack changes",
        Some(SyncState::NotSynced) => "Changes here haven't reached another machine",
    }
}

/// How long ago something happened, in words, from milliseconds.
fn ago(milliseconds: i64) -> String {
    let seconds = (milliseconds / 1000).max(0);
    match seconds {
        0..10 => "just now".to_owned(),
        10..60 => format!("{seconds} seconds ago"),
        60..120 => "a minute ago".to_owned(),
        120..3600 => format!("{} minutes ago", seconds / 60),
        3600..7200 => "an hour ago".to_owned(),
        7200..86_400 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}
