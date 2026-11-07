//! The GTK app: the window that `activate` opens, and the bridge between GTK's main loop and the
//! core on Tokio, as decision ZIB0011 describes.

use std::future::Future;
use std::sync::{Arc, OnceLock};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::state::{State, UiEvent};
use crate::window::Window;

static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

#[cfg(test)]
pub(crate) fn test_runtime() -> &'static tokio::runtime::Runtime {
    static TEST_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = TEST_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a test runtime")
    });
    let _ = RUNTIME.set(runtime.handle().clone());
    runtime
}

/// Runs `future` on the core's runtime, and lets GTK's main loop await its result.
pub fn core<F, T>(future: F) -> impl Future<Output = T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let task = RUNTIME.get().expect("the runtime starts first").spawn(future);
    async move { task.await.expect("a core task panicked") }
}

/// Runs the app until the owner quits it. The instance starts with only its tray icon, and opens
/// its window when asked.
pub fn run(state: Arc<State>, runtime: tokio::runtime::Handle) -> glib::ExitCode {
    let _ = RUNTIME.set(runtime);
    let app = adw::Application::builder()
        .application_id(state.channel.app_id())
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(move |app| startup(app, state.clone()));
    // The app runs without a window, so `activate` from GApplication has nothing to do.
    app.connect_activate(|_| {});
    app.run_with_args::<&str>(&[])
}

fn startup(app: &adw::Application, state: Arc<State>) {
    // Keep running with no window open, until the owner quits.
    std::mem::forget(app.hold());
    let quit = gio::SimpleAction::new("quit", None);
    let quitting = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = quitting.upgrade() {
            app.quit();
        }
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    let (sender, receiver) = async_channel::unbounded();
    state.set_ui(sender);
    let window: std::rc::Rc<std::cell::RefCell<Option<Window>>> = std::rc::Rc::default();
    let app = app.clone();
    glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            match event {
                UiEvent::Activate(uri) => {
                    let shown = window.borrow().clone();
                    let shown = if let Some(shown) = shown {
                        shown
                    } else {
                        let made = Window::new(&app, state.clone());
                        *window.borrow_mut() = Some(made.clone());
                        made
                    };
                    shown.present();
                    if let Some(uri) = uri {
                        shown.open_uri(&uri);
                    }
                }
                UiEvent::Quit => app.quit(),
                UiEvent::Changed(space) => {
                    if let Some(shown) = window.borrow().clone() {
                        shown.space_changed(space);
                    }
                }
                UiEvent::ConfirmPairing { peer, code, reply } => {
                    let parent = window.borrow().clone();
                    crate::adding::confirm_request(
                        parent.as_ref(),
                        state.clone(),
                        peer,
                        code,
                        reply,
                    );
                }
                UiEvent::Notice(text) => {
                    if let Some(shown) = window.borrow().clone() {
                        shown.toast(&text);
                    }
                }
                UiEvent::NameSpace { space, from, arrived } => {
                    if let Some(shown) = window.borrow().clone() {
                        shown.name_space(space, from, arrived);
                    }
                }
            }
        }
    });
}
