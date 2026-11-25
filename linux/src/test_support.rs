use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

/// Require the private display before initializing GTK, even if a desktop display is available.
pub(crate) fn init_gtk() {
    assert_eq!(
        std::env::var("ZIB_TEST_DISPLAY").as_deref(),
        Ok("1"),
        "run GTK tests with devenv shell -- cargo nextest run --workspace"
    );
    adw::init().expect("GTK connects to the private test display");
}

/// Drive GTK's queued work without waiting for a display event or the periodic status timer.
#[track_caller]
pub(crate) fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "GTK work didn't finish");
        if !glib::MainContext::default().iteration(false) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Wait for the first frame so Wayland can place popovers and GTK can snapshot mapped children.
pub(crate) fn present(window: &gtk::Window) {
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
