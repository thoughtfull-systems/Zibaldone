//! Remote selections use the same source-to-layout mapping as the owner's selection. They never
//! change which syntax is revealed, take focus, or scroll the view.

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene, pango};
use zib_core::Id;
use zib_core::editor::presence::RemoteSelection;
use zib_core::sync::Selection;

use super::{EditorWidget, Line, caret_rect, image_rect, selection_rects, source_rect};

impl EditorWidget {
    pub fn session(&self) -> u64 {
        self.imp().session.get()
    }

    pub fn encoded_selection(&self) -> Option<Selection> {
        self.imp().editor.borrow().as_ref()?.encoded_selection()
    }

    pub fn remote_started(&self, machine: Id) {
        if let Some(editor) = self.imp().editor.borrow_mut().as_mut() {
            editor.presence().started(machine);
        }
    }

    pub fn remote_ended(&self, machine: Id) {
        if let Some(editor) = self.imp().editor.borrow_mut().as_mut() {
            editor.presence().ended(machine);
        }
        self.queue_draw();
    }

    pub fn remote_received(&self, machine: Id, selections: Vec<Selection>) {
        if let Some(editor) = self.imp().editor.borrow_mut().as_mut() {
            editor.presence().received(machine, selections);
        }
        self.queue_draw();
    }

    pub fn remote_name(&self, session: u64, machine: Id, name: String) {
        if session == self.session()
            && let Some(editor) = self.imp().editor.borrow_mut().as_mut()
        {
            editor.presence().set_name(machine, name);
            self.queue_draw();
        }
    }

    pub fn expire_remote(&self) {
        if self.imp().editor.borrow_mut().as_mut().is_some_and(|e| e.presence().expire()) {
            self.queue_draw();
        }
    }
}

/// Paint above native table children too, so a selection covering a table stays visible.
pub(super) fn draw(
    widget: &EditorWidget,
    snapshot: &gtk::Snapshot,
    lines: &[Line],
    selections: &[RemoteSelection],
) {
    for remote in selections {
        let color = crate::remote::color(remote.machine, 1.0);
        // Composite the union once: an image can overlap its visual row's selection rectangle.
        snapshot.push_opacity(0.25);
        for rect in rects(lines, &remote.selection.range()) {
            snapshot.append_color(&color, &rect);
        }
        snapshot.pop();
        let Some((x, y, height)) = caret(lines, remote.selection.head) else { continue };
        #[allow(clippy::cast_possible_truncation)]
        let bar = graphene::Rect::new(x as f32 - 1.0, y as f32, 2.0, height as f32);
        snapshot.append_color(&color, &bar);
        let label = widget.create_pango_layout(Some(&remote.name));
        let attributes = pango::AttrList::new();
        attributes.insert(pango::AttrFloat::new_scale(pango::SCALE_X_SMALL));
        label.set_attributes(Some(&attributes));
        // Long names fit the editor, and names are plain text rather than Pango markup.
        label.set_width(super::units(f64::from((widget.width() - 4).max(1))));
        label.set_ellipsize(pango::EllipsizeMode::End);
        let (width, label_height) = label.pixel_size();
        let left = x.max(0.0).min(f64::from((widget.width() - width - 4).max(0)));
        let top = (y - f64::from(label_height)).max(0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let background =
            graphene::Rect::new(left as f32, top as f32, (width + 4) as f32, label_height as f32);
        snapshot.append_color(&color, &background);
        snapshot.save();
        #[allow(clippy::cast_possible_truncation)]
        snapshot.translate(&graphene::Point::new((left + 2.0) as f32, top as f32));
        snapshot.append_layout(&label, &gdk::RGBA::WHITE);
        snapshot.restore();
    }
}

fn caret(lines: &[Line], head: usize) -> Option<(f64, f64, f64)> {
    for line in lines {
        for block in &line.blocks {
            if block.collapsed && block.range.contains(&head) {
                return Some((
                    line.left + f64::from(block.rect.x()),
                    line.y + f64::from(block.rect.y()),
                    f64::from(block.rect.height()),
                ));
            }
        }
    }
    caret_rect(lines, head)
}

/// All rectangles use widget coordinates, including revealed image source and collapsed blocks.
fn rects(lines: &[Line], selected: &std::ops::Range<usize>) -> Vec<graphene::Rect> {
    if selected.is_empty() {
        return Vec::new();
    }
    let mut rects = Vec::new();
    for line in lines {
        #[allow(clippy::cast_possible_truncation)]
        let offset = graphene::Point::new(line.left as f32, line.y as f32);
        let source_only = line.images.iter().any(|image| {
            image.source_layout.is_some()
                && image.source <= selected.start
                && selected.end <= image.end
        });
        if !source_only && selected.start <= line.end && line.start < selected.end {
            for rect in selection_rects(
                &line.layout,
                line.shown(selected.start.max(line.start)),
                line.shown(selected.end.min(line.end)),
                selected.end > line.end,
            ) {
                rects.push(translated(rect, offset.x(), offset.y()));
            }
        }
        for image in &line.images {
            let (from, to) = (selected.start.max(image.source), selected.end.min(image.end));
            if from < to {
                if image.source_layout.is_none()
                    || (selected.start <= image.source && image.end <= selected.end)
                {
                    rects.push(translated(image_rect(line, image), offset.x(), offset.y()));
                }
                if let (Some(layout), Some(source)) =
                    (&image.source_layout, source_rect(line, image))
                {
                    for rect in
                        selection_rects(layout, from - image.source, to - image.source, false)
                    {
                        rects.push(translated(
                            rect,
                            offset.x() + source.x(),
                            offset.y() + source.y(),
                        ));
                    }
                }
            }
        }
        for block in &line.blocks {
            if selected.start < block.range.end && block.range.start < selected.end {
                rects.push(translated(block.rect, offset.x(), offset.y()));
            }
        }
    }
    rects
}

fn translated(mut rect: graphene::Rect, x: f32, y: f32) -> graphene::Rect {
    rect.offset(x, y);
    rect
}

#[cfg(test)]
mod tests;
