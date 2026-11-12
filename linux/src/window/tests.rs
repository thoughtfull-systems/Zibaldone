use std::time::{Duration, Instant};

use zib_core::channel::Dirs;

use super::*;

/// Drive GTK's queued work without waiting for a display event or the periodic status timer.
fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "GTK work didn't finish");
        glib::MainContext::default().iteration(false);
        std::thread::yield_now();
    }
}

/// Present the window and wait for its first frame. GTK maps the window before it draws, but the
/// compositor shows it only after that frame, and GTK blocks while it waits for the compositor to
/// place a popover on a window it doesn't show yet.
fn present(window: &gtk::Window) {
    window.present();
    until(|| window.is_mapped());
    let clock = window.frame_clock().expect("the window's frame clock");
    let painted = Rc::new(Cell::new(false));
    let handler = clock.connect_after_paint({
        let painted = painted.clone();
        move |_| painted.set(true)
    });
    window.queue_draw();
    until(|| painted.get());
    clock.disconnect(handler);
}

fn input(widget: &EditorWidget) -> gtk::IMContext {
    let controllers = widget.observe_controllers();
    (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .find_map(|keys| keys.im_context())
        .expect("the editor's input method")
}

fn undo(widget: &EditorWidget) {
    let controllers = widget.observe_controllers();
    let keys = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .find(|keys| keys.im_context().is_some())
        .unwrap();
    keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::z, &0u32, &gdk::ModifierType::CONTROL_MASK],
    );
}

#[test]
fn last_note_restores_after_close_and_restart_and_explicit_navigation_takes_precedence() {
    if adw::init().is_err() {
        return;
    }
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, first, second) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let first = state.create_note(space, "First note").await.unwrap();
        let second = state.create_note(space, "Second note").await.unwrap();
        (space, first, second)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.RestoreTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, first);
    until(|| runtime.block_on(state.last_note()) == Some((space, first)));
    window.close_editor();
    window.restore_last_note();
    until(|| window.inner().widget.text().as_deref() == Some("First note"));
    window.close_editor();
    window.restore_last_note();
    window.open_uri(&format!("zib:note/{second}"));
    until(|| runtime.block_on(state.last_note()) == Some((space, second)));
    assert_eq!(window.inner().widget.text().as_deref(), Some("Second note"));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();

    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    let window = Window::new(&app, reopened.clone());
    window.restore_last_note();
    until(|| window.inner().widget.text().as_deref() == Some("Second note"));
    window.close_editor();
    runtime.block_on(reopened.delete_note(space, second)).unwrap();
    assert_eq!(runtime.block_on(reopened.last_note()), None);
    runtime.block_on(reopened.finish());
    window.inner().window.destroy();
}

#[test]
fn chat_draft_undo_is_independent_and_transcripts_are_editable_notes() {
    if adw::init().is_err() {
        return;
    }
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, note) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        (space, state.create_note(space, "Original note").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ChatTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, note);
    let chat = &window.inner().chat;
    let draft = chat.composer();
    until(|| {
        window.inner().widget.text().as_deref() == Some("Original note") && draft.is_sensitive()
    });
    input(&window.inner().widget).emit_by_name::<()>("commit", &[&"Note edit "]);
    input(draft).emit_by_name::<()>("commit", &[&"**Chat draft**"]);
    assert_eq!(state.agent.snapshot().draft, "**Chat draft**");
    undo(draft);
    assert_eq!(draft.text().as_deref(), Some(""));
    assert_eq!(window.inner().widget.text().as_deref(), Some("Note edit Original note"));
    input(draft).emit_by_name::<()>("commit", &[&"Archive this discussion"]);
    // Agent-created notes use excluded origins even while the main editor has an undo manager.
    let creation = zib_core::agent::Creation { id: Id::generate(), space: Some(space) };
    runtime
        .block_on(zib_core::agent::Host::create_note(
            &*state,
            creation.clone(),
            "note".into(),
            "Agent-created note".into(),
        ))
        .unwrap();
    runtime
        .block_on(zib_core::agent::Host::write_note(
            &*state,
            note,
            "Note edit Original note".into(),
            "Note edit Original note\n\nAgent addition".into(),
        ))
        .unwrap();
    undo(&window.inner().widget);
    assert_eq!(window.inner().widget.text().as_deref(), Some("Original note\n\nAgent addition"));
    assert_eq!(draft.text().as_deref(), Some("Archive this discussion"));
    assert!(runtime.block_on(state.note(space, creation.id)).is_ok());
    runtime.block_on(state.agent.archive(&*state, space)).unwrap();
    let transcript = runtime
        .block_on(state.notes(space))
        .unwrap()
        .into_iter()
        .find(|note| note.type_name == "agent-transcript")
        .unwrap();
    window.open_note(space, transcript.id);
    until(|| {
        window.inner().widget.text().is_some_and(|text| text.contains("Archive this discussion"))
    });
    input(&window.inner().widget).emit_by_name::<()>("commit", &[&"Editable "]);
    assert!(
        runtime.block_on(state.note(space, transcript.id)).unwrap().body.starts_with("Editable ")
    );
    // The instance saves the latest draft even when its debounce timer hasn't fired.
    state.agent.edit_draft("Unsent at shutdown".into());
    runtime.block_on(state.finish());
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(reopened.agent.snapshot().draft, "Unsent at shutdown");
    runtime.block_on(reopened.finish());
    window.inner().window.destroy();
}

#[test]
fn default_editor_edits_the_shared_body_and_keeps_it_through_navigation_and_close() {
    if adw::init().is_err() {
        return;
    }
    let runtime = crate::app::test_runtime();
    let root = std::env::temp_dir().join(format!("zib-window-{}", Id::generate()));
    let dirs = Dirs::under(&root);
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, first, second, target) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let first = state.create_note(space, "prefix\n[[").await.unwrap();
        let second = state.create_note(space, "another note").await.unwrap();
        let target = state.create_note(space, "Linked title").await.unwrap();
        (space, first, second, target)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.WindowTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, first);
    let widget = &window.inner().widget;
    until(|| widget.text().as_deref() == Some("prefix\n[["));
    assert!(widget.ancestor(adw::ApplicationWindow::static_type()).is_some());
    input(widget).emit_by_name::<()>("commit", &[&"Typed "]);
    assert_eq!(widget.text().as_deref(), Some("Typed prefix\n[["));
    assert_eq!(window.inner().title.title(), "Typed prefix");
    let text = widget.text().unwrap();
    widget.insert_link(text.len() - 2..text.len(), "[[", target);
    let expected = format!("Typed prefix\n[[{target}]]");
    // The body changes before the metadata timer, and displayed titles never enter it.
    assert_eq!(runtime.block_on(state.note(space, first)).unwrap().body, expected);
    let peer = Id::generate();
    window.live_event(space, first, LiveEvent::Started { machine: peer });
    assert_eq!(window.inner().title.subtitle(), "Editing live");
    window.live_event(space, first, LiveEvent::Ended { machine: peer });
    assert_eq!(window.inner().title.subtitle(), "");
    window.open_note(space, second);
    until(|| widget.text().as_deref() == Some("another note"));
    window.go(true);
    until(|| widget.text().as_deref() == Some(expected.as_str()));
    // An immediate note switch reports the old note, without copying a view buffer over its body.
    input(widget).emit_by_name::<()>("commit", &[&"Again "]);
    let expected = format!("Again {expected}");
    window.open_note(space, second);
    until(|| widget.text().as_deref() == Some("another note"));
    window.close_editor();
    assert_eq!(widget.text(), None);
    // Closing a note reports pending edits to the search index too.
    until(|| state.search("Again", 20).iter().any(|(s, hit)| *s == space && hit.id == first));
    runtime.block_on(state.finish());
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(runtime.block_on(reopened.note(space, first)).unwrap().body, expected);
    runtime.block_on(reopened.finish());
    window.inner().window.destroy();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn link_picker_creates_a_note_from_the_search_and_links_to_it() {
    if adw::init().is_err() {
        return;
    }
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, source) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        state.create_note(space, "Existing topic").await.unwrap();
        (space, state.create_note(space, "See [[").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.LinkPickerTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.inner().window.present();
    window.open_note(space, source);
    let widget = &window.inner().widget;
    until(|| {
        window.current_space_id() == Some(space) && widget.text().as_deref() == Some("See [[")
    });
    window.open_link_picker(widget, 4..6);
    let popover = widget.last_child().unwrap().downcast::<gtk::Popover>().unwrap();
    let content = popover.child().unwrap();
    let search = content.first_child().unwrap().downcast::<gtk::SearchEntry>().unwrap();
    let scroll = content.last_child().unwrap().downcast::<gtk::ScrolledWindow>().unwrap();
    let viewport = scroll.child().unwrap().downcast::<gtk::Viewport>().unwrap();
    let list = viewport.child().unwrap().downcast::<gtk::ListBox>().unwrap();

    search.set_text("Existing topic");
    until(|| list.selected_row().is_some_and(|row| row.index() == 1));
    search.set_text("  New topic & details  ");
    until(|| list.selected_row().is_some_and(|row| row.index() == 0));
    search.emit_by_name::<()>("activate", &[]);
    until(|| widget.text().is_some_and(|text| text.ends_with("]]")));

    let created = runtime
        .block_on(state.notes(space))
        .unwrap()
        .into_iter()
        .find(|note| note.plain == "New topic & details")
        .expect("the note created by the link picker");
    assert_eq!(
        runtime.block_on(state.note(space, created.id)).unwrap().body,
        "# New topic & details"
    );
    assert_eq!(widget.text().unwrap(), format!("See [[{}]]", created.id));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn space_dictionary_menu_opens_the_note_created_by_adding_a_word() {
    if adw::init().is_err() {
        return;
    }
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, note) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        (space, state.create_note(space, "Original note").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.DictionaryTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, note);
    let widget = &window.inner().widget;
    until(|| widget.text().as_deref() == Some("Original note"));
    window.spelling_dictionary(Some("Quxley".into()));
    until(|| !state.search("quxley", 20).is_empty());
    assert_eq!(widget.text().as_deref(), Some("Original note"));
    WidgetExt::activate_action(&window.inner().window, "win.edit-space-dictionary", None).unwrap();
    until(|| widget.text().as_deref() == Some("# Spelling Dictionary\n\nQuxley"));
    let dictionary = window.inner().editor.borrow().as_ref().unwrap().note;
    input(widget).emit_by_name::<()>("commit", &[&"Edited "]);
    assert_eq!(widget.text().as_deref(), Some("Edited # Spelling Dictionary\n\nQuxley"));
    window.close_editor();
    until(|| state.search("edited", 20).iter().any(|(_, hit)| hit.id == dictionary));
    runtime.block_on(state.finish());
    window.inner().window.destroy();

    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(runtime.block_on(reopened.spelling_dictionary(space, None)).unwrap(), dictionary);
    assert_eq!(
        runtime.block_on(reopened.note(space, dictionary)).unwrap().body,
        "Edited # Spelling Dictionary\n\nQuxley"
    );
    runtime.block_on(reopened.finish());
}

fn detail_values(details: &gtk::MenuButton) -> Vec<(String, gtk::Label)> {
    let scroll =
        details.popover().unwrap().child().unwrap().downcast::<gtk::ScrolledWindow>().unwrap();
    assert!(scroll.measure(gtk::Orientation::Horizontal, -1).0 >= 420);
    let viewport = scroll.child().unwrap().downcast::<gtk::Viewport>().unwrap();
    let rows = viewport.child().unwrap().downcast::<gtk::Grid>().unwrap();
    (0..)
        .map_while(|row| {
            let key = rows.child_at(0, row)?.downcast::<gtk::Label>().unwrap();
            let value = rows.child_at(1, row).unwrap().downcast::<gtk::Label>().unwrap();
            Some((key.text().to_string(), value))
        })
        .collect()
}

#[test]
fn note_metadata_shows_the_note_id_and_populated_fields_without_selecting_them() {
    use zib_core::space::FileHash;
    use zib_core::web_clip::{Clip, Part};

    if adw::init().is_err() {
        return;
    }
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let url = "https://example.org/article?a=1&b=2";
    let bytes = b"file contents";
    let (space, article, file, other) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let article = Id::generate();
        let clip = Clip {
            space,
            note: article,
            title: "Article title".into(),
            url: url.into(),
            captured_at: 1_700_000_000_000,
            published_on: Some("2023-02-28".into()),
            authors: vec!["Test Author".into()],
            body: vec![Part::Markdown("Article body".into())],
        };
        state.import_web_clip(clip, Vec::new()).await.unwrap();
        let file = state
            .add_bytes(
                space,
                bytes.to_vec(),
                "Example file".into(),
                "application/octet-stream".into(),
            )
            .await
            .unwrap();
        (space, article, file, state.create_note(space, "Other note").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ArticleTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, article);
    until(|| window.inner().widget.text().as_deref() == Some("# Article title\n\nArticle body"));

    let details = &window.inner().note_metadata;
    assert!(details.get_visible());
    assert_eq!(details.label().as_deref(), Some("Note metadata"));
    let values = detail_values(details);
    assert_eq!(
        values.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["authors", "captured-at", "note-id", "published-on", "url"],
    );
    assert_eq!(values[0].1.text(), "Test Author");
    assert!(!values[1].1.text().is_empty());
    assert_eq!(values[2].1.text(), article.to_string());
    assert_eq!(values[3].1.text(), "2023-02-28");
    let source = &values[4].1;
    assert_eq!(source.text(), url);
    assert!(source.label().starts_with("<a href=\"https://example.org/article?a=1&amp;b=2\">"));
    assert_eq!(source.xalign().to_bits(), values[0].1.xalign().to_bits());

    present(window.inner().window.upcast_ref());
    details.popup();
    let popover = details.popover().unwrap();
    until(|| popover.is_mapped());
    assert!(values.iter().all(|(_, label)| label.selection_bounds().is_none()));
    let authors = &values[0].1;
    assert!(authors.grab_focus());
    authors.select_region(0, 4);
    assert_eq!(authors.selection_bounds(), Some((0, 4)));
    details.popdown();
    until(|| !popover.is_mapped());
    details.popup();
    until(|| popover.is_mapped());
    assert!(values.iter().all(|(_, label)| label.selection_bounds().is_none()));
    details.popdown();

    window.open_note(space, file);
    until(|| window.inner().widget.text().is_some_and(|body| body.starts_with("Example file")));
    assert!(details.get_visible());
    let values = detail_values(details);
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].0, "file");
    assert_eq!(values[1].0, "note-id");
    assert_eq!(values[1].1.text(), file.to_string());
    let hash = values[0].1.text();
    assert_eq!(hash, FileHash::of(bytes).to_string());

    window.open_note(space, other);
    until(|| window.inner().widget.text().as_deref() == Some("Other note"));
    assert!(details.get_visible());
    let values = detail_values(details);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].0, "note-id");
    assert_eq!(values[0].1.text(), other.to_string());
    assert!(values[0].1.is_selectable());
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}
