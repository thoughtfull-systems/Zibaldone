//! Existing table grids, kept as native children and fitted to the editor's available width.

use std::rc::Rc;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{graphene, gsk};
use zib_core::body::ListTable;

use super::{EditorWidget, Line};

pub(super) struct Data {
    pub refs: crate::tables::Refs,
    pub table: ListTable,
}

pub(super) struct View {
    data: Rc<Data>,
    used: bool,
    widget: gtk::Widget,
}

pub(super) struct Layout {
    pub height: i32,
    pub scale: f32,
    pub widget: gtk::Widget,
    pub width: i32,
}

pub(super) fn begin(editor: &EditorWidget) {
    for view in editor.imp().table_views.borrow_mut().iter_mut() {
        view.used = false;
    }
}

pub(super) fn finish(editor: &EditorWidget) {
    editor.imp().table_views.borrow_mut().retain(|view| {
        if !view.used {
            view.widget.unparent();
        }
        view.used
    });
}

/// Each occurrence needs its own widget, even when two tables share the same rendered content.
/// The same data keeps its grid through typing and resizing, until a replacement is ready.
pub(super) fn layout(editor: &EditorWidget, data: &Rc<Data>, available: f64) -> Layout {
    let mut views = editor.imp().table_views.borrow_mut();
    let index = views.iter().position(|v| !v.used && Rc::ptr_eq(&v.data, data));
    let widget = if let Some(index) = index {
        views[index].used = true;
        views[index].widget.clone()
    } else {
        let widget = crate::tables::widget(&data.table, &data.refs);
        widget.set_parent(editor);
        views.push(View { data: data.clone(), used: true, widget: widget.clone() });
        widget
    };
    let (minimum, natural, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
    #[allow(clippy::cast_possible_truncation)]
    let width = natural.min(available.floor() as i32).max(minimum).max(1);
    let (_, height, _, _) = widget.measure(gtk::Orientation::Vertical, width);
    #[allow(clippy::cast_possible_truncation)]
    let scale = (available / f64::from(width)).min(1.0) as f32;
    Layout { height: height.max(1), scale, widget, width }
}

pub(super) fn allocate(lines: &[Line]) {
    for line in lines {
        for block in &line.blocks {
            if let super::drawings::Content::Table(layout) = &block.content {
                #[allow(clippy::cast_possible_truncation)]
                let point = graphene::Point::new(
                    line.left as f32 + block.rect.x(),
                    line.y as f32 + block.rect.y(),
                );
                let transform =
                    gsk::Transform::new().translate(&point).scale(layout.scale, layout.scale);
                layout.widget.allocate(layout.width, layout.height, -1, Some(transform));
            }
        }
    }
}

pub(super) fn snapshot(editor: &EditorWidget, snapshot: &gtk::Snapshot, line: &Line) {
    for block in &line.blocks {
        if let super::drawings::Content::Table(layout) = &block.content {
            editor.snapshot_child(&layout.widget, snapshot);
        }
    }
}

#[cfg(test)]
mod tests;
