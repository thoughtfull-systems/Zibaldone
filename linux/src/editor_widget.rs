//! The editor of decision ZIB0017: a widget that lays out and draws a note's body with Pango, while
//! the core's `Editor` holds the text and the selection and makes every edit. Each line of the
//! text is a Pango paragraph. The widget answers what depends on how lines wrap, such as the place
//! one line down, and turns keys, clicks, and the input method's text into the core's commands.
//!
//! It shows a note while `ZIB_EDITOR=new` is set in the development build, until it does
//! everything the text view does.

use std::ops::Range;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, pango};
use zib_core::editor::display::{Look, Shape, display};
use zib_core::editor::{Direction, Editor, ListKey, Unit};

/// Space around the text, in pixels, as the text view has.
const MARGIN_SIDES: f64 = 24.0;
const MARGIN_TOP: f64 = 16.0;
const MARGIN_BOTTOM: f64 = 24.0;
/// The width of the caret, in pixels.
const CARET: f32 = 1.5;
/// The space above a heading, in pixels.
const HEADING_SPACE: f64 = 6.0;
/// The colors of syntax, links, highlights, and the shade behind code, as red, green, blue, and
/// alpha, as the text view has them.
const SYNTAX: (f64, f64, f64, f64) = (0.5, 0.5, 0.5, 0.9);
const LINK: (f64, f64, f64, f64) = (0.21, 0.52, 0.89, 1.0);
const HIGHLIGHT: (f64, f64, f64, f64) = (0.96, 0.83, 0.18, 0.45);
const SHADE: (f64, f64, f64, f64) = (0.5, 0.5, 0.5, 0.15);

/// Whether the development build shows notes in this editor.
pub fn enabled() -> bool {
    !cfg!(feature = "production") && std::env::var("ZIB_EDITOR").is_ok_and(|v| v == "new")
}

/// A line of the text, laid out with its hidden syntax left out.
pub struct Line {
    /// Where the line starts and ends in the text, without its line break.
    start: usize,
    end: usize,
    /// The parts of the line that show, in order, which make up the layout's text.
    pieces: Vec<Piece>,
    layout: pango::Layout,
    /// Its top, from the widget's top.
    y: f64,
    height: f64,
    /// Whether it draws on a shade across the editor's width, as a code block's lines do.
    shaded: bool,
    /// Where its layout starts, from the widget's left edge: past the margin, and past the room
    /// that lines up a numbered list's one-digit numbers.
    left: f64,
    /// The bullets and task boxes drawn over its text.
    markers: Vec<(Range<usize>, Shape)>,
}

/// A run of the text that shows: where it starts in the text and in the layout's text, and its
/// length in bytes.
#[derive(Clone, Copy)]
struct Piece {
    source: usize,
    shown: usize,
    len: usize,
}

impl Line {
    /// Where the text's byte `at`, on this line, falls in the layout's text. Hidden syntax falls
    /// where the text after it shows.
    fn shown(&self, at: usize) -> usize {
        pieces_shown(&self.pieces, at)
    }

    /// The text's byte where the layout's character at `shown` starts, after any hidden syntax
    /// before it.
    fn source(&self, shown: usize) -> usize {
        for piece in &self.pieces {
            if shown < piece.shown + piece.len {
                return piece.source + shown.saturating_sub(piece.shown);
            }
        }
        self.pieces.last().map_or(self.start, |piece| piece.source + piece.len)
    }

    /// The text's byte where the layout's character before `shown` ends, before any hidden syntax
    /// after it.
    fn source_after(&self, shown: usize) -> usize {
        let text = self.layout.text();
        match text[..shown.min(text.len())].chars().next_back() {
            Some(c) => self.source(shown - c.len_utf8()) + c.len_utf8(),
            None => self.source(0),
        }
    }
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
        /// The selection that decides what shows while a mouse button is down, as it was when the
        /// button went down, so text doesn't move under the pointer and turn a click into a drag.
        pub held: RefCell<Option<Range<usize>>>,
        /// Whether the mouse button went down on a task box, which a drag then leaves alone.
        pub on_box: Cell<bool>,
        /// Whether the next edit leaves the view where it is, rather than scrolling to the cursor.
        pub keep_view: Cell<bool>,
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
        // Headings and other colors change between the light and dark styles.
        let w = self.downgrade();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(w) = w.upgrade() {
                w.relayout();
            }
        });

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
        // A legacy controller sees every press and release, before the gestures and without taking
        // part in them.
        let buttons = gtk::EventControllerLegacy::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = self.downgrade();
        buttons.connect_event(move |_, event| {
            let Some(w) = w.upgrade() else { return glib::Propagation::Proceed };
            match event.event_type() {
                gdk::EventType::ButtonPress => {
                    let held = w.imp().editor.borrow().as_ref().map(|e| e.selection().range());
                    w.imp().held.borrow_mut().clone_from(&held);
                }
                gdk::EventType::ButtonRelease if w.imp().held.take().is_some() => w.relayout(),
                _ => {}
            }
            glib::Propagation::Proceed
        });
        self.add_controller(buttons);
        // The pointer shows as an arrow over a task box, as over a button.
        let motion = gtk::EventControllerMotion::new();
        let w = self.downgrade();
        motion.connect_motion(move |_, x, y| {
            let Some(w) = w.upgrade() else { return };
            let name = if w.box_at(x, y).is_some() { "default" } else { "text" };
            if w.cursor().and_then(|cursor| cursor.name()).as_deref() != Some(name) {
                w.set_cursor_from_name(Some(name));
            }
        });
        self.add_controller(motion);
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
            // While a mouse button is down, what shows stays as it was when it went down.
            let selection = imp
                .held
                .borrow()
                .clone()
                .or_else(|| editor.as_ref().map(|e| e.selection().range()))
                .unwrap_or_default();
            let shown = display(text, &selection);
            let dark = adw::StyleManager::default().is_dark();
            let wrap = (f64::from(width) - 2.0 * MARGIN_SIDES).max(1.0);
            let column = monospace_width(self.upcast_ref());
            let mut lines = Vec::new();
            let mut y = MARGIN_TOP;
            let mut start = 0;
            for line in text.split('\n') {
                let end = start + line.len();
                let pieces = pieces(start, end, &shown.hidden);
                let shown_text: String =
                    pieces.iter().map(|p| &text[p.source..p.source + p.len]).collect();
                let columns = shown.pads.iter().filter(|(lines, _)| lines.contains(&start));
                #[allow(clippy::cast_precision_loss)]
                let pad = columns.map(|(_, columns)| *columns).max().unwrap_or(0) as f64 * column;
                let layout = self.create_pango_layout(Some(&shown_text));
                layout.set_width(units((wrap - pad).max(1.0)));
                layout.set_wrap(pango::WrapMode::WordChar);
                let attributes = pango::AttrList::new();
                let mut heading = false;
                for styled in &shown.styles {
                    if styled.range.start >= end.max(start + 1) || styled.range.end <= start {
                        continue;
                    }
                    heading |= matches!(styled.look, Look::Heading(_));
                    for piece in &pieces {
                        let from = styled.range.start.max(piece.source);
                        let to = styled.range.end.min(piece.source + piece.len);
                        if from >= to {
                            continue;
                        }
                        let index = |at: usize| u32::try_from(piece.shown + at - piece.source);
                        for mut attribute in attributes_of(styled.look, dark) {
                            attribute.set_start_index(index(from).unwrap_or(0));
                            attribute.set_end_index(index(to).unwrap_or(u32::MAX));
                            attributes.insert(attribute);
                        }
                    }
                }
                // The text under a bullet or a box stays, unseen, to keep its room.
                let markers: Vec<(Range<usize>, Shape)> = shown
                    .markers
                    .iter()
                    .filter(|m| m.range.start >= start && m.range.start < end.max(start + 1))
                    .map(|m| (m.range.clone(), m.shape))
                    .collect();
                for (range, _) in &markers {
                    let (from, to) =
                        (pieces_shown(&pieces, range.start), pieces_shown(&pieces, range.end));
                    // An alpha of 0 counts as no alpha at all, and draws the text.
                    for mut attribute in foreground((0.5, 0.5, 0.5, 0.004)) {
                        attribute.set_start_index(u32::try_from(from).unwrap_or(0));
                        attribute.set_end_index(u32::try_from(to).unwrap_or(u32::MAX));
                        attributes.insert(attribute);
                    }
                }
                layout.set_attributes(Some(&attributes));
                if heading {
                    y += HEADING_SPACE;
                }
                let shaded = shown.shaded.iter().any(|shaded| shaded.start == start);
                let height = pixels(layout.size().1);
                let left = MARGIN_SIDES + pad;
                lines.push(Line { start, end, pieces, layout, y, height, shaded, left, markers });
                y += height;
                start = end + 1;
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
        // At once, not at GTK's next layout, so a key that comes before then, such as a held
        // arrow key, finds the lines where they are.
        self.lay_out(self.imp().laid_width.get());
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
        let (red, green, blue, alpha) = SHADE;
        #[allow(clippy::cast_possible_truncation)]
        let shade = gdk::RGBA::new(red as f32, green as f32, blue as f32, alpha as f32);
        #[allow(clippy::cast_precision_loss)]
        let width = self.width() as f32;
        #[allow(clippy::cast_possible_truncation)]
        for line in lines.iter() {
            if line.shaded {
                let rect = graphene::Rect::new(0.0, line.y as f32, width, line.height as f32);
                snapshot.append_color(&shade, &rect);
            }
            snapshot.save();
            snapshot.translate(&graphene::Point::new(line.left as f32, line.y as f32));
            let end = line.end;
            // The line break after a line counts as selected with it.
            if selected.start <= end && line.start < selected.end {
                let from = line.shown(selected.start.max(line.start));
                let to = line.shown(selected.end.min(end));
                let reaches_break = selected.end > end;
                for rect in selection_rects(&line.layout, from, to, reaches_break) {
                    snapshot.append_color(&highlight, &rect);
                }
            }
            snapshot.append_layout(&line.layout, &color);
            for (range, shape) in &line.markers {
                draw_marker(snapshot, line, range, *shape, &color);
            }
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

    /// The start of the source of the drawn task box at a point in the widget, if there's one
    /// there. A box can reach past its source, over the character before it.
    fn box_at(&self, x: f64, y: f64) -> Option<usize> {
        let lines = self.imp().lines.borrow();
        #[allow(clippy::cast_possible_truncation)]
        let point = graphene::Point::new(x as f32, y as f32);
        lines.iter().filter(|line| line.y <= y && y < line.y + line.height).find_map(|line| {
            line.markers.iter().find_map(|(range, shape)| {
                if !matches!(shape, Shape::TaskBox(_)) {
                    return None;
                }
                #[allow(clippy::cast_possible_truncation)]
                let area = run_rect(line, range)
                    .union(&marker_rect(line, range))
                    .offset_r(line.left as f32, line.y as f32);
                area.contains_point(&point).then_some(range.start)
            })
        })
    }

    /// The place in the text nearest a point in the widget.
    fn position_at(&self, x: f64, y: f64) -> usize {
        let lines = self.imp().lines.borrow();
        // A point in the space above a heading belongs to the line before it, so a move up from
        // the heading's first row reaches that line. Above the first line, it's the first line.
        let Some(line) = lines.iter().rev().find(|line| line.y <= y).or(lines.first()) else {
            return 0;
        };
        // Pango puts a point past a layout's top or bottom on its first or last row.
        let (_, index, trailing) = line.layout.xy_to_index(units(x - line.left), units(y - line.y));
        let index = usize::try_from(index).unwrap_or(0);
        // A click on a character's right half lands after it, before any hidden syntax that
        // follows, and on its left half before it, after any hidden syntax before it.
        if trailing > 0 {
            let text = line.layout.text();
            let after = index + text[index..].chars().next().map_or(0, char::len_utf8);
            line.source_after(after)
        } else {
            line.source(index)
        }
    }

    /// The place one visual line up or down from `at`, at the left edge `goal`, or `None` past
    /// the first or last line.
    fn vertical(&self, at: usize, goal: f64, down: bool) -> Option<usize> {
        let lines = self.imp().lines.borrow();
        // Past the edge of the row the cursor is on, which can be taller than the cursor, as on a
        // list item, whose monospace prefix is shorter than its text.
        let (y, height) = row_extents(&lines, at)?;
        let mut target = if down { y + height + 1.0 } else { y - 1.0 };
        let last = lines.last()?;
        if target < MARGIN_TOP || target >= last.y + last.height {
            return None;
        }
        // A move that leaves its line goes past the space above a heading, to the next line's
        // first row or the last row of the line before.
        let index = lines.iter().rposition(|line| line.start <= at)?;
        let current = &lines[index];
        if down && target >= current.y + current.height {
            if let Some(next) = lines.get(index + 1) {
                target = target.max(next.y);
            }
        } else if !down
            && target < current.y
            && let Some(before) = index.checked_sub(1).and_then(|i| lines.get(i))
        {
            target = target.min(before.y + before.height - 1.0);
        }
        drop(lines);
        Some(self.position_at(goal, target))
    }

    /// The start or end of the visual line that `at` lies on, as the text wraps.
    fn visual_line_edge(&self, at: usize, end: bool) -> usize {
        let lines = self.imp().lines.borrow();
        let Some(line) = lines.iter().rev().find(|line| line.start <= at) else { return at };
        let local = i32::try_from(line.shown(at)).unwrap_or(0);
        let (number, _) = line.layout.index_to_line_x(local, false);
        let Some(row) = line.layout.line_readonly(number) else { return at };
        let start = usize::try_from(row.start_index()).unwrap_or(0);
        let mut edge = start + usize::try_from(row.length()).unwrap_or(0);
        let text = line.layout.text();
        // A row that wraps ends with the space it broke at, and the end goes before it.
        if edge < text.len() && text[..edge].ends_with(' ') {
            edge -= 1;
        }
        if end && edge > start { line.source_after(edge) } else { line.source(start) }
    }

    /// Runs `f` on the editor, and lays out, redraws, and reports the edit after.
    fn edit(&self, f: impl FnOnce(&mut Editor)) {
        let Some(mut editor) = self.imp().editor.borrow_mut().take() else { return };
        let before = editor.text().to_owned();
        f(&mut editor);
        let changed = before != editor.text();
        *self.imp().editor.borrow_mut() = Some(editor);
        // What hides depends on the selection, so a move can change the layout too.
        self.relayout();
        if changed && let Some(edited) = self.imp().edited.borrow().as_ref() {
            edited();
        }
        self.after_selection();
    }

    /// Offers the selection as the primary selection, and scrolls the cursor into view.
    fn after_selection(&self) {
        let selected = self.imp().editor.borrow().as_ref().map(|e| e.selected_text().to_owned());
        if let Some(selected) = selected.filter(|s| !s.is_empty()) {
            self.primary_clipboard().set_text(&selected);
        }
        if self.imp().keep_view.take() {
            return;
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
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let key = if shift { ListKey::ShiftEnter } else { ListKey::Enter };
                self.edit(|e| {
                    if !e.list_key(key) {
                        e.insert("\n");
                    }
                });
            }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                let back = shift || key == gdk::Key::ISO_Left_Tab;
                let key = if back { ListKey::ShiftTab } else { ListKey::Tab };
                // In a list item, Tab and Shift+Tab move the item, and never type a tab.
                self.edit(|e| {
                    if !e.list_key(key) && !back {
                        e.insert("\t");
                    }
                });
            }
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
        self.imp().on_box.set(false);
        // Each click on a drawn task box toggles it, a double click's second too, and leaves the
        // cursor where it was.
        let plain = gesture.current_event_state() & gtk::accelerator_get_default_mod_mask();
        if gesture.current_button() == gdk::BUTTON_PRIMARY
            && plain.is_empty()
            && let Some(at) = self.box_at(x, y)
        {
            self.imp().on_box.set(true);
            // The cursor stays where it was, perhaps off screen, and the view stays on the box.
            self.imp().keep_view.set(true);
            self.edit(|e| {
                e.toggle_task(at);
            });
            return;
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
                if let Some((range, _)) = origin {
                    self.edit(|e| e.select(range.start, range.end));
                } else {
                    let showing = self.imp().held.borrow().clone();
                    self.edit(|e| {
                        e.move_to(at, shift);
                        e.take_in_syntax(&showing.unwrap_or_else(|| e.selection().range()));
                    });
                }
            }
            _ => {}
        }
    }

    /// Extends the selection to the point a drag reached: by whole words or lines from the one a
    /// double or triple click selected, or else from where the drag started.
    fn dragged(&self, x: f64, y: f64) {
        if self.imp().on_box.get() {
            return;
        }
        let at = self.position_at(x, y);
        let origin = self.imp().drag_origin.borrow().clone();
        let showing = self.imp().held.borrow().clone();
        self.edit(|e| {
            match origin {
                Some((origin, unit)) => e.select_by_unit(origin, at, unit),
                None => e.select(e.selection().anchor, at),
            }
            // Up to syntax that hides on screen, the selection takes it in too.
            e.take_in_syntax(&showing.unwrap_or_else(|| e.selection().range()));
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
    let local = i32::try_from(line.shown(at)).ok()?;
    let (strong, _) = line.layout.cursor_pos(local);
    Some((line.left + pixels(strong.x()), line.y + pixels(strong.y()), pixels(strong.height())))
}

/// The Pango attributes that give text `look`, in the light or `dark` style.
fn attributes_of(look: Look, dark: bool) -> Vec<pango::Attribute> {
    let monospace = || pango::AttrString::new_family("monospace").upcast();
    match look {
        Look::Heading(level) => {
            let scale = match level {
                1 => 1.6,
                2 => 1.4,
                3 => 1.2,
                _ => 1.0,
            };
            let color = gdk::RGBA::parse(crate::editor::heading_color(level, dark))
                .map_or((0.0, 0.0, 0.0, 1.0), |c| {
                    (f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), 1.0)
                });
            let mut attributes = vec![
                pango::AttrInt::new_weight(pango::Weight::Bold).upcast(),
                pango::AttrFloat::new_scale(scale).upcast(),
            ];
            attributes.extend(foreground(color));
            attributes
        }
        Look::Emphasis => vec![pango::AttrInt::new_style(pango::Style::Italic).upcast()],
        Look::Strong => vec![pango::AttrInt::new_weight(pango::Weight::Bold).upcast()],
        Look::Strikethrough => vec![pango::AttrInt::new_strikethrough(true).upcast()],
        Look::Underline => vec![pango::AttrInt::new_underline(pango::Underline::Single).upcast()],
        Look::Highlight => background(HIGHLIGHT),
        Look::Code => {
            let mut attributes = vec![monospace()];
            attributes.extend(background(SHADE));
            attributes
        }
        Look::Monospace => vec![monospace()],
        Look::Link => {
            let mut attributes =
                vec![pango::AttrInt::new_underline(pango::Underline::Single).upcast()];
            attributes.extend(foreground(LINK));
            attributes
        }
        Look::Quote => {
            let mut attributes = vec![pango::AttrInt::new_style(pango::Style::Italic).upcast()];
            attributes.extend(foreground(SYNTAX));
            attributes
        }
        Look::Completed | Look::Syntax => foreground(SYNTAX),
    }
}

/// A Pango color and its alpha, from red, green, blue, and alpha between 0 and 1.
fn color(
    (red, green, blue, alpha): (f64, f64, f64, f64),
    color: fn(u16, u16, u16) -> pango::AttrColor,
    opacity: fn(u16) -> pango::AttrInt,
) -> Vec<pango::Attribute> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
    vec![
        color(channel(red), channel(green), channel(blue)).upcast(),
        opacity(channel(alpha)).upcast(),
    ]
}

fn foreground(rgba: (f64, f64, f64, f64)) -> Vec<pango::Attribute> {
    color(rgba, pango::AttrColor::new_foreground, pango::AttrInt::new_foreground_alpha)
}

fn background(rgba: (f64, f64, f64, f64)) -> Vec<pango::Attribute> {
    color(rgba, pango::AttrColor::new_background, pango::AttrInt::new_background_alpha)
}

/// The parts of the line from `start` to `end` that show, around the `hidden` syntax.
fn pieces(start: usize, end: usize, hidden: &[Range<usize>]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let (mut at, mut shown) = (start, 0);
    for gap in hidden.iter().filter(|gap| gap.start < end && gap.end > start) {
        if gap.start > at {
            pieces.push(Piece { source: at, shown, len: gap.start - at });
            shown += gap.start - at;
        }
        at = at.max(gap.end);
    }
    if at < end || pieces.is_empty() {
        pieces.push(Piece { source: at.min(end), shown, len: end.saturating_sub(at) });
    }
    pieces
}

/// The top and height of the row, as the text wraps, that `at` lies on, in the widget's
/// coordinates.
fn row_extents(lines: &[Line], at: usize) -> Option<(f64, f64)> {
    let line = lines.iter().rev().find(|line| line.start <= at)?;
    let local = i32::try_from(line.shown(at)).ok()?;
    let (number, _) = line.layout.index_to_line_x(local, false);
    let mut rows = line.layout.iter();
    for _ in 0..number {
        if !rows.next_line() {
            break;
        }
    }
    let (_, logical) = rows.line_extents();
    Some((line.y + pixels(logical.y()), pixels(logical.height())))
}

/// Where the text's byte `at` falls in the text that `pieces` show. Hidden syntax falls where the
/// text after it shows.
fn pieces_shown(pieces: &[Piece], at: usize) -> usize {
    for piece in pieces {
        if at <= piece.source + piece.len {
            return piece.shown + at.saturating_sub(piece.source).min(piece.len);
        }
    }
    pieces.last().map_or(0, |piece| piece.shown + piece.len)
}

/// The width of a character of the monospace font that list prefixes use, in pixels.
fn monospace_width(widget: &gtk::Widget) -> f64 {
    let layout = widget.create_pango_layout(Some("0"));
    let mut font = widget.pango_context().font_description().unwrap_or_default();
    font.set_family("monospace");
    layout.set_font_description(Some(&font));
    f64::from(layout.pixel_size().0)
}

/// The cells of the text from `range` on `line`, in the line's coordinates, on its first row.
fn run_rect(line: &Line, range: &Range<usize>) -> graphene::Rect {
    let cell = |at: usize| line.layout.index_to_pos(i32::try_from(line.shown(at)).unwrap_or(0));
    let first = cell(range.start);
    let last = cell(range.end.max(range.start + 1) - 1);
    let rect = |x: i32, y: i32, width: i32, height: i32| {
        #[allow(clippy::cast_possible_truncation)]
        graphene::Rect::new(
            pixels(x) as f32,
            pixels(y) as f32,
            pixels(width) as f32,
            pixels(height) as f32,
        )
    };
    rect(first.x(), first.y(), last.x() + last.width() - first.x(), first.height())
}

/// Where a shape over `range` goes, in the line's coordinates: a square a little smaller than a
/// row, in the middle of a box's own source, or on a list item's marker, where its siblings'
/// bullets are.
fn marker_rect(line: &Line, range: &Range<usize>) -> graphene::Rect {
    let run = run_rect(line, range);
    let side = (run.height() * 0.8).round();
    let own_source = line.layout.text()[line.shown(range.start)..].starts_with('[');
    let center = if own_source {
        run.x() + run.width() / 2.0
    } else {
        let first = run_rect(line, &(range.start..range.start + 1));
        first.x() + first.width() / 2.0
    };
    graphene::Rect::new(
        (center - side / 2.0).round(),
        (run.y() + (run.height() - side) / 2.0).round(),
        side,
        side,
    )
}

/// The width of the border of a task box whose side is `side`.
fn box_border(side: f32) -> f32 {
    (side * 0.1).max(1.5)
}

/// Draws a bullet or a task box over `range` on `line`, in the line's coordinates.
fn draw_marker(
    snapshot: &gtk::Snapshot,
    line: &Line,
    range: &Range<usize>,
    shape: Shape,
    color: &gdk::RGBA,
) {
    let rect = marker_rect(line, range);
    let side = rect.width();
    match shape {
        Shape::TaskBox(checked) => {
            let rounded = gtk::gsk::RoundedRect::from_rect(rect, side * 0.2);
            if !checked {
                let outline = color.with_alpha(0.55);
                snapshot.append_border(&rounded, &[box_border(side); 4], &[outline; 4]);
                return;
            }
            let accent = adw::StyleManager::default().accent_color_rgba();
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&accent, &rect);
            snapshot.pop();
            let (x, y) = (rect.x(), rect.y());
            let check = gtk::gsk::PathBuilder::new();
            check.move_to(x + side * 0.26, y + side * 0.52);
            check.line_to(x + side * 0.43, y + side * 0.69);
            check.line_to(x + side * 0.75, y + side * 0.33);
            let stroke = gtk::gsk::Stroke::new(side * 0.13);
            stroke.set_line_cap(gtk::gsk::LineCap::Round);
            stroke.set_line_join(gtk::gsk::LineJoin::Round);
            snapshot.append_stroke(&check.to_path(), &stroke, &gdk::RGBA::WHITE);
        }
        Shape::Bullet(marker) => {
            // As wide as the inside of a task box, so bullets and boxes look alike in size.
            let (glyph, weight) = match marker {
                '*' => ("•", pango::Weight::Normal),
                '+' => ("＋", pango::Weight::Bold),
                _ => ("⁃", pango::Weight::Normal),
            };
            let layout = pango::Layout::new(&line.layout.context());
            layout.set_text(glyph);
            let attributes = pango::AttrList::new();
            attributes.insert(pango::AttrInt::new_weight(weight));
            layout.set_attributes(Some(&attributes));
            let (ink, _) = layout.extents();
            if ink.width() <= 0 || ink.height() <= 0 {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let (ink_x, ink_y, ink_width, ink_height) = (
                pixels(ink.x()) as f32,
                pixels(ink.y()) as f32,
                pixels(ink.width()) as f32,
                pixels(ink.height()) as f32,
            );
            let scale = (side - 2.0 * box_border(side)) / ink_width;
            snapshot.save();
            snapshot.translate(&rect.center());
            snapshot.scale(scale, scale);
            snapshot.translate(&graphene::Point::new(
                -(ink_x + ink_width / 2.0),
                -(ink_y + ink_height / 2.0),
            ));
            snapshot.append_layout(&layout, color);
            snapshot.restore();
        }
    }
}

#[cfg(test)]
mod tests {
    use zib_core::editor::{Body, Editor};

    use super::*;

    /// A widget showing `text`, laid out 600 pixels wide, with the cursor at `|`. `None` without a
    /// display to lay out on, such as in the Nix build, where these tests have nothing to check.
    fn widget(text: &str) -> Option<EditorWidget> {
        adw::init().ok()?;
        let at = text.find('|')?;
        let mut editor = Editor::new(Body::detached(&text.replace('|', "")));
        editor.select(at, at);
        let widget = EditorWidget::default();
        widget.set_editor(Some(editor));
        widget.lay_out(600);
        Some(widget)
    }

    fn head(widget: &EditorWidget) -> usize {
        widget.head().unwrap_or_default()
    }

    #[test]
    fn moves_up_and_down_between_list_items() {
        for text in [
            "- [x] done\n  - [ ] this that\n|  - [ ] test\n",
            concat!(
                "    here's the title card ![[01m6pxatk1eear5tn3pseyqr9s]]\n",
                "  - [ ] this ![[01m6pxatkxed0t26ngds48kzrv]] that\n",
                "|  - [ ] test\n",
            ),
        ] {
            let Some(widget) = widget(text) else { return };
            let start = text.find('|').unwrap();
            widget.move_vertically(false, false, 1);
            let up = head(&widget);
            assert!(up < start, "up from {start} stayed at {up} in {text:?}");
            widget.move_vertically(true, false, 1);
            assert_eq!(head(&widget), start, "{text:?}");
        }
    }

    #[test]
    fn moves_up_and_down_around_headings() {
        let text = "this is me editing the note\n|## a heading\n";
        let Some(widget) = widget(text) else { return };
        widget.move_vertically(false, false, 1);
        assert_eq!(head(&widget), 0);
        // The heading's `## ` hides while the cursor is above it, and a move down lands after it,
        // as a click there does.
        widget.move_vertically(true, false, 1);
        assert_eq!(head(&widget), text.find("a heading").unwrap() - 1);
    }
}
