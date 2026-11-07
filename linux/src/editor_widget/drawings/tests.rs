use gtk::subclass::prelude::*;
use gtk::{gdk, glib};
use zib_core::editor::{Body, Editor};

use super::*;
use crate::editor_widget::tests::{head, texture, widget};
use crate::editor_widget::{EditorWidget, image_rect};

fn supply(widget: &EditorWidget, size: ImageSize) -> Drawing {
    let (session, targets) = widget.drawing_requests();
    assert_eq!(targets.len(), 1);
    let source = targets[0].clone();
    widget.set_drawing(session, source.clone(), Ok((texture(200, 100), size)));
    source
}

#[test]
fn inline_math_uses_natural_size_and_reveals_its_exact_source() {
    let Some(widget) = widget("before $x^2$ after\n|end") else { return };
    let source = supply(&widget, ImageSize { width: 60, height: 30 });
    assert_eq!(source, Drawing::InlineMath("x^2".into()));
    let lines = widget.imp().lines.borrow();
    let line = &lines[0];
    assert_eq!(line.layout.text(), "before \u{fffc} after");
    assert_eq!(line.images.len(), 1);
    let image = &line.images[0];
    assert!((image.width - 60.0 * crate::editor_widget::math_scale(&widget)).abs() < 0.01);
    assert_eq!(image.source, 7);
    assert_eq!(image.end, 12);
    let rect = image_rect(line, image);
    let (x, y) = (line.left + f64::from(rect.x()) + 2.0, line.y + f64::from(rect.y()) + 2.0);
    drop(lines);
    assert_eq!(widget.position_at(x, y), 7);
    widget.edit(|e| e.move_to(7, false));
    assert_eq!(widget.imp().lines.borrow()[0].layout.text(), "before $x^2$ after");
    assert_eq!(widget.imp().lines.borrow()[0].images.len(), 0);
    widget.edit(|e| e.move_to(e.text().len(), false));
    assert_eq!(widget.imp().lines.borrow()[0].images.len(), 1);
    assert_eq!(widget.text().as_deref(), Some("before $x^2$ after\nend"));
    assert!(!widget.imp().editor.borrow().as_ref().unwrap().can_undo());
}

#[test]
fn blocks_collapse_their_lines_and_stay_below_revealed_source() {
    let text = "- ```mermaid\n  flowchart LR\n  a-->b\n  ```\n\n|end";
    let Some(widget) = widget(text) else { return };
    supply(&widget, ImageSize { width: 480, height: 160 });
    let lines = widget.imp().lines.borrow();
    assert_eq!(lines.len(), 3);
    let line = &lines[0];
    assert_eq!(line.layout.text(), "- ");
    assert!(!line.shaded);
    let block = &line.blocks[0];
    let natural = f64::from(block.rect.width());
    assert_eq!(natural, 480.0);
    assert!(line.left + f64::from(block.rect.x()) > 44.0);
    assert!(line.left + f64::from(block.rect.x() + block.rect.width()) <= 576.01);
    let entry = widget.text().unwrap().find("flowchart").unwrap();
    assert_eq!(block.cursor, entry);
    let (x, y) =
        (line.left + f64::from(block.rect.x()) + 1.0, line.y + f64::from(block.rect.y()) + 1.0);
    drop(lines);
    assert_eq!(widget.position_at(x, y), entry);
    widget.edit(|e| e.move_to(entry, false));
    let lines = widget.imp().lines.borrow();
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[1].layout.text(), "  flowchart LR");
    assert!(lines[3].shaded);
    assert_eq!(lines[3].blocks.len(), 1);
    let block = &lines[3].blocks[0];
    assert_eq!(f64::from(block.rect.width()), natural);
    assert!(f64::from(block.rect.y()) > pixels(lines[3].layout.size().1));
}

#[test]
fn display_math_preserves_text_before_and_after_its_source() {
    let Some(widget) = widget("before $$\nx+y\n$$ after\n|end") else { return };
    supply(&widget, ImageSize { width: 80, height: 40 });
    let lines = widget.imp().lines.borrow();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].layout.text(), "before ");
    assert_eq!(lines[0].blocks.len(), 1);
    assert_eq!(lines[1].layout.text(), " after");
    assert_eq!(lines[2].layout.text(), "end");
    assert!(lines[0].y + lines[0].height <= lines[1].y);
}

#[test]
fn failures_keep_source_and_block_errors_wrap_without_entering_the_body() {
    for text in ["$bad$\n|end", "```mermaid\nbad\n```\n|end"] {
        let Some(widget) = widget(text) else { return };
        let (session, targets) = widget.drawing_requests();
        widget.set_drawing(session, targets[0].clone(), Err("cannot draw this source ".repeat(12)));
        widget.lay_out(240);
        assert_eq!(widget.text(), Some(text.replace('|', "")));
        let lines = widget.imp().lines.borrow();
        assert!(lines.iter().all(|line| line.images.is_empty()));
        if text.starts_with('$') {
            assert_eq!(lines[0].layout.text(), "$bad$");
            assert_eq!(lines[0].blocks.len(), 0);
        } else {
            assert_eq!(lines[0].layout.text(), "```mermaid");
            let block = &lines[2].blocks[0];
            assert!(matches!(block.content, Content::Error(_)));
            assert!(block.rect.width() <= 192.01);
            assert!(block.rect.height() > 30.0);
        }
        assert!(!widget.imp().editor.borrow().as_ref().unwrap().can_undo());
    }
}

#[test]
fn render_results_from_previous_notes_or_previous_source_cannot_replace_current_math() {
    let Some(widget) = widget("$old$\n|end") else { return };
    let (old_session, old) = widget.drawing_requests();
    assert_eq!(widget.drawing_requests().1.len(), 0);
    widget.set_editor(Some(Editor::new(Body::detached("$new$\nend"))));
    widget.edit(|e| e.move_to(e.text().len(), false));
    widget.set_drawing(
        old_session,
        old[0].clone(),
        Ok((texture(100, 50), ImageSize { width: 50, height: 25 })),
    );
    assert_eq!(widget.imp().drawings.borrow().len(), 0);
    let (session, new) = widget.drawing_requests();
    widget.set_drawing(
        session,
        old[0].clone(),
        Ok((texture(100, 50), ImageSize { width: 50, height: 25 })),
    );
    assert_eq!(widget.imp().lines.borrow()[0].layout.text(), "$new$");
    widget.set_drawing(
        session,
        new[0].clone(),
        Ok((texture(100, 50), ImageSize { width: 50, height: 25 })),
    );
    assert_eq!(widget.imp().lines.borrow()[0].images.len(), 1);
}

#[test]
fn moving_into_a_drawn_block_reveals_its_source_for_editing() {
    let Some(widget) = widget("above|\n```math\nx+y\n```\nend") else { return };
    supply(&widget, ImageSize { width: 80, height: 40 });
    widget.key(gdk::Key::Down, gdk::ModifierType::SHIFT_MASK);
    let entry = widget.text().unwrap().find("x+y").unwrap();
    assert_eq!(head(&widget), entry);
    assert_eq!(widget.imp().lines.borrow().len(), 5);
    assert_eq!(widget.imp().lines.borrow()[3].blocks.len(), 1);
    widget.edit(|e| {
        e.move_to(entry, false);
        e.insert("z");
    });
    assert!(widget.text().unwrap().contains("\nzx+y\n"));
}

#[test]
fn quoted_drawings_keep_their_bars_and_fit_the_remaining_width() {
    for (text, left) in [
        ("> ```mermaid\n> a-->b\n> ```\n\n|end", 42.0),
        ("- > ```mermaid\n  > a-->b\n  > ```\n\n|end", 24.0),
    ] {
        let Some(widget) = widget(text) else { return };
        supply(&widget, ImageSize { width: 900, height: 300 });
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        let block = &line.blocks[0];
        assert_eq!(line.left, left);
        let bar = super::super::quote_rects(line)[0];
        let origin = f64::from(bar.x()) + 14.0;
        assert!((line.left + f64::from(block.rect.x()) - origin).abs() < 0.01);
        assert!((line.left + f64::from(block.rect.x() + block.rect.width()) - 216.0).abs() < 0.01);
        assert!(bar.height() > block.rect.height());
    }
}

#[test]
fn renderer_output_decodes_and_draws_actual_math_and_mermaid() {
    use zib_core::render::{Kind, Renderer};

    let text = concat!(
        "Inline $x^2 + y^2$ after\n\n",
        "$$\n\\frac{a}{b} = c\n$$\n\n",
        "```mermaid\nflowchart LR\nidea-->note\n```\n\n|end"
    );
    let Some(widget) = widget(text) else { return };
    let _window = gtk::Window::builder().child(&widget).build();
    let provider = gtk::CssProvider::new();
    provider.load_from_string("* { color: #222; }");
    gtk::style_context_add_provider_for_display(
        &widget.display(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let stamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let cache = std::env::temp_dir().join(format!("zib-editor-render-{stamp}"));
    let renderer = Renderer::new(&cache);
    let (session, targets) = widget.drawing_requests();
    assert_eq!(targets.len(), 3);
    for source in targets {
        let (kind, text) = match &source {
            Drawing::InlineMath(s) => (Kind::InlineMath, s),
            Drawing::DisplayMath(s) => (Kind::DisplayMath, s),
            Drawing::Diagram(s) => (Kind::Mermaid, s),
            _ => unreachable!(),
        };
        let image = renderer.image(kind, text).unwrap();
        let texture = gdk::Texture::from_bytes(&glib::Bytes::from(&image.png)).unwrap();
        assert!(u32::try_from(texture.width()).unwrap() > image.width);
        widget.set_drawing(
            session,
            source,
            Ok((texture, ImageSize { width: image.width, height: image.height })),
        );
    }
    widget.measure(gtk::Orientation::Horizontal, -1);
    widget.measure(gtk::Orientation::Vertical, 600);
    widget.allocate(600, 600, -1, None);
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None::<&gdk::Surface>).unwrap();
    let snapshot = gtk::Snapshot::new();
    let viewport = graphene::Rect::new(0.0, 0.0, 600.0, 600.0);
    snapshot.append_color(&gdk::RGBA::WHITE, &viewport);
    widget.draw(&snapshot);
    let output = renderer.render_texture(snapshot.to_node().unwrap(), Some(&viewport));
    renderer.unrealize();
    let mut bytes = vec![0; 600 * 600 * 4];
    output.download(&mut bytes, 600 * 4);
    let lines = widget.imp().lines.borrow();
    assert_eq!(lines[0].images.len(), 1);
    assert_eq!(lines.iter().map(|line| line.blocks.len()).sum::<usize>(), 2);
    for line in lines.iter() {
        for block in &line.blocks {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (left, top, width, height) = (
                (line.left + f64::from(block.rect.x())) as usize,
                (line.y + f64::from(block.rect.y())) as usize,
                block.rect.width() as usize,
                block.rect.height() as usize,
            );
            let ink = (top..top + height).any(|y| {
                (left..left + width).any(|x| bytes[(y * 600 + x) * 4..][..3] != [255, 255, 255])
            });
            assert!(ink);
        }
    }
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn a_render_finishing_above_the_caret_preserves_its_viewport_position() {
    let text =
        format!("{}\n```mermaid\na-->b\n```\n|{}", "before\n".repeat(20), "after\n".repeat(40));
    let Some(widget) = widget(&text) else { return };
    let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
    scrolled.measure(gtk::Orientation::Horizontal, -1);
    scrolled.measure(gtk::Orientation::Vertical, 600);
    scrolled.allocate(600, 500, -1, None);
    let height = widget.lay_out(600);
    let y = super::super::caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
    let adjustment = scrolled.vadjustment();
    adjustment.configure(y - 200.0, 0.0, f64::from(height), 10.0, 480.0, 500.0);
    supply(&widget, ImageSize { width: 200, height: 500 });
    scrolled.measure(gtk::Orientation::Horizontal, -1);
    scrolled.measure(gtk::Orientation::Vertical, 600);
    scrolled.allocate(600, 500, -1, None);
    let y = super::super::caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
    assert!((y - adjustment.value() - 200.0).abs() < 0.01);
}

#[test]
fn diagram_and_display_math_keep_their_pixels_and_height_until_replacement_is_ready() {
    fn render(widget: &EditorWidget) -> Vec<u8> {
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer.realize(None::<&gdk::Surface>).unwrap();
        let bytes = crate::editor_widget::tests::render(widget, &renderer);
        renderer.unrealize();
        bytes
    }

    for text in ["```mermaid\na-->b|\n```\nend", "$$\nx+y|\n$$\nend"] {
        let Some(widget) = widget(text) else { return };
        let original = supply(&widget, ImageSize { width: 120, height: 80 });
        let before = render(&widget);
        let (rect, before_texture, bottom) = {
            let lines = widget.imp().lines.borrow();
            let line = lines.iter().find(|line| !line.blocks.is_empty()).unwrap();
            (line.blocks[0].rect, line.blocks[0].texture().unwrap(), lines.last().unwrap().y)
        };
        widget.edit(|e| e.insert("z"));
        let (session, pending) = widget.drawing_requests();
        assert_eq!(pending.len(), 1);
        assert_ne!(pending[0], original);
        let during = render(&widget);
        let pixels = |bytes: &[u8]| {
            let lines = widget.imp().lines.borrow();
            let line = lines.iter().find(|line| !line.blocks.is_empty()).unwrap();
            let rect = line.blocks[0].rect;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (left, top, right, bottom) = (
                (line.left + f64::from(rect.x())).ceil() as usize,
                (line.y + f64::from(rect.y())).ceil() as usize,
                (line.left + f64::from(rect.x() + rect.width())).floor() as usize,
                (line.y + f64::from(rect.y() + rect.height())).floor() as usize,
            );
            (top..bottom)
                .flat_map(|y| bytes[(y * 600 + left) * 4..(y * 600 + right) * 4].iter().copied())
                .collect::<Vec<_>>()
        };
        assert!(pixels(&during).iter().eq(pixels(&before).iter()), "preview changed while pending");
        {
            let lines = widget.imp().lines.borrow();
            let line = lines.iter().find(|line| !line.blocks.is_empty()).unwrap();
            assert_eq!(line.blocks[0].rect, rect);
            assert_eq!(line.blocks[0].texture().unwrap(), before_texture);
            assert_eq!(lines.last().unwrap().y, bottom);
        }
        // A completed replacement of the same size changes only the picture, in one update.
        let replacement = texture(1, 1);
        widget.set_drawing(
            session,
            pending[0].clone(),
            Ok((replacement.clone(), ImageSize { width: 120, height: 80 })),
        );
        let after = render(&widget);
        assert!(
            !pixels(&after).iter().eq(pixels(&during).iter()),
            "new preview did not replace the old one"
        );
        let lines = widget.imp().lines.borrow();
        let line = lines.iter().find(|line| !line.blocks.is_empty()).unwrap();
        assert_eq!(line.blocks[0].rect, rect);
        assert_eq!(line.blocks[0].texture().unwrap(), replacement);
        assert_eq!(lines.last().unwrap().y, bottom);
    }
}

#[test]
fn inline_math_keeps_its_old_atom_when_leaving_source_before_the_new_render_finishes() {
    let Some(widget) = widget("before $x+y|$ after\nend") else { return };
    let original = supply(&widget, ImageSize { width: 90, height: 30 });
    widget.edit(|e| e.insert("z"));
    let (session, pending) = widget.drawing_requests();
    assert_ne!(pending[0], original);
    widget.edit(|e| e.move_to(e.text().len(), false));
    let original_width = {
        let lines = widget.imp().lines.borrow();
        let image = &lines[0].images[0];
        assert_eq!(lines[0].layout.text(), "before \u{fffc} after");
        image.width
    };
    widget.set_drawing(
        session,
        pending[0].clone(),
        Ok((texture(100, 50), ImageSize { width: 120, height: 30 })),
    );
    assert!(widget.imp().lines.borrow()[0].images[0].width > original_width);
}
