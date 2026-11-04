//! The main window: the spaces that show, one list of their notes, search, and the note editor
//! and its preview.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use zib_core::body::{self, ShownTitles};
use zib_core::editing::{LinkTarget, link_at};
use zib_core::search::{MATCH_END, MATCH_START};
use zib_core::sync::{LiveEvent, Selection, SyncState};
use zib_core::{Channel, Id};

use crate::app::core;
use crate::editor;
use crate::state::State;

/// How long the editor waits after typing stops before it commits, and how long while another
/// machine edits the same note live.
const COMMIT_PAUSE: Duration = Duration::from_secs(3);
const COMMIT_LIVE: Duration = Duration::from_millis(50);

/// The window, which the instance opens on `activate`.
#[derive(Clone)]
pub struct Window(Rc<Inner>);

pub struct Inner {
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
    title: adw::WindowTitle,
    stack: gtk::Stack,
    buffer: gtk::TextBuffer,
    preview_toggle: gtk::ToggleButton,
    preview: gtk::ScrolledWindow,
    open_file: gtk::Button,
    status: gtk::Button,
    editor: RefCell<Option<Editor>>,
    loading: Cell<bool>,
}

/// The note open in the editor.
struct Editor {
    space: Id,
    note: Id,
    type_name: String,
    /// The body as stored, without filled-in labels.
    stored: String,
    shown: ShownTitles,
    insertions: Vec<(usize, usize)>,
    titles: HashMap<Id, String>,
    timer: Option<glib::SourceId>,
    live: Cell<bool>,
    cursors: Option<tokio::sync::broadcast::Sender<(Id, Vec<Selection>)>>,
}

impl Window {
    // The window's widgets read best built in one place.
    #[allow(clippy::too_many_lines)]
    pub fn new(app: &adw::Application, state: Arc<State>) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(state.channel.display_name())
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
        let open_file = gtk::Button::from_icon_name("document-open-symbolic");
        open_file.set_tooltip_text(Some("Open the file"));
        open_file.set_visible(false);
        let preview_toggle = gtk::ToggleButton::builder()
            .icon_name("view-reveal-symbolic")
            .tooltip_text("Preview")
            .build();
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Delete the note"));
        let status = gtk::Button::from_icon_name("emblem-synchronizing-symbolic");
        status.add_css_class("flat");
        content_header.pack_start(&open_file);
        content_header.pack_end(&status);
        content_header.pack_end(&delete);
        content_header.pack_end(&preview_toggle);

        let view = editor::new_view();
        let buffer = view.buffer();
        let editor_scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let preview = gtk::ScrolledWindow::builder().vexpand(true).build();
        let empty = adw::StatusPage::builder()
            .icon_name("accessories-text-editor-symbolic")
            .title(state.channel.display_name())
            .description("Pick a note, or make a new one.")
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&editor_scroll, Some("editor"));
        stack.add_named(&preview, Some("preview"));
        let content = adw::ToolbarView::new();
        content.add_top_bar(&content_header);
        content.set_content(Some(&stack));

        let split = adw::NavigationSplitView::new();
        split.set_sidebar(Some(&adw::NavigationPage::new(&sidebar, "Notes")));
        split.set_content(Some(&adw::NavigationPage::new(&content, "Note")));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        let this = Self(Rc::new(Inner {
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
            title,
            stack,
            buffer,
            preview_toggle,
            preview,
            open_file,
            status,
            editor: RefCell::default(),
            loading: Cell::new(false),
        }));
        this.connect(&new_note, &add_file, &delete, &view);
        this.reload_spaces();
        this.watch_status();
        this
    }

    fn inner(&self) -> &Inner {
        &self.0
    }

    pub fn present(&self) {
        self.inner().window.present();
    }

    pub fn toast(&self, text: &str) {
        self.inner().toasts.add_toast(adw::Toast::new(text));
    }

    fn connect(
        &self,
        new_note: &adw::SplitButton,
        add_file: &gtk::Button,
        delete: &gtk::Button,
        view: &sourceview5::View,
    ) {
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
        self.inner().preview_toggle.connect_toggled(move |_| w.show_mode());
        let w = self.clone();
        self.inner().buffer.connect_changed(move |_| w.edited());
        let w = self.clone();
        self.inner().buffer.connect_mark_set(move |_, _, mark| {
            if mark.name().as_deref() == Some("insert") {
                w.cursor_moved();
            }
        });
        let w = self.clone();
        let links = gtk::GestureClick::new();
        links.connect_released(move |gesture, _, x, y| {
            let modifiers = gesture.current_event_state();
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && w.open_link_at(x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        view.add_controller(links);
        let w = self.clone();
        self.inner().window.connect_close_request(move |window| {
            w.close_editor();
            window.set_visible(false);
            glib::Propagation::Stop
        });
        let w = self.clone();
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(move |_, value, _, _| {
            if let Ok(files) = value.get::<gdk::FileList>() {
                w.add_files(files.files());
            }
            true
        });
        view.add_controller(target);
        let actions = gio::SimpleActionGroup::new();
        for (name, handler) in [
            ("new-space", Self::new_space as fn(&Self)),
            ("rename-space", Self::rename_space as fn(&Self)),
            ("add-machine", Self::add_machine as fn(&Self)),
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
        if !self.inner().spaces.borrow().iter().any(|(id, _, _)| *id == space) {
            self.reload_spaces();
            return;
        }
        self.reload_notes();
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        if let Some((open_space, note)) = open.filter(|(s, _)| *s == space) {
            self.refresh_open(open_space, note);
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
        glib::spawn_future_local(async move {
            match core(async move { state.find_note(note).await }).await {
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
                w.open_note(space, note);
                w.inner().preview_toggle.set_active(false);
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
            let _ = core(async move { state.delete_note(space, note).await }).await;
            w.inner().stack.set_visible_child_name("empty");
            w.reload_notes();
        });
    }

    /// Opens a note in the editor: syncs its space, starts live sessions, and fills in labels.
    pub fn open_note(&self, space: Id, note: Id) {
        self.close_editor();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let loaded = {
                let state = state.clone();
                core(async move {
                    let note = state.note(space, note).await?;
                    let targets: Vec<Id> = body::link_targets(&note.body).into_iter().collect();
                    let titles = state.titles(&targets).await;
                    Ok::<_, zib_core::Error>((note, titles))
                })
                .await
            };
            let Ok((loaded, titles)) = loaded else {
                w.toast("This note isn't here any more");
                return;
            };
            let mut shown = ShownTitles::default();
            let (text, insertions) = body::fill_labels_mapped(
                &loaded.body,
                &mut |id| titles.get(&id).cloned(),
                &mut shown,
            );
            let editor = Editor {
                space,
                note,
                type_name: loaded.type_name.clone(),
                stored: loaded.body.clone(),
                shown,
                insertions,
                titles,
                timer: None,
                live: Cell::new(false),
                cursors: None,
            };
            w.set_text(&text);
            *w.inner().editor.borrow_mut() = Some(editor);
            w.inner()
                .open_file
                .set_visible(loaded.type_name == "file" || loaded.type_name == "image");
            w.update_title();
            w.show_mode();
            let opened = core({
                let state = state.clone();
                async move { state.open_note(space, note).await }
            })
            .await;
            if let Ok(mut opened) = opened {
                if let Some(editor) = w.inner().editor.borrow_mut().as_mut() {
                    editor.cursors = Some(opened.cursors.clone());
                }
                let listening = w.clone();
                glib::spawn_future_local(async move {
                    while let Some(event) = core_recv(&mut opened.events).await {
                        listening.live_event(space, note, event);
                    }
                });
            }
        });
    }

    fn set_text(&self, text: &str) {
        self.inner().loading.set(true);
        self.inner().buffer.set_text(text);
        self.inner().loading.set(false);
    }

    fn update_title(&self) {
        let editor = self.inner().editor.borrow();
        let Some(editor) = editor.as_ref() else { return };
        let title = body::title(&editor.stored, &mut |id| {
            editor
                .titles
                .get(&id)
                .map(|t| body::Title { plain: body::plain_text(t), markdown: t.clone() })
        });
        let shown = if title.is_empty() { "Untitled".to_owned() } else { title.plain };
        self.inner().title.set_title(&shown);
        self.inner().title.set_subtitle(if editor.live.get() { "Editing live" } else { "" });
    }

    /// The owner typed: commit after a pause, or at once while editing live.
    fn edited(&self) {
        if self.inner().loading.get() {
            return;
        }
        let mut editor = self.inner().editor.borrow_mut();
        let Some(editor) = editor.as_mut() else { return };
        if let Some(timer) = editor.timer.take() {
            timer.remove();
        }
        let wait = if editor.live.get() { COMMIT_LIVE } else { COMMIT_PAUSE };
        let w = self.clone();
        editor.timer = Some(glib::timeout_add_local_once(wait, move || {
            if let Some(editor) = w.inner().editor.borrow_mut().as_mut() {
                editor.timer = None;
            }
            w.commit();
        }));
    }

    /// Stores the editor's text without filled-in labels, applying only the differences.
    fn commit(&self) {
        let (space, note, stripped) = {
            let mut editor = self.inner().editor.borrow_mut();
            let Some(editor) = editor.as_mut() else { return };
            let buffer = &self.inner().buffer;
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
            let titles = editor.titles.clone();
            let stripped =
                body::strip_labels(&text, &mut |id| titles.get(&id).cloned(), &editor.shown);
            if stripped == editor.stored {
                return;
            }
            editor.stored.clone_from(&stripped);
            // Labels the owner kept, and new links, change where filled-in labels sit.
            let mut shown = editor.shown.clone();
            let (_, insertions) =
                body::fill_labels_mapped(&stripped, &mut |id| titles.get(&id).cloned(), &mut shown);
            editor.insertions = insertions;
            (editor.space, editor.note, stripped)
        };
        self.update_title();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let _ = core(async move { state.save_body(space, note, &stripped).await }).await;
            w.reload_notes();
        });
    }

    /// Commits pending typing, and ends the note's live sessions.
    fn close_editor(&self) {
        let pending = self.inner().editor.borrow_mut().as_mut().and_then(|e| e.timer.take());
        if let Some(timer) = pending {
            timer.remove();
            self.commit();
        }
        if let Some(editor) = self.inner().editor.borrow_mut().take() {
            let state = self.inner().state.clone();
            let (space, note) = (editor.space, editor.note);
            glib::spawn_future_local(async move {
                core(async move { state.close_note(space, note) }).await;
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
        match event {
            LiveEvent::Changed => {
                if let Some(editor) = self.inner().editor.borrow().as_ref() {
                    editor.live.set(true);
                }
                self.refresh_open(space, note);
            }
            LiveEvent::Cursor { selections, .. } => {
                if let Some(editor) = self.inner().editor.borrow().as_ref() {
                    editor.live.set(true);
                }
                self.update_title();
                self.show_remote(space, selections);
            }
            LiveEvent::Ended => {
                if let Some(editor) = self.inner().editor.borrow().as_ref() {
                    editor.live.set(false);
                }
                let buffer = &self.inner().buffer;
                buffer.remove_tag_by_name(editor::REMOTE, &buffer.start_iter(), &buffer.end_iter());
                self.update_title();
            }
        }
    }

    /// Reloads the open note after another machine changed it, keeping the owner's typing.
    fn refresh_open(&self, space: Id, note: Id) {
        let pending = self.inner().editor.borrow_mut().as_mut().and_then(|e| e.timer.take());
        if let Some(timer) = pending {
            timer.remove();
            self.commit();
        }
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let loaded = core(async move {
                let note = state.note(space, note).await?;
                let targets: Vec<Id> = body::link_targets(&note.body).into_iter().collect();
                let titles = state.titles(&targets).await;
                Ok::<_, zib_core::Error>((note.body, titles))
            })
            .await;
            let Ok((stored, titles)) = loaded else { return };
            let text = {
                let mut editor = w.inner().editor.borrow_mut();
                let Some(editor) = editor.as_mut().filter(|e| e.space == space && e.note == note)
                else {
                    return;
                };
                if editor.stored == stored && editor.titles == titles {
                    return;
                }
                let (text, insertions) = body::fill_labels_mapped(
                    &stored,
                    &mut |id| titles.get(&id).cloned(),
                    &mut editor.shown,
                );
                editor.stored = stored;
                editor.insertions = insertions;
                editor.titles = titles;
                text
            };
            let buffer = &w.inner().buffer;
            let cursor = buffer.cursor_position();
            w.set_text(&text);
            buffer.place_cursor(&buffer.iter_at_offset(cursor.min(buffer.char_count())));
            w.update_title();
            if w.inner().preview_toggle.is_active() {
                w.show_mode();
            }
        });
    }

    /// Sends the owner's cursor and selection to machines editing the same note.
    fn cursor_moved(&self) {
        let (space, note, sender, insertions) = {
            let editor = self.inner().editor.borrow();
            let Some(editor) = editor.as_ref() else { return };
            let Some(sender) = editor.cursors.clone().filter(|_| editor.live.get()) else { return };
            (editor.space, editor.note, sender, editor.insertions.clone())
        };
        let buffer = &self.inner().buffer;
        let (anchor, head) = buffer.selection_bounds().map_or_else(
            || {
                let at = buffer.cursor_position();
                (at, at)
            },
            |(a, b)| (a.offset(), b.offset()),
        );
        let to_stored =
            |offset: i32| body::shown_to_stored(usize::try_from(offset).unwrap_or(0), &insertions);
        let (anchor, head) = (to_stored(anchor), to_stored(head));
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let encoded = core(async move {
                let a = state.cursor_at(space, note, anchor).await?;
                let h = state.cursor_at(space, note, head).await?;
                Some(Selection { anchor: a, head: h })
            })
            .await;
            if let Some(selection) = encoded {
                let _ = sender.send((note, vec![selection]));
            }
        });
    }

    /// Marks another machine's cursors and selections in the editor.
    fn show_remote(&self, space: Id, selections: Vec<Selection>) {
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let positions = core(async move {
                let mut out = Vec::new();
                for selection in selections {
                    if let (Some(a), Some(h)) = (
                        state.cursor_position(space, &selection.anchor).await,
                        state.cursor_position(space, &selection.head).await,
                    ) {
                        out.push((a.min(h), a.max(h)));
                    }
                }
                out
            })
            .await;
            let insertions = w
                .inner()
                .editor
                .borrow()
                .as_ref()
                .map(|e| e.insertions.clone())
                .unwrap_or_default();
            let buffer = &w.inner().buffer;
            buffer.remove_tag_by_name(editor::REMOTE, &buffer.start_iter(), &buffer.end_iter());
            for (start, end) in positions {
                let start = i32::try_from(body::stored_to_shown(start, &insertions)).unwrap_or(0);
                let end = i32::try_from(body::stored_to_shown(end, &insertions))
                    .unwrap_or(0)
                    .max(start + 1);
                buffer.apply_tag_by_name(
                    editor::REMOTE,
                    &buffer.iter_at_offset(start),
                    &buffer.iter_at_offset(end),
                );
            }
        });
    }

    /// Shows the editor or the preview.
    fn show_mode(&self) {
        // The preview shows what's stored, so typing that waits to commit goes first.
        if self.inner().preview_toggle.is_active() {
            let pending = self.inner().editor.borrow_mut().as_mut().and_then(|e| e.timer.take());
            if let Some(timer) = pending {
                timer.remove();
                self.commit();
            }
        }
        let open = self
            .inner()
            .editor
            .borrow()
            .as_ref()
            .map(|e| (e.space, e.stored.clone(), e.titles.clone()));
        let Some((space, stored, titles)) = open else {
            self.inner().stack.set_visible_child_name("empty");
            return;
        };
        if self.inner().preview_toggle.is_active() {
            let preview =
                crate::preview::build(&self.0.state, space, &stored, &titles, self.clone());
            self.inner().preview.set_child(Some(&preview));
            self.inner().stack.set_visible_child_name("preview");
        } else {
            self.inner().stack.set_visible_child_name("editor");
        }
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

    /// Opens the wikilink or web link under a point in the editor. Returns whether there was one.
    fn open_link_at(&self, x: f64, y: f64) -> bool {
        let Some(view) = self.inner().stack.child_by_name("editor").and_then(|scroll| {
            scroll.downcast::<gtk::ScrolledWindow>().ok()?.child()?.downcast::<gtk::TextView>().ok()
        }) else {
            return false;
        };
        #[allow(clippy::cast_possible_truncation)]
        let (bx, by) =
            view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let Some(iter) = view.iter_at_location(bx, by) else { return false };
        let text = editor::text(&self.inner().buffer);
        let byte = editor::byte_at(&text, iter.offset());
        match link_at(&text, byte) {
            Some(LinkTarget::Note(note)) => self.open_uri(&format!("zib:note/{note}")),
            Some(LinkTarget::Web(url)) => {
                let launched =
                    gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>);
                if launched.is_err() {
                    self.toast(&format!("No app opens {url}"));
                }
            }
            None => return false,
        }
        true
    }

    fn add_machine(&self) {
        crate::adding::show(self, self.inner().state.clone());
    }

    pub fn gtk_window(&self) -> gtk::Window {
        self.inner().window.clone().upcast()
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
