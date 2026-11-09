//! The main window: the spaces that show, one list of their notes, search, and the note editor.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use zib_core::body;
use zib_core::editing::{Drawing, LinkTarget};
use zib_core::render::Kind;
use zib_core::search::{MATCH_END, MATCH_START};
use zib_core::sync::{LiveEvent, Selection, SyncState};
use zib_core::{Channel, Id};

use crate::app::core;
use crate::editor_widget::{EditorWidget, PasteTarget};
use crate::state::State;
use crate::tables;

#[cfg(test)]
mod tests;

/// How long metadata and search updates wait after typing stops, or while editing live.
const REPORT_LIVE: Duration = Duration::from_millis(50);
const REPORT_PAUSE: Duration = Duration::from_secs(3);

/// The window, which the instance opens on `activate`.
#[derive(Clone)]
pub struct Window(Rc<Inner>);

pub struct Inner {
    chat: crate::chat::Chat,
    state: Arc<State>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    /// A toggle for each space, which shows or hides its notes.
    space_toggles: gtk::FlowBox,
    /// Each space, by pet name, with whether its notes are hidden.
    spaces: RefCell<Vec<(Id, String, bool)>>,
    /// The space the owner last put a note into.
    last_space: Cell<Option<Id>>,
    new_note_menu: gio::Menu,
    search: gtk::SearchEntry,
    list: gtk::ListBox,
    rows: RefCell<Vec<(Id, Id)>>,
    /// Counts reloads of the list, so only the latest one fills it.
    generation: Cell<u64>,
    /// Orders restoration, URI lookups, and direct note navigation.
    opening: Cell<u64>,
    /// Counts title lookups, so an older result cannot overwrite a newer title.
    label_generation: Cell<u64>,
    /// Counts drawing refreshes so typing waits for a short pause before rendering.
    drawing_generation: Cell<u64>,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    backlinks: gtk::MenuButton,
    /// The notes opened before the open one, the latest last, and those to go forward to again.
    back: RefCell<Vec<(Id, Id)>>,
    forward: RefCell<Vec<(Id, Id)>>,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    open_file: gtk::Button,
    status: gtk::MenuButton,
    editor: RefCell<Option<Editor>>,
    /// Whether the note that opens next takes the focus, as a new note does.
    focus_body: Cell<bool>,
    /// The note editor, backed directly by the space's Loro text.
    widget: EditorWidget,
    /// The metadata and search update after a pause in typing.
    body_timer: RefCell<Option<glib::SourceId>>,
}

/// The note open in the editor.
struct Editor {
    space: Id,
    note: Id,
    type_name: String,
    titles: HashMap<Id, String>,
    cursors: Option<tokio::sync::broadcast::Sender<(Id, Vec<Selection>)>>,
}

impl Window {
    // The window's widgets read best built in one place.
    #[allow(clippy::too_many_lines)]
    pub fn new(app: &adw::Application, state: Arc<State>) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(state.channel.title())
            .default_width(1100)
            .default_height(760)
            .build();
        if state.channel == Channel::Development {
            window.add_css_class("devel");
        }
        let new_note_menu = gio::Menu::new();
        let new_note = adw::SplitButton::builder()
            .icon_name("document-new-symbolic")
            .tooltip_text("New note")
            .dropdown_tooltip("New note in a space")
            .menu_model(&new_note_menu)
            .build();
        let add_file = gtk::Button::from_icon_name("mail-attachment-symbolic");
        add_file.set_tooltip_text(Some("Add files"));
        let menu = gio::Menu::new();
        menu.append(Some("New space…"), Some("win.new-space"));
        menu.append(Some("Rename space…"), Some("win.rename-space"));
        menu.append(Some("Share space…"), Some("win.share-space"));
        menu.append(Some("Move note to space…"), Some("win.move-note"));
        menu.append(Some("Move notes to space…"), Some("win.move-notes"));
        menu.append(Some("Settings…"), Some("win.settings"));
        menu.append(Some("Add machine…"), Some("win.add-machine"));
        menu.append(Some("Quit"), Some("app.quit"));
        let menu_button =
            gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).build();
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.set_title_widget(Some(&adw::WindowTitle::new("Notes", "")));
        sidebar_header.pack_start(&new_note);
        sidebar_header.pack_start(&add_file);
        sidebar_header.pack_end(&menu_button);
        let space_toggles = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(4)
            .row_spacing(4)
            .max_children_per_line(8)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(6)
            .build();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search the spaces that show")
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(6)
            .build();
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar_box.append(&space_toggles);
        sidebar_box.append(&search);
        sidebar_box.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build());
        let sidebar = adw::ToolbarView::new();
        sidebar.add_top_bar(&sidebar_header);
        sidebar.set_content(Some(&sidebar_box));

        let title = adw::WindowTitle::new("", "");
        let content_header = adw::HeaderBar::new();
        content_header.set_title_widget(Some(&title));
        let chat_toggle = gtk::Button::from_icon_name("user-available-symbolic");
        chat_toggle.set_tooltip_text(Some("Show or hide AI chat (Ctrl+Tab)"));
        content_header.pack_end(&chat_toggle);
        let open_file = gtk::Button::from_icon_name("document-open-symbolic");
        open_file.set_tooltip_text(Some("Open the file"));
        open_file.set_visible(false);
        let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
        back_button.set_tooltip_text(Some("Back"));
        let forward_button = gtk::Button::from_icon_name("go-next-symbolic");
        forward_button.set_tooltip_text(Some("Forward"));
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Delete the note"));
        let status = gtk::MenuButton::builder()
            .icon_name("emblem-synchronizing-symbolic")
            .popover(&crate::status::popover(state.clone()))
            .build();
        status.add_css_class("flat");
        content_header.pack_start(&back_button);
        content_header.pack_start(&forward_button);
        content_header.pack_start(&open_file);
        content_header.pack_end(&status);
        content_header.pack_end(&delete);
        let print_menu = gio::Menu::new();
        print_menu.append(Some("Print with linked notes…"), Some("win.print-linked"));
        let print = adw::SplitButton::builder()
            .icon_name("printer-symbolic")
            .tooltip_text("Print")
            .dropdown_tooltip("More ways to print")
            .menu_model(&print_menu)
            .action_name("win.print")
            .build();
        content_header.pack_end(&print);
        let backlinks = gtk::MenuButton::builder()
            .icon_name("mail-reply-sender-symbolic")
            .tooltip_text("Linked from")
            .build();
        content_header.pack_end(&backlinks);

        let widget = EditorWidget::default();
        let editor_scroll = gtk::ScrolledWindow::builder()
            .child(&widget)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("accessories-text-editor-symbolic")
            .title(state.channel.display_name())
            .description("Pick a note, or make a new one.")
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&editor_scroll, Some("editor"));
        let content = adw::ToolbarView::new();
        content.add_top_bar(&content_header);
        content.set_content(Some(&stack));

        let split = adw::NavigationSplitView::new();
        split.set_sidebar(Some(&adw::NavigationPage::new(&sidebar, "Notes")));
        split.set_content(Some(&adw::NavigationPage::new(&content, "Note")));
        let chat = crate::chat::Chat::new(state.clone());
        let panes = gtk::Paned::new(gtk::Orientation::Horizontal);
        panes.set_start_child(Some(&split));
        panes.set_end_child(Some(chat.widget()));
        panes.set_resize_end_child(false);
        panes.set_shrink_end_child(false);
        panes.set_position(720);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&panes));
        window.set_content(Some(&toasts));

        let this = Self(Rc::new(Inner {
            chat,
            state,
            window,
            toasts,
            space_toggles,
            spaces: RefCell::default(),
            last_space: Cell::new(None),
            new_note_menu,
            search,
            list,
            rows: RefCell::default(),
            generation: Cell::new(0),
            opening: Cell::new(0),
            label_generation: Cell::new(0),
            drawing_generation: Cell::new(0),
            title,
            stack,
            backlinks,
            back: RefCell::default(),
            forward: RefCell::default(),
            back_button,
            forward_button,
            open_file,
            status,
            editor: RefCell::default(),
            focus_body: Cell::new(false),
            widget,
            body_timer: RefCell::default(),
        }));
        this.connect(&new_note, &add_file, &delete);
        this.inner().chat.connect(this.clone());
        let w = this.clone();
        chat_toggle.connect_clicked(move |_| w.inner().chat.toggle());
        this.connect_files();
        let widget = &this.inner().widget;
        let w = this.clone();
        widget.connect_edited(move || w.body_edited());
        let w = this.clone();
        widget.connect_selection_changed(move || w.cursor_moved());
        let w = this.clone();
        widget.connect_open_link(move |target| w.open_link(target));
        let w = this.clone();
        widget.connect_pick_link(move |range| w.open_link_picker(range));
        let w = this.clone();
        widget.connect_paste_image(move |clipboard, target| {
            w.paste_image(clipboard, target);
        });
        this.update_history();
        this.reload_spaces();
        this.watch_status();
        this.resend_cursor();
        this
    }

    fn inner(&self) -> &Inner {
        &self.0
    }

    pub fn present(&self) {
        self.inner().window.present();
    }

    fn begin_open(&self) -> u64 {
        let next = self.inner().opening.get().wrapping_add(1);
        self.inner().opening.set(next);
        next
    }

    /// Restores only an available note, and yields to any explicit navigation started meanwhile.
    pub fn restore_last_note(&self) {
        if self.inner().editor.borrow().is_some() {
            return;
        }
        let opening = self.begin_open();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let last = core(async move { state.last_note().await }).await;
            if w.inner().opening.get() == opening
                && let Some((space, note)) = last
            {
                w.open_note(space, note);
            }
        });
    }

    pub fn toast(&self, text: &str) {
        self.inner().toasts.add_toast(adw::Toast::new(text));
    }

    fn connect(&self, new_note: &adw::SplitButton, add_file: &gtk::Button, delete: &gtk::Button) {
        let w = self.clone();
        self.inner().search.connect_search_changed(move |_| w.reload_notes());
        let w = self.clone();
        self.inner().list.connect_row_activated(move |_, row| {
            let picked = w
                .inner()
                .rows
                .borrow()
                .get(usize::try_from(row.index()).unwrap_or(usize::MAX))
                .copied();
            if let Some((space, note)) = picked {
                w.open_note(space, note);
            }
        });
        let w = self.clone();
        new_note.connect_clicked(move |_| {
            if let Some(space) = w.current_space_id() {
                w.new_note(space);
            } else {
                w.toast("Make a space first");
            }
        });
        let w = self.clone();
        add_file.connect_clicked(move |_| w.pick_files());
        let w = self.clone();
        delete.connect_clicked(move |_| w.delete_note());
        let w = self.clone();
        self.inner().open_file.connect_clicked(move |_| w.open_file());
        let w = self.clone();
        self.inner().back_button.connect_clicked(move |_| w.go(true));
        let w = self.clone();
        self.inner().forward_button.connect_clicked(move |_| w.go(false));
        let w = self.clone();
        self.inner().backlinks.set_create_popup_func(move |button| w.show_backlinks(button));
        let w = self.clone();
        self.inner().window.connect_close_request(move |window| {
            w.close_editor();
            window.set_visible(false);
            glib::Propagation::Stop
        });
        let actions = gio::SimpleActionGroup::new();
        for (name, handler) in [
            ("add-machine", Self::add_machine as fn(&Self)),
            ("move-note", Self::move_note),
            ("move-notes", |w: &Self| crate::sharing::move_all(w, w.inner().state.clone())),
            ("new-space", Self::new_space),
            ("print", |w: &Self| w.print(false)),
            ("print-linked", |w: &Self| w.print(true)),
            ("rename-space", Self::rename_space),
            ("settings", |w: &Self| crate::settings::show(w, w.inner().state.clone())),
            ("share-space", |w: &Self| crate::sharing::share(w, w.inner().state.clone())),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let w = self.clone();
            action.connect_activate(move |_, _| handler(&w));
            actions.add_action(&action);
        }
        let new_note_in = gio::SimpleAction::new("new-note-in", Some(glib::VariantTy::STRING));
        let w = self.clone();
        new_note_in.connect_activate(move |_, parameter| {
            let space = parameter.and_then(glib::Variant::str).and_then(|s| Id::parse(s).ok());
            if let Some(space) = space {
                w.new_note(space);
            }
        });
        actions.add_action(&new_note_in);
        self.inner().window.insert_action_group("win", Some(&actions));
        self.connect_history();
    }

    /// Lets Alt+Left and Alt+Right, and a mouse's back and forward buttons, walk the history.
    fn connect_history(&self) {
        // The capture phase sees Alt+Left and Alt+Right before the editor, which would move by
        // words.
        let keys = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = self.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
            if modifiers == gdk::ModifierType::CONTROL_MASK && key == gdk::Key::Tab {
                if w.inner().chat.has_focus() {
                    w.inner().widget.grab_focus();
                } else {
                    w.inner().chat.focus();
                }
                return glib::Propagation::Stop;
            }
            if modifiers != gdk::ModifierType::ALT_MASK {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::Left | gdk::Key::KP_Left => w.go(true),
                gdk::Key::Right | gdk::Key::KP_Right => w.go(false),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.inner().window.add_controller(keys);
        // A mouse's back and forward buttons.
        let buttons = gtk::GestureClick::builder()
            .button(0)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = self.clone();
        buttons.connect_pressed(move |gesture, _, _, _| match gesture.current_button() {
            8 => w.go(true),
            9 => w.go(false),
            _ => {}
        });
        self.inner().window.add_controller(buttons);
    }

    /// Opens the note before the open one in the history, or with `back` false, the one after.
    fn go(&self, back: bool) {
        let inner = self.inner();
        let (from, to) =
            if back { (&inner.back, &inner.forward) } else { (&inner.forward, &inner.back) };
        let Some(next) = from.borrow_mut().pop() else { return };
        if let Some(open) = inner.editor.borrow().as_ref().map(|e| (e.space, e.note)) {
            to.borrow_mut().push(open);
        }
        self.update_history();
        self.load_note(next.0, next.1);
    }

    fn update_history(&self) {
        let inner = self.inner();
        inner.back_button.set_sensitive(!inner.back.borrow().is_empty());
        inner.forward_button.set_sensitive(!inner.forward.borrow().is_empty());
    }

    /// The space that new notes and files go into: the one the owner last used, if it shows, or
    /// else the open note's, or else the first that shows.
    pub fn current_space_id(&self) -> Option<Id> {
        let spaces = self.inner().spaces.borrow();
        let shows = |id: &Id| spaces.iter().any(|(s, _, hidden)| s == id && !hidden);
        let open = self.inner().editor.borrow().as_ref().map(|e| e.space);
        self.inner()
            .last_space
            .get()
            .filter(shows)
            .or(open.filter(shows))
            .or_else(|| spaces.iter().find(|(_, _, hidden)| !hidden).map(|(id, _, _)| *id))
            .or_else(|| spaces.first().map(|(id, _, _)| *id))
    }

    /// The spaces whose notes show, with their pet names.
    fn shown_spaces(&self) -> Vec<(Id, String)> {
        let spaces = self.inner().spaces.borrow();
        spaces.iter().filter(|(_, _, hidden)| !hidden).map(|(id, n, _)| (*id, n.clone())).collect()
    }

    /// Fills the space toggles and the new-note menu, then the note list.
    fn reload_spaces(&self) {
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let (spaces, last) =
                core(async move { (state.space_list().await, state.last_space().await) }).await;
            let toggles = &w.inner().space_toggles;
            toggles.remove_all();
            let menu = &w.inner().new_note_menu;
            menu.remove_all();
            for (id, name, hidden) in &spaces {
                let toggle = gtk::ToggleButton::builder()
                    .label(name)
                    .active(!hidden)
                    .tooltip_text("Show this space's notes")
                    .build();
                let (window, id) = (w.clone(), *id);
                toggle.connect_toggled(move |toggle| window.show_space(id, toggle.is_active()));
                toggles.append(&toggle);
                menu.append(
                    Some(&format!("New note in {name}")),
                    Some(&format!("win.new-note-in('{id}')")),
                );
            }
            // With one space, there's nothing to choose between.
            toggles.set_visible(spaces.len() > 1);
            *w.inner().spaces.borrow_mut() = spaces;
            w.inner().last_space.set(last);
            w.reload_notes();
        });
    }

    /// Shows or hides a space's notes, and remembers the choice on this machine.
    fn show_space(&self, space: Id, shown: bool) {
        for entry in self.inner().spaces.borrow_mut().iter_mut().filter(|(id, _, _)| *id == space) {
            entry.2 = !shown;
        }
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let _ = core(async move { state.set_hidden(space, !shown).await }).await;
        });
        self.reload_notes();
    }

    /// Fills the note list with the notes of the spaces that show, most recently changed first, or
    /// with search results from them.
    fn reload_notes(&self) {
        let generation = self.inner().generation.get() + 1;
        self.inner().generation.set(generation);
        let w = self.clone();
        let state = self.inner().state.clone();
        let query = self.inner().search.text().to_string();
        let spaces = self.shown_spaces();
        let names: HashMap<Id, String> = spaces.iter().cloned().collect();
        let several = spaces.len() > 1;
        glib::spawn_future_local(async move {
            // Each row: its space, its note, its title, and its subtitle, as Pango markup.
            let mut rows: Vec<(Id, Id, String, String)> = Vec::new();
            if query.trim().is_empty() {
                let notes = core(async move {
                    let mut all = Vec::new();
                    for (space, _) in spaces {
                        let notes = state.notes(space).await.unwrap_or_default();
                        all.extend(notes.into_iter().map(|note| (space, note)));
                    }
                    all.sort_by_key(|(_, note)| std::cmp::Reverse(note.updated));
                    all
                })
                .await;
                for (space, note) in notes {
                    let title = if note.plain.trim().is_empty() { "Untitled" } else { &note.plain };
                    let mut subtitle = Vec::new();
                    if several {
                        subtitle.extend(names.get(&space).cloned());
                    }
                    if note.type_name != "note" {
                        subtitle.push(note.type_name.clone());
                    }
                    let subtitle = glib::markup_escape_text(&subtitle.join(" · ")).to_string();
                    rows.push((space, note.id, escape(title), subtitle));
                }
            } else {
                let hits = core(async move { state.search(&query, 50) }).await;
                for (space, hit) in hits.into_iter().filter(|(space, _)| names.contains_key(space))
                {
                    rows.push((space, hit.id, escape(&hit.title), snippet_markup(&hit.snippet)));
                }
            }
            // A later reload started while this one waited, and fills the list instead.
            if w.inner().generation.get() != generation {
                return;
            }
            let list = &w.inner().list;
            list.remove_all();
            for (_, _, title, subtitle) in &rows {
                let row = adw::ActionRow::builder()
                    .title(title.as_str())
                    .subtitle(subtitle.as_str())
                    .subtitle_lines(2)
                    .activatable(true)
                    .build();
                list.append(&row);
            }
            *w.inner().rows.borrow_mut() =
                rows.into_iter().map(|(space, note, _, _)| (space, note)).collect();
        });
    }

    /// Something in a space changed through sync or a derivation.
    pub fn space_changed(&self, space: Id) {
        self.inner().chat.refresh_resources(self);
        if !self.inner().spaces.borrow().iter().any(|(id, _, _)| *id == space) {
            self.reload_spaces();
            return;
        }
        self.reload_notes();
        self.refresh_link_titles();
        self.refresh_images(true);
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        if open.is_some_and(|(s, _)| s == space) {
            self.refresh_open();
        }
    }

    /// Opens a `zib:note/` URI.
    pub fn open_uri(&self, uri: &str) {
        let Some(rest) = uri.get(9..).filter(|_| uri.to_ascii_lowercase().starts_with("zib:note/"))
        else {
            self.toast(&format!("Zib can't open {uri}"));
            return;
        };
        let id = rest.split('#').next().unwrap_or("");
        let Ok(note) = Id::parse(id) else {
            self.toast("That link doesn't name a note");
            return;
        };
        let w = self.clone();
        let state = self.inner().state.clone();
        let opening = self.begin_open();
        glib::spawn_future_local(async move {
            let space = core(async move { state.find_note(note).await }).await;
            if w.inner().opening.get() != opening {
                return;
            }
            match space {
                Some(space) => w.open_note(space, note),
                None => w.toast("This machine doesn't hold that note"),
            }
        });
    }

    fn new_note(&self, space: Id) {
        self.use_space(space);
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            if let Ok(note) = core(async move { state.create_note(space, "").await }).await {
                w.reload_notes();
                w.inner().focus_body.set(true);
                w.open_note(space, note);
            }
        });
    }

    fn delete_note(&self) {
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let dialog = adw::AlertDialog::new(
                Some("Delete this note?"),
                Some("Links to it will show its last title."),
            );
            dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            if dialog.choose_future(Some(&w.inner().window)).await != "delete" {
                return;
            }
            w.close_editor();
            for history in [&w.inner().back, &w.inner().forward] {
                history.borrow_mut().retain(|&(_, id)| id != note);
            }
            w.update_history();
            let _ = core(async move { state.delete_note(space, note).await }).await;
            w.inner().stack.set_visible_child_name("empty");
            w.reload_notes();
        });
    }

    /// Opens a note in the editor, and adds the open one to the history.
    pub fn open_note(&self, space: Id, note: Id) {
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        if let Some(open) = open.filter(|&(_, open)| open != note) {
            let mut back = self.inner().back.borrow_mut();
            back.push(open);
            // The history stops growing at a length nobody walks back through.
            if back.len() > 100 {
                back.remove(0);
            }
            self.inner().forward.borrow_mut().clear();
        }
        self.update_history();
        self.load_note(space, note);
    }

    /// Opens the shared body and starts live sessions. A newer load discards earlier results.
    fn load_note(&self, space: Id, note: Id) {
        self.close_editor();
        let session = self.inner().widget.session();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let loaded = core({
                let state = state.clone();
                async move {
                    let loaded = state.note(space, note).await?;
                    let body = state.open_body(space, note).await?;
                    let targets: Vec<Id> = body::link_targets(&loaded.body).into_iter().collect();
                    let titles = state.titles(&targets).await;
                    Ok::<_, zib_core::Error>((loaded, body, titles))
                }
            })
            .await;
            if w.inner().widget.session() != session {
                return;
            }
            let (loaded, body, titles) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    w.toast(&format!("Couldn't open the note: {error}"));
                    return;
                }
            };
            *w.inner().editor.borrow_mut() = Some(Editor {
                space,
                note,
                type_name: loaded.type_name.clone(),
                titles: titles.clone(),
                cursors: None,
            });
            let widget = &w.inner().widget;
            widget.set_editor(Some(zib_core::editor::Editor::new(body)));
            widget.set_titles(titles);
            w.refresh_images(false);
            w.inner()
                .open_file
                .set_visible(loaded.type_name == "file" || loaded.type_name == "image");
            w.update_title();
            w.show_mode();
            if w.inner().focus_body.take() {
                widget.grab_focus();
            }
            let session = widget.session();
            let remembering = state.clone();
            if let Err(error) =
                core(async move { remembering.remember_note(space, note).await }).await
            {
                w.toast(&format!("Couldn't remember the open note: {error}"));
            }
            if widget.session() != session {
                return;
            }
            let opened = core(async move { state.open_note(space, note).await }).await;
            if w.inner().widget.session() != session {
                return;
            }
            if let Ok(mut opened) = opened {
                if let Some(editor) = w.inner().editor.borrow_mut().as_mut() {
                    editor.cursors = Some(opened.cursors.clone());
                }
                w.cursor_moved();
                let listening = w.clone();
                glib::spawn_future_local(async move {
                    while let Some(event) = core_recv(&mut opened.events).await {
                        if listening.inner().widget.session() != session {
                            break;
                        }
                        listening.live_event(space, note, event);
                    }
                });
            }
        });
    }

    fn update_title(&self) {
        let editor = self.inner().editor.borrow();
        let Some(editor) = editor.as_ref() else { return };
        let text = self.inner().widget.text().unwrap_or_default();
        let title = body::title(&text, &mut |id| {
            editor
                .titles
                .get(&id)
                .map(|t| body::Title { plain: body::plain_text(t), markdown: t.clone() })
        });
        let shown = if title.is_empty() { "Untitled".to_owned() } else { title.plain };
        self.inner().title.set_title(&shown);
        self.inner().title.set_subtitle(if self.inner().widget.is_live() {
            "Editing live"
        } else {
            ""
        });
    }

    /// Edits already live in the shared body. Metadata and search follow after a short pause.
    fn body_edited(&self) {
        if self.inner().editor.borrow().is_none() {
            return;
        }
        self.update_title();
        self.refresh_link_titles();
        self.refresh_images(false);
        if let Some(timer) = self.inner().body_timer.take() {
            timer.remove();
        }
        let w = self.clone();
        let wait = if self.inner().widget.is_live() { REPORT_LIVE } else { REPORT_PAUSE };
        let timer = glib::timeout_add_local_once(wait, move || {
            w.inner().body_timer.take();
            w.report_body_edited_now();
        });
        *self.inner().body_timer.borrow_mut() = Some(timer);
    }

    /// Resolves labels across all spaces, and discards a lookup if the open body changed.
    fn refresh_link_titles(&self) {
        let widget = self.inner().widget.clone();
        let Some(text) = widget.text() else { return };
        let generation = self.inner().label_generation.get().wrapping_add(1);
        self.inner().label_generation.set(generation);
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        let state = self.inner().state.clone();
        let w = self.clone();
        glib::spawn_future_local(async move {
            let targets: Vec<Id> = body::link_targets(&text).into_iter().collect();
            let titles = core(async move { state.titles(&targets).await }).await;
            if w.inner().label_generation.get() == generation
                && w.inner().editor.borrow().as_ref().map(|e| (e.space, e.note)) == open
                && widget.text().as_deref() == Some(&text)
            {
                if let Some(editor) = w.inner().editor.borrow_mut().as_mut() {
                    editor.titles.clone_from(&titles);
                }
                widget.set_titles(titles);
                w.update_title();
            }
        });
    }

    /// Shows a local preview first, then replaces it with the original once it arrives.
    fn refresh_images(&self, reload: bool) {
        self.refresh_drawings(reload);
        let widget = self.inner().widget.clone();
        let (generation, targets) = widget.image_requests(reload);
        for target in targets {
            let (widget, state) = (widget.clone(), self.inner().state.clone());
            glib::spawn_future_local(async move {
                let preview = core({
                    let state = state.clone();
                    async move { state.image_preview(target).await }
                })
                .await
                .and_then(|path| gdk::Texture::from_filename(path).ok());
                if preview.is_some() {
                    widget.set_image(generation, target, preview.clone());
                }
                let original = embedded_texture(state, target).await;
                widget.set_image(generation, target, original.or(preview));
            });
        }
    }

    /// Render after a short pause, then let the widget reject results from an older note session.
    /// Results use the complete source as their key, so an older formula can't replace an edit.
    fn refresh_drawings(&self, reload: bool) {
        let widget = self.inner().widget.clone();
        if reload {
            widget.reload_tables();
        }
        let generation = self.inner().drawing_generation.get().wrapping_add(1);
        self.inner().drawing_generation.set(generation);
        let w = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            if w.inner().drawing_generation.get() != generation {
                return;
            }
            let (session, targets) = widget.drawing_requests();
            for drawing in targets {
                if let Drawing::Table(table) = &drawing {
                    let (state, widget, table) =
                        (w.inner().state.clone(), widget.clone(), table.clone());
                    let revision = widget.table_revision();
                    glib::spawn_future_local(async move {
                        let refs = table_refs(state, &table).await;
                        widget.set_table(session, revision, drawing, refs);
                    });
                    continue;
                }
                let (kind, source) = match &drawing {
                    Drawing::InlineMath(s) => (Kind::InlineMath, s.clone()),
                    Drawing::DisplayMath(s) => (Kind::DisplayMath, s.clone()),
                    Drawing::Diagram(s) => (Kind::Mermaid, s.clone()),
                    _ => continue,
                };
                let (state, widget) = (w.inner().state.clone(), widget.clone());
                glib::spawn_future_local(async move {
                    let result = core(async move { state.render(kind, source).await }).await;
                    let result = result.map_err(|error| error.to_string()).and_then(|image| {
                        let size = zib_core::editor::display::ImageSize {
                            width: image.width,
                            height: image.height,
                        };
                        gdk::Texture::from_bytes(&glib::Bytes::from(&image.png))
                            .map(|texture| (texture, size))
                            .map_err(|error| error.to_string())
                    });
                    widget.set_drawing(session, drawing, result);
                });
            }
        });
    }

    /// Reports the editor's edits now if a report waits.
    fn report_body_edited(&self) {
        if let Some(timer) = self.inner().body_timer.take() {
            timer.remove();
            self.report_body_edited_now();
        }
    }

    /// Has the core record the open note's edits: its metadata, search, saving, and sync.
    fn report_body_edited_now(&self) {
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let _ = core(async move { state.body_edited(space, note).await }).await;
            w.reload_notes();
        });
    }

    /// Reports pending metadata and ends the note's live sessions.
    fn close_editor(&self) {
        self.begin_open();
        self.inner().label_generation.set(self.inner().label_generation.get().wrapping_add(1));
        self.inner().drawing_generation.set(self.inner().drawing_generation.get().wrapping_add(1));
        self.report_body_edited();
        self.inner().widget.set_editor(None);
        if let Some(editor) = self.inner().editor.borrow_mut().take() {
            let state = self.inner().state.clone();
            let (space, note) = (editor.space, editor.note);
            glib::spawn_future_local(async move {
                core(async move { state.close_note(space, note).await }).await;
            });
        }
        self.inner().open_file.set_visible(false);
    }

    /// Another machine's change or cursor arrived while the note is open.
    fn live_event(&self, space: Id, note: Id, event: LiveEvent) {
        let is_open = self
            .inner()
            .editor
            .borrow()
            .as_ref()
            .is_some_and(|e| e.space == space && e.note == note);
        if !is_open {
            return;
        }
        let widget = &self.inner().widget;
        match event {
            LiveEvent::Started { machine } => {
                widget.remote_started(machine);
                self.report_body_edited();
                self.cursor_moved();
            }
            LiveEvent::Changed { machine } => {
                widget.remote_started(machine);
                self.refresh_open();
                let state = self.inner().state.clone();
                glib::spawn_future_local(async move {
                    core(async move { state.live_changed(space).await }).await;
                });
            }
            LiveEvent::Cursor { machine, selections } => {
                // A delayed name lookup mustn't replace a newer selection or revive a session.
                widget.remote_received(machine, selections);
                let (state, widget, session) =
                    (self.inner().state.clone(), widget.clone(), widget.session());
                glib::spawn_future_local(async move {
                    if let Some(name) =
                        core(async move { state.machine_name(space, machine).await }).await
                    {
                        widget.remote_name(session, machine, name);
                    }
                });
            }
            LiveEvent::Ended { machine } => widget.remote_ended(machine),
        }
        self.update_title();
    }

    /// Reads imported edits from the shared body without replacing the owner's editor state.
    fn refresh_open(&self) {
        self.inner().widget.refresh();
        self.update_title();
        self.refresh_link_titles();
        self.refresh_images(false);
    }

    /// Sends the owner's actual Loro selection to machines editing the same note.
    fn cursor_moved(&self) {
        let editor = self.inner().editor.borrow();
        let Some(editor) = editor.as_ref() else { return };
        if let Some(sender) = &editor.cursors
            && let Some(selection) = self.inner().widget.encoded_selection()
        {
            let _ = sender.send((editor.note, vec![selection]));
        }
    }

    /// Repeats the owner's selection and removes remote cursors that go quiet.
    fn resend_cursor(&self) {
        let w = Rc::downgrade(&self.0);
        glib::timeout_add_local(zib_core::sync::CURSOR_REPEAT, move || {
            let Some(w) = w.upgrade().map(Self) else { return glib::ControlFlow::Break };
            w.cursor_moved();
            w.inner().widget.expire_remote();
            glib::ControlFlow::Continue
        });
    }

    /// Shows the editor, or the empty page when no note is open.
    fn show_mode(&self) {
        let open = self.inner().editor.borrow().is_some();
        self.inner().stack.set_visible_child_name(if open { "editor" } else { "empty" });
    }

    /// Adds files dropped on the editor as notes. The widget handles image clipboard input.
    fn connect_files(&self) {
        let w = self.clone();
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(move |_, value, _, _| {
            if let Ok(files) = value.get::<gdk::FileList>() {
                w.add_files(files.files());
            }
            true
        });
        self.inner().widget.add_controller(target);
    }

    fn pick_files(&self) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Add files").modal(true).build();
            if let Ok(files) = dialog.open_multiple_future(Some(&w.inner().window)).await {
                let files: Vec<gio::File> = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_then(|o| o.downcast::<gio::File>().ok()))
                    .collect();
                w.add_files(files);
            }
        });
    }

    /// Adds files as notes in the current space, and opens the last one.
    pub fn add_files(&self, files: Vec<gio::File>) {
        let Some(space) = self.current_space_id() else {
            self.toast("Make a space first");
            return;
        };
        self.use_space(space);
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let mut last = None;
            for file in files {
                let Some(path) = file.path() else { continue };
                let (content_type, _) = gio::content_type_guess(Some(&path), None::<&[u8]>);
                let media = gio::content_type_get_mime_type(&content_type)
                    .map_or_else(|| "application/octet-stream".to_owned(), |m| m.to_string());
                let state = state.clone();
                match core(async move { state.add_file(space, path, media).await }).await {
                    Ok(note) => last = Some(note),
                    Err(error) => w.toast(&format!("Couldn't add the file: {error}")),
                }
            }
            w.reload_notes();
            if let Some(note) = last {
                w.open_note(space, note);
            }
        });
    }

    fn paste_image(&self, clipboard: gdk::Clipboard, target: PasteTarget) {
        let Some(space) = self.inner().editor.borrow().as_ref().map(|e| e.space) else { return };
        let widget = self.inner().widget.clone();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            match pasted_image(state.clone(), space, clipboard).await {
                Ok((note, _)) => {
                    widget.insert_image(&target, note);
                    w.report_body_edited();
                    w.reload_notes();
                    let _ = core(async move { state.derive(space).await }).await;
                }
                Err(error) => w.toast(&format!("Couldn't paste the image: {error}")),
            }
        });
    }

    /// Opens the file a `file` or `image` note holds in the desktop's app for it.
    fn open_file(&self) {
        let Some((space, note, type_name)) =
            self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note, e.type_name.clone()))
        else {
            return;
        };
        let field = if type_name == "image" { "image" } else { "file" };
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let path = core(async move {
                let (hash, _) = state
                    .note_file(space, note, field)
                    .await?
                    .ok_or_else(|| zib_core::Error::NotFound("a file".into()))?;
                state.blob_path(space, hash).await
            })
            .await;
            match path {
                Ok(path) => open_path(&path, &w),
                Err(error) => w.toast(&format!("Couldn't open the file: {error}")),
            }
        });
    }

    fn new_space(&self) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            let Some(name) = ask(&w.inner().window, "New space", "", "Create").await else {
                return;
            };
            let state = w.inner().state.clone();
            match core(async move { state.create_space(&name).await }).await {
                Ok(_) => w.reload_spaces(),
                Err(error) => w.toast(&format!("Couldn't make the space: {error}")),
            }
        });
    }

    /// Asks which space to rename, and its new name.
    fn rename_space(&self) {
        let spaces = self.inner().spaces.borrow().clone();
        if spaces.is_empty() {
            return;
        }
        let names: Vec<&str> = spaces.iter().map(|(_, name, _)| name.as_str()).collect();
        let picker = gtk::DropDown::from_strings(&names);
        let current = self.current_space_id();
        let index = spaces.iter().position(|(id, _, _)| Some(*id) == current).unwrap_or(0);
        picker.set_selected(u32::try_from(index).unwrap_or(0));
        let entry = gtk::Entry::builder().activates_default(true).build();
        let fill = {
            let (entry, spaces) = (entry.clone(), spaces.clone());
            move |picker: &gtk::DropDown| {
                let index = usize::try_from(picker.selected()).unwrap_or(0);
                entry.set_text(spaces.get(index).map_or("", |(_, name, _)| name.as_str()));
            }
        };
        fill(&picker);
        picker.connect_selected_notify(fill);
        let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
        fields.append(&picker);
        fields.append(&entry);
        let dialog = adw::AlertDialog::new(Some("Rename space"), None);
        dialog.add_responses(&[("cancel", "Cancel"), ("ok", "Rename")]);
        dialog.set_default_response(Some("ok"));
        dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dialog.set_extra_child(Some(&fields));
        let w = self.clone();
        glib::spawn_future_local(async move {
            let response = dialog.choose_future(Some(&w.inner().window)).await;
            let name = entry.text().trim().to_owned();
            let index = usize::try_from(picker.selected()).unwrap_or(0);
            let Some(&(space, _, _)) = spaces.get(index) else { return };
            if response != "ok" || name.is_empty() {
                return;
            }
            let state = w.inner().state.clone();
            let _ = core(async move { state.rename_space(space, &name).await }).await;
            w.reload_spaces();
        });
    }

    /// Asks the owner to name a space that `from` shared with this machine, which the pairing
    /// specification asks for. Without an answer, it keeps the name it arrived with.
    pub fn name_space(&self, space: Id, from: String, arrived: String) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            let heading = format!("Name the space from {from}");
            let Some(name) = ask(&w.inner().window, &heading, &arrived, "Name").await else {
                return;
            };
            let state = w.inner().state.clone();
            let _ = core(async move { state.rename_space(space, &name).await }).await;
            w.reload_spaces();
        });
    }

    /// Prints the open note, and with `linked`, the notes it links to, through GTK's print dialog,
    /// which sends the PDF to a printer or saves it.
    fn print(&self, linked: bool) {
        self.report_body_edited();
        let Some(note) = self.inner().editor.borrow().as_ref().map(|e| e.note) else { return };
        let w = self.clone();
        let state = self.inner().state.clone();
        self.toast("Preparing to print…");
        glib::spawn_future_local(async move {
            let path = state.dirs.cache.join(format!("print-{note}.pdf"));
            let written = {
                let path = path.clone();
                core(async move {
                    let pdf = state.print(&[note], linked).await?;
                    std::fs::write(&path, pdf).map_err(|e| zib_core::Error::io(path, e))
                })
                .await
            };
            if let Err(error) = written {
                w.toast(&format!("Couldn't print: {error}"));
                return;
            }
            let dialog = gtk::PrintDialog::new();
            let file = gio::File::for_path(&path);
            let printed = dialog
                .print_file_future(Some(&w.inner().window), None::<&gtk::PrintSetup>, &file)
                .await;
            if let Err(error) = printed
                && !error.matches(gtk::DialogError::Dismissed)
                && !error.matches(gtk::DialogError::Cancelled)
            {
                w.toast(&format!("Couldn't print: {error}"));
            }
            let _ = std::fs::remove_file(&path);
        });
    }

    /// Remembers the space a note or file went into, where the next ones go.
    fn use_space(&self, space: Id) {
        if self.inner().last_space.replace(Some(space)) == Some(space) {
            return;
        }
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let _ = core(async move { state.set_last_space(space).await }).await;
        });
    }

    fn open_link_picker(&self, range: std::ops::Range<usize>) {
        let widget = self.inner().widget.clone();
        let Some(text) = widget.text() else { return };
        let Some(expected) = text.get(range.clone()).map(str::to_owned) else { return };
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        let w = self.clone();
        let editing = widget.clone();
        let pick = move |target| {
            if w.inner().editor.borrow().as_ref().map(|e| (e.space, e.note)) == open {
                editing.insert_link(range.clone(), &expected, target);
            }
        };
        let editing = widget.clone();
        self.show_link_picker(&widget, &widget.cursor_rectangle(), pick, move || {
            editing.grab_focus();
        });
    }

    /// The link picker's search and results.
    fn show_link_picker(
        &self,
        parent: &impl IsA<gtk::Widget>,
        place: &gdk::Rectangle,
        on_pick: impl Fn(Id) + 'static,
        on_closed: impl Fn() + 'static,
    ) {
        let popover =
            gtk::Popover::builder().pointing_to(place).position(gtk::PositionType::Bottom).build();
        popover.set_parent(parent);
        let entry = gtk::SearchEntry::builder().placeholder_text("Link to a note").build();
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        // A minimum height keeps several notes in view when the popover has little room.
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_width(360)
            .min_content_height(240)
            .max_content_height(320)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&entry);
        content.append(&scroll);
        popover.set_child(Some(&content));
        let found: Rc<RefCell<Vec<Id>>> = Rc::default();
        let generation = Rc::new(Cell::new(0u64));
        let fill = {
            let (w, list, found) = (self.clone(), list.clone(), found.clone());
            move |query: String| {
                generation.set(generation.get() + 1);
                let (w, list, found, generation, mine) =
                    (w.clone(), list.clone(), found.clone(), generation.clone(), generation.get());
                glib::spawn_future_local(async move {
                    let notes = w.link_candidates(query).await;
                    if generation.get() != mine {
                        return;
                    }
                    list.remove_all();
                    for (_, title, subtitle) in &notes {
                        let row = adw::ActionRow::builder()
                            .title(escape(title))
                            .subtitle(escape(subtitle))
                            .activatable(true)
                            .build();
                        list.append(&row);
                    }
                    if let Some(first) = list.row_at_index(0) {
                        list.select_row(Some(&first));
                    }
                    *found.borrow_mut() = notes.into_iter().map(|(id, _, _)| id).collect();
                });
            }
        };
        fill(String::new());
        let chosen = Rc::new(Cell::new(false));
        let pick = {
            let (popover, found) = (popover.clone(), found.clone());
            let chosen = chosen.clone();
            let on_pick = Rc::new(on_pick);
            move |index: i32| {
                let target =
                    usize::try_from(index).ok().and_then(|i| found.borrow().get(i).copied());
                if let Some(target) = target
                    && !chosen.replace(true)
                {
                    on_pick(target);
                }
                popover.popdown();
            }
        };
        let picked = pick.clone();
        list.connect_row_activated(move |_, row| picked(row.index()));
        let selected = list.clone();
        entry.connect_activate(move |_| pick(selected.selected_row().map_or(0, |r| r.index())));
        entry.connect_search_changed(move |entry| fill(entry.text().to_string()));
        entry.add_controller(arrow_keys(&list));
        let closing = popover.clone();
        entry.connect_stop_search(move |_| closing.popdown());
        popover.connect_closed(move |popover| {
            on_closed();
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
        entry.grab_focus();
    }

    /// The notes the picker offers for `query`, from every space: the newest notes of the spaces
    /// that show when it's empty, or else search results. Each comes with its title and its space.
    async fn link_candidates(&self, query: String) -> Vec<(Id, String, String)> {
        let state = self.inner().state.clone();
        let names: HashMap<Id, String> =
            self.inner().spaces.borrow().iter().map(|(id, name, _)| (*id, name.clone())).collect();
        let shown: Vec<Id> = self.shown_spaces().into_iter().map(|(id, _)| id).collect();
        let open = self.inner().editor.borrow().as_ref().map(|e| e.note);
        let found: Vec<(Id, Id, String)> = core(async move {
            if query.trim().is_empty() {
                let mut all = Vec::new();
                for space in shown {
                    let notes = state.notes(space).await.unwrap_or_default();
                    all.extend(notes.into_iter().map(|n| (space, n)));
                }
                all.sort_by_key(|(_, note)| std::cmp::Reverse(note.updated));
                all.into_iter().take(20).map(|(space, n)| (space, n.id, n.plain)).collect()
            } else {
                let hits = state.search(&query, 20);
                hits.into_iter().map(|(space, hit)| (space, hit.id, hit.title)).collect()
            }
        })
        .await;
        found
            .into_iter()
            .filter(|(_, id, _)| Some(*id) != open)
            .map(|(space, id, title)| {
                let title = if title.trim().is_empty() { "Untitled".to_owned() } else { title };
                (id, title, names.get(&space).cloned().unwrap_or_default())
            })
            .collect()
    }

    /// Fills the popover of notes that link to the open note.
    fn show_backlinks(&self, button: &gtk::MenuButton) {
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_width(320)
            .max_content_height(400)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let popover = gtk::Popover::builder().child(&scroll).build();
        button.set_popover(Some(&popover));
        let Some(note) = self.inner().editor.borrow().as_ref().map(|e| e.note) else { return };
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let sources = core(async move {
                let sources = state.backlinks(note);
                let ids: Vec<Id> = sources.iter().map(|(_, id)| *id).collect();
                let titles = state.titles(&ids).await;
                sources
                    .into_iter()
                    .map(|(space, id)| {
                        let title =
                            titles.get(&id).map(|t| body::plain_text(t)).unwrap_or_default();
                        (space, id, title)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            if sources.is_empty() {
                list.append(
                    &gtk::Label::builder()
                        .label("No notes link here")
                        .margin_top(12)
                        .margin_bottom(12)
                        .build(),
                );
                return;
            }
            for (space, id, title) in sources {
                let title = if title.trim().is_empty() { "Untitled".to_owned() } else { title };
                let row = adw::ActionRow::builder().title(escape(&title)).activatable(true).build();
                let (w, popover) = (w.clone(), popover.clone());
                row.connect_activated(move |_| {
                    popover.popdown();
                    w.open_note(space, id);
                });
                list.append(&row);
            }
        });
    }

    fn open_link(&self, target: LinkTarget) {
        match target {
            LinkTarget::Note(note) => self.open_uri(&format!("zib:note/{note}")),
            LinkTarget::Web(url) => {
                let launched =
                    gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>);
                if launched.is_err() {
                    self.toast(&format!("No app opens {url}"));
                }
            }
        }
    }

    fn add_machine(&self) {
        crate::adding::show(self, self.inner().state.clone());
    }

    /// Moves the open note to another space, once its pending edits are in the document.
    fn move_note(&self) {
        self.report_body_edited();
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        crate::sharing::move_note(self, self.inner().state.clone(), space, note);
    }

    /// Follows notes that moved to another space, so the open note and the history point there.
    pub fn notes_moved(&self, from: Id, to: Id, notes: &[Id]) {
        for history in [&self.inner().back, &self.inner().forward] {
            for entry in history.borrow_mut().iter_mut() {
                if entry.0 == from && notes.contains(&entry.1) {
                    entry.0 = to;
                }
            }
        }
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        if let Some((_, note)) = open.filter(|(s, n)| *s == from && notes.contains(n)) {
            self.load_note(to, note);
        }
        self.reload_notes();
    }

    pub fn gtk_window(&self) -> gtk::Window {
        self.inner().window.clone().upcast()
    }

    pub(crate) fn agent_context(&self) -> Option<zib_core::agent::Context> {
        let editor = self.inner().editor.borrow();
        Some(zib_core::agent::Context {
            id: editor.as_ref()?.note,
            title: self.inner().title.title().to_string(),
        })
    }

    /// Connects the same link, image, table, and drawing features for detached markdown editors.
    pub(crate) fn connect_aux_editor(&self, widget: &EditorWidget, editable: bool) {
        let w = self.clone();
        widget.connect_open_link(move |target| w.open_link(target));
        if !editable {
            return;
        }
        let (w, editing) = (self.clone(), widget.clone());
        widget.connect_pick_link(move |range| {
            let Some(text) = editing.text() else { return };
            let Some(expected) = text.get(range.clone()).map(str::to_owned) else { return };
            let (target, closed) = (editing.clone(), editing.clone());
            let (range, expected) = (range.clone(), expected.clone());
            w.show_link_picker(
                &editing,
                &editing.cursor_rectangle(),
                move |id| {
                    target.insert_link(range.clone(), &expected, id);
                },
                move || {
                    closed.grab_focus();
                },
            );
        });
        let (w, editing) = (self.clone(), widget.clone());
        widget.connect_paste_image(move |clipboard, target| {
            let (w, editing) = (w.clone(), editing.clone());
            glib::spawn_future_local(async move {
                let state = w.inner().state.clone();
                let spaces = core(async move { state.spaces().await }).await;
                let Some(space) =
                    crate::chat::choose_destination(&w, &spaces, "Pasted image").await
                else {
                    return;
                };
                match pasted_image(w.inner().state.clone(), space, clipboard).await {
                    Ok((note, _)) => {
                        editing.insert_image(&target, note);
                        w.reload_notes();
                    }
                    Err(error) => w.toast(&format!("Couldn't paste the image: {error}")),
                }
            });
        });
    }

    pub(crate) fn refresh_aux_editor(&self, widget: &EditorWidget) {
        self.aux_resources(widget, false);
    }

    pub(crate) fn reload_aux_editor(&self, widget: &EditorWidget) {
        self.aux_resources(widget, true);
    }

    fn aux_resources(&self, widget: &EditorWidget, reload: bool) {
        if reload {
            widget.reload_tables();
        }
        let (widget, state) = (widget.clone(), self.inner().state.clone());
        let text = widget.text().unwrap_or_default();
        let session = widget.session();
        let (labels, label_state) = (widget.clone(), state.clone());
        glib::spawn_future_local(async move {
            let targets: Vec<Id> = body::link_targets(&text).into_iter().collect();
            let titles = core(async move { label_state.titles(&targets).await }).await;
            if labels.session() == session && labels.text().as_deref() == Some(&text) {
                labels.set_titles(titles);
            }
        });
        let (generation, images) = widget.image_requests(reload);
        for target in images {
            let (widget, state) = (widget.clone(), state.clone());
            glib::spawn_future_local(async move {
                let preview = core({
                    let state = state.clone();
                    async move { state.image_preview(target).await }
                })
                .await
                .and_then(|path| gdk::Texture::from_filename(path).ok());
                if preview.is_some() {
                    widget.set_image(generation, target, preview.clone());
                }
                let texture = embedded_texture(state, target).await;
                widget.set_image(generation, target, texture.or(preview));
            });
        }
        let (session, drawings) = widget.drawing_requests();
        for drawing in drawings {
            let (widget, state) = (widget.clone(), state.clone());
            if let Drawing::Table(table) = &drawing {
                let table = table.clone();
                let revision = widget.table_revision();
                glib::spawn_future_local(async move {
                    let refs = table_refs(state, &table).await;
                    widget.set_table(session, revision, drawing, refs);
                });
                continue;
            }
            let (kind, source) = match &drawing {
                Drawing::InlineMath(s) => (Kind::InlineMath, s.clone()),
                Drawing::DisplayMath(s) => (Kind::DisplayMath, s.clone()),
                Drawing::Diagram(s) => (Kind::Mermaid, s.clone()),
                _ => continue,
            };
            glib::spawn_future_local(async move {
                let image = core(async move { state.render(kind, source).await }).await;
                let result = image.map_err(|e| e.to_string()).and_then(|image| {
                    let size = zib_core::editor::display::ImageSize {
                        width: image.width,
                        height: image.height,
                    };
                    gdk::Texture::from_bytes(&glib::Bytes::from(&image.png))
                        .map(|texture| (texture, size))
                        .map_err(|e| e.to_string())
                });
                widget.set_drawing(session, drawing, result);
            });
        }
    }

    pub(crate) fn agent_notes_changed(&self) {
        self.reload_notes();
    }

    /// Shows the sync status in the header, every few seconds.
    fn watch_status(&self) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            loop {
                let state = w.inner().state.clone();
                let status = core(async move { state.status().await }).await;
                let (icon, text, class) = match status {
                    SyncState::Synced => {
                        ("emblem-ok-symbolic", "Every machine has every change", "success")
                    }
                    SyncState::PartlySynced => {
                        ("emblem-synchronizing-symbolic", "Some machines lack changes", "warning")
                    }
                    SyncState::NotSynced => (
                        "network-offline-symbolic",
                        "Changes here haven't reached another machine",
                        "error",
                    ),
                };
                let button = &w.inner().status;
                button.set_icon_name(icon);
                button.set_tooltip_text(Some(text));
                for name in ["success", "warning", "error"] {
                    button.remove_css_class(name);
                }
                button.add_css_class(class);
                glib::timeout_future(Duration::from_secs(5)).await;
            }
        });
    }
}

/// Receives the next live event from the core's channel, on GTK's main loop.
async fn core_recv(events: &mut tokio::sync::mpsc::Receiver<LiveEvent>) -> Option<LiveEvent> {
    events.recv().await
}

/// Opens a file in the desktop's app for its type.
pub fn open_path(path: &std::path::Path, window: &Window) {
    let uri = gio::File::for_path(path).uri();
    if let Err(error) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
        window.toast(&format!("No app opens this file: {error}"));
    }
}

/// Asks for a line of text.
pub async fn ask(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    text: &str,
    action: &str,
) -> Option<String> {
    let entry = gtk::Entry::builder().text(text).activates_default(true).build();
    let dialog = adw::AlertDialog::new(Some(heading), None);
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", action)]);
    dialog.set_default_response(Some("ok"));
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_extra_child(Some(&entry));
    // The dialog focuses its buttons as it opens, so the entry takes the focus once it shows.
    entry.connect_map(|entry| {
        let entry = entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
        });
    });
    let response = dialog.choose_future(Some(parent)).await;
    let name = entry.text().trim().to_owned();
    (response == "ok" && !name.is_empty()).then_some(name)
}

/// Escapes text for Pango markup.
fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// A search snippet as Pango markup, with each match in bold. The markers are split out before
/// escaping, which would otherwise turn them into character references.
fn snippet_markup(snippet: &str) -> String {
    let mut out = String::new();
    for (i, part) in snippet.replace('\n', " ").split([MATCH_START, MATCH_END]).enumerate() {
        if i % 2 == 1 {
            let _ = write!(out, "<b>{}</b>", escape(part));
        } else {
            out.push_str(&escape(part));
        }
    }
    out
}

/// Lets Up and Down in a search entry move the selection in `list`.
fn arrow_keys(list: &gtk::ListBox) -> gtk::EventControllerKey {
    let list = list.clone();
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, _| {
        let step = match key {
            gdk::Key::Down => 1,
            gdk::Key::Up => -1,
            _ => return glib::Propagation::Proceed,
        };
        let at = list.selected_row().map_or(-1, |row| row.index()) + step;
        if let Some(row) = list.row_at_index(at.max(0)) {
            list.select_row(Some(&row));
        }
        glib::Propagation::Stop
    });
    keys
}

/// Resolves drawing images and linked titles throughout a table, including nested cells.
async fn table_refs(state: Arc<State>, table: &zib_core::body::ListTable) -> tables::Refs {
    let targets = tables::link_targets(table);
    let drawings = tables::drawings(table);
    core(async move {
        let mut refs = tables::Refs { titles: state.titles(&targets).await, ..Default::default() };
        for (kind, source) in drawings {
            let key = tables::drawing_key(kind, &source);
            if let Ok(image) = state.render(kind, source).await {
                refs.images.insert(key, image);
            }
        }
        refs
    })
    .await
}

/// The original image, fetching its blob when this machine doesn't hold it yet.
async fn embedded_texture(state: Arc<State>, target: Id) -> Option<gdk::Texture> {
    let path = core(async move {
        let space = state.find_note(target).await?;
        if state.note(space, target).await.ok()?.type_name != "image" {
            return None;
        }
        let (hash, _) = state.note_file(space, target, "image").await.ok()??;
        state.blob_path(space, hash).await.ok()
    })
    .await?;
    gdk::Texture::from_filename(&path).ok()
}

/// Adds a clipboard image as a note in the current space.
async fn pasted_image(
    state: Arc<State>,
    space: Id,
    clipboard: gdk::Clipboard,
) -> Result<(Id, String), String> {
    let now = glib::DateTime::now_local().and_then(|t| t.format("%F %H.%M.%S"));
    let name =
        now.map_or_else(|_| "Pasted image.png".to_owned(), |now| format!("Pasted image {now}.png"));
    let texture = clipboard
        .read_texture_future()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "the clipboard is empty".to_owned())?;
    let bytes = texture.save_to_png_bytes().to_vec();
    let title = name.clone();
    let note =
        core(async move { state.add_bytes(space, bytes, title, "image/png".to_owned()).await })
            .await
            .map_err(|error| error.to_string())?;
    Ok((note, name))
}
