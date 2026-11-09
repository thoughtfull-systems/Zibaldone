use gtk::{gdk, glib};
use zib_core::editing::Drawing;
use zib_core::editor::{Body, Editor};

use super::*;
use crate::editor_widget::drawings::Content;
use crate::editor_widget::tests::allocate_viewport;

fn editor(text: &str, width: i32) -> Option<(EditorWidget, gtk::Window)> {
    adw::init().ok()?;
    let widget = EditorWidget::default();
    let window = gtk::Window::builder().child(&widget).build();
    let mut editor = Editor::new(Body::detached(text));
    editor.move_to(text.len(), false);
    widget.set_editor(Some(editor));
    let (session, tables) = widget.drawing_requests();
    for table in tables {
        assert!(matches!(table, Drawing::Table(_)));
        widget.set_table(session, widget.table_revision(), table, crate::tables::Refs::default());
    }
    widget.measure(gtk::Orientation::Horizontal, -1);
    widget.measure(gtk::Orientation::Vertical, width);
    widget.allocate(width, 700, -1, None);
    Some((widget, window))
}

fn grids(editor: &EditorWidget) -> Vec<gtk::Widget> {
    editor
        .imp()
        .lines
        .borrow()
        .iter()
        .flat_map(|line| &line.blocks)
        .filter_map(|block| {
            if let Content::Table(layout) = &block.content {
                Some(layout.widget.clone())
            } else {
                None
            }
        })
        .collect()
}

fn selected(editor: &EditorWidget) -> String {
    editor.imp().editor.borrow().as_ref().unwrap().selected_text().to_owned()
}

fn label(widget: &gtk::Widget, text: &str) -> Option<gtk::Widget> {
    if widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == text) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(found) = label(&widget, text) {
            return Some(found);
        }
    }
    None
}

fn click_label(editor: &EditorWidget, grid: &gtk::Widget, text: &str) {
    let cell = label(grid, text).unwrap();
    let point = cell.compute_bounds(editor).unwrap().center();
    assert!(editor.select_cell_at(f64::from(point.x()), f64::from(point.y())));
}

#[test]
fn pipe_and_list_tables_use_the_existing_grid_headers_and_cell_selection() {
    for text in [
        "| h | other |\n|---|---|\n| a | b |\n\nend",
        "```table\n- - h\n  - other\n- - a\n  - b\n```\nend",
    ] {
        let Some((editor, _window)) = editor(text, 600) else { return };
        let grid = grids(&editor).remove(0);
        assert_eq!(grid.parent().as_ref(), Some(editor.upcast_ref()));
        let native = grid.downcast_ref::<gtk::Grid>().unwrap();
        assert!(native.child_at(0, 0).unwrap().has_css_class("zib-table-header"));
        assert!(!native.child_at(0, 1).unwrap().has_css_class("zib-table-header"));
        click_label(&editor, &grid, "a");
        assert_eq!(selected(&editor), "a");
        assert_eq!(grids(&editor)[0], grid);
        assert!(editor.imp().lines.borrow().len() >= 5);
        if text.starts_with('|') {
            editor.key(gdk::Key::Tab, gdk::ModifierType::empty());
            assert_eq!(selected(&editor), "b");
            editor.key(gdk::Key::ISO_Left_Tab, gdk::ModifierType::SHIFT_MASK);
            assert_eq!(selected(&editor), "a");
        } else {
            click_label(&editor, &grid, "b");
            editor.key(gdk::Key::ISO_Left_Tab, gdk::ModifierType::SHIFT_MASK);
            assert!(editor.text().unwrap().contains("\n- b"));
            assert_eq!(selected(&editor), "b");
            editor.key(gdk::Key::Tab, gdk::ModifierType::empty());
            assert_eq!(editor.text().as_deref(), Some(text));
            assert_eq!(selected(&editor), "b");
        }
    }
}

#[test]
fn identical_tables_get_distinct_native_children_and_note_switches_remove_them() {
    let text = "| h |\n|---|\n| a |\n\n| h |\n|---|\n| a |\n\nend";
    let Some((editor, _window)) = editor(text, 600) else { return };
    let grids = grids(&editor);
    assert_eq!(grids.len(), 2);
    assert_ne!(grids[0], grids[1]);
    assert!(
        grids[0].compute_bounds(&editor).unwrap().y()
            < grids[1].compute_bounds(&editor).unwrap().y()
    );
    editor.set_editor(Some(Editor::new(Body::detached("another note"))));
    assert_eq!(editor.first_child(), None::<gtk::Widget>);
    assert!(grids.iter().all(|grid| grid.parent().is_none()));
}

#[test]
fn small_tables_start_at_the_left_margin() {
    for text in ["| h |\n|---|\n| 1 |\n\nend", "```table\n- - h\n```\n\nend"] {
        let Some((editor, _window)) = editor(text, 600) else { return };
        let grid = grids(&editor).remove(0);
        let bounds = grid.compute_bounds(&editor).unwrap();
        assert!(bounds.width() < 300.0);
        assert!((bounds.x() - 24.0).abs() < 0.01, "{text}: {}", bounds.x());
    }
}

#[test]
fn arrows_enter_rendered_tables_from_the_nearest_source_line() {
    for (table, first, last) in [
        ("head | other\n--- | ---\nlast | cell", "head | other", "last | cell"),
        ("```table\n- - head\n  - cell\n```", "- - head", "```"),
    ] {
        let text = format!("above\n\n{table}\n\nbelow");
        let Some((editor, _window)) = editor(&text, 600) else { return };
        let keys = gdk::ModifierType::empty();
        editor.key(gdk::Key::Up, keys);
        editor.key(gdk::Key::Up, keys);
        assert_eq!(editor.head(), text.rfind(last), "{table}");

        editor.edit(|e| e.move_to(0, false));
        editor.key(gdk::Key::Down, keys);
        editor.key(gdk::Key::Down, keys);
        assert_eq!(editor.head(), text.find(first), "{table}");
    }
}

#[test]
fn entering_and_leaving_a_table_near_the_title_does_not_extend_the_document() {
    for table in ["| header |\n|---|\n| value |", "```table\n- - header\n- - value\n```"] {
        let text = format!("# Title\n\n{table}\n\nend");
        let Some((editor, window)) = editor(&text, 600) else { return };
        window.set_child(None::<&gtk::Widget>);
        let scrolled = gtk::ScrolledWindow::builder().child(&editor).build();
        allocate_viewport(&scrolled, 600, 700);
        let height = editor.lay_out(600);
        let title_y = editor.imp().lines.borrow()[0].y;
        let adjustment = scrolled.vadjustment();
        adjustment.configure(0.0, 0.0, f64::from(height), 10.0, 680.0, 700.0);
        for _ in 0..4 {
            let grid = grids(&editor).remove(0);
            click_label(&editor, &grid, "value");
            editor.scroll_to_cursor();
            assert_eq!(editor.imp().lines.borrow()[0].y, title_y);
            editor.edit(|e| e.move_to(text.len(), false));
            editor.scroll_to_cursor();
            assert_eq!(editor.imp().lines.borrow()[0].y, title_y);
            assert_eq!(editor.lay_out(600), height);
            assert_eq!(adjustment.value(), 0.0);
        }
        assert_eq!(editor.text().as_deref(), Some(text.as_str()));
    }
}

#[test]
fn narrow_tables_fit_and_scaled_cell_hit_testing_selects_the_right_source() {
    let text = concat!(
        "| h | h | h | h | h | h | h | h |\n",
        "|---|---|---|---|---|---|---|---|\n",
        "| 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |\n\nend"
    );
    let Some((editor, _window)) = editor(text, 240) else { return };
    let grid = grids(&editor).remove(0);
    let bounds = grid.compute_bounds(&editor).unwrap();
    assert!(bounds.width() <= 192.01);
    assert!(bounds.x() >= 23.99);
    assert!(bounds.x() + bounds.width() <= 216.01);
    click_label(&editor, &grid, "8");
    assert_eq!(selected(&editor), "8");
}

#[test]
fn nested_pipe_and_list_table_cells_select_the_inner_source() {
    for inner in ["```table\n- - inner\n  - value\n```", "| inner | value |\n|---|---|"] {
        let text =
            format!("````table\n- - outer\n  - {}\n````\nend", inner.replace('\n', "\n    "));
        let Some((editor, _window)) = editor(&text, 600) else { return };
        let grid = grids(&editor).remove(0);
        click_label(&editor, &grid, "value");
        assert_eq!(selected(&editor), "value");
    }
}

#[test]
fn table_edits_retain_the_old_grid_until_its_replacement_is_ready() {
    let text = "```table\n- - header\n- - value\n```\nend";
    let Some((editor, _window)) = editor(text, 600) else { return };
    let old = grids(&editor).remove(0);
    click_label(&editor, &old, "value");
    editor.edit(|e| e.insert("changed"));
    assert_eq!(grids(&editor)[0], old);
    let (session, requests) = editor.drawing_requests();
    assert_eq!(requests.len(), 1);
    editor.set_table(
        session,
        editor.table_revision(),
        requests[0].clone(),
        crate::tables::Refs::default(),
    );
    let new = grids(&editor).remove(0);
    assert_ne!(old, new);
    assert_eq!(old.parent(), None::<gtk::Widget>);
    assert!(label(&new, "changed").is_some());
}

#[test]
fn table_snapshots_include_native_borders_and_headers() {
    // Native child snapshots need a mapped window. Broadway keeps this off the owner's desktop.
    if std::env::var("GDK_BACKEND").as_deref() != Ok("broadway") {
        return;
    }
    let Some((editor, window)) = editor("| h | other |\n|---|---|\n| a | b |\n\nend", 600) else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string("* { color: #222; }");
    gtk::style_context_add_provider_for_display(
        &editor.display(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    window.present();
    for _ in 0..100 {
        if !glib::MainContext::default().iteration(false) {
            break;
        }
    }
    editor.rebuild_layout();
    editor.measure(gtk::Orientation::Horizontal, -1);
    editor.measure(gtk::Orientation::Vertical, 600);
    editor.allocate(600, 700, -1, None);
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None::<&gdk::Surface>).unwrap();
    let snapshot = gtk::Snapshot::new();
    let viewport = graphene::Rect::new(0.0, 0.0, 600.0, 260.0);
    snapshot.append_color(&gdk::RGBA::WHITE, &viewport);
    editor.draw(&snapshot);
    let output = renderer.render_texture(snapshot.to_node().unwrap(), Some(&viewport));
    renderer.unrealize();
    let mut bytes = vec![0; 600 * 260 * 4];
    output.download(&mut bytes, 600 * 4);
    let grid = grids(&editor).remove(0);
    let bounds = grid.compute_bounds(&editor).unwrap();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x, y) = ((bounds.x() + 1.0) as usize, (bounds.y() + 1.0) as usize);
    assert_ne!(&bytes[(y * 600 + x) * 4..][..3], &[255, 255, 255]);
}

#[test]
fn table_reference_reloads_reject_older_results_and_results_from_other_notes() {
    let id: zib_core::Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let text = format!("| [[{id}]] |\n|---|\n\nend");
    let Some((editor, _window)) = editor(&text, 600) else { return };
    let old = grids(&editor).remove(0);
    let source = editor.imp().editor.borrow().as_ref().unwrap().drawing_targets()[0].clone();
    let session = editor.imp().session.get();
    let revision = editor.table_revision();
    editor.reload_tables();
    let refs = || crate::tables::Refs {
        titles: std::collections::HashMap::from([(id, "new title".to_owned())]),
        ..Default::default()
    };
    editor.set_table(session, revision, source.clone(), refs());
    assert_eq!(grids(&editor)[0], old);
    editor.set_table(session, editor.table_revision(), source.clone(), refs());
    assert!(label(&grids(&editor)[0], "new title").is_some());
    editor.set_editor(Some(Editor::new(Body::detached("another note"))));
    editor.set_table(session, editor.table_revision(), source, refs());
    assert_eq!(editor.imp().drawings.borrow().len(), 0);
    assert_eq!(editor.first_child(), None::<gtk::Widget>);
}
