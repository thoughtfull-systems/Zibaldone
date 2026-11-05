//! The main window: the spaces that show, one list of their notes, search, and the note editor.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use zib_core::body::{self, ShownTitles};
use zib_core::editing::{Drawing, LinkTarget, link_at};
use zib_core::render::Kind;
use zib_core::search::{MATCH_END, MATCH_START};
use zib_core::sync::{LiveEvent, Selection, SyncState};
use zib_core::{Channel, Id};

use crate::app::core;
use crate::drawings::Draw;
use crate::editor;
use crate::state::State;
use crate::tables;

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
    view: sourceview5::View,
    buffer: gtk::TextBuffer,
    backlinks: gtk::MenuButton,
    /// The notes opened before the open one, the latest last, and those to go forward to again.
    back: RefCell<Vec<(Id, Id)>>,
    forward: RefCell<Vec<(Id, Id)>>,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    open_file: gtk::Button,
    status: gtk::MenuButton,
    editor: RefCell<Option<Editor>>,
    loading: Cell<bool>,
    /// Whether the note that opens next takes the focus, as a new note does.
    focus_body: Cell<bool>,
    /// Other machines' cursors in the open note.
    remote: Rc<crate::remote::Remote>,
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
    /// Link targets whose titles the editor asked for, so it asks once.
    asked: std::collections::HashSet<Id>,
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

        let draw: Draw = {
            let state = state.clone();
            Rc::new(move |drawing, done| {
                let (state, drawing) = (state.clone(), drawing.clone());
                glib::spawn_future_local(async move { done(draw(state, drawing).await) });
            })
        };
        let view = editor::new_view(draw);
        let buffer = view.buffer();
        let editor_scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
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
            view: view.clone(),
            buffer,
            backlinks,
            back: RefCell::default(),
            forward: RefCell::default(),
            back_button,
            forward_button,
            open_file,
            status,
            editor: RefCell::default(),
            loading: Cell::new(false),
            focus_body: Cell::new(false),
            remote: crate::remote::Remote::new(view.upcast_ref()),
        }));
        this.connect(&new_note, &add_file, &delete, &view);
        this.connect_files(&view);
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
        self.inner().back_button.connect_clicked(move |_| w.go(true));
        let w = self.clone();
        self.inner().forward_button.connect_clicked(move |_| w.go(false));
        let w = self.clone();
        self.inner().buffer.connect_changed(move |_| w.edited());
        let w = self.clone();
        self.inner().buffer.connect_insert_text(move |_, at, text| w.maybe_pick_link(at, text));
        let w = self.clone();
        self.inner().backlinks.set_create_popup_func(move |button| w.show_backlinks(button));
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
        let actions = gio::SimpleActionGroup::new();
        for (name, handler) in [
            ("new-space", Self::new_space as fn(&Self)),
            ("rename-space", Self::rename_space as fn(&Self)),
            ("add-machine", Self::add_machine as fn(&Self)),
            ("share-space", |w: &Self| crate::sharing::share(w, w.inner().state.clone())),
            ("move-note", Self::move_note),
            ("move-notes", |w: &Self| crate::sharing::move_all(w, w.inner().state.clone())),
            ("print", |w: &Self| w.print(false)),
            ("print-linked", |w: &Self| w.print(true)),
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

    /// Loads a note into the editor: syncs its space, starts live sessions, and fills in labels.
    fn load_note(&self, space: Id, note: Id) {
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
                asked: std::collections::HashSet::new(),
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
            if w.inner().focus_body.take() {
                w.inner().view.grab_focus();
            }
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

    /// Changes the text to `text` by replacing only what differs between the parts the two share
    /// at the start and at the end. The owner's cursor and selection, and other machines' cursors,
    /// then keep their place in the text around them, as another machine's edit arrives.
    fn replace_text(&self, text: &str) {
        let buffer = &self.inner().buffer;
        let old: Vec<char> = editor::text(buffer).chars().collect();
        let new: Vec<char> = text.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let offset = |chars: usize| i32::try_from(chars).unwrap_or(i32::MAX);
        let middle: String = new[prefix..new.len() - suffix].iter().collect();
        self.inner().loading.set(true);
        buffer.begin_irreversible_action();
        let (mut from, mut to) = (
            buffer.iter_at_offset(offset(prefix)),
            buffer.iter_at_offset(offset(old.len() - suffix)),
        );
        buffer.delete(&mut from, &mut to);
        buffer.insert(&mut buffer.iter_at_offset(offset(prefix)), &middle);
        buffer.end_irreversible_action();
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
        let mut open = self.inner().editor.borrow_mut();
        let Some(editor) = open.as_mut() else { return };
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
        drop(open);
        self.fill_new_labels();
    }

    /// Gives links the owner wrote without a label their target's title, as links show when a
    /// note opens, asking the core for titles the editor lacks.
    fn fill_new_labels(&self) {
        let text = editor::text(&self.inner().buffer);
        let links = body::wikilinks(&text);
        let (note, wanted) = {
            let mut editor = self.inner().editor.borrow_mut();
            let Some(editor) = editor.as_mut() else { return };
            let wanted: Vec<Id> = links
                .iter()
                .filter(|link| link.label.is_none() && !editor.titles.contains_key(&link.target))
                .map(|link| link.target)
                .filter(|target| editor.asked.insert(*target))
                .collect();
            (editor.note, wanted)
        };
        if wanted.is_empty() {
            // After the buffer finishes reporting the change that called this.
            let w = self.clone();
            glib::idle_add_local_once(move || w.insert_labels());
            return;
        }
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let titles = core(async move { state.titles(&wanted).await }).await;
            if let Some(editor) = w.inner().editor.borrow_mut().as_mut().filter(|e| e.note == note)
            {
                editor.titles.extend(titles);
            }
            w.insert_labels();
        });
    }

    /// Puts the title the editor knows into each link without a label, except a link the cursor is
    /// in, which the owner may still be writing.
    fn insert_labels(&self) {
        let buffer = &self.inner().buffer;
        let text = editor::text(buffer);
        let cursor = editor::byte_at(&text, buffer.cursor_position());
        let mut labels = Vec::new();
        {
            let mut editor = self.inner().editor.borrow_mut();
            let Some(editor) = editor.as_mut() else { return };
            for link in body::wikilinks(&text) {
                let typing = link.range.start < cursor && cursor < link.range.end;
                let title = editor.titles.get(&link.target).filter(|title| !title.is_empty());
                let Some(title) = title.filter(|_| link.label.is_none() && !typing) else {
                    continue;
                };
                editor.shown.remember(link.target, title);
                let at = text[..link.range.end - 2].chars().count();
                labels.push((at, format!("|{}", body::escape_label(title))));
            }
        }
        // From the end, so each insertion leaves the offsets before it as they were.
        for (at, label) in labels.into_iter().rev() {
            buffer.insert(&mut buffer.iter_at_offset(i32::try_from(at).unwrap_or(0)), &label);
        }
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
            // Labels the owner kept, and new links and their filled-in labels, change where
            // filled-in labels sit, even when the stored text stays the same.
            let mut shown = editor.shown.clone();
            let (_, insertions) =
                body::fill_labels_mapped(&stripped, &mut |id| titles.get(&id).cloned(), &mut shown);
            editor.insertions = insertions;
            if stripped == editor.stored {
                return;
            }
            editor.stored.clone_from(&stripped);
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
        self.inner().remote.clear();
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
        let remote = &self.inner().remote;
        match event {
            LiveEvent::Started { machine } => {
                remote.started(machine);
                // The other machine sees this one's cursor at once, not at the next move.
                self.cursor_moved();
            }
            LiveEvent::Changed { machine } => {
                remote.started(machine);
                self.refresh_open(space, note);
                let state = self.inner().state.clone();
                glib::spawn_future_local(async move {
                    core(async move { state.live_changed(space).await }).await;
                });
            }
            LiveEvent::Cursor { machine, selections } => {
                let name = self.inner().state.clone();
                let w = self.clone();
                glib::spawn_future_local(async move {
                    let name = core(async move { name.machine_name(space, machine).await })
                        .await
                        .unwrap_or_default();
                    w.inner().remote.received(machine, name, selections);
                    w.place_remote(space);
                });
            }
            LiveEvent::Ended { machine } => remote.ended(machine),
        }
        if let Some(editor) = self.inner().editor.borrow().as_ref() {
            editor.live.set(remote.live());
        }
        self.update_title();
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
            w.replace_text(&text);
            w.update_title();
            // Replacing the text moved every mark, so the cursors go back where they belong.
            w.place_remote(space);
        });
    }

    /// Sends the owner's cursor and selection to machines editing the same note.
    fn cursor_moved(&self) {
        let (space, note, sender, insertions) = {
            let editor = self.inner().editor.borrow();
            let Some(editor) = editor.as_ref() else { return };
            let Some(sender) = editor.cursors.clone() else { return };
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

    /// Turns each machine's cursors into places in the text, and shows them there.
    fn place_remote(&self, space: Id) {
        let w = self.clone();
        let state = self.inner().state.clone();
        let selections = self.inner().remote.selections();
        glib::spawn_future_local(async move {
            let positions = core(async move {
                let mut out = Vec::new();
                for (machine, selections) in selections {
                    let mut places = Vec::new();
                    for selection in selections {
                        if let (Some(a), Some(h)) = (
                            state.cursor_position(space, &selection.anchor).await,
                            state.cursor_position(space, &selection.head).await,
                        ) {
                            places.push((a, h));
                        }
                    }
                    out.push((machine, places));
                }
                out
            })
            .await;
            let insertions = w
                .inner()
                .editor
                .borrow()
                .as_ref()
                .filter(|e| e.space == space)
                .map(|e| e.insertions.clone());
            let Some(insertions) = insertions else { return };
            let shown = |stored: usize| {
                i32::try_from(body::stored_to_shown(stored, &insertions)).unwrap_or(i32::MAX)
            };
            for (machine, places) in positions {
                let offsets: Vec<(i32, i32)> =
                    places.into_iter().map(|(a, h)| (shown(a), shown(h))).collect();
                w.inner().remote.place_at(machine, &offsets);
            }
        });
    }

    /// Sends the owner's cursor again while it stays still, and drops other machines' cursors
    /// that went quiet, as live editing in the sync specification asks.
    fn resend_cursor(&self) {
        let w = Rc::downgrade(&self.0);
        glib::timeout_add_local(crate::remote::RESEND, move || {
            let Some(w) = w.upgrade().map(Self) else { return glib::ControlFlow::Break };
            w.cursor_moved();
            w.inner().remote.expire();
            glib::ControlFlow::Continue
        });
    }

    /// Shows the editor, or the empty page when no note is open.
    fn show_mode(&self) {
        let open = self.inner().editor.borrow().is_some();
        self.inner().stack.set_visible_child_name(if open { "editor" } else { "empty" });
    }

    /// Adds files dropped on the editor, and images pasted into it, as notes.
    fn connect_files(&self, view: &sourceview5::View) {
        let w = self.clone();
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(move |_, value, _, _| {
            if let Ok(files) = value.get::<gdk::FileList>() {
                w.add_files(files.files());
            }
            true
        });
        view.add_controller(target);
        let w = self.clone();
        view.connect_paste_clipboard(move |view| {
            if w.paste_image() {
                view.stop_signal_emission_by_name("paste-clipboard");
            }
        });
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

    /// Pastes an image from the clipboard as an `image` note in the open note's space, with a link
    /// to it in place of the selection. Returns whether the clipboard held an image to paste. A
    /// clipboard that also holds text, as a copied spreadsheet range does, pastes the text.
    fn paste_image(&self) -> bool {
        let Some(space) = self.inner().editor.borrow().as_ref().map(|e| e.space) else {
            return false;
        };
        let clipboard = self.inner().view.clipboard();
        let formats = clipboard.formats().union_deserialize_types();
        if !formats.contains_type(gdk::Texture::static_type())
            || formats.contains_type(glib::Type::STRING)
        {
            return false;
        }
        let buffer = &self.inner().buffer;
        let (from, to) = buffer.selection_bounds().unwrap_or_else(|| {
            let cursor = buffer.iter_at_mark(&buffer.get_insert());
            (cursor, cursor)
        });
        let start = buffer.create_mark(None, &from, true);
        let end = buffer.create_mark(None, &to, false);
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let added = match clipboard.read_texture_future().await {
                Ok(Some(texture)) => {
                    let bytes = texture.save_to_png_bytes().to_vec();
                    let now = glib::DateTime::now_local().and_then(|t| t.format("%F %H.%M.%S"));
                    let name = now.map_or_else(
                        |_| "Pasted image.png".to_owned(),
                        |now| format!("Pasted image {now}.png"),
                    );
                    let media = "image/png".to_owned();
                    core(async move { state.add_bytes(space, bytes, name, media).await })
                        .await
                        .map_err(|e| e.to_string())
                }
                Ok(None) => Err("the clipboard is empty".to_owned()),
                Err(error) => Err(error.to_string()),
            };
            match added {
                Ok(note) => {
                    w.reload_notes();
                    w.insert_link(&start, &end, note, None);
                }
                Err(error) => {
                    let buffer = &w.inner().buffer;
                    buffer.delete_mark(&start);
                    buffer.delete_mark(&end);
                    w.toast(&format!("Couldn't paste the image: {error}"));
                }
            }
        });
        true
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
        let pending = self.inner().editor.borrow_mut().as_mut().and_then(|e| e.timer.take());
        if let Some(timer) = pending {
            timer.remove();
            self.commit();
        }
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

    /// Opens the link picker when the owner types `[[`, or wraps a selection in it, which then
    /// becomes the label. Runs before the buffer inserts `text` at `at`.
    fn maybe_pick_link(&self, at: &gtk::TextIter, text: &str) {
        if self.inner().loading.get() || self.inner().editor.borrow().is_none() {
            return;
        }
        let mut before = *at;
        let typed = text == "[" && before.backward_char() && before.char() == '[';
        let wrapped = text.len() > 4
            && text.starts_with("[[")
            && text.ends_with("]]")
            && !text[2..text.len() - 2].contains('\n')
            && body::wikilinks(text).is_empty();
        if !typed && !wrapped {
            return;
        }
        let start = if typed { at.offset() - 1 } else { at.offset() };
        let end = at.offset() + i32::try_from(text.chars().count()).unwrap_or(0);
        let label = wrapped.then(|| text[2..text.len() - 2].to_owned());
        let w = self.clone();
        // The picker works on the text after the buffer inserts it.
        glib::idle_add_local_once(move || w.open_link_picker(start, end, label));
    }

    /// Shows a search for the note to link to, which replaces the characters from `start` to `end`
    /// with a link to it.
    fn open_link_picker(&self, start: i32, end: i32, label: Option<String>) {
        let view = &self.inner().view;
        let buffer = &self.inner().buffer;
        let start = buffer.create_mark(None, &buffer.iter_at_offset(start), true);
        let end = buffer.create_mark(None, &buffer.iter_at_offset(end), false);
        let place = view.iter_location(&buffer.iter_at_mark(&end));
        let (x, y) =
            view.buffer_to_window_coords(gtk::TextWindowType::Widget, place.x(), place.y());
        let popover = gtk::Popover::builder()
            .pointing_to(&gdk::Rectangle::new(x, y, 1, place.height()))
            .position(gtk::PositionType::Bottom)
            .build();
        popover.set_parent(view);
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
        // Whether the owner picked a note. The link goes in once its title arrives, so the marks
        // outlive the popover.
        let chosen = Rc::new(Cell::new(false));
        let pick = {
            let (w, popover, found) = (self.clone(), popover.clone(), found.clone());
            let (start, end, chosen) = (start.clone(), end.clone(), chosen.clone());
            move |index: i32| {
                let target =
                    usize::try_from(index).ok().and_then(|i| found.borrow().get(i).copied());
                if let Some(target) = target
                    && !chosen.replace(true)
                {
                    w.insert_link(&start, &end, target, label.clone());
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
        let w = self.clone();
        popover.connect_closed(move |popover| {
            if !chosen.get() {
                let buffer = &w.inner().buffer;
                buffer.delete_mark(&start);
                buffer.delete_mark(&end);
            }
            w.inner().view.grab_focus();
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

    /// Replaces the text between two marks with a link to `target`, and deletes the marks. A link
    /// without a label the owner chose shows its target's title, which the editor removes again
    /// when it commits.
    fn insert_link(
        &self,
        start: &gtk::TextMark,
        end: &gtk::TextMark,
        target: Id,
        label: Option<String>,
    ) {
        let w = self.clone();
        let state = self.inner().state.clone();
        let (start, end) = (start.clone(), end.clone());
        let open = self.inner().editor.borrow().as_ref().map(|e| e.note);
        glib::spawn_future_local(async move {
            let link = if let Some(label) = label {
                body::wikilink(target, Some(&label))
            } else {
                let titles = core(async move { state.titles(&[target]).await }).await;
                let title = titles.get(&target).cloned().unwrap_or_default();
                if let Some(editor) = w.inner().editor.borrow_mut().as_mut() {
                    editor.shown.remember(target, &title);
                    editor.titles.insert(target, title.clone());
                }
                if title.is_empty() {
                    body::wikilink(target, None)
                } else {
                    format!("[[{target}|{}]]", body::escape_label(&title))
                }
            };
            let buffer = &w.inner().buffer;
            let mut from = buffer.iter_at_mark(&start);
            let mut to = buffer.iter_at_mark(&end);
            buffer.delete_mark(&start);
            buffer.delete_mark(&end);
            // Another note took the editor while the title loaded.
            if w.inner().editor.borrow().as_ref().map(|e| e.note) != open {
                return;
            }
            buffer.begin_user_action();
            buffer.delete(&mut from, &mut to);
            buffer.insert(&mut from, &link);
            buffer.end_user_action();
            buffer.place_cursor(&from);
        });
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

    /// Opens the wikilink or web link under a point in the editor. Returns whether there was one.
    fn open_link_at(&self, x: f64, y: f64) -> bool {
        let view = &self.inner().view;
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

    /// Moves the open note to another space, once its pending edits are in the document.
    fn move_note(&self) {
        let pending = self.inner().editor.borrow_mut().as_mut().and_then(|e| e.timer.take());
        if let Some(timer) = pending {
            timer.remove();
            self.commit();
        }
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

/// Draws math, a diagram, or an embedded image for the editor, at its natural size. Math that
/// can't be drawn keeps its source in the line; a block that can't shows its source and the error.
async fn draw(state: Arc<State>, drawing: Drawing) -> Option<gtk::Widget> {
    let (kind, source) = match drawing {
        Drawing::InlineMath(source) => (Kind::InlineMath, source),
        Drawing::DisplayMath(source) => (Kind::DisplayMath, source),
        Drawing::Diagram(source) => (Kind::Mermaid, source),
        Drawing::Embed(target) => return embedded_image(state, target).await,
        Drawing::Table(table) => {
            let targets = tables::link_targets(&table);
            let drawings = tables::drawings(&table);
            let refs = core(async move {
                let mut refs =
                    tables::Refs { titles: state.titles(&targets).await, ..Default::default() };
                for (kind, source) in drawings {
                    let key = tables::drawing_key(kind, &source);
                    // A drawing that fails shows its source in the cell.
                    if let Ok(image) = state.render(kind, source).await {
                        refs.images.insert(key, image);
                    }
                }
                refs
            })
            .await;
            return Some(tables::widget(&table, &refs));
        }
    };
    let shown = source.clone();
    match core(async move { state.render(kind, source).await }).await {
        Ok(image) => {
            let texture = gdk::Texture::from_bytes(&glib::Bytes::from(&image.png)).ok()?;
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_can_shrink(true);
            picture.set_size_request(
                i32::try_from(image.width).unwrap_or(1),
                i32::try_from(image.height).unwrap_or(1),
            );
            Some(picture.upcast())
        }
        Err(_) if kind == Kind::InlineMath => None,
        Err(error) => {
            let label = gtk::Label::builder()
                .label(format!("{}\n{error}", shown.trim()))
                .xalign(0.0)
                .build();
            label.add_css_class("error");
            label.add_css_class("monospace");
            Some(label.upcast())
        }
    }
}

/// The picture of an image note, for an embed of it. Embeds of other notes stay links.
async fn embedded_image(state: Arc<State>, target: Id) -> Option<gtk::Widget> {
    let path = core(async move {
        let space = state.find_note(target).await?;
        if state.note(space, target).await.ok()?.type_name != "image" {
            return None;
        }
        let (hash, _) = state.note_file(space, target, "image").await.ok()??;
        state.blob_path(space, hash).await.ok()
    })
    .await?;
    let texture = gdk::Texture::from_filename(&path).ok()?;
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_can_shrink(true);
    picture.set_size_request(texture.width(), texture.height());
    Some(picture.upcast())
}
