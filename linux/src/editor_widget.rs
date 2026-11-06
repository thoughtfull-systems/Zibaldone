//! The editor of decision ZIB0017: a widget that lays out and draws a note's body with Pango, while
//! the core's `Editor` holds the text and the selection and makes every edit. Each line of the
//! text is a Pango paragraph. The widget answers what depends on how lines wrap, such as the place
//! one line down, and turns keys, clicks, and the input method's text into the core's commands.
//!
//! It shows a note while `ZIB_EDITOR=new` is set in the development build, until it does
//! everything the text view does.

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, pango};
use zib_core::editor::{Direction, Editor, Unit};

/// Space around the text, in pixels, as the text view has.
const MARGIN_SIDES: f64 = 24.0;
const MARGIN_TOP: f64 = 16.0;
const MARGIN_BOTTOM: f64 = 24.0;
/// The width of the caret, in pixels.
const CARET: f32 = 1.5;

/// Whether the development build shows notes in this editor.
pub fn enabled() -> bool {
    !cfg!(feature = "production") && std::env::var("ZIB_EDITOR").is_ok_and(|v| v == "new")
}

/// A line of the text, laid out.
pub struct Line {
    /// Where the line starts in the text.
    start: usize,
    layout: pango::Layout,
    /// Its top, from the widget's top.
    y: f64,
    height: f64,
}

mod imp {
    use std::cell::{Cell, RefCell};
    use std::ops::Range;

    use gtk::glib;
    use gtk::subclass::prelude::*;
    use zib_core::editor::{Editor, Unit};

    use super::Line;

    #[derive(Default)]
    pub struct EditorWidget {
        pub editor: RefCell<Option<Editor>>,
        pub lines: RefCell<Vec<Line>>,
        /// The width the lines last laid out at.
        pub laid_width: Cell<i32>,
        pub im: RefCell<Option<gtk::IMMulticontext>>,
        /// Called after each edit the owner makes.
        pub edited: RefCell<Option<Box<dyn Fn()>>>,
        /// The word or line a double or triple click selected, which a drag with the button still
        /// down grows from by whole words or lines.
        pub drag_origin: RefCell<Option<(Range<usize>, Unit)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditorWidget {
        const NAME: &'static str = "ZibEditorWidget";
        type Type = super::EditorWidget;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for EditorWidget {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for EditorWidget {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                return (200, 600, -1, -1);
            }
            let width = if for_size > 0 { for_size } else { 600 };
            let height = self.obj().lay_out(width);
            (height, height, -1, -1)
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            self.obj().lay_out(width);
            self.obj().update_im_cursor();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct EditorWidget(ObjectSubclass<imp::EditorWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for EditorWidget {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl EditorWidget {
    /// Shows `editor`'s text, or nothing.
    pub fn set_editor(&self, editor: Option<Editor>) {
        *self.imp().editor.borrow_mut() = editor;
        self.relayout();
    }

    /// Calls `f` after each edit the owner makes.
    pub fn connect_edited(&self, f: impl Fn() + 'static) {
        *self.imp().edited.borrow_mut() = Some(Box::new(f));
    }

    /// The text shown, if a note is open.
    pub fn text(&self) -> Option<String> {
        self.imp().editor.borrow().as_ref().map(|e| e.text().to_owned())
    }

    /// Shows edits that arrived from elsewhere.
    pub fn refresh(&self) {
        let changed = self.imp().editor.borrow_mut().as_mut().is_some_and(Editor::refresh);
        if changed {
            self.relayout();
        }
    }

    fn setup(&self) {
        self.set_focusable(true);
        self.set_cursor_from_name(Some("text"));
        self.set_overflow(gtk::Overflow::Hidden);

        let im = gtk::IMMulticontext::new();
        im.set_client_widget(Some(self));
        let w = self.downgrade();
        im.connect_commit(move |_, text| {
            if let Some(w) = w.upgrade() {
                w.edit(|e| e.insert(text));
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_im_context(Some(&im));
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            w.upgrade().map_or(glib::Propagation::Proceed, |w| w.key(key, state))
        });
        self.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let (entered, left) = (im.clone(), im.clone());
        let w = self.downgrade();
        focus.connect_enter(move |_| {
            entered.focus_in();
            if let Some(w) = w.upgrade() {
                w.queue_draw();
            }
        });
        let w = self.downgrade();
        focus.connect_leave(move |_| {
            left.focus_out();
            if let Some(w) = w.upgrade() {
                w.queue_draw();
            }
        });
        self.add_controller(focus);
        *self.imp().im.borrow_mut() = Some(im);

        let click = gtk::GestureClick::builder().button(0).build();
        let w = self.downgrade();
        click.connect_pressed(move |gesture, count, x, y| {
            if let Some(w) = w.upgrade() {
                w.pressed(gesture, count, x, y);
            }
        });
        self.add_controller(click);
        let drag = gtk::GestureDrag::new();
        let w = self.downgrade();
        drag.connect_drag_update(move |drag, dx, dy| {
            let (Some(w), Some((x, y))) = (w.upgrade(), drag.start_point()) else { return };
            w.dragged(x + dx, y + dy);
        });
        self.add_controller(drag);
    }

    /// Lays the lines out at `width` if the text or the width changed since, and returns the
    /// height they take.
    fn lay_out(&self, width: i32) -> i32 {
        let imp = self.imp();
        if imp.laid_width.get() != width || imp.lines.borrow().is_empty() {
            imp.laid_width.set(width);
            let editor = imp.editor.borrow();
            let text = editor.as_ref().map_or("", |e| e.text());
            let wrap = (f64::from(width) - 2.0 * MARGIN_SIDES).max(1.0);
            let mut lines = Vec::new();
            let mut y = MARGIN_TOP;
            let mut start = 0;
            for line in text.split('\n') {
                let layout = self.create_pango_layout(Some(line));
                layout.set_width(units(wrap));
                layout.set_wrap(pango::WrapMode::WordChar);
                let height = pixels(layout.size().1);
                lines.push(Line { start, layout, y, height });
                y += height;
                start += line.len() + 1;
            }
            *imp.lines.borrow_mut() = lines;
        }
        let lines = imp.lines.borrow();
        let bottom = lines.last().map_or(MARGIN_TOP, |line| line.y + line.height);
        #[allow(clippy::cast_possible_truncation)]
        let height = (bottom + MARGIN_BOTTOM).ceil() as i32;
        height
    }

    /// Lays the lines out again after the text changed, and redraws.
    fn relayout(&self) {
        self.imp().lines.borrow_mut().clear();
        self.queue_resize();
        self.queue_draw();
        self.update_im_cursor();
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let lines = imp.lines.borrow();
        let editor = imp.editor.borrow();
        let Some(editor) = editor.as_ref() else { return };
        let color = self.color();
        let selected = editor.selection().range();
        let highlight = adw::StyleManager::default().accent_color_rgba().with_alpha(0.3);
        #[allow(clippy::cast_possible_truncation)]
        for line in lines.iter() {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(MARGIN_SIDES as f32, line.y as f32));
            let end = line.start + line.layout.text().len();
            // The line break after a line counts as selected with it.
            if selected.start <= end && line.start < selected.end {
                let from = selected.start.saturating_sub(line.start);
                let to = (selected.end - line.start).min(end - line.start);
                let reaches_break = selected.end > end;
                for rect in selection_rects(&line.layout, from, to, reaches_break) {
                    snapshot.append_color(&highlight, &rect);
                }
            }
            snapshot.append_layout(&line.layout, &color);
            snapshot.restore();
        }
        if self.has_focus()
            && let Some((x, y, height)) = caret_rect(&lines, editor.selection().head)
        {
            #[allow(clippy::cast_possible_truncation)]
            let rect = graphene::Rect::new(x as f32, y as f32, CARET, height as f32);
            snapshot.append_color(&color, &rect);
        }
    }

    /// The place in the text nearest a point in the widget.
    fn position_at(&self, x: f64, y: f64) -> usize {
        let lines = self.imp().lines.borrow();
        let Some(line) = lines.iter().find(|line| y < line.y + line.height).or(lines.last()) else {
            return 0;
        };
        if y < line.y {
            return line.start;
        }
        let (_, index, trailing) =
            line.layout.xy_to_index(units(x - MARGIN_SIDES), units(y - line.y));
        let text = line.layout.text();
        let mut at = usize::try_from(index).unwrap_or(0);
        for _ in 0..trailing {
            at += text[at..].chars().next().map_or(0, char::len_utf8);
        }
        line.start + at
    }

    /// The place one visual line up or down from `at`, at the left edge `goal`, or `None` past
    /// the first or last line.
    fn vertical(&self, at: usize, goal: f64, down: bool) -> Option<usize> {
        let lines = self.imp().lines.borrow();
        let (_, y, height) = caret_rect(&lines, at)?;
        let target = if down { y + height + 1.0 } else { y - 1.0 };
        let last = lines.last()?;
        if target < MARGIN_TOP || target >= last.y + last.height {
            return None;
        }
        drop(lines);
        Some(self.position_at(goal, target))
    }

    /// The start or end of the visual line that `at` lies on, as the text wraps.
    fn visual_line_edge(&self, at: usize, end: bool) -> usize {
        let lines = self.imp().lines.borrow();
        let Some(line) = lines.iter().rev().find(|line| line.start <= at) else { return at };
        let local = i32::try_from(at - line.start).unwrap_or(0);
        let (number, _) = line.layout.index_to_line_x(local, false);
        let Some(row) = line.layout.line_readonly(number) else { return at };
        let start = usize::try_from(row.start_index()).unwrap_or(0);
        let mut edge = start + usize::try_from(row.length()).unwrap_or(0);
        let text = line.layout.text();
        // A row that wraps ends with the space it broke at, and the end goes before it.
        if edge < text.len() && text[..edge].ends_with(' ') {
            edge -= 1;
        }
        line.start + if end { edge } else { start }
    }

    /// Runs `f` on the editor, and lays out, redraws, and reports the edit after.
    fn edit(&self, f: impl FnOnce(&mut Editor)) {
        let Some(mut editor) = self.imp().editor.borrow_mut().take() else { return };
        let before = editor.text().to_owned();
        f(&mut editor);
        let changed = before != editor.text();
        *self.imp().editor.borrow_mut() = Some(editor);
        if changed {
            self.relayout();
            if let Some(edited) = self.imp().edited.borrow().as_ref() {
                edited();
            }
        } else {
            self.queue_draw();
            self.update_im_cursor();
        }
        self.after_selection();
    }

    /// Offers the selection as the primary selection, and scrolls the cursor into view.
    fn after_selection(&self) {
        let selected = self.imp().editor.borrow().as_ref().map(|e| e.selected_text().to_owned());
        if let Some(selected) = selected.filter(|s| !s.is_empty()) {
            self.primary_clipboard().set_text(&selected);
        }
        let w = self.clone();
        glib::idle_add_local_once(move || w.scroll_to_cursor());
    }

    fn scroll_to_cursor(&self) {
        let Some(scrolled) = self.ancestor(gtk::ScrolledWindow::static_type()) else { return };
        let Some(scrolled) = scrolled.downcast_ref::<gtk::ScrolledWindow>() else { return };
        let Some(head) = self.imp().editor.borrow().as_ref().map(|e| e.selection().head) else {
            return;
        };
        let lines = self.imp().lines.borrow();
        let Some((_, y, height)) = caret_rect(&lines, head) else { return };
        let adjustment = scrolled.vadjustment();
        if y < adjustment.value() {
            adjustment.set_value(y);
        } else if y + height > adjustment.value() + adjustment.page_size() {
            adjustment.set_value(y + height - adjustment.page_size());
        }
    }

    fn update_im_cursor(&self) {
        let head = self.imp().editor.borrow().as_ref().map(|e| e.selection().head);
        let lines = self.imp().lines.borrow();
        let im = self.imp().im.borrow();
        if let (Some(im), Some(head)) = (im.as_ref(), head)
            && let Some((x, y, height)) = caret_rect(&lines, head)
        {
            #[allow(clippy::cast_possible_truncation)]
            im.set_cursor_location(&gdk::Rectangle::new(x as i32, y as i32, 2, height as i32));
        }
    }

    // Each key reads best as one arm of one match.
    #[allow(clippy::too_many_lines)]
    fn key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let modifiers = state & gtk::accelerator_get_default_mod_mask();
        let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        if modifiers.contains(gdk::ModifierType::ALT_MASK) {
            return glib::Propagation::Proceed;
        }
        let (back, forward) = (Direction::Back, Direction::Forward);
        let by = |unit, direction| move |e: &mut Editor| e.move_by(unit, direction, shift);
        let word = if control { Unit::Word } else { Unit::Grapheme };
        match key {
            gdk::Key::Left | gdk::Key::KP_Left => self.edit(by(word, back)),
            gdk::Key::Right | gdk::Key::KP_Right => self.edit(by(word, forward)),
            gdk::Key::Home | gdk::Key::KP_Home if control => self.edit(by(Unit::Text, back)),
            gdk::Key::End | gdk::Key::KP_End if control => self.edit(by(Unit::Text, forward)),
            gdk::Key::Home | gdk::Key::KP_Home | gdk::Key::End | gdk::Key::KP_End => {
                let end = matches!(key, gdk::Key::End | gdk::Key::KP_End);
                let Some(head) = self.head() else { return glib::Propagation::Stop };
                let to = self.visual_line_edge(head, end);
                self.edit(|e| e.move_to(to, shift));
            }
            gdk::Key::Up | gdk::Key::KP_Up | gdk::Key::Down | gdk::Key::KP_Down => {
                let down = matches!(key, gdk::Key::Down | gdk::Key::KP_Down);
                self.move_vertically(down, shift, 1);
            }
            gdk::Key::Page_Up
            | gdk::Key::KP_Page_Up
            | gdk::Key::Page_Down
            | gdk::Key::KP_Page_Down => {
                let down = matches!(key, gdk::Key::Page_Down | gdk::Key::KP_Page_Down);
                self.move_vertically(down, shift, self.page_lines());
            }
            gdk::Key::BackSpace => self.edit(|e| e.delete(word, back)),
            gdk::Key::Delete | gdk::Key::KP_Delete => self.edit(|e| e.delete(word, forward)),
            gdk::Key::Return | gdk::Key::KP_Enter => self.edit(|e| e.insert("\n")),
            gdk::Key::Tab => self.edit(|e| e.insert("\t")),
            gdk::Key::a if control => self.edit(Editor::select_all),
            gdk::Key::z if control => self.edit(|e| {
                e.undo();
            }),
            gdk::Key::Z | gdk::Key::y if control => self.edit(|e| {
                e.redo();
            }),
            gdk::Key::c | gdk::Key::Insert if control => self.copy(),
            gdk::Key::x if control => {
                let mut cut = String::new();
                self.edit(|e| cut = e.cut());
                if !cut.is_empty() {
                    self.clipboard().set_text(&cut);
                }
            }
            gdk::Key::v if control => self.paste(&self.clipboard()),
            gdk::Key::Insert if shift => self.paste(&self.clipboard()),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn head(&self) -> Option<usize> {
        self.imp().editor.borrow().as_ref().map(|e| e.selection().head)
    }

    /// Moves up or down by `count` visual lines, keeping the left edge the moves started from.
    fn move_vertically(&self, down: bool, extend: bool, count: usize) {
        let Some(head) = self.head() else { return };
        let goal = self.imp().editor.borrow().as_ref().and_then(Editor::goal).or_else(|| {
            let lines = self.imp().lines.borrow();
            caret_rect(&lines, head).map(|(x, _, _)| x)
        });
        let Some(goal) = goal else { return };
        let mut at = head;
        for _ in 0..count {
            if let Some(next) = self.vertical(at, goal, down) {
                at = next;
            } else {
                at = if down { usize::MAX } else { 0 };
                break;
            }
        }
        if at == usize::MAX {
            let direction = Direction::Forward;
            self.edit(|e| e.move_by(Unit::Text, direction, extend));
        } else {
            self.edit(|e| e.move_vertically(at, goal, extend));
        }
    }

    /// How many lines of the editor's font a page holds.
    fn page_lines(&self) -> usize {
        let page = self
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_then(|s| s.downcast::<gtk::ScrolledWindow>().ok())
            .map_or(400.0, |s| s.vadjustment().page_size());
        let line = self.create_pango_layout(Some("X")).pixel_size().1.max(1);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let lines = (page / f64::from(line)).floor().max(1.0) as usize;
        lines.saturating_sub(1).max(1)
    }

    fn copy(&self) {
        let selected = self.imp().editor.borrow().as_ref().map(|e| e.selected_text().to_owned());
        if let Some(selected) = selected.filter(|s| !s.is_empty()) {
            self.clipboard().set_text(&selected);
        }
    }

    fn paste(&self, clipboard: &gdk::Clipboard) {
        let w = self.clone();
        clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |text| {
            if let Ok(Some(text)) = text {
                w.edit(|e| e.insert(&text));
            }
        });
    }

    fn pressed(&self, gesture: &gtk::GestureClick, count: i32, x: f64, y: f64) {
        self.grab_focus();
        if let Some(im) = self.imp().im.borrow().as_ref() {
            im.reset();
        }
        let at = self.position_at(x, y);
        let shift = gesture.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
        match gesture.current_button() {
            gdk::BUTTON_MIDDLE => {
                self.edit(|e| e.move_to(at, false));
                self.paste(&self.primary_clipboard());
            }
            gdk::BUTTON_PRIMARY => {
                let origin = self.imp().editor.borrow().as_ref().and_then(|e| match count {
                    2 => Some((e.word_at(at), Unit::Word)),
                    3 => Some((e.line_at(at), Unit::Line)),
                    _ => None,
                });
                self.imp().drag_origin.borrow_mut().clone_from(&origin);
                match origin {
                    Some((range, _)) => self.edit(|e| e.select(range.start, range.end)),
                    None => self.edit(|e| e.move_to(at, shift)),
                }
            }
            _ => {}
        }
    }

    /// Extends the selection to the point a drag reached: by whole words or lines from the one a
    /// double or triple click selected, or else from where the drag started.
    fn dragged(&self, x: f64, y: f64) {
        let at = self.position_at(x, y);
        let origin = self.imp().drag_origin.borrow().clone();
        self.edit(|e| match origin {
            Some((origin, unit)) => e.select_by_unit(origin, at, unit),
            None => e.select(e.selection().anchor, at),
        });
    }
}

/// The rectangles of the selected bytes `from` to `to` of a laid out line, in its coordinates,
/// reaching past the line's end when the selection goes on past its line break.
fn selection_rects(
    layout: &pango::Layout,
    from: usize,
    to: usize,
    reaches_break: bool,
) -> Vec<graphene::Rect> {
    let mut rects = Vec::new();
    let mut rows = layout.iter();
    let width = pixels(layout.width());
    loop {
        let (_, logical) = rows.line_extents();
        let final_row = rows.at_last_line();
        if let Some(row) = rows.line_readonly() {
            let start = usize::try_from(row.start_index()).unwrap_or(0);
            let end = start + usize::try_from(row.length()).unwrap_or(0);
            // A selection that goes on past a row, where it wraps or at the line's break, reaches
            // the right edge.
            let past =
                if final_row { reaches_break && from <= end } else { to > end && from < end };
            let (here_from, here_to) = (from.max(start), to.min(end));
            if here_from < here_to || past {
                let index = |at: usize| i32::try_from(at).unwrap_or(0);
                let ranges = row.x_ranges(index(here_from), index(here_to.max(here_from)));
                let edges = ranges.chunks(2).filter_map(|pair| match pair {
                    [left, right] => Some((pixels(*left), pixels(*right))),
                    _ => None,
                });
                let (left, mut right) = edges
                    .reduce(|(left, right), (l, r)| (left.min(l), right.max(r)))
                    .unwrap_or_else(|| {
                        let x = pixels(row.index_to_x(index(here_from), false));
                        (x, x)
                    });
                if past {
                    right = right.max(width);
                }
                let (top, height) = (pixels(logical.y()), pixels(logical.height()));
                #[allow(clippy::cast_possible_truncation)]
                rects.push(graphene::Rect::new(
                    left as f32,
                    top as f32,
                    (right - left).max(0.0) as f32,
                    height as f32,
                ));
            }
        }
        if !rows.next_line() {
            break;
        }
    }
    rects
}

fn units(pixels: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    let units = (pixels * f64::from(pango::SCALE)).round() as i32;
    units
}

fn pixels(units: i32) -> f64 {
    f64::from(units) / f64::from(pango::SCALE)
}

/// The caret's left edge, top, and height at `at`, in the widget's coordinates.
fn caret_rect(lines: &[Line], at: usize) -> Option<(f64, f64, f64)> {
    let line = lines.iter().rev().find(|line| line.start <= at)?;
    let local = i32::try_from(at - line.start).ok()?;
    let (strong, _) = line.layout.cursor_pos(local);
    Some((MARGIN_SIDES + pixels(strong.x()), line.y + pixels(strong.y()), pixels(strong.height())))
}
