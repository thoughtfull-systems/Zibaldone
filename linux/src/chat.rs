//! The right-hand chat sidebar. Conversation state and requests belong to the core.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::Id;
use zib_core::agent::{Session, Status};
use zib_core::editor::{Body, Editor};

use crate::app::core;
use crate::editor_widget::EditorWidget;
use crate::settings::{ModelPicker, load_models, model_picker, selected_model, set_model};
use crate::state::State;
use crate::window::Window;

#[derive(Clone)]
pub struct Chat(Rc<Inner>);

struct Inner {
    state: Arc<State>,
    root: adw::ToolbarView,
    title: gtk::Button,
    model: ModelPicker,
    model_updating: Cell<bool>,
    saved_model: RefCell<String>,
    history: gtk::Box,
    scroll: gtk::ScrolledWindow,
    views: RefCell<Vec<TurnView>>,
    composer: EditorWidget,
    send: gtk::Button,
    stop: gtk::Button,
    retry: gtk::Button,
    new: gtk::Button,
    message: gtk::Label,
    session: Cell<Option<Id>>,
    initialized: Cell<bool>,
    focus_pending: Cell<bool>,
    busy: Cell<bool>,
    archiving: Cell<bool>,
    draft_timer: RefCell<Option<glib::SourceId>>,
}

struct TurnView {
    assistant_heading: gtk::Label,
    reply: EditorWidget,
    tools: EditorWidget,
    user: EditorWidget,
}

impl Chat {
    pub fn new(state: Arc<State>) -> Self {
        let root = adw::ToolbarView::new();
        root.set_width_request(320);
        root.set_visible(false);
        let title = gtk::Button::with_label("New chat");
        if let Some(label) = title.child().and_downcast::<gtk::Label>() {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(24);
        }
        title.add_css_class("flat");
        title.set_tooltip_text(Some("Rename this chat"));
        let new = gtk::Button::from_icon_name("list-add-symbolic");
        new.set_tooltip_text(Some("Archive this chat and start a new one"));
        let header = adw::HeaderBar::new();
        header.set_show_end_title_buttons(false);
        header.set_show_start_title_buttons(false);
        header.set_title_widget(Some(&title));
        header.pack_start(&new);
        root.add_top_bar(&header);
        let model = model_picker();
        let history = gtk::Box::new(gtk::Orientation::Vertical, 16);
        history.set_margin_start(12);
        history.set_margin_end(12);
        history.set_margin_top(12);
        history.set_margin_bottom(12);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&history)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let composer = EditorWidget::default();
        composer.set_sensitive(false);
        model.widget.set_sensitive(false);
        composer.set_editor(Some(Editor::new(Body::detached(""))));
        composer.set_tooltip_text(Some("Write markdown; Ctrl+Enter sends"));
        let draft = gtk::ScrolledWindow::builder()
            .child(&composer)
            .min_content_height(140)
            .max_content_height(280)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let message =
            gtk::Label::builder().wrap(true).xalign(0.0).margin_start(12).margin_end(12).build();
        message.add_css_class("dim-label");
        let send = gtk::Button::with_label("Send");
        send.set_tooltip_text(Some("Send (Ctrl+Enter)"));
        send.add_css_class("suggested-action");
        let stop = gtk::Button::with_label("Stop");
        let retry = gtk::Button::with_label("Retry");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_end(12);
        buttons.set_margin_bottom(12);
        buttons.append(&retry);
        buttons.append(&stop);
        buttons.append(&send);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.append(&model.widget);
        content.append(&scroll);
        content.append(&message);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&draft);
        content.append(&buttons);
        root.set_content(Some(&content));
        Self(Rc::new(Inner {
            state,
            root,
            title,
            model,
            model_updating: Cell::new(false),
            saved_model: RefCell::default(),
            history,
            scroll,
            views: RefCell::default(),
            composer,
            send,
            stop,
            retry,
            new,
            message,
            session: Cell::new(None),
            initialized: Cell::new(false),
            focus_pending: Cell::new(false),
            busy: Cell::new(false),
            archiving: Cell::new(false),
            draft_timer: RefCell::default(),
        }))
    }

    pub fn widget(&self) -> &adw::ToolbarView {
        &self.0.root
    }

    #[cfg(test)]
    pub(crate) fn composer(&self) -> &EditorWidget {
        &self.0.composer
    }

    pub fn toggle(&self) {
        if self.0.root.is_visible() {
            self.0.root.set_visible(false);
        } else {
            self.focus();
        }
    }

    pub fn focus(&self) {
        self.0.root.set_visible(true);
        self.0.focus_pending.set(!self.0.composer.grab_focus());
    }

    pub fn has_focus(&self) -> bool {
        self.0.root.focus_child().is_some()
    }

    pub(crate) fn refresh_resources(&self, window: &Window) {
        window.reload_aux_editor(&self.0.composer);
        for view in self.0.views.borrow().iter() {
            for widget in [&view.reply, &view.tools, &view.user] {
                window.reload_aux_editor(widget);
            }
        }
    }

    // Keep the sidebar's controls and their signal handlers together, as in the main window.
    #[allow(clippy::too_many_lines)]
    pub fn connect(&self, window: Window) {
        window.connect_aux_editor(&self.0.composer, true);
        let (chat, w) = (self.clone(), window.clone());
        self.0.composer.connect_edited(move || {
            chat.0.state.agent.edit_draft(chat.0.composer.text().unwrap_or_default());
            w.refresh_aux_editor(&chat.0.composer);
            chat.schedule_draft(w.clone());
        });
        let (chat, w) = (self.clone(), window.clone());
        self.0.send.connect_clicked(move |_| chat.submit(w.clone(), false));
        let (chat, w) = (self.clone(), window.clone());
        self.0.retry.connect_clicked(move |_| chat.submit(w.clone(), true));
        let chat = self.clone();
        self.0.stop.connect_clicked(move |_| chat.0.state.agent.stop());
        let (chat, w) = (self.clone(), window.clone());
        self.0.new.connect_clicked(move |_| chat.archive(w.clone()));
        let (chat, w) = (self.clone(), window.clone());
        self.0.title.connect_clicked(move |_| chat.rename(w.clone()));

        let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings.set_tooltip_text(Some("Agent settings"));
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Load models from OpenAI"));
        let hide = gtk::Button::from_icon_name("sidebar-hide-symbolic");
        hide.set_tooltip_text(Some("Hide chat"));
        let tools = adw::HeaderBar::new();
        tools.set_show_start_title_buttons(false);
        tools.set_show_end_title_buttons(false);
        tools.pack_start(&settings);
        tools.pack_start(&refresh);
        tools.pack_end(&hide);
        self.0.root.add_bottom_bar(&tools);
        let (w, state) = (window.clone(), self.0.state.clone());
        settings.connect_clicked(move |_| crate::settings::show(&w, state.clone()));
        let chat = self.clone();
        hide.connect_clicked(move |_| chat.0.root.set_visible(false));
        let (chat, w) = (self.clone(), window.clone());
        refresh.connect_clicked(move |button| {
            button.set_sensitive(false);
            let (chat, w, button) = (chat.clone(), w.clone(), button.clone());
            glib::spawn_future_local(async move {
                let key = core({
                    let state = chat.0.state.clone();
                    async move { state.agent_settings().await.api_key }
                })
                .await;
                if let Err(error) = load_models(&chat.0.model, key).await {
                    w.toast(&error);
                }
                button.set_sensitive(true);
            });
        });
        let (chat, w) = (self.clone(), window.clone());
        self.0.model.connect_changed(move |picker| {
            if chat.0.model_updating.get() {
                return;
            }
            let model = selected_model(picker);
            chat.0.saved_model.borrow_mut().clone_from(&model);
            let (state, w) = (chat.0.state.clone(), w.clone());
            glib::spawn_future_local(async move {
                if let Err(error) = core(async move { state.set_agent_model(model).await }).await {
                    w.toast(&format!("Couldn't save the model: {error}"));
                }
            });
        });
        let keys = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let (chat, w) = (self.clone(), window.clone());
        keys.connect_key_pressed(move |_, key, _, state| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                && state & gtk::accelerator_get_default_mod_mask()
                    == gdk::ModifierType::CONTROL_MASK
            {
                chat.submit(w.clone(), false);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.0.composer.add_controller(keys);

        let chat = self.clone();
        glib::spawn_future_local(async move {
            loop {
                let (session, running, model) = core({
                    let state = chat.0.state.clone();
                    async move {
                        (
                            state.agent.snapshot(),
                            state.agent.is_running(),
                            state.agent_settings().await.model,
                        )
                    }
                })
                .await;
                chat.render(&window, &session, running);
                if *chat.0.saved_model.borrow() != model && !chat.0.model.has_focus() {
                    chat.0.model_updating.set(true);
                    set_model(&chat.0.model, &model);
                    chat.0.model_updating.set(false);
                    *chat.0.saved_model.borrow_mut() = model;
                }
                glib::timeout_future(Duration::from_millis(200)).await;
            }
        });
    }

    fn cancel_draft_timer(&self) {
        if let Some(timer) = self.0.draft_timer.take() {
            timer.remove();
        }
    }

    fn schedule_draft(&self, window: Window) {
        self.cancel_draft_timer();
        let chat = self.clone();
        *self.0.draft_timer.borrow_mut() =
            Some(glib::timeout_add_local_once(Duration::from_millis(150), move || {
                chat.0.draft_timer.take();
                let state = chat.0.state.clone();
                glib::spawn_future_local(async move {
                    if let Err(error) = core(async move { state.agent.save() }).await {
                        window.toast(&format!("Couldn't save the chat draft: {error}"));
                    }
                });
            }));
    }

    fn submit(&self, window: Window, retry: bool) {
        if !self.0.initialized.get() || self.0.busy.replace(true) {
            return;
        }
        self.cancel_draft_timer();
        let text = self.0.composer.text().unwrap_or_default();
        if !retry && text.trim().is_empty() {
            self.0.busy.set(false);
            return;
        }
        let submission = (!retry).then(|| (text.clone(), window.agent_context()));
        let model = selected_model(&self.0.model);
        let chat = self.clone();
        glib::spawn_future_local(async move {
            let state = chat.0.state.clone();
            let original_count = core({
                let state = state.clone();
                async move { state.agent.snapshot().turns.len() }
            })
            .await;
            if !retry {
                chat.0.composer.set_editor(Some(Editor::new(Body::detached(""))));
                chat.0.state.agent.edit_draft(String::new());
            }
            let result = core(async move {
                state.set_agent_model(model).await?;
                let settings = state.agent_settings().await;
                state.agent.run(&*state, settings, submission).await
            })
            .await;
            chat.0.busy.set(false);
            if let Err(error) = result {
                window.toast(&error.to_string());
                let accepted = core({
                    let state = chat.0.state.clone();
                    async move { state.agent.snapshot().turns.len() > original_count }
                })
                .await;
                if !retry && !accepted && chat.0.composer.text().unwrap_or_default().is_empty() {
                    chat.0.composer.set_editor(Some(Editor::new(Body::detached(&text))));
                    chat.0.state.agent.edit_draft(text);
                    chat.schedule_draft(window.clone());
                    window.refresh_aux_editor(&chat.0.composer);
                }
            }
            window.agent_notes_changed();
        });
    }

    fn archive(&self, window: Window) {
        if !self.0.initialized.get() || self.0.busy.replace(true) {
            return;
        }
        self.0.archiving.set(true);
        self.0.composer.set_sensitive(false);
        self.cancel_draft_timer();
        let (chat, text) = (self.clone(), self.0.composer.text().unwrap_or_default());
        glib::spawn_future_local(async move {
            let state = chat.0.state.clone();
            let snapshot = core({
                let state = state.clone();
                async move {
                    state.agent.set_draft(text)?;
                    Ok::<_, zib_core::Error>(state.agent.snapshot())
                }
            })
            .await;
            let result = match snapshot {
                Ok(session) => {
                    let destination = if let Some(creation) = session.archive {
                        creation.space
                    } else if session.is_empty() {
                        // No note gets created; the core ignores the destination for an empty chat.
                        Some(Id::generate())
                    } else {
                        let spaces = core({
                            let state = state.clone();
                            async move { state.space_list().await }
                        })
                        .await;
                        if let Some(space) = unambiguous_archive_space(&spaces) {
                            Some(space)
                        } else {
                            let choices: Vec<_> =
                                spaces.into_iter().map(|(id, name, _)| (id, name)).collect();
                            choose_destination(&window, &choices, &session.title).await
                        }
                    };
                    if let Some(space) = destination {
                        Some(core(async move { state.agent.archive(&*state, space).await }).await)
                    } else {
                        None
                    }
                }
                Err(error) => Some(Err(error)),
            };
            chat.0.busy.set(false);
            chat.0.archiving.set(false);
            if let Some(Err(error)) = result {
                window.toast(&format!("Couldn't archive the chat: {error}"));
            }
            window.agent_notes_changed();
        });
    }

    fn rename(&self, window: Window) {
        let entry = gtk::Entry::new();
        entry.set_text(&self.0.title.label().unwrap_or_default());
        let dialog = adw::AlertDialog::new(Some("Rename chat"), None);
        dialog.add_responses(&[("cancel", "Cancel"), ("rename", "Rename")]);
        dialog.set_default_response(Some("rename"));
        dialog.set_extra_child(Some(&entry));
        let state = self.0.state.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&window.gtk_window())).await == "rename" {
                let name = entry.text().to_string();
                if let Err(error) = core(async move { state.agent.rename(&name) }).await {
                    window.toast(&error.to_string());
                }
            }
        });
    }

    fn render(&self, window: &Window, session: &Session, running: bool) {
        if self.0.session.replace(Some(session.id)) != Some(session.id) {
            if self.0.initialized.get() {
                self.0.focus_pending.set(true);
            }
            while let Some(child) = self.0.history.first_child() {
                self.0.history.remove(&child);
            }
            self.0.views.borrow_mut().clear();
            self.0.composer.set_editor(Some(Editor::new(Body::detached(&session.draft))));
            window.refresh_aux_editor(&self.0.composer);
        }
        self.0.initialized.set(true);
        self.0.model.widget.set_sensitive(true);
        self.0.composer.set_sensitive(session.archive.is_none() && !self.0.archiving.get());
        if self.0.root.is_visible() && self.0.focus_pending.get() && self.0.composer.grab_focus() {
            self.0.focus_pending.set(false);
        }
        self.0.title.set_sensitive(session.archive.is_none());
        self.0.title.set_label(&session.title);
        let busy = running || self.0.busy.get();
        self.0.send.set_sensitive(!busy && session.archive.is_none());
        self.0.stop.set_sensitive(running);
        self.0.new.set_sensitive(!busy);
        self.0.retry.set_visible(session.can_retry() && !busy && session.archive.is_none());
        self.0.message.set_text(if busy {
            "Working…"
        } else {
            session.turns.last().and_then(|t| t.error.as_deref()).unwrap_or("Ctrl+Enter to send")
        });
        let adjustment = self.0.scroll.vadjustment();
        let follow = adjustment.value() + adjustment.page_size() >= adjustment.upper() - 40.0;
        let mut changed = false;
        let mut views = self.0.views.borrow_mut();
        for (index, turn) in session.turns.iter().enumerate() {
            if views.len() == index {
                let user = readonly(window, &turn.message);
                let reply = readonly(window, "");
                let tools = readonly(window, "");
                let heading = gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .build();
                heading.add_css_class("heading");
                let user_heading = gtk::Label::builder().label("You").xalign(0.0).build();
                user_heading.add_css_class("heading");
                for widget in [
                    user_heading.upcast::<gtk::Widget>(),
                    user.clone().upcast(),
                    heading.clone().upcast(),
                    reply.clone().upcast(),
                    tools.clone().upcast(),
                ] {
                    self.0.history.append(&widget);
                }
                views.push(TurnView { assistant_heading: heading, reply, tools, user });
                changed = true;
            }
            let view = &views[index];
            let models = if turn.reported_models.is_empty() {
                turn.model.clone()
            } else {
                turn.reported_models.join(", ")
            };
            view.assistant_heading.set_text(&format!("Assistant · {models}"));
            changed |= update_text(window, &view.reply, &turn.reply);
            let mut activity = String::new();
            for tool in &turn.tools {
                let _ = write!(activity, "**{}**", tool.name);
                let result = tool.result.as_ref();
                let id = if tool.name == "create_note" {
                    result.and_then(|r| r["id"].as_str())
                } else {
                    tool.arguments["id"].as_str()
                };
                if let Some(id) = id {
                    let _ = write!(activity, " [[{id}]]");
                }
                if let Some(error) = result.and_then(|r| r["error"].as_str()) {
                    let _ = write!(activity, ": {error}");
                }
                activity.push_str("\n\n");
            }
            if turn.status == Status::Stopped {
                activity.push_str("Stopped.\n");
            }
            view.tools.set_visible(!activity.is_empty());
            changed |= update_text(window, &view.tools, &activity);
        }
        if follow && changed {
            glib::idle_add_local_once(move || {
                adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
            });
        }
    }
}

fn readonly(window: &Window, text: &str) -> EditorWidget {
    let widget = EditorWidget::default();
    widget.set_read_only(true);
    widget.set_editor(Some(Editor::new(Body::detached(text))));
    window.connect_aux_editor(&widget, false);
    window.refresh_aux_editor(&widget);
    widget
}

fn update_text(window: &Window, widget: &EditorWidget, text: &str) -> bool {
    if widget.text().as_deref() == Some(text) {
        return false;
    }
    widget.set_editor(Some(Editor::new(Body::detached(text))));
    window.refresh_aux_editor(widget);
    true
}

fn unambiguous_archive_space(spaces: &[(Id, String, bool)]) -> Option<Id> {
    if let [(id, _, _)] = spaces {
        return Some(*id);
    }
    let mut shown = spaces.iter().filter(|(_, _, hidden)| !hidden);
    let (id, _, _) = shown.next()?;
    shown.next().is_none().then_some(*id)
}

pub(crate) async fn choose_destination(
    window: &Window,
    spaces: &[(Id, String)],
    title: &str,
) -> Option<Id> {
    let names: Vec<&str> = spaces.iter().map(|(_, name)| name.as_str()).collect();
    let picker = gtk::DropDown::from_strings(&names);
    if let Some(index) = spaces.iter().position(|(id, _)| Some(*id) == window.current_space_id()) {
        picker.set_selected(u32::try_from(index).unwrap_or(0));
    }
    let dialog = adw::AlertDialog::new(Some("Create note in a space"), Some(title));
    dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
    dialog.set_response_enabled("create", !spaces.is_empty());
    dialog.set_extra_child(Some(&picker));
    if dialog.choose_future(Some(&window.gtk_window())).await == "create" {
        spaces.get(usize::try_from(picker.selected()).ok()?).map(|(id, _)| *id)
    } else {
        None
    }
}

/// The sender closes when Stop cancels the tool, so its modal picker closes too.
pub(crate) fn request_destination(
    window: Window,
    state: Arc<State>,
    title: String,
    mut reply: tokio::sync::oneshot::Sender<Option<Id>>,
) {
    glib::spawn_future_local(async move {
        // Polling the sender here also handles cancellation before a dialog is presented.
        if reply.is_closed() {
            return;
        }
        window.present();
        let spaces = core(async move { state.spaces().await }).await;
        let names: Vec<&str> = spaces.iter().map(|(_, name)| name.as_str()).collect();
        let picker = gtk::DropDown::from_strings(&names);
        let dialog = adw::AlertDialog::new(Some("Create agent note"), Some(&title));
        dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
        dialog.set_response_enabled("create", !spaces.is_empty());
        dialog.set_extra_child(Some(&picker));
        let response = tokio::select! {
            response = dialog.clone().choose_future(Some(&window.gtk_window())) => response,
            () = reply.closed() => { dialog.close(); return; }
        };
        let space = (response == "create")
            .then(|| {
                usize::try_from(picker.selected())
                    .ok()
                    .and_then(|index| spaces.get(index))
                    .map(|(id, _)| *id)
            })
            .flatten();
        let _ = reply.send(space);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_destination_uses_the_only_space_or_the_only_shown_space() {
        let first = Id::generate();
        let second = Id::generate();
        let mut spaces = vec![(first, "First".into(), false)];
        assert_eq!(unambiguous_archive_space(&spaces), Some(first));
        spaces[0].2 = true;
        assert_eq!(unambiguous_archive_space(&spaces), Some(first));

        spaces.push((second, "Second".into(), false));
        assert_eq!(unambiguous_archive_space(&spaces), Some(second));
        spaces[0].2 = false;
        assert_eq!(unambiguous_archive_space(&spaces), None);
        spaces[0].2 = true;
        spaces[1].2 = true;
        assert_eq!(unambiguous_archive_space(&spaces), None);
        assert_eq!(unambiguous_archive_space(&[]), None);
    }
}
