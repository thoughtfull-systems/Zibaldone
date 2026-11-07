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

fn input(widget: &EditorWidget) -> gtk::IMContext {
    let controllers = widget.observe_controllers();
    (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .find_map(|keys| keys.im_context())
        .expect("the editor's input method")
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
