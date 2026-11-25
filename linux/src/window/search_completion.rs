//! Field name completion at the cursor in the search box.

use std::cell::Cell;
use std::ops::Range;

use adw::prelude::*;
use gtk::{gdk, glib};

use super::Window;
use crate::app::core;

pub(super) struct SearchCompletion {
    focus: gtk::EventControllerFocus,
    generation: Cell<u64>,
    list: gtk::ListBox,
    picking: Cell<bool>,
    popover: gtk::Popover,
    scroll: gtk::ScrolledWindow,
}

impl SearchCompletion {
    pub fn new() -> Self {
        let list = gtk::ListBox::builder()
            .activate_on_single_click(true)
            .focusable(false)
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        list.add_css_class("navigation-sidebar");
        list.update_property(&[gtk::accessible::Property::Label("Search fields")]);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .focusable(false)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(240)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .build();
        let popover = gtk::Popover::builder()
            .autohide(false)
            .child(&scroll)
            .focusable(false)
            .position(gtk::PositionType::Bottom)
            .build();
        Self {
            focus: gtk::EventControllerFocus::new(),
            generation: Cell::new(0),
            list,
            picking: Cell::new(false),
            popover,
            scroll,
        }
    }

    pub fn connect(&self, window: &Window) {
        let search = &window.inner().search;
        search.add_controller(self.focus.clone());
        let w = window.clone();
        self.focus.connect_enter(move |_| w.refresh_search_completion());
        let w = window.clone();
        self.focus.connect_leave(move |_| w.inner().search_completion.dismiss());
        let w = window.clone();
        search.buffer().connect_changed(move |_| w.refresh_search_completion());
        let w = window.clone();
        search.buffer().connect_cursor_position_notify(move |_| w.refresh_search_completion());
        let w = window.clone();
        search.connect_unrealize(move |_| {
            let completion = &w.inner().search_completion;
            completion.dismiss();
            if completion.popover.parent().is_some() {
                completion.popover.unparent();
            }
        });
        let w = window.clone();
        self.list.connect_row_activated(move |_, row| {
            w.inner().search_completion.pick(&w.inner().search, row);
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = window.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let inner = w.inner();
            inner.search_completion.key(&inner.search, key)
        });
        search.add_controller(keys);
    }

    fn dismiss(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if self.popover.is_visible() {
            self.popover.popdown();
        }
    }

    fn show(&self, search: &gtk::TextView, names: &[String]) {
        if names.is_empty() {
            self.dismiss();
            return;
        }
        self.set_names(names);
        if self.popover.parent().is_none() {
            self.popover.set_parent(search);
        }
        let buffer = search.buffer();
        let rect = search.iter_location(&buffer.iter_at_offset(buffer.cursor_position()));
        let (x, y) =
            search.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
        self.popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, rect.width(), rect.height())));
        if self.popover.is_mapped() {
            self.popover.present();
        } else {
            self.popover.popup();
        }
    }

    fn set_names(&self, names: &[String]) {
        self.list.remove_all();
        for name in names {
            let label = gtk::Label::builder()
                .label(name)
                .margin_bottom(6)
                .margin_end(10)
                .margin_start(10)
                .margin_top(6)
                .xalign(0.0)
                .build();
            let row =
                gtk::ListBoxRow::builder().activatable(true).child(&label).focusable(false).build();
            self.list.append(&row);
        }
        self.list.select_row(self.list.row_at_index(0).as_ref());
        self.scroll.vadjustment().set_value(0.0);
    }

    fn pick(&self, search: &gtk::TextView, row: &gtk::ListBoxRow) {
        let Some(label) = row.child().and_downcast::<gtk::Label>() else { return };
        let buffer = search.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
        let Some(edit) = edit_at_cursor(&text, search.buffer().cursor_position()) else { return };
        let name = label.text();
        if !name.starts_with(edit.prefix) {
            return;
        }
        let start = i32::try_from(text[..edit.name.start].chars().count()).unwrap_or(i32::MAX);
        let end = i32::try_from(text[..edit.name.end].chars().count()).unwrap_or(i32::MAX);
        self.picking.set(true);
        self.dismiss();
        buffer.begin_user_action();
        let mut at = buffer.iter_at_offset(start);
        buffer.delete(&mut at, &mut buffer.iter_at_offset(end));
        buffer.insert(&mut at, &name);
        buffer.end_user_action();
        search.grab_focus();
        buffer.place_cursor(&at);
        self.picking.set(false);
    }

    fn key(&self, search: &gtk::TextView, key: gdk::Key) -> glib::Propagation {
        if !self.popover.is_mapped() {
            return glib::Propagation::Proceed;
        }
        match key {
            gdk::Key::Escape => self.dismiss(),
            gdk::Key::Down | gdk::Key::Up => {
                let step = if key == gdk::Key::Down { 1 } else { -1 };
                let at = self.list.selected_row().map_or(0, |row| row.index());
                if let Some(row) = self.list.row_at_index((at + step).max(0)) {
                    self.list.select_row(Some(&row));
                    if let Some(bounds) = row.compute_bounds(&self.list) {
                        let top = f64::from(bounds.y());
                        self.scroll.vadjustment().clamp_page(top, top + f64::from(bounds.height()));
                    }
                }
            }
            gdk::Key::Tab | gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(row) = self.list.selected_row() {
                    self.pick(search, &row);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}

impl Window {
    pub(super) fn refresh_search_completion(&self) {
        let inner = self.inner();
        let chooser = &inner.search_completion;
        let search = &inner.search;
        let buffer = search.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
        let Some(edit) = edit_at_cursor(&text, search.buffer().cursor_position()).filter(|_| {
            !chooser.picking.get()
                && chooser.focus.contains_focus()
                && search.is_mapped()
                && buffer.selection_bounds().is_none()
        }) else {
            chooser.dismiss();
            return;
        };
        let prefix = edit.prefix.to_owned();
        let generation = chooser.generation.get().wrapping_add(1);
        chooser.generation.set(generation);
        let spaces: Vec<_> = self.search_spaces().into_iter().map(|(id, _)| id).collect();
        let w = self.clone();
        let state = inner.state.clone();
        glib::spawn_future_local(async move {
            let names = core(async move { state.search_fields(&spaces) }).await;
            let inner = w.inner();
            let chooser = &inner.search_completion;
            if chooser.generation.get() == generation {
                let matches: Vec<_> =
                    names.into_iter().filter(|name| name.starts_with(&prefix)).collect();
                chooser.show(&inner.search, &matches);
            }
        });
    }
}

struct FieldEdit<'a> {
    name: Range<usize>,
    prefix: &'a str,
}

fn edit_at_cursor(text: &str, position: i32) -> Option<FieldEdit<'_>> {
    // GTK positions count characters. String ranges count bytes.
    let cursor = usize::try_from(position)
        .ok()
        .and_then(|n| text.char_indices().nth(n))
        .map_or(text.len(), |(offset, _)| offset);
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in text[..cursor].char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if !quoted && (ch.is_whitespace() || matches!(ch, '(' | ')')) {
            start = i + ch.len_utf8();
        }
    }
    let prefix = text.get(start..cursor)?.strip_prefix('@')?;
    let field_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
    if quoted || !prefix.bytes().all(field_char) {
        return None;
    }
    let suffix = text[cursor..].bytes().take_while(|&b| field_char(b)).count();
    Some(FieldEdit { name: start + 1..cursor + suffix, prefix })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completing_a_field_preserves_multiline_query_and_undo() {
        crate::test_support::init_gtk();
        let search = gtk::TextView::new();
        let buffer = search.buffer();
        buffer.set_enable_undo(true);
        let before = "apple OR (\n@i-draft=123)";
        buffer.set_text(before);
        buffer.place_cursor(&buffer.iter_at_offset(13));
        let chooser = SearchCompletion::new();
        chooser.set_names(&["id".into()]);
        chooser.pick(&search, &chooser.list.row_at_index(0).unwrap());
        let text = || buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
        assert_eq!(text(), "apple OR (\n@id=123)");
        buffer.undo();
        assert_eq!(text(), before);
    }

    #[test]
    fn completion_list_measures_its_current_field_names() {
        crate::test_support::init_gtk();
        let chooser = SearchCompletion::new();
        let size = |names: &[&str]| {
            chooser.set_names(&names.iter().map(ToString::to_string).collect::<Vec<_>>());
            let (_, width, _, _) = chooser.scroll.measure(gtk::Orientation::Horizontal, -1);
            let (_, height, _, _) = chooser.scroll.measure(gtk::Orientation::Vertical, width);
            (width, height)
        };
        let (wide, tall) = size(&["id", "longer-custom-field-name", "published-on"]);
        let (narrow, short) = size(&["id"]);
        assert!(wide > narrow && tall > short);
        let label = chooser.list.row_at_index(0).unwrap().child().unwrap();
        let (label_width, _, _, _) = label.measure(gtk::Orientation::Horizontal, -1);
        assert!(narrow >= label_width && short < 80);
    }

    #[test]
    fn completes_only_the_field_under_the_cursor() {
        assert_eq!(edit_at_cursor("@", 1).unwrap().prefix, "");
        let text = "café @note-draft=old @type=note";
        let edit = edit_at_cursor(text, 9).unwrap();
        assert_eq!(edit.prefix, "not");
        assert_eq!(&text[edit.name], "note-draft");
        assert!(edit_at_cursor("mail@example", -1).is_none());
        assert!(edit_at_cursor("@type=", -1).is_none());
        assert!(edit_at_cursor(r#"@authors:"Name @no"#, -1).is_none());
        assert_eq!(edit_at_cursor("apple OR (@pub", -1).unwrap().prefix, "pub");
    }
}
