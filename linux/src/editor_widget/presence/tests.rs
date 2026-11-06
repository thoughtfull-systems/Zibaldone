use std::cell::Cell;
use std::rc::Rc;

use zib_core::Space;
use zib_core::editor::Editor;
use zib_core::editor::display::ImageSize;

use super::*;
use crate::editor_widget::tests::{render, texture};

fn editor(text: &str) -> Option<(EditorWidget, Space, Id)> {
    adw::init().ok()?;
    let mut space = Space::create(Id::generate()).unwrap();
    let note = space.create_note("note", text).unwrap();
    let mut core = Editor::new(space.open_body(note).unwrap());
    core.move_to(text.len(), false);
    let widget = EditorWidget::default();
    widget.set_editor(Some(core));
    widget.measure(gtk::Orientation::Horizontal, -1);
    widget.measure(gtk::Orientation::Vertical, 600);
    widget.allocate(600, 700, -1, None);
    Some((widget, space, note))
}

fn selection(space: &Space, note: Id, anchor: usize, head: usize) -> Selection {
    Selection {
        anchor: space.cursor_at(note, anchor).unwrap(),
        head: space.cursor_at(note, head).unwrap(),
    }
}

#[test]
fn cursor_notifications_send_the_new_editors_directed_source_selection() {
    let Some((widget, space, note)) = editor("é **bold**\nend") else { return };
    let changes = Rc::new(Cell::new(0));
    let count = changes.clone();
    widget.connect_selection_changed(move || count.set(count.get() + 1));
    widget.edit(|e| e.select(9, 5));
    let encoded = widget.encoded_selection().unwrap();
    assert_eq!(space.cursor_position(&encoded.anchor), Some(8));
    assert_eq!(space.cursor_position(&encoded.head), Some(4));
    assert_eq!(changes.get(), 1);
    widget.edit(|e| e.insert("tea"));
    let encoded = widget.encoded_selection().unwrap();
    assert_eq!(space.cursor_position(&encoded.head), Some(7));
    assert_eq!(space.note(note).unwrap().body, "é **tea**\nend");
    assert_eq!(changes.get(), 2);
}

#[test]
fn remote_highlights_use_hidden_syntax_mapping_and_render_without_revealing_source() {
    let Some((widget, space, note)) = editor("before **bold** after\nend") else { return };
    let machine = Id::generate();
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None::<&gdk::Surface>).unwrap();
    let before = render(&widget, &renderer);
    let original = widget.imp().lines.borrow()[0].layout.text();
    widget.remote_started(machine);
    widget.remote_received(machine, vec![selection(&space, note, 13, 9)]);
    widget.remote_name(widget.session(), machine, "Peer <&>".to_owned());
    assert_eq!(widget.imp().lines.borrow()[0].layout.text(), original);
    assert_eq!(original, "before bold after");
    let selected = widget.imp().editor.borrow().as_ref().unwrap().remote_selections();
    let highlights = rects(&widget.imp().lines.borrow(), &selected[0].selection.range());
    assert_eq!(highlights.len(), 1);
    assert!(highlights[0].width() > 20.0);
    assert_ne!(render(&widget, &renderer), before);
    widget.remote_ended(machine);
    assert_eq!(render(&widget, &renderer), before);
    renderer.unrealize();
    assert!(!widget.imp().editor.borrow().as_ref().unwrap().can_undo());
}

#[test]
fn image_and_block_selections_cover_the_preview_without_opening_its_source() {
    let image = Id::generate();
    let text = format!("before ![[{image}]] after\n```mermaid\na-->b\n```\nend");
    let Some((widget, space, note)) = editor(&text) else { return };
    let (session, _) = widget.image_requests(false);
    widget.set_image(session, image, Some(texture(160, 240)));
    let (session, drawings) = widget.drawing_requests();
    widget.set_drawing(
        session,
        drawings[0].clone(),
        Ok((texture(100, 100), ImageSize { width: 100, height: 100 })),
    );
    let machine = Id::generate();
    widget.remote_started(machine);
    let block = text.find("a-->").unwrap();
    widget.remote_received(machine, vec![selection(&space, note, 7, block + 2)]);
    let selected = widget.imp().editor.borrow().as_ref().unwrap().remote_selections();
    let lines = widget.imp().lines.borrow();
    assert!(lines[0].images[0].source_layout.is_none());
    assert!(lines[1].blocks[0].collapsed);
    let highlights = rects(&lines, &selected[0].selection.range());
    assert!(highlights.iter().any(|r| r.height() >= 240.0));
    assert!(
        highlights
            .iter()
            .any(|r| { (r.width() - 100.0).abs() < 0.01 && (r.height() - 100.0).abs() < 0.01 })
    );
    let (_, y, height) = caret(&lines, block + 2).unwrap();
    assert_eq!(height, 100.0);
    assert!(y >= lines[1].y);
}

#[test]
fn incoming_edits_keep_the_visible_caret_still_and_edit_before_the_notification_is_safe() {
    let text = format!("{}café\n{}", "earlier\n".repeat(35), "later\n".repeat(35));
    let Some((widget, mut space, note)) = editor(&text) else { return };
    let at = text.find("café").unwrap();
    widget.edit(|e| e.select(at, at + "café".len()));
    let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
    scrolled.measure(gtk::Orientation::Horizontal, -1);
    scrolled.measure(gtk::Orientation::Vertical, 600);
    scrolled.allocate(600, 400, -1, None);
    let height = widget.lay_out(600);
    let adjustment = scrolled.vadjustment();
    let caret_y = caret_rect(&widget.imp().lines.borrow(), widget.head().unwrap()).unwrap().1;
    adjustment.configure(caret_y - 120.0, 0.0, f64::from(height), 10.0, 380.0, 400.0);
    let screen_y = caret_y - adjustment.value();
    space.insert_text(note, 0, "new\n".repeat(10).as_str()).unwrap();
    space.commit();
    widget.refresh();
    let y = caret_rect(&widget.imp().lines.borrow(), widget.head().unwrap()).unwrap().1;
    assert!((y - adjustment.value() - screen_y).abs() < 0.01);
    assert_eq!(widget.imp().editor.borrow().as_ref().unwrap().selected_text(), "café");
    // Simulate an import reaching the shared body before the window handles its Changed event.
    space.insert_text(note, 0, "pending\n").unwrap();
    space.commit();
    widget.edit(|e| e.insert("tea"));
    assert!(widget.text().unwrap().contains("tea"));
    assert!(!widget.text().unwrap().contains("café"));
    assert!(widget.text().unwrap().starts_with("pending\nnew\n"));
}

#[test]
fn incoming_edits_offscreen_do_not_scroll_to_the_owner_and_note_switches_clear_presence() {
    let text = "line\n".repeat(80);
    let Some((widget, mut space, note)) = editor(&text) else { return };
    let machine = Id::generate();
    widget.remote_started(machine);
    widget.remote_received(machine, vec![selection(&space, note, 1, 3)]);
    let session = widget.session();
    let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
    scrolled.measure(gtk::Orientation::Horizontal, -1);
    scrolled.measure(gtk::Orientation::Vertical, 600);
    scrolled.allocate(600, 400, -1, None);
    let height = widget.lay_out(600);
    let adjustment = scrolled.vadjustment();
    adjustment.configure(200.0, 0.0, f64::from(height), 10.0, 380.0, 400.0);
    space.insert_text(note, 0, "incoming\n").unwrap();
    space.commit();
    widget.refresh();
    assert_eq!(adjustment.value(), 200.0);
    assert_eq!(widget.imp().editor.borrow().as_ref().unwrap().remote_selections().len(), 1);
    let other = space.create_note("note", "another note").unwrap();
    widget.set_editor(Some(Editor::new(space.open_body(other).unwrap())));
    widget.remote_name(session, machine, "late name".to_owned());
    widget.remote_received(machine, vec![selection(&space, note, 1, 3)]);
    assert_eq!(widget.imp().editor.borrow().as_ref().unwrap().remote_selections(), vec![]);
}
