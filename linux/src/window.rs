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
use zib_core::schema::Value;
use zib_core::search::{Hit, MATCH_END, MATCH_START, Sort};
use zib_core::sync::{LiveEvent, Selection, SyncState};
use zib_core::{Channel, Id};

use crate::app::core;
use crate::bottom_pane::BottomPane;
use crate::editor_widget::{EditorWidget, PasteTarget, SpellingAction};
use crate::state::State;
use crate::tables;

mod project_pane;
mod quick_search;
mod results;
mod search_completion;
mod search_pane;
mod search_sidebar;
mod tasks;
#[cfg(test)]
mod tests;

use project_pane::ProjectPane;
use search_completion::SearchCompletion;
use search_pane::SearchPane;
use search_sidebar::SearchSidebar;

/// How long metadata and search updates wait after typing stops, or while editing live.
const REPORT_LIVE: Duration = Duration::from_millis(50);
const REPORT_PAUSE: Duration = Duration::from_secs(3);
/// How long the chat's width waits after the divider stops moving before the machine saves it.
const CHAT_WIDTH_PAUSE: Duration = Duration::from_millis(500);

/// The window, which the instance opens on `activate`.
#[derive(Clone)]
pub struct Window(Rc<Inner>);

pub struct Inner {
    chat: crate::chat::Chat,
    /// The note's side and the chat's, with the divider between them.
    panes: gtk::Paned,
    /// The chat's width on this machine, once loaded, which it takes each time it shows.
    chat_width: Cell<Option<i32>>,
    /// Whether the shown chat has taken its width, so moving the divider now changes it.
    chat_width_restored: Cell<bool>,
    chat_width_timer: RefCell<Option<glib::SourceId>>,
    state: Arc<State>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    search_sidebar: SearchSidebar,
    sidebar_split: SearchPane,
    /// Each space, by pet name, with whether its notes are hidden.
    spaces: RefCell<Vec<(Id, String, bool)>>,
    /// The space the owner last put a note into.
    last_space: Cell<Option<Id>>,
    new_note_menu: gio::Menu,
    quick_search: gtk::Popover,
    search: gtk::TextView,
    search_completion: SearchCompletion,
    sort: Cell<Sort>,
    list: gtk::ListBox,
    results_scroll: gtk::ScrolledWindow,
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
    note_metadata: gtk::MenuButton,
    /// The note's text above the notes that link to it, which show only when there are some.
    backlinks: Rc<BottomPane>,
    backlinks_panel: gtk::Box,
    backlinks_list: gtk::ListBox,
    /// The space, identifier, and title of each note in the backlinks list, in its order.
    backlink_rows: RefCell<Vec<(Id, Id, String)>>,
    /// Counts backlink lookups, so an older result cannot replace a newer list.
    backlinks_generation: Cell<u64>,
    /// Set while a lookup that also restores the open note's backlinks height is under way.
    backlinks_restoring: Cell<bool>,
    editor_scroll: gtk::ScrolledWindow,
    project_pane: ProjectPane,
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
    work: tasks::WorkPanel,
    /// The metadata and search update after a pause in typing.
    body_timer: RefCell<Option<glib::SourceId>>,
}

/// The note open in the editor.
struct Editor {
    space: Id,
    note: Id,
    type_name: String,
    fields: Vec<(String, Value)>,
    titles: HashMap<Id, String>,
    cursors: Option<tokio::sync::broadcast::Sender<(Id, Vec<Selection>)>>,
}

/// A link target that already exists, or a note to create before inserting its link.
#[derive(Clone)]
enum LinkPick {
    Create { space: Id, title: String },
    Note(PickerNote),
}

impl LinkPick {
    fn row(&self, names: &HashMap<Id, String>) -> adw::ActionRow {
        match self {
            Self::Create { space, title } => {
                picker_row(&format!("Create note “{title}”"), *space, names)
            }
            Self::Note(note) => note.row(names),
        }
    }
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
        menu.append(Some("Edit Space Dictionary"), Some("win.edit-space-dictionary"));
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
        let sort_menu = gio::Menu::new();
        sort_menu.append(Some("Due date"), Some("win.sort-notes::due"));
        sort_menu.append(Some("Last modified"), Some("win.sort-notes::modified"));
        sort_menu.append(Some("Recurrence date"), Some("win.sort-notes::recurrence"));
        sort_menu.append(Some("Relevance"), Some("win.sort-notes::relevance"));
        sort_menu.append(Some("Title"), Some("win.sort-notes::title"));
        let sort = gtk::MenuButton::builder()
            .icon_name("view-sort-ascending-symbolic")
            .menu_model(&sort_menu)
            .tooltip_text("Sort notes")
            .build();
        sort.update_property(&[gtk::accessible::Property::Label("Sort notes")]);
        let search = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .monospace(true)
            .top_margin(4)
            .bottom_margin(4)
            .left_margin(6)
            .right_margin(6)
            .build();
        search.buffer().set_enable_undo(true);
        search.update_property(&[gtk::accessible::Property::Label("Search definition")]);
        let search_sidebar = SearchSidebar::new(&search, &sort);
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let sidebar_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_bottom(10)
            .margin_end(10)
            .margin_start(10)
            .margin_top(10)
            .build();
        sidebar_box.append(&search_sidebar.widget);
        let results_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
        sidebar_box.append(&results_scroll);

        let title = adw::WindowTitle::new("", "");
        let content_header = adw::HeaderBar::new();
        content_header.set_title_widget(Some(&title));
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Show or hide search sidebar")
            .build();
        sidebar_toggle.update_property(&[gtk::accessible::Property::Label("Search sidebar")]);
        let quick_search = gtk::Popover::builder().position(gtk::PositionType::Bottom).build();
        let popup = quick_search.downgrade();
        title.connect_unrealize(move |_| {
            if let Some(popup) = popup.upgrade()
                && popup.parent().is_some()
            {
                popup.unparent();
            }
        });
        content_header.pack_start(&sidebar_toggle);
        content_header.pack_start(&new_note);
        content_header.pack_start(&add_file);
        content_header.pack_end(&menu_button);

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
        let note_metadata = gtk::MenuButton::builder().label("Note metadata").build();
        note_metadata.set_visible(false);
        content_header.pack_start(&note_metadata);
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

        let widget = EditorWidget::default();
        let work = tasks::WorkPanel::new();
        let editor_scroll = gtk::ScrolledWindow::builder()
            .child(&widget)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        if let Some(viewport) = editor_scroll.child().and_downcast::<gtk::Viewport>() {
            viewport.set_scroll_to_focus(false);
        }
        let backlinks_list = gtk::ListBox::new();
        backlinks_list.add_css_class("navigation-sidebar");
        backlinks_list.set_selection_mode(gtk::SelectionMode::None);
        let backlinks_heading = gtk::Label::builder()
            .label("Linked from")
            .xalign(0.0)
            .css_classes(["caption-heading", "dim-label"])
            .build();
        let backlinks_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_bottom(12)
            .margin_end(12)
            .margin_start(12)
            .margin_top(12)
            .build();
        backlinks_panel.append(&backlinks_heading);
        backlinks_panel.append(
            &gtk::ScrolledWindow::builder()
                .child(&backlinks_list)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .vexpand(true)
                .build(),
        );
        backlinks_panel.set_visible(false);
        let backlinks = BottomPane::new(&editor_scroll, &backlinks_panel);
        let project_pane = ProjectPane::new(&backlinks.widget);
        project_pane.content.append(&work.header);
        project_pane.content.append(&work.widget);
        let empty = adw::StatusPage::builder()
            .icon_name("accessories-text-editor-symbolic")
            .title(state.channel.display_name())
            .description("Pick a note, or make a new one.")
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&project_pane.widget, Some("editor"));
        let content = adw::ToolbarView::new();
        content.add_top_bar(&content_header);
        content.set_content(Some(&stack));

        let split = SearchPane::new(&sidebar_box, &content, &sidebar_toggle);
        let chat = crate::chat::Chat::new(state.clone());
        let panes = gtk::Paned::new(gtk::Orientation::Horizontal);
        panes.set_start_child(Some(&split.widget));
        panes.set_end_child(Some(chat.widget()));
        panes.set_resize_end_child(false);
        panes.set_shrink_end_child(false);
        panes.set_position(720);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&panes));
        window.set_content(Some(&toasts));

        let this = Self(Rc::new(Inner {
            chat,
            panes,
            chat_width: Cell::new(None),
            chat_width_restored: Cell::new(false),
            chat_width_timer: RefCell::default(),
            state,
            window,
            toasts,
            search_sidebar,
            sidebar_split: split,
            spaces: RefCell::default(),
            last_space: Cell::new(None),
            new_note_menu,
            quick_search,
            search,
            search_completion: SearchCompletion::new(),
            sort: Cell::new(Sort::Relevance),
            list,
            results_scroll,
            rows: RefCell::default(),
            generation: Cell::new(0),
            opening: Cell::new(0),
            label_generation: Cell::new(0),
            drawing_generation: Cell::new(0),
            title,
            stack,
            note_metadata,
            backlinks,
            backlinks_panel,
            backlinks_list,
            backlink_rows: RefCell::default(),
            backlinks_generation: Cell::new(0),
            backlinks_restoring: Cell::new(false),
            editor_scroll,
            project_pane,
            back: RefCell::default(),
            forward: RefCell::default(),
            back_button,
            forward_button,
            open_file,
            status,
            editor: RefCell::default(),
            focus_body: Cell::new(false),
            widget,
            work,
            body_timer: RefCell::default(),
        }));
        this.connect(&new_note, &add_file, &delete);
        this.connect_quick_search();
        this.inner().search_completion.connect(&this);
        this.connect_search_sidebar();
        this.connect_search_pane();
        this.connect_project_pane();
        this.connect_work();
        this.inner().chat.connect(this.clone());
        let w = this.clone();
        chat_toggle.connect_clicked(move |_| w.inner().chat.toggle());
        this.connect_chat_width();
        this.connect_files();
        let widget = &this.inner().widget;
        let w = this.clone();
        widget.connect_spelling_word(move |action, word| match action {
            SpellingAction::AddToSpace => w.spelling_dictionary(Some(word)),
            SpellingAction::IgnoreInNote => w.ignore_spelling_word(word),
        });
        let w = this.clone();
        widget.connect_edited(move || w.body_edited());
        let w = this.clone();
        widget.connect_selection_changed(move || w.cursor_moved());
        let w = this.clone();
        widget.connect_open_link(move |target| w.open_link(target));
        this.connect_link_picker(widget);
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
        self.cancel_work_title_open();
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

    /// Gives the chat the width that the owner last chose on this machine each time it shows, or a
    /// third of the window before they choose one, and saves the width when the owner moves the
    /// divider. The chat keeps its width as the window resizes, so only moving the divider changes
    /// it.
    fn connect_chat_width(&self) {
        let chat = self.inner().chat.widget();
        let w = self.clone();
        chat.connect_visible_notify(move |chat| {
            w.inner().chat_width_restored.set(false);
            if chat.is_visible() {
                w.restore_chat_width();
            }
        });
        let w = self.clone();
        self.inner().panes.connect_position_notify(move |_| {
            if !w.inner().chat_width_restored.get() {
                return;
            }
            if let Some(timer) = w.inner().chat_width_timer.take() {
                timer.remove();
            }
            let saving = w.clone();
            let timer = glib::timeout_add_local_once(CHAT_WIDTH_PAUSE, move || {
                saving.inner().chat_width_timer.take();
                saving.save_chat_width();
            });
            *w.inner().chat_width_timer.borrow_mut() = Some(timer);
        });
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let width = core(async move { state.chat_width().await }).await;
            w.inner().chat_width.set(width.and_then(|width| i32::try_from(width).ok()));
            if w.inner().chat.widget().is_visible() {
                w.restore_chat_width();
            }
        });
    }

    /// Moves the divider once the shown chat has a width, so the chat takes its saved width.
    fn restore_chat_width(&self) {
        let w = self.clone();
        self.inner().chat.widget().add_tick_callback(move |chat, _| {
            if !chat.is_visible() || w.inner().chat_width_restored.get() {
                return glib::ControlFlow::Break;
            }
            let actual = chat.width();
            if actual == 0 {
                return glib::ControlFlow::Continue;
            }
            // Until the owner chooses a width here, the chat takes a third of the window.
            let panes = &w.inner().panes;
            let width = w.inner().chat_width.get().unwrap_or(panes.width() / 3);
            panes.set_position(panes.position() + actual - width);
            w.inner().chat_width_restored.set(true);
            glib::ControlFlow::Break
        });
    }

    fn save_chat_width(&self) {
        let chat = self.inner().chat.widget();
        let width = chat.width();
        if !chat.is_visible() || width == 0 || self.inner().chat_width.get() == Some(width) {
            return;
        }
        self.inner().chat_width.set(Some(width));
        let state = self.inner().state.clone();
        let Ok(width) = u32::try_from(width) else { return };
        glib::spawn_future_local(async move {
            let _ = core(async move { state.set_chat_width(width).await }).await;
        });
    }

    pub fn toast(&self, text: &str) {
        self.inner().toasts.add_toast(adw::Toast::new(text));
    }

    fn connect(&self, new_note: &adw::SplitButton, add_file: &gtk::Button, delete: &gtk::Button) {
        let w = self.clone();
        self.inner().list.connect_row_activated(move |_, row| {
            let picked = w
                .inner()
                .rows
                .borrow()
                .get(usize::try_from(row.index()).unwrap_or(usize::MAX))
                .copied();
            if let Some((space, note)) = picked {
                w.open_search_result(space, note);
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
        self.inner().backlinks_list.connect_row_activated(move |_, row| {
            let picked = w
                .inner()
                .backlink_rows
                .borrow()
                .get(usize::try_from(row.index()).unwrap_or(usize::MAX))
                .map(|(space, note, _)| (*space, *note));
            if let Some((space, note)) = picked {
                w.open_note(space, note);
            }
        });
        let w = self.clone();
        self.inner().backlinks.connect_resized(move |height| w.save_backlinks_height(height));
        let w = self.clone();
        self.inner().window.connect_close_request(move |window| {
            w.save_search_pane();
            w.save_project_pane();
            w.close_editor();
            window.set_visible(false);
            glib::Propagation::Stop
        });
        let actions = gio::SimpleActionGroup::new();
        for (name, handler) in [
            ("add-machine", Self::add_machine as fn(&Self)),
            ("edit-space-dictionary", |w: &Self| w.spelling_dictionary(None)),
            ("move-note", Self::move_note),
            ("move-notes", |w: &Self| crate::sharing::move_all(w, w.inner().state.clone())),
            ("new-project", |w: &Self| w.new_work(zib_core::schema::PROJECT)),
            ("new-space", Self::new_space),
            ("new-task", |w: &Self| w.new_work(zib_core::schema::TASK)),
            ("print", |w: &Self| w.print(false)),
            ("print-linked", |w: &Self| w.print(true)),
            ("rename-space", Self::rename_space),
            ("settings", |w: &Self| crate::settings::show(w, w.inner().state.clone())),
            ("share-space", |w: &Self| crate::sharing::share(w, w.inner().state.clone())),
            ("toggle-search", Self::toggle_search),
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
        self.connect_sort(&actions);
        self.inner().window.insert_action_group("win", Some(&actions));
        self.connect_navigation();
    }

    fn connect_sort(&self, actions: &gio::SimpleActionGroup) {
        let sort = gio::SimpleAction::new_stateful(
            "sort-notes",
            Some(glib::VariantTy::STRING),
            &"relevance".to_variant(),
        );
        let w = self.clone();
        sort.connect_change_state(move |action, value| {
            let Some(value) = value else { return };
            let order = match value.str() {
                Some("due") => Sort::Due,
                Some("modified") => Sort::Modified,
                Some("recurrence") => Sort::Recurrence,
                Some("relevance") => Sort::Relevance,
                Some("title") => Sort::Title,
                _ => return,
            };
            action.set_state(value);
            w.inner().sort.set(order);
            w.search_edited();
        });
        actions.add_action(&sort);
    }

    /// Handles navigation shortcuts before the focused widget processes them.
    fn connect_navigation(&self) {
        let keys = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = self.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
            if modifiers == gdk::ModifierType::CONTROL_MASK
                && matches!(key, gdk::Key::o | gdk::Key::O)
            {
                w.open_quick_search();
                return glib::Propagation::Stop;
            }
            // Ctrl+Tab shows the chat and moves into it, or hides it and returns to the note.
            if modifiers == gdk::ModifierType::CONTROL_MASK && key == gdk::Key::Tab {
                if w.inner().chat.has_focus() {
                    w.inner().chat.hide();
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

    /// Refreshes the available spaces, new-note menu, saved searches, and results.
    fn reload_spaces(&self) {
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let (spaces, last) =
                core(async move { (state.space_list().await, state.last_space().await) }).await;
            let menu = &w.inner().new_note_menu;
            menu.remove_all();
            menu.append(Some("New project"), Some("win.new-project"));
            menu.append(Some("New standalone task"), Some("win.new-task"));
            for (id, name, _) in &spaces {
                menu.append(
                    Some(&format!("New note in {name}")),
                    Some(&format!("win.new-note-in('{id}')")),
                );
            }
            *w.inner().spaces.borrow_mut() = spaces;
            w.inner().last_space.set(last);
            w.reload_saved_searches();
            w.refresh_search_completion();
            w.reload_notes();
        });
    }

    fn search_error(&self, error: &zib_core::Error) {
        self.search_validated(false);
        self.inner().rows.borrow_mut().clear();
        self.inner().list.remove_all();
        let row = adw::ActionRow::builder()
            .title("Couldn't search")
            .subtitle(escape(&error.to_string()))
            .selectable(false)
            .build();
        self.inner().list.append(&row);
    }

    /// Something in a space changed through sync or a derivation.
    pub fn space_changed(&self, space: Id) {
        self.inner().chat.refresh_resources(self);
        if !self.inner().spaces.borrow().iter().any(|(id, _, _)| *id == space) {
            self.reload_spaces();
            return;
        }
        self.reload_saved_searches();
        self.refresh_search_completion();
        self.reload_notes();
        self.refresh_link_titles();
        self.refresh_images(true);
        // Notes in any space can link to the open note.
        self.refresh_backlinks(false);
        let open = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note));
        if open.is_some_and(|(s, _)| s == space) {
            self.refresh_open();
            self.refresh_note_metadata();
            self.refresh_work();
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
        self.delete_note_at(space, note);
    }

    fn delete_note_at(&self, space: Id, note: Id) {
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let reading = state.clone();
            let item = match core(async move { reading.note(space, note).await }).await {
                Ok(item) => item,
                Err(error) => {
                    w.toast(&format!("Couldn't delete note: {error}"));
                    return;
                }
            };
            if item.is_task_occurrence() {
                w.toast("Skip the occurrence to remove it from open work");
                return;
            }
            let (heading, body) = match item.type_name.as_str() {
                "project" => (
                    "Delete this project?",
                    "This also deletes all subprojects and tasks, including completed tasks. \
                      Links keep each item's last title.",
                ),
                "task" => (
                    "Delete this task?",
                    "This also deletes all subtasks, including completed tasks. \
                      Links keep each item's last title.",
                ),
                _ => ("Delete this note?", "Links to it will show its last title."),
            };
            let dialog = adw::AlertDialog::new(Some(heading), Some(body));
            dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            if dialog.choose_future(Some(&w.inner().window)).await != "delete" {
                return;
            }
            match core(async move { state.delete_note(space, note).await }).await {
                Ok(deleted) => w.forget_deleted_notes(space, &deleted),
                Err(error) => w.toast(&format!("Couldn't delete note: {error}")),
            }
        });
    }

    /// Opens a note in the editor, and adds the open one to the history.
    fn open_note_direct(&self, space: Id, note: Id) {
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
                    let (loaded, fields) = state.note_with_fields(space, note).await?;
                    let body = state.open_body(space, note).await?;
                    let targets: Vec<Id> = body::link_targets(&loaded.body).into_iter().collect();
                    let titles = state.titles(&targets).await;
                    Ok::<_, zib_core::Error>((loaded, body, titles, fields))
                }
            })
            .await;
            if w.inner().widget.session() != session {
                return;
            }
            let (loaded, body, titles, fields) = match loaded {
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
                fields: Vec::new(),
                titles: titles.clone(),
                cursors: None,
            });
            w.inner().project_pane.show(zib_core::work::is_work(&loaded.type_name));
            let widget = &w.inner().widget;
            widget.set_editor(Some(zib_core::editor::Editor::new(body)));
            widget.set_titles(titles);
            w.show_note_metadata(fields);
            w.refresh_images(false);
            w.inner()
                .open_file
                .set_visible(loaded.type_name == "file" || loaded.type_name == "image");
            w.update_title();
            w.show_mode();
            w.refresh_work();
            w.refresh_backlinks(true);
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

    fn refresh_note_metadata(&self) {
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let session = self.inner().widget.session();
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            if let Ok((_, fields)) =
                core(async move { state.note_with_fields(space, note).await }).await
                && w.inner().widget.session() == session
            {
                w.show_note_metadata(fields);
            }
        });
    }

    fn show_note_metadata(&self, fields: Vec<(String, Value)>) {
        let details = &self.inner().note_metadata;
        let mut editor = self.inner().editor.borrow_mut();
        let Some(editor) = editor.as_mut() else { return };
        if editor.fields == fields && details.popover().is_some() {
            return;
        }
        editor.fields = fields;
        let fields = &editor.fields;
        let work = matches!(editor.type_name.as_str(), "project" | "task");
        let note_id = Value::Text(editor.note.to_string());
        let mut entries: Vec<_> = fields
            .iter()
            .filter(|(name, value)| {
                !(matches!(name.as_str(), "file" | "text" | "thumbnail")
                    || (work
                        && name == "status"
                        && matches!(value, Value::Choice(status) if status == "none")))
            })
            .map(|(name, value)| (name.as_str(), value))
            .collect();
        entries.push(("id", &note_id));
        entries.sort_unstable_by_key(|(name, _)| *name);
        let rows = gtk::Grid::builder()
            .column_spacing(16)
            .row_spacing(8)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        let mut labels = Vec::with_capacity(entries.len());
        for (row, (name, value)) in (0..).zip(entries) {
            let key = gtk::Label::builder()
                .label(name)
                .max_width_chars(24)
                .valign(gtk::Align::Start)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .xalign(0.0)
                .build();
            key.add_css_class("heading");
            rows.attach(&key, 0, row, 1, 1);
            let label = gtk::Label::builder()
                .hexpand(true)
                .label(field_text(value))
                .max_width_chars(64)
                .selectable(true)
                .valign(gtk::Align::Start)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .xalign(0.0)
                .build();
            if let Value::Text(url) = value
                && zib_core::web_clip::source_url(url).is_some()
            {
                let url = escape(url);
                label.set_markup(&format!("<a href=\"{url}\">{url}</a>"));
            }
            rows.attach(&label, 1, row, 1, 1);
            labels.push(label);
        }
        let scroll = gtk::ScrolledWindow::builder()
            .child(&rows)
            .focusable(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(360)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .width_request(420)
            .build();
        scroll.connect_map(move |scroll| {
            // Keep the popover's initial focus off the selectable text.
            scroll.grab_focus();
            for label in &labels {
                label.select_region(0, 0);
            }
        });
        if let Some(popover) = details.popover() {
            popover.set_child(Some(&scroll));
        } else {
            details.set_popover(Some(&gtk::Popover::builder().child(&scroll).build()));
        }
        details.set_visible(true);
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
            w.refresh_note_metadata();
            w.refresh_work();
        });
    }

    /// Reports pending metadata and ends the note's live sessions.
    fn close_editor(&self) {
        self.save_project_pane();
        self.inner().project_pane.show(false);
        self.save_task_titles();
        self.inner().work.header.set_visible(false);
        self.inner().work.widget.set_visible(false);
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
        let inner = self.inner();
        inner.backlinks_generation.set(inner.backlinks_generation.get().wrapping_add(1));
        inner.backlinks_restoring.set(false);
        inner.backlinks_panel.set_visible(false);
        inner.backlinks_list.remove_all();
        inner.backlink_rows.borrow_mut().clear();
        self.inner().note_metadata.popdown();
        self.inner().note_metadata.set_visible(false);
        self.inner().note_metadata.set_popover(None::<&gtk::Popover>);
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
                let w = self.clone();
                glib::spawn_future_local(async move {
                    core(async move { state.live_changed(space).await }).await;
                    w.refresh_work();
                    w.reload_notes();
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
        self.inner().widget.refresh_spelling();
        self.update_title();
        self.refresh_link_titles();
        self.refresh_images(false);
    }

    fn spelling_dictionary(&self, word: Option<String>) {
        let open = self.inner().editor.borrow().as_ref().map(|e| e.space);
        let Some(space) = open.or_else(|| self.current_space_id()) else {
            self.toast("Make a space first");
            return;
        };
        let opening = word.is_none().then(|| self.begin_open());
        let state = self.inner().state.clone();
        let w = self.clone();
        glib::spawn_future_local(async move {
            let result =
                core(async move { state.spelling_dictionary(space, word.as_deref()).await }).await;
            match result {
                Ok(note) => {
                    w.reload_notes();
                    if opening == Some(w.inner().opening.get()) {
                        w.inner().focus_body.set(true);
                        w.open_note(space, note);
                    } else if w.inner().editor.borrow().as_ref().is_some_and(|e| e.space == space) {
                        w.inner().widget.refresh();
                        w.inner().widget.refresh_spelling();
                    }
                }
                Err(error) => w.toast(&format!("Couldn't update the space dictionary: {error}")),
            }
        });
    }

    fn ignore_spelling_word(&self, word: String) {
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let state = self.inner().state.clone();
        let w = self.clone();
        glib::spawn_future_local(async move {
            match core(async move { state.ignore_spelling_word(space, note, &word).await }).await {
                Ok(()) => {
                    if w.inner()
                        .editor
                        .borrow()
                        .as_ref()
                        .is_some_and(|e| e.space == space && e.note == note)
                    {
                        w.inner().widget.refresh_spelling();
                    }
                }
                Err(error) => w.toast(&format!("Couldn't ignore the word: {error}")),
            }
        });
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

    fn connect_link_picker(&self, widget: &EditorWidget) {
        let (w, editing) = (self.clone(), widget.clone());
        widget.connect_pick_link(move |range| w.open_link_picker(&editing, range));
    }

    fn open_link_picker(&self, widget: &EditorWidget, range: std::ops::Range<usize>) {
        let Some(text) = widget.text() else { return };
        let Some(expected) = text.get(range.clone()).map(str::to_owned) else { return };
        let session = widget.session();
        let editing = widget.clone();
        let pick = move |target| {
            if editing.session() == session {
                editing.insert_link(range.clone(), &expected, target);
            }
        };
        let editing = widget.clone();
        self.show_link_picker(widget, &widget.cursor_rectangle(), pick, move || {
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
        let scroll = picker_scroll(&list);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&entry);
        content.append(&scroll);
        popover.set_child(Some(&content));
        let found: Rc<RefCell<Vec<LinkPick>>> = Rc::default();
        let generation = Rc::new(Cell::new(0u64));
        let fill = {
            let (w, list, found) = (self.clone(), list.clone(), found.clone());
            move |query: String| {
                list.remove_all();
                found.borrow_mut().clear();
                generation.set(generation.get() + 1);
                let (w, list, found, generation, mine) =
                    (w.clone(), list.clone(), found.clone(), generation.clone(), generation.get());
                glib::spawn_future_local(async move {
                    let notes = w.link_candidates(query).await;
                    if generation.get() != mine {
                        return;
                    }
                    let names = w
                        .inner()
                        .spaces
                        .borrow()
                        .iter()
                        .map(|(id, name, _)| (*id, name.clone()))
                        .collect();
                    for note in &notes {
                        list.append(&note.row(&names));
                    }
                    // Keep creation visible at the top, but prefer an existing match for Enter.
                    let default =
                        i32::from(notes.len() > 1 && matches!(notes[0], LinkPick::Create { .. }));
                    if let Some(row) = list.row_at_index(default) {
                        list.select_row(Some(&row));
                    }
                    *found.borrow_mut() = notes;
                });
            }
        };
        fill(String::new());
        let chosen = Rc::new(Cell::new(false));
        let pick = {
            let (popover, found) = (popover.clone(), found.clone());
            let chosen = chosen.clone();
            let on_pick = Rc::new(on_pick);
            let w = self.clone();
            move |index: i32| {
                let target =
                    usize::try_from(index).ok().and_then(|i| found.borrow().get(i).cloned());
                let Some(target) = target else { return };
                if !chosen.replace(true) {
                    w.pick_link(target, on_pick.clone());
                    popover.popdown();
                }
            }
        };
        let picked = pick.clone();
        list.connect_row_activated(move |_, row| picked(row.index()));
        let selected = list.clone();
        entry.connect_activate(move |_| pick(selected.selected_row().map_or(0, |r| r.index())));
        entry.connect_changed(move |entry| fill(entry.text().to_string()));
        entry.add_controller(arrow_keys(&list, &scroll));
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

    /// Inserts an existing target, or creates the requested note without leaving the editor.
    fn pick_link(&self, target: LinkPick, on_pick: Rc<impl Fn(Id) + 'static>) {
        let (space, title) = match target {
            LinkPick::Create { space, title } => (space, title),
            LinkPick::Note(note) => {
                on_pick(note.hit.id);
                return;
            }
        };
        let w = self.clone();
        let state = self.inner().state.clone();
        let body = format!("# {title}");
        glib::spawn_future_local(async move {
            match core(async move { state.create_note(space, &body).await }).await {
                Ok(note) => {
                    w.use_space(space);
                    w.reload_notes();
                    on_pick(note);
                }
                Err(error) => w.toast(&format!("Couldn't make the note: {error}")),
            }
        });
    }

    /// Offers recent notes or search results and creation with a nonblank query as the title.
    async fn link_candidates(&self, query: String) -> Vec<LinkPick> {
        let state = self.inner().state.clone();
        let shown: Vec<Id> = self.shown_spaces().into_iter().map(|(id, _)| id).collect();
        let open = self.inner().editor.borrow().as_ref().map(|e| e.note);
        let creation = self
            .current_space_id()
            .filter(|_| !query.trim().is_empty())
            .map(|space| LinkPick::Create { space, title: query.trim().to_owned() });
        let found = core(async move {
            let hits = if query.trim().is_empty() {
                let mut all = Vec::new();
                for space in shown {
                    let notes = state.notes(space).await.unwrap_or_default();
                    all.extend(notes.into_iter().map(|n| (space, n)));
                }
                all.sort_by_key(|(_, note)| std::cmp::Reverse(note.updated));
                all.into_iter().take(20).map(|(space, note)| (space, note.into())).collect()
            } else {
                state.search(&query, 20)
            };
            let hits = hits.into_iter().filter(|(_, hit)| Some(hit.id) != open).collect();
            picker_notes(&state, hits).await
        })
        .await;
        creation.into_iter().chain(found.into_iter().map(LinkPick::Note)).collect()
    }

    /// Fills the pane of notes that link to the open note, and hides it when there are none. When
    /// `restore` is set, the pane also takes the height the owner gave this note's pane.
    fn refresh_backlinks(&self, restore: bool) {
        let generation = self.inner().backlinks_generation.get().wrapping_add(1);
        self.inner().backlinks_generation.set(generation);
        // A lookup that replaces one still restoring the height restores it in its place.
        let restore = restore || self.inner().backlinks_restoring.get();
        self.inner().backlinks_restoring.set(restore);
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let (sources, height) = core(async move {
                let sources = state.backlinks(note);
                let ids: Vec<Id> = sources.iter().map(|(_, id)| *id).collect();
                let titles = state.titles(&ids).await;
                let height = if restore { state.backlinks_height(space, note).await } else { None };
                let sources = sources
                    .into_iter()
                    .map(|(space, id)| {
                        let title =
                            titles.get(&id).map(|t| body::plain_text(t)).unwrap_or_default();
                        let title =
                            if title.trim().is_empty() { "Untitled".to_owned() } else { title };
                        (space, id, title)
                    })
                    .collect::<Vec<_>>();
                (sources, height)
            })
            .await;
            if w.inner().backlinks_generation.get() != generation {
                return;
            }
            let inner = w.inner();
            inner.backlinks_restoring.set(false);
            // The note keeps two lines of text in view, however far the owner moves the divider.
            inner.editor_scroll.set_min_content_height(inner.widget.lines_height(2));
            // Filling the list again only when it changes keeps where the owner scrolled it.
            if *inner.backlink_rows.borrow() != sources {
                inner.backlinks_list.remove_all();
                for (_, _, title) in &sources {
                    let row =
                        adw::ActionRow::builder().title(escape(title)).activatable(true).build();
                    inner.backlinks_list.append(&row);
                }
                inner.backlinks_panel.set_visible(!sources.is_empty());
                *inner.backlink_rows.borrow_mut() = sources;
            }
            if restore {
                inner.backlinks.show(height);
            } else {
                inner.backlinks.update();
            }
        });
    }

    /// Keeps the height the owner gave the open note's backlinks pane, or forgets it when they
    /// unpin it.
    fn save_backlinks_height(&self, height: Option<i32>) {
        let Some((space, note)) = self.inner().editor.borrow().as_ref().map(|e| (e.space, e.note))
        else {
            return;
        };
        let w = self.clone();
        let state = self.inner().state.clone();
        glib::spawn_future_local(async move {
            let saved = core(async move { state.set_backlinks_height(space, note, height).await });
            if let Err(error) = saved.await {
                w.toast(&format!("Couldn't keep the backlinks height: {error}"));
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
        self.connect_link_picker(widget);
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
fn field_text(value: &Value) -> String {
    match value {
        Value::Text(text) if text.is_empty() => "(empty)".to_owned(),
        Value::Text(text)
        | Value::Decimal(text)
        | Value::Date(text)
        | Value::Time(text)
        | Value::Choice(text) => text.clone(),
        Value::Integer(number) => number.to_string(),
        Value::Boolean(value) => value.to_string(),
        Value::Timestamp(when) => glib::DateTime::from_unix_utc(when.div_euclid(1000))
            .and_then(|date| date.to_local())
            .and_then(|date| date.format("%x %X"))
            .map_or_else(|_| when.to_string(), |date| date.to_string()),
        Value::Link(id) => id.to_string(),
        Value::File(hash) => hash.to_string(),
        Value::List(items) if items.is_empty() => "[]".to_owned(),
        Value::List(items) => items.iter().map(field_text).collect::<Vec<_>>().join("\n"),
    }
}

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

fn picker_scroll(list: &gtk::ListBox) -> gtk::ScrolledWindow {
    // A minimum height keeps several notes in view when the popover has little room.
    gtk::ScrolledWindow::builder()
        .child(list)
        .width_request(360)
        .min_content_height(240)
        .max_content_height(320)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build()
}

#[derive(Clone)]
struct PickerNote {
    hit: Hit,
    recurring: bool,
    space: Id,
}

impl PickerNote {
    fn row(&self, names: &HashMap<Id, String>) -> adw::ActionRow {
        let title = if self.hit.title.trim().is_empty() { "Untitled" } else { &self.hit.title };
        let row = picker_row(title, self.space, names);
        row.add_suffix(&results::type_pill(&self.hit.type_name));
        if self.recurring {
            row.add_suffix(&tasks::recurrence_indicator());
        }
        row
    }
}

async fn picker_notes(state: &State, hits: Vec<(Id, Hit)>) -> Vec<PickerNote> {
    let mut notes = Vec::with_capacity(hits.len());
    for (space, hit) in hits {
        let recurring = hit.type_name == zib_core::schema::TASK
            && state.recurrence_rule(space, hit.id).await.is_ok_and(|rule| rule.is_some());
        notes.push(PickerNote { hit, recurring, space });
    }
    notes
}

fn picker_row(title: &str, space: Id, names: &HashMap<Id, String>) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(escape(title))
        .title_lines(1)
        .subtitle(escape(names.get(&space).map_or("", String::as_str)))
        .subtitle_lines(1)
        .activatable(true)
        .build()
}

/// Lets Up and Down in a search entry move the selection in `list`.
fn arrow_keys(list: &gtk::ListBox, scroll: &gtk::ScrolledWindow) -> gtk::EventControllerKey {
    let (list, scroll) = (list.clone(), scroll.downgrade());
    let keys = gtk::EventControllerKey::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    keys.connect_key_pressed(move |_, key, _, _| {
        let step = match key {
            gdk::Key::Down | gdk::Key::KP_Down => 1,
            gdk::Key::Up | gdk::Key::KP_Up => -1,
            _ => return glib::Propagation::Proceed,
        };
        let at = list.selected_row().map_or(-1, |row| row.index()) + step;
        if let Some(row) = list.row_at_index(at.max(0)) {
            list.select_row(Some(&row));
            if let (Some(scroll), Some(bounds)) = (scroll.upgrade(), row.compute_bounds(&list)) {
                let top = f64::from(bounds.y());
                scroll.vadjustment().clamp_page(top, top + f64::from(bounds.height()));
            }
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
