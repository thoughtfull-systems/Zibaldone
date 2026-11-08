//! The editor of decision ZIB0017: a widget that lays out and draws a note's body with Pango, while
//! the core's `Editor` holds the text and the selection and makes every edit. Each line of the
//! text is a Pango paragraph. The widget answers what depends on how lines wrap, such as the place
//! one line down, and turns keys, clicks, and the input method's text into the core's commands.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

mod drawings;
mod presence;
mod tables;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, pango};
use zib_core::Id;
use zib_core::editing::{Drawing, LinkTarget, link_at};
use zib_core::editor::display::{Display, ImageSize, Look, Rendered, Shape};
use zib_core::editor::{Direction, EditTarget, Editor, ListKey, Unit};

/// Space around the text, in pixels, as the text view has.
const MARGIN_SIDES: f64 = 24.0;
const MARGIN_TOP: f64 = 16.0;
const MARGIN_BOTTOM: f64 = 24.0;
/// The width of the caret, in pixels.
const CARET: f32 = 1.5;
/// The space above a heading, in pixels.
const HEADING_SPACE: f64 = 6.0;
/// Room between a revealed image's bottom and its source text.
const IMAGE_SOURCE_GAP: f64 = 4.0;
/// Room for each block quote level, and the bar's offset and width inside it.
const QUOTE_BAR_OFFSET: f64 = 4.0;
const QUOTE_BAR_WIDTH: f32 = 3.0;
const QUOTE_INDENT: f64 = 18.0;
/// The colors of syntax, links, highlights, and the shade behind code, as red, green, blue, and
/// alpha, as the text view has them.
const SYNTAX: (f64, f64, f64, f64) = (0.5, 0.5, 0.5, 0.9);
const LINK: (f64, f64, f64, f64) = (0.21, 0.52, 0.89, 1.0);
const HIGHLIGHT: (f64, f64, f64, f64) = (0.96, 0.83, 0.18, 0.45);
const SHADE: (f64, f64, f64, f64) = (0.5, 0.5, 0.5, 0.15);

/// A line of the text, with hidden syntax left out and link titles drawn in its place.
pub struct Line {
    blocks: Vec<drawings::Block>,
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
    images: Vec<InlineImage>,
    backgrounds: Vec<Background>,
    quote: Option<QuoteRoom>,
}

/// Room for quote bars: before the paragraph or inside it, past a list item's prefix.
struct QuoteRoom {
    above: f64,
    continued: usize,
    depth: usize,
    inline: bool,
    left: f64,
}

/// A background behind glyphs, which doesn't grow to the height of a neighboring image.
struct Background {
    range: Range<usize>,
    color: (f64, f64, f64, f64),
}

/// An image's source, layout index, texture, and displayed size.
struct InlineImage {
    descent: f64,
    source: usize,
    index: usize,
    texture: gdk::Texture,
    width: f64,
    height: f64,
    end: usize,
    source_layout: Option<pango::Layout>,
}

/// What a run draws in place of its source.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Content {
    Source,
    Label,
    Image(Id),
    Drawing(usize),
}

/// Context shared by the source paragraphs in a single layout pass.
struct Layout<'a> {
    column: f64,
    display: &'a Display,
    text: &'a str,
    wrap: f64,
}

/// Where a paste started, including which open editor it belongs to.
pub struct PasteTarget {
    session: u64,
    selection: EditTarget,
}

/// A run's source and layout offsets and lengths, in bytes. A title maps every glyph back to
/// the whole link that it stands for.
#[derive(Clone, Copy)]
struct Piece {
    source: usize,
    shown: usize,
    len: usize,
    source_len: usize,
    content: Content,
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
                return piece.source
                    + if piece.content == Content::Source {
                        shown.saturating_sub(piece.shown)
                    } else {
                        0
                    };
            }
        }
        self.pieces.last().map_or(self.start, |piece| piece.source + piece.source_len)
    }

    /// The text's byte where the layout's character before `shown` ends, before any hidden syntax
    /// after it.
    fn source_after(&self, shown: usize) -> usize {
        for piece in self.pieces.iter().rev() {
            if shown > piece.shown {
                return piece.source
                    + if piece.content == Content::Source {
                        shown.saturating_sub(piece.shown).min(piece.len)
                    } else {
                        piece.source_len
                    };
            }
        }
        self.source(0)
    }
}

mod imp {
    use std::cell::{Cell, RefCell};
    use std::ops::Range;

    use adw::prelude::*;
    use gtk::glib;
    use gtk::subclass::prelude::*;
    use zib_core::editor::{Editor, Unit};

    use super::{Drawing, HashMap, HashSet, Line, LinkTarget, PasteTarget, drawings, tables};

    type Callback<T> = RefCell<Option<Box<dyn Fn(T)>>>;

    #[derive(Default)]
    pub struct EditorWidget {
        pub read_only: Cell<bool>,
        pub asked_drawings: RefCell<HashSet<Drawing>>,
        pub(super) drawings: RefCell<drawings::Resources>,
        pub(super) table_views: RefCell<Vec<tables::View>>,
        pub table_revision: Cell<u64>,
        pub editor: RefCell<Option<Editor>>,
        pub images: RefCell<HashMap<zib_core::Id, gtk::gdk::Texture>>,
        pub asked_images: RefCell<HashSet<zib_core::Id>>,
        pub image_generation: Cell<u64>,
        pub session: Cell<u64>,
        pub paste_image: Callback<(gtk::gdk::Clipboard, PasteTarget)>,
        pub lines: RefCell<Vec<Line>>,
        /// The note's viewport controls its trailing scroll space, including after a resize.
        pub scroll_adjustment: RefCell<Option<(gtk::Adjustment, glib::SignalHandlerId)>>,
        /// The page height applied after the viewport finishes allocating.
        pub viewport_height: Cell<f64>,
        /// The width the lines last laid out at.
        pub laid_width: Cell<i32>,
        pub im: RefCell<Option<gtk::IMMulticontext>>,
        /// Called after each edit the owner makes.
        pub edited: RefCell<Option<Box<dyn Fn()>>>,
        pub selection_changed: RefCell<Option<Box<dyn Fn()>>>,
        pub open_link: Callback<LinkTarget>,
        pub pick_link: Callback<Range<usize>>,
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
        fn dispose(&self) {
            if let Some((adjustment, handler)) = self.scroll_adjustment.borrow_mut().take() {
                adjustment.disconnect(handler);
            }
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }

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
            tables::allocate(&self.lines.borrow());
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
    /// A conversation message still allows selection, copying, and opening links.
    pub fn set_read_only(&self, read_only: bool) {
        self.imp().read_only.set(read_only);
    }

    /// Shows `editor`'s text, or nothing.
    pub fn set_editor(&self, editor: Option<Editor>) {
        let imp = self.imp();
        imp.session.set(imp.session.get().wrapping_add(1));
        imp.image_generation.set(imp.image_generation.get().wrapping_add(1));
        imp.asked_images.borrow_mut().clear();
        imp.images.borrow_mut().clear();
        imp.asked_drawings.borrow_mut().clear();
        imp.drawings.borrow_mut().clear();
        while let Some(child) = self.first_child() {
            child.unparent();
        }
        imp.table_views.borrow_mut().clear();
        *imp.editor.borrow_mut() = editor;
        self.rebuild_layout();
    }

    /// Calls `f` after each edit the owner makes.
    pub fn connect_edited(&self, f: impl Fn() + 'static) {
        *self.imp().edited.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        *self.imp().selection_changed.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_open_link(&self, f: impl Fn(LinkTarget) + 'static) {
        *self.imp().open_link.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_pick_link(&self, f: impl Fn(Range<usize>) + 'static) {
        *self.imp().pick_link.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_titles(&self, titles: HashMap<Id, String>) {
        if let Some(editor) = self.imp().editor.borrow_mut().as_mut() {
            editor.set_titles(titles);
        }
        self.relayout();
    }

    pub fn insert_link(&self, range: Range<usize>, expected: &str, target: Id) {
        self.edit(|e| {
            e.insert_link(range, expected, target);
        });
    }

    pub fn connect_paste_image(&self, f: impl Fn(gdk::Clipboard, PasteTarget) + 'static) {
        *self.imp().paste_image.borrow_mut() = Some(Box::new(move |(clipboard, target)| {
            f(clipboard, target);
        }));
    }

    /// Images not yet requested for this note, or every embed after a space changes.
    pub fn image_requests(&self, reload: bool) -> (u64, Vec<Id>) {
        let imp = self.imp();
        if reload {
            imp.image_generation.set(imp.image_generation.get().wrapping_add(1));
            imp.asked_images.borrow_mut().clear();
        }
        let targets = imp.editor.borrow().as_ref().map(Editor::image_targets).unwrap_or_default();
        let mut asked = imp.asked_images.borrow_mut();
        (imp.image_generation.get(), targets.into_iter().filter(|id| asked.insert(*id)).collect())
    }

    pub fn drawing_requests(&self) -> (u64, Vec<Drawing>) {
        let imp = self.imp();
        let targets = imp.editor.borrow().as_ref().map(Editor::drawing_targets).unwrap_or_default();
        let mut asked = imp.asked_drawings.borrow_mut();
        (imp.session.get(), targets.into_iter().filter(|d| asked.insert(d.clone())).collect())
    }

    pub fn reload_tables(&self) {
        self.imp().table_revision.set(self.imp().table_revision.get().wrapping_add(1));
        self.imp().asked_drawings.borrow_mut().retain(|d| !matches!(d, Drawing::Table(_)));
    }

    pub fn table_revision(&self) -> u64 {
        self.imp().table_revision.get()
    }

    pub fn set_table(
        &self,
        session: u64,
        revision: u64,
        drawing: Drawing,
        refs: crate::tables::Refs,
    ) {
        if session != self.imp().session.get() || revision != self.table_revision() {
            return;
        }
        let Drawing::Table(table) = &drawing else { return };
        let data = std::rc::Rc::new(tables::Data { table: table.clone(), refs });
        self.imp().drawings.borrow_mut().insert(drawing.clone(), drawings::Resource::Table(data));
        if let Some(editor) = self.imp().editor.borrow_mut().as_mut() {
            editor.set_drawing(drawing, Rendered::Table);
        }
        self.relayout();
    }

    /// GTK answers geometry; the core maps the resulting cell path to the current body.
    fn table_at(&self, x: f64, y: f64) -> Option<(Range<usize>, crate::tables::CellPath)> {
        #[allow(clippy::cast_possible_truncation)]
        let point = graphene::Point::new(x as f32, y as f32);
        for line in self.imp().lines.borrow().iter() {
            for block in &line.blocks {
                if let drawings::Content::Table(layout) = &block.content
                    && let Some(cell) =
                        crate::tables::cell_at(&layout.widget, self.upcast_ref(), &point)
                {
                    return Some((block.range.clone(), cell));
                }
            }
        }
        None
    }

    fn select_cell_at(&self, x: f64, y: f64) -> bool {
        let Some((block, (row, column, nested))) = self.table_at(x, y) else { return false };
        let mut selected = false;
        self.edit(|editor| selected = editor.select_table_cell(block, row, column, &nested));
        selected
    }

    /// Render results are keyed by source, and a result from another note session is ignored.
    pub fn set_drawing(&self, session: u64, drawing: Drawing, result: drawings::ImageResult) {
        let imp = self.imp();
        if session != imp.session.get() {
            return;
        }
        let result = result.and_then(|(texture, size)| {
            if size.width == 0 || size.height == 0 {
                Err("The drawing has no size".to_owned())
            } else {
                Ok((texture, size))
            }
        });
        let rendered = result.as_ref().map_or_else(
            |error| Rendered::Error(error.clone()),
            |(_, size)| Rendered::Image(*size),
        );
        let resource = match result {
            Ok((texture, size)) => drawings::Resource::Image(texture, size),
            Err(error) => drawings::Resource::Error(error),
        };
        imp.drawings.borrow_mut().insert(drawing.clone(), resource);
        if let Some(editor) = imp.editor.borrow_mut().as_mut() {
            editor.set_drawing(drawing, rendered);
        }
        self.relayout();
    }

    /// Shows an asynchronously loaded image only in the editor that requested it.
    pub fn set_image(&self, generation: u64, target: Id, texture: Option<gdk::Texture>) {
        let imp = self.imp();
        if generation != imp.image_generation.get() {
            return;
        }
        let mut images = imp.images.borrow_mut();
        if let Some(texture) = texture {
            images.insert(target, texture);
        } else {
            images.remove(&target);
        }
        if let Some(editor) = imp.editor.borrow_mut().as_mut() {
            editor.set_images(
                images
                    .iter()
                    .map(|(id, texture)| {
                        (
                            *id,
                            ImageSize {
                                width: u32::try_from(texture.width()).unwrap_or(0),
                                height: u32::try_from(texture.height()).unwrap_or(0),
                            },
                        )
                    })
                    .collect(),
            );
        }
        drop(images);
        self.relayout();
    }

    pub fn insert_image(&self, target: &PasteTarget, image: Id) {
        if target.session == self.imp().session.get() {
            self.edit(|e| {
                e.insert_image(&target.selection, image);
            });
        }
    }

    /// The caret rectangle that anchors the link picker.
    pub fn cursor_rectangle(&self) -> gdk::Rectangle {
        let rect = self.head().and_then(|at| caret_rect(&self.imp().lines.borrow(), at));
        let (x, y, height) = rect.unwrap_or((MARGIN_SIDES, MARGIN_TOP, 16.0));
        #[allow(clippy::cast_possible_truncation)]
        gdk::Rectangle::new(x as i32, y as i32, 1, height as i32)
    }

    /// Modifier-assisted input commits literal text, bypassing structural typing shortcuts.
    fn commit_text(&self, text: &str, state: gdk::ModifierType) {
        if self.imp().read_only.get() {
            return;
        }
        let modifiers = state & gtk::accelerator_get_default_mod_mask();
        if (modifiers - gdk::ModifierType::SHIFT_MASK).is_empty() {
            self.type_text(text);
        } else {
            self.edit(|e| e.insert(text));
        }
    }

    fn type_text(&self, text: &str) {
        let mut request = None;
        self.edit(|e| request = e.type_text(text));
        if let Some(range) = request
            && let Some(pick) = self.imp().pick_link.borrow().as_ref()
        {
            pick(range);
        }
    }

    /// The text shown, if a note is open.
    pub fn text(&self) -> Option<String> {
        self.imp().editor.borrow().as_ref().map(|e| e.text().to_owned())
    }

    /// Shows edits that arrived from elsewhere.
    pub fn refresh(&self) {
        let scrolled = self.own_scroller();
        let anchor = self.head().and_then(|head| {
            let (_, y, height) = caret_rect(&self.imp().lines.borrow(), head)?;
            let adjustment = scrolled.as_ref()?.vadjustment();
            (y >= adjustment.value() && y + height <= adjustment.value() + adjustment.page_size())
                .then_some(y - adjustment.value())
        });
        let changed = self.imp().editor.borrow_mut().as_mut().is_some_and(Editor::refresh);
        if changed {
            self.imp().held.borrow_mut().take();
            self.imp().drag_origin.borrow_mut().take();
            self.rebuild_layout();
            let caret = self.head().and_then(|head| caret_rect(&self.imp().lines.borrow(), head));
            if let (Some(scrolled), Some(screen_y), Some((_, y, _))) = (scrolled, anchor, caret) {
                self.restore_view(&scrolled, y - screen_y);
            }
            self.offer_selection();
        }
        // An earlier cursor message may refer to operations this refresh has just imported.
        self.queue_draw();
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
        let keys = gtk::EventControllerKey::new();
        keys.set_im_context(Some(&im));
        let w = self.downgrade();
        let controller = keys.downgrade();
        im.connect_commit(move |_, text| {
            if let Some(w) = w.upgrade() {
                let state = controller
                    .upgrade()
                    .map_or_else(gdk::ModifierType::empty, |keys| keys.current_event_state());
                w.commit_text(text, state);
            }
        });
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
            self.prune_drawings();
            imp.laid_width.set(width);
            let editor = imp.editor.borrow();
            let text = editor.as_ref().map_or("", |e| e.text());
            // A drag keeps what showed at its press, so changing syntax doesn't move its target.
            let selection = imp
                .held
                .borrow()
                .clone()
                .or_else(|| editor.as_ref().map(|e| e.selection().range()))
                .unwrap_or_default();
            let shown = editor.as_ref().map(|e| e.display_for(&selection)).unwrap_or_default();
            let context = Layout {
                column: monospace_width(self.upcast_ref()),
                display: &shown,
                text,
                wrap: (f64::from(width) - 2.0 * MARGIN_SIDES).max(1.0),
            };
            tables::begin(self);
            let mut lines = Vec::new();
            let mut y = MARGIN_TOP;
            for paragraph in shown.paragraphs(text) {
                let mut line = self.layout_line(&context, paragraph.range, y, lines.last());
                if let Some(index) = paragraph.drawing {
                    let drawing = &shown.drawings[index];
                    let origin = drawing_origin(&line, drawing.indent);
                    drawings::collapse(&mut line, drawing);
                    if let Some(resource) = imp
                        .drawings
                        .borrow()
                        .get(drawing.preview.as_ref().unwrap_or(&drawing.source))
                    {
                        drawings::append(self, &mut line, drawing, resource, origin, width);
                    }
                } else {
                    for drawing in shown.drawings.iter().filter(|d| d.block && !d.collapsed()) {
                        if line.start <= drawing.range.end && drawing.range.end <= line.end {
                            let first = if line.start <= drawing.indent {
                                &line
                            } else {
                                line_at(&lines, drawing.indent).unwrap_or(&line)
                            };
                            let origin = drawing_origin(first, drawing.indent);
                            if let Some(resource) = imp
                                .drawings
                                .borrow()
                                .get(drawing.preview.as_ref().unwrap_or(&drawing.source))
                            {
                                drawings::append(self, &mut line, drawing, resource, origin, width);
                            }
                        }
                    }
                }
                y = line.y + line.height;
                lines.push(line);
            }
            *imp.lines.borrow_mut() = lines;
            tables::finish(self);
            tables::allocate(&imp.lines.borrow());
        }
        let lines = imp.lines.borrow();
        let bottom = lines.last().map_or(MARGIN_TOP, |line| line.y + line.height);
        #[allow(clippy::cast_possible_truncation)]
        let height = (bottom + MARGIN_BOTTOM + self.end_space()).ceil() as i32;
        height
    }

    /// Keep current sources and the previous results that still appear while replacements render.
    /// Evicted requests can run again after undo, and late results don't accumulate in the cache.
    fn prune_drawings(&self) {
        let imp = self.imp();
        let retained = imp.editor.borrow().as_ref().map(Editor::drawing_keys).unwrap_or_default();
        imp.drawings.borrow_mut().retain(|drawing, _| retained.contains(drawing));
        imp.asked_drawings.borrow_mut().retain(|drawing| retained.contains(drawing));
    }

    fn layout_line(
        &self,
        context: &Layout<'_>,
        range: Range<usize>,
        mut y: f64,
        previous: Option<&Line>,
    ) -> Line {
        let (start, end) = (range.start, range.end);
        let Layout { column, display: shown, text, wrap } = *context;
        let imp = self.imp();
        let dark = adw::StyleManager::default().is_dark();
        let (pieces, shown_text) = pieces(text, start, end, shown);
        let columns = shown.pads.iter().filter(|(lines, _)| lines.contains(&start));
        #[allow(clippy::cast_precision_loss)]
        let pad = columns.map(|(_, columns)| *columns).max().unwrap_or(0) as f64 * column;
        let quoted = shown.quotes.iter().find(|q| q.line.start <= start && start <= q.line.end);
        #[allow(clippy::cast_precision_loss)]
        let carried = quoted
            .filter(|q| q.start < start)
            .map_or(0.0, |q| text[q.line.start..q.start].chars().count() as f64 * column);
        #[allow(clippy::cast_precision_loss)]
        let inset = carried
            + quoted
                .filter(|q| q.start == q.line.start || q.start < start)
                .map_or(0.0, |q| q.depth as f64 * QUOTE_INDENT);
        let layout = self.create_pango_layout(Some(&shown_text));
        layout.set_width(units((wrap - pad - inset).max(1.0)));
        layout.set_wrap(pango::WrapMode::WordChar);
        let attributes = pango::AttrList::new();
        let mut backgrounds = Vec::new();
        let heading = style_source(&attributes, &pieces, shown, start..end, dark, &mut backgrounds);
        style_labels(&attributes, &pieces, shown, dark, &mut backgrounds);
        // The text under a bullet or a box stays, unseen, to keep its room.
        let markers: Vec<(Range<usize>, Shape)> = shown
            .markers
            .iter()
            .filter(|m| m.range.start >= start && m.range.start < end.max(start + 1))
            .map(|m| (m.range.clone(), m.shape))
            .collect();
        style_markers(&attributes, &pieces, &markers);
        let quote = quoted.map(|q| {
            let inline = q.start > start;
            let offset =
                if inline { reserve_quote_room(&layout, &attributes, &pieces, q) } else { carried };
            QuoteRoom {
                above: if heading { HEADING_SPACE } else { 0.0 },
                continued: quote_continuation(previous, MARGIN_SIDES + pad + offset, y),
                depth: q.depth,
                inline,
                left: MARGIN_SIDES + pad + offset,
            }
        });
        let images = shape_images(
            &layout,
            &attributes,
            &pieces,
            &ImageContext {
                available: wrap - pad - inset,
                display: shown,
                math_scale: math_scale(self),
                renders: &imp.drawings.borrow(),
                text,
                textures: &imp.images.borrow(),
            },
        );
        layout.set_attributes(Some(&attributes));
        if heading {
            y += HEADING_SPACE;
        }
        let shaded = shown.shaded.iter().any(|shaded| shaded.start == start);
        let height = pixels(layout.size().1);
        let left = MARGIN_SIDES + pad + inset;
        Line {
            blocks: Vec::new(),
            start,
            end,
            pieces,
            layout,
            y,
            height,
            shaded,
            left,
            markers,
            images,
            backgrounds,
            quote,
        }
    }

    /// Keeps the text's baseline at the selection in place while an image rises above its source.
    /// Changes elsewhere above the selection keep the view focused on the same text too.
    fn relayout(&self) {
        let scrolled = self.own_scroller();
        let anchor = self.head().and_then(|head| {
            let lines = self.imp().lines.borrow();
            let line = line_at(&lines, head)?;
            // Following words may wrap while source is open. Keep the image's baseline as the
            // anchor when leaving it, rather than anchoring those words on a different row.
            let at = if line.images.iter().any(|image| image.source <= head && head <= image.end) {
                head
            } else {
                line.images
                    .iter()
                    .find(|image| image.source_layout.is_some())
                    .map_or(head, |image| image.source)
            };
            let y = paragraph_baseline(&lines, at)?;
            Some((at, y - scrolled.as_ref()?.vadjustment().value()))
        });
        self.rebuild_layout();
        let caret = anchor.and_then(|(head, screen_y)| {
            paragraph_baseline(&self.imp().lines.borrow(), head).map(|y| (y, screen_y))
        });
        if let (Some(scrolled), Some((y, screen_y))) = (scrolled, caret) {
            self.restore_view(&scrolled, y - screen_y);
        }
    }

    /// A conversation contains several editors in one scroll area. Only its direct content may
    /// change the adjustment's extent or add trailing scroll space.
    fn own_scroller(&self) -> Option<gtk::ScrolledWindow> {
        let (scrolled, content) = self.scroll_parent()?;
        (content == self.clone().upcast::<gtk::Widget>()).then_some(scrolled)
    }

    fn scroll_parent(&self) -> Option<(gtk::ScrolledWindow, gtk::Widget)> {
        let scrolled = self
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())?;
        let child = scrolled.child()?;
        let content = if let Some(viewport) = child.downcast_ref::<gtk::Viewport>() {
            viewport.child()?
        } else {
            child
        };
        Some((scrolled, content))
    }

    /// An 80% viewport buffer below a note keeps its last lines visible. Editors inside a chat's
    /// shared history don't add space between messages.
    fn end_space(&self) -> f64 {
        let adjustment = self.own_scroller().map(|scrolled| scrolled.vadjustment());
        let mut watched = self.imp().scroll_adjustment.borrow_mut();
        if watched.as_ref().map(|(adjustment, _)| adjustment) != adjustment.as_ref() {
            if let Some((adjustment, handler)) = watched.take() {
                adjustment.disconnect(handler);
            }
            self.imp().viewport_height.set(0.0);
            if let Some(adjustment) = &adjustment {
                let weak = self.downgrade();
                let handler = adjustment.connect_page_size_notify(move |adjustment| {
                    if let Some(widget) = weak.upgrade() {
                        widget.update_end_space_later(adjustment);
                    }
                });
                *watched = Some((adjustment.clone(), handler));
                self.update_end_space_later(adjustment);
            }
        }
        self.imp().viewport_height.get() * 0.8
    }

    /// GTK can change the page size or first attach the adjustment while allocating the viewport.
    /// Keep the old buffer until that allocation finishes, then invalidate the cached request.
    fn update_end_space_later(&self, adjustment: &gtk::Adjustment) {
        let adjustment = adjustment.clone();
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(widget) = weak.upgrade()
                && widget
                    .own_scroller()
                    .is_some_and(|scrolled| scrolled.vadjustment() == adjustment)
            {
                let page = adjustment.page_size();
                let previous = widget.imp().viewport_height.replace(page);
                if (previous - page).abs() > f64::EPSILON {
                    widget.queue_resize();
                }
            }
        });
    }

    /// Keeps the editing position stable as far as the document's natural scroll range allows.
    /// Revealing or hiding source must not add blank space above the note or grow its end buffer.
    fn restore_view(&self, scrolled: &gtk::ScrolledWindow, requested: f64) {
        let adjustment = scrolled.vadjustment();
        let height = f64::from(self.lay_out(self.imp().laid_width.get()));
        let upper = height.max(adjustment.page_size());
        adjustment.set_upper(upper);
        adjustment.set_value(requested.clamp(0.0, upper - adjustment.page_size()));
    }

    /// Lays the lines out again after the text changed, and redraws.
    fn rebuild_layout(&self) {
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
                let height = pixels(line.layout.size().1);
                let rect = graphene::Rect::new(0.0, line.y as f32, width, height as f32);
                snapshot.append_color(&shade, &rect);
            }
            for rect in quote_rects(line) {
                snapshot.append_color(&color.with_alpha(0.3), &rect);
            }
            snapshot.save();
            snapshot.translate(&graphene::Point::new(line.left as f32, line.y as f32));
            let end = line.end;
            // The line break after a line counts as selected with it.
            let source_only = line.images.iter().any(|image| {
                image.source_layout.is_some()
                    && image.source <= selected.start
                    && selected.end <= image.end
            });
            if !source_only && selected.start <= end && line.start < selected.end {
                let from = line.shown(selected.start.max(line.start));
                let to = line.shown(selected.end.min(end));
                let reaches_break = selected.end > end;
                for rect in selection_rects(&line.layout, from, to, reaches_break) {
                    snapshot.append_color(&highlight, &rect);
                }
            }
            for background in &line.backgrounds {
                let (r, g, b, a) = background.color;
                let color = gdk::RGBA::new(r as f32, g as f32, b as f32, a as f32);
                for rect in run_rects(&line.layout, &background.range) {
                    snapshot.append_color(&color, &rect);
                }
            }
            snapshot.append_layout(&line.layout, &color);
            for image in &line.images {
                let rect = image_rect(line, image);
                snapshot.append_texture(&image.texture, &rect);
                if selected.start <= image.source && image.end <= selected.end {
                    snapshot.append_color(&highlight, &rect);
                }
                draw_image_source(snapshot, line, image, &selected, &highlight, &color);
            }
            for block in &line.blocks {
                block.paint(snapshot, &selected, &highlight);
            }
            for (range, shape) in &line.markers {
                draw_marker(snapshot, line, range, *shape, &color);
            }
            snapshot.restore();
            tables::snapshot(self, snapshot, line);
        }
        presence::draw(self, snapshot, &lines, &editor.remote_selections());
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

    /// Images map every point of their box to the start of their embed's source.
    fn image_at(&self, x: f64, y: f64) -> Option<usize> {
        #[allow(clippy::cast_possible_truncation)]
        let point = graphene::Point::new(x as f32, y as f32);
        self.imp().lines.borrow().iter().find_map(|line| {
            line.images.iter().find_map(|image| {
                #[allow(clippy::cast_possible_truncation)]
                let rect = image_rect(line, image).offset_r(line.left as f32, line.y as f32);
                rect.contains_point(&point).then_some(image.source)
            })
        })
    }

    /// The editable source within an image atom, as both its byte offset and its embed's start.
    fn source_position(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        #[allow(clippy::cast_possible_truncation)]
        let point = graphene::Point::new(x as f32, y as f32);
        self.imp().lines.borrow().iter().find_map(|line| {
            line.images.iter().find_map(|image| {
                let layout = image.source_layout.as_ref()?;
                let rect = source_rect(line, image)?;
                #[allow(clippy::cast_possible_truncation)]
                let area = rect.offset_r(line.left as f32, line.y as f32);
                if !area.contains_point(&point) {
                    return None;
                }
                let (_, at) =
                    layout_position(layout, x - f64::from(area.x()), y - f64::from(area.y()));
                Some((image.source + at, image.source))
            })
        })
    }

    /// The link under an actual glyph, including any part of a drawn title.
    fn link_at(&self, x: f64, y: f64) -> Option<LinkTarget> {
        let target =
            self.source_position(x, y).map(|(_, start)| start).or_else(|| self.image_at(x, y));
        if let Some(at) = target {
            let editor = self.imp().editor.borrow();
            return link_at(editor.as_ref()?.text(), at);
        }
        let lines = self.imp().lines.borrow();
        let line = lines.iter().find(|l| l.y <= y && y < l.y + l.height)?;
        let (inside, index, _) = line.layout.xy_to_index(units(x - line.left), units(y - line.y));
        if !inside {
            return None;
        }
        let index = usize::try_from(index).ok()?;
        if line.images.iter().any(|image| image.index <= index && index < image.index + 3) {
            return None;
        }
        let at = line.source(index);
        let editor = self.imp().editor.borrow();
        link_at(editor.as_ref()?.text(), at)
    }

    /// The place in the text nearest a point in the widget.
    fn position_at(&self, x: f64, y: f64) -> usize {
        for line in self.imp().lines.borrow().iter() {
            if let Some(block) = line.blocks.iter().find(|block| {
                let top = line.y + f64::from(block.rect.y()) - 8.0;
                let bottom = top + f64::from(block.rect.height()) + 16.0;
                line.left <= x && top <= y && y < bottom
            }) {
                return block.cursor;
            }
        }
        if let Some((at, _)) = self.source_position(x, y) {
            return at;
        }
        if let Some(at) = self.image_at(x, y) {
            return at;
        }
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
        let (current_y, height) = row_extents(&lines, at)?;
        let mut candidates = Vec::new();
        for line in lines.iter() {
            let mut layouts = vec![(&line.layout, line.y)];
            for image in &line.images {
                if let (Some(layout), Some(rect)) = (&image.source_layout, source_rect(line, image))
                {
                    layouts.push((layout, line.y + f64::from(rect.y())));
                }
            }
            for (layout, y) in layouts {
                let mut rows = layout.iter();
                loop {
                    let (_, logical) = rows.line_extents();
                    let top = y + pixels(logical.y());
                    let bottom = top + pixels(logical.height());
                    if (down && top >= current_y + height - 0.01)
                        || (!down && bottom <= current_y + 0.01)
                    {
                        candidates.push((top, f64::midpoint(top, bottom)));
                    }
                    if !rows.next_line() {
                        break;
                    }
                }
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !down {
            candidates.reverse();
        }
        for (top, center) in candidates {
            let next = self.position_at(goal, center);
            // An image points into its source row below. Skip that non-editable row when moving
            // out of the source, rather than landing in the source again.
            if row_extents(&lines, next).is_some_and(|(y, _)| (y - top).abs() < 0.01) {
                return Some(next);
            }
        }
        None
    }

    /// The start or end of the visual line that `at` lies on, as the text wraps.
    fn visual_line_edge(&self, at: usize, end: bool) -> usize {
        let lines = self.imp().lines.borrow();
        let Some(line) = line_at(&lines, at) else { return at };
        if let Some(image) = source_image(line, at) {
            let layout = image.source_layout.as_ref().expect("revealed source");
            return image.source + visual_edge(layout, at - image.source, end);
        }
        let edge = visual_edge(&line.layout, line.shown(at), end);
        if end { line.source_after(edge) } else { line.source(edge) }
    }

    /// Runs `f` on the editor, and lays out, redraws, and reports the edit after.
    fn edit(&self, f: impl FnOnce(&mut Editor)) {
        // An import can reach the shared document before its UI notification is delivered.
        self.refresh();
        let Some(mut editor) = self.imp().editor.borrow_mut().take() else { return };
        let before = editor.text().to_owned();
        let head = editor.selection().head;
        let images = images_through(&self.imp().lines.borrow(), head);
        let viewport = self.own_scroller();
        let anchor = viewport.as_ref().and_then(|scrolled| {
            let (_, y, height) = caret_rect(&self.imp().lines.borrow(), head)?;
            let adjustment = scrolled.vadjustment();
            (y >= adjustment.value() && y + height <= adjustment.value() + adjustment.page_size())
                .then_some(y - adjustment.value())
        });
        f(&mut editor);
        let changed = before != editor.text();
        *self.imp().editor.borrow_mut() = Some(editor);
        // What hides depends on the selection, so a move can change the layout too.
        if changed {
            self.rebuild_layout();
            let after = self.head().map(|head| {
                let lines = self.imp().lines.borrow();
                (images_through(&lines, head), caret_rect(&lines, head))
            });
            if let (Some(scrolled), Some(screen_y)) = (viewport, anchor)
                && !self.imp().keep_view.get()
                && let Some((after_images, Some((_, y, _)))) = after
                && images != after_images
            {
                self.restore_view(&scrolled, y - screen_y);
            }
        } else {
            self.relayout();
        }
        if changed && let Some(edited) = self.imp().edited.borrow().as_ref() {
            edited();
        }
        self.after_selection();
    }

    /// Offers the selection as the primary selection, and scrolls the cursor into view.
    fn after_selection(&self) {
        self.offer_selection();
        if self.imp().keep_view.take() {
            return;
        }
        let w = self.clone();
        glib::idle_add_local_once(move || w.scroll_to_cursor());
    }

    fn offer_selection(&self) {
        self.selection_changed();
        let selected = self.imp().editor.borrow().as_ref().map(|e| e.selected_text().to_owned());
        if let Some(selected) = selected.filter(|s| !s.is_empty()) {
            self.primary_clipboard().set_text(&selected);
        }
    }

    fn selection_changed(&self) {
        if let Some(changed) = self.imp().selection_changed.borrow().as_ref() {
            changed();
        }
    }

    fn scroll_to_cursor(&self) {
        let Some((scrolled, content)) = self.scroll_parent() else { return };
        let Some(head) = self.imp().editor.borrow().as_ref().map(|e| e.selection().head) else {
            return;
        };
        let lines = self.imp().lines.borrow();
        let Some((_, y, height)) = caret_rect(&lines, head) else { return };
        // Message coordinates are local to the message; the adjustment belongs to the whole chat.
        #[allow(clippy::cast_possible_truncation)]
        let Some(position) = self.compute_point(&content, &graphene::Point::new(0.0, y as f32))
        else {
            return;
        };
        let y = f64::from(position.y());
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
        if self.imp().read_only.get()
            && !matches!(
                key,
                gdk::Key::Left
                    | gdk::Key::KP_Left
                    | gdk::Key::Right
                    | gdk::Key::KP_Right
                    | gdk::Key::Home
                    | gdk::Key::KP_Home
                    | gdk::Key::End
                    | gdk::Key::KP_End
                    | gdk::Key::Up
                    | gdk::Key::KP_Up
                    | gdk::Key::Down
                    | gdk::Key::KP_Down
                    | gdk::Key::Page_Up
                    | gdk::Key::KP_Page_Up
                    | gdk::Key::Page_Down
                    | gdk::Key::KP_Page_Down
            )
            && !(control && matches!(key, gdk::Key::a | gdk::Key::c | gdk::Key::Insert))
        {
            return glib::Propagation::Stop;
        }
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
                if control {
                    let key = if shift { ListKey::MoveUp } else { ListKey::MoveDown };
                    self.edit(|e| {
                        e.list_key(key);
                    });
                } else {
                    self.edit(|e| e.newline(shift));
                }
            }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                let back = shift || key == gdk::Key::ISO_Left_Tab;
                let key = if back { ListKey::ShiftTab } else { ListKey::Tab };
                // In a list item, Tab and Shift+Tab move the item, and never type a tab.
                self.edit(|e| {
                    if !e.table_key(back) && !e.list_key(key) && !back {
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
        if self.imp().read_only.get() {
            return;
        }
        let target = self.imp().editor.borrow().as_ref().map(|editor| PasteTarget {
            session: self.imp().session.get(),
            selection: editor.edit_target(),
        });
        let Some(target) = target else { return };
        if image_only(clipboard.formats())
            && let Some(paste) = self.imp().paste_image.borrow().as_ref()
        {
            paste((clipboard.clone(), target));
            return;
        }
        let w = self.clone();
        clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |text| {
            if let Ok(Some(text)) = text {
                w.paste_text(&target, &text);
            }
        });
    }

    fn paste_text(&self, target: &PasteTarget, text: &str) {
        if target.session == self.session() {
            self.edit(|e| {
                e.paste(&target.selection, text);
            });
        }
    }

    fn pressed(&self, gesture: &gtk::GestureClick, count: i32, x: f64, y: f64) {
        self.grab_focus();
        if let Some(im) = self.imp().im.borrow().as_ref() {
            im.reset();
        }
        self.imp().on_box.set(false);
        if gesture.current_button() == gdk::BUTTON_PRIMARY
            && gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK)
            && let Some(target) = self.link_at(x, y)
        {
            self.imp().on_box.set(true);
            if let Some(open) = self.imp().open_link.borrow().as_ref() {
                open(target);
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            return;
        }
        // Each click on a drawn task box toggles it, a double click's second too, and leaves the
        // cursor where it was.
        let plain = gesture.current_event_state() & gtk::accelerator_get_default_mod_mask();
        if gesture.current_button() == gdk::BUTTON_PRIMARY
            && plain.is_empty()
            && !self.imp().read_only.get()
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
        if gesture.current_button() == gdk::BUTTON_PRIMARY
            && plain.is_empty()
            && self.select_cell_at(x, y)
        {
            self.imp().on_box.set(true);
            gesture.set_state(gtk::EventSequenceState::Claimed);
            return;
        }
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
/// reaching past the line's end when the selection goes on past its line break. Each rectangle
/// uses the full row height so neighboring font runs and selected lines have no gaps.
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

/// Reserves quote indentation after a list prefix, without moving that prefix. The preceding
/// space keeps its font height, so the room doesn't change baselines or collide with images.
fn reserve_quote_room(
    layout: &pango::Layout,
    attributes: &pango::AttrList,
    pieces: &[Piece],
    quote: &zib_core::editing::QuotedLine,
) -> f64 {
    let from = pieces_shown(pieces, quote.start.saturating_sub(1));
    let to = pieces_shown(pieces, quote.start);
    layout.set_attributes(attributes.copy().as_ref());
    let cell = layout.index_to_pos(i32::try_from(from).unwrap_or(0));
    #[allow(clippy::cast_precision_loss)]
    let room = units(quote.depth as f64 * QUOTE_INDENT);
    let rect = pango::Rectangle::new(
        0,
        cell.y() - baseline_at(layout, from),
        cell.width() + room,
        cell.height(),
    );
    let mut shape = pango::AttrShape::new(&rect, &rect);
    shape.set_start_index(u32::try_from(from).unwrap_or(0));
    shape.set_end_index(u32::try_from(to).unwrap_or(u32::MAX));
    attributes.change(shape);
    pixels(cell.x() + cell.width())
}

/// The depth that continues directly from the previous line at the same horizontal position.
/// A bar inside a wrapped list paragraph stops at its first row, so the next line starts anew.
fn quote_continuation(previous: Option<&Line>, left: f64, top: f64) -> usize {
    let Some(previous) = previous else { return 0 };
    let Some(quote) = &previous.quote else { return 0 };
    let height = if quote.inline && previous.layout.line_count() > 1 {
        pixels(previous.layout.iter().line_extents().1.height())
    } else {
        previous.height
    };
    if (quote.left - left).abs() < 0.01 && (previous.y + height - top).abs() < 0.01 {
        quote.depth
    } else {
        0
    }
}

/// Quote bars join through blank lines and heading spacing. Each new bar starts slightly below
/// the row's top, clear of the previous line's descenders. Continuing bars keep their full height.
/// Room inside a paragraph belongs to its first row; quote margins follow every wrapped row.
fn quote_rects(line: &Line) -> Vec<graphene::Rect> {
    let Some(quote) = &line.quote else { return Vec::new() };
    let height = if quote.inline && line.layout.line_count() > 1 {
        pixels(line.layout.iter().line_extents().1.height())
    } else {
        line.height
    };
    let metrics = line.layout.iter().run_readonly().map_or_else(
        || line.layout.context().metrics(None, None),
        |run| run.item().analysis().font().metrics(None),
    );
    // Font metrics keep this gap proportional to text even when a tall image enlarges the row.
    let inset = (pixels(metrics.ascent() + metrics.descent()) / 5.0).ceil().min(height);
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    (0..quote.depth)
        .map(|level| {
            let inset = if level < quote.continued { 0.0 } else { inset };
            graphene::Rect::new(
                (quote.left + level as f64 * QUOTE_INDENT + QUOTE_BAR_OFFSET) as f32,
                (line.y - quote.above + inset) as f32,
                QUOTE_BAR_WIDTH,
                (height + quote.above - inset) as f32,
            )
        })
        .collect()
}

/// The text under a bullet or task box keeps its room but stays unseen. An alpha of zero would
/// make Pango draw it at full opacity, so use the smallest visible alpha instead.
fn style_markers(
    attributes: &pango::AttrList,
    pieces: &[Piece],
    markers: &[(Range<usize>, Shape)],
) {
    for (range, _) in markers {
        let (from, to) = (pieces_shown(pieces, range.start), pieces_shown(pieces, range.end));
        for mut attribute in foreground((0.5, 0.5, 0.5, 0.004)) {
            attribute.set_start_index(u32::try_from(from).unwrap_or(0));
            attribute.set_end_index(u32::try_from(to).unwrap_or(u32::MAX));
            attributes.insert(attribute);
        }
    }
}

/// The core shares the same math scale between the editor and printing.
pub(crate) fn math_scale(widget: &impl IsA<gtk::Widget>) -> f64 {
    let font = widget.pango_context().font_description().unwrap_or_default();
    let size = f64::from(font.size()) / f64::from(pango::SCALE);
    let pixels = if font.is_size_absolute() { size } else { size * 96.0 / 72.0 };
    zib_core::render::math_scale(pixels)
}

/// The color of a heading of `level`: a hue from the GNOME palette for each of the first four
/// levels, purple, blue, green, and orange, in a darker shade on a light background and a lighter
/// one on a dark background. Deeper levels share the fourth's.
fn heading_color(level: u8, dark: bool) -> &'static str {
    let (light, dark_shade) = match level {
        1 => ("#813d9c", "#dc8add"),
        2 => ("#1a5fb4", "#99c1f1"),
        3 => ("#26a269", "#8ff0a4"),
        _ => ("#c64600", "#ffbe6f"),
    };
    if dark { dark_shade } else { light }
}

fn units(pixels: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    let units = (pixels * f64::from(pango::SCALE)).round() as i32;
    units
}

fn pixels(units: i32) -> f64 {
    f64::from(units) / f64::from(pango::SCALE)
}

/// The source paragraph that holds an offset.
fn line_at(lines: &[Line], at: usize) -> Option<&Line> {
    lines.iter().rev().find(|line| line.start <= at)
}

/// Images determine the height of earlier paragraphs and of the entire paragraph at the caret.
/// Source offsets can change during ordinary typing without changing which pictures are there.
fn images_through(lines: &[Line], at: usize) -> Vec<glib::Object> {
    lines
        .iter()
        .take_while(|line| line.start <= at)
        .flat_map(|line| {
            line.images
                .iter()
                .map(|image| image.texture.clone().upcast::<glib::Object>())
                .chain(line.blocks.iter().filter_map(drawings::Block::visual))
        })
        .collect()
}

fn drawing_origin(line: &Line, at: usize) -> f64 {
    let (_, x) = line.layout.index_to_line_x(i32::try_from(line.shown(at)).unwrap_or(0), false);
    line.left + pixels(x)
}

fn source_image(line: &Line, at: usize) -> Option<&InlineImage> {
    line.images
        .iter()
        .find(|image| image.source_layout.is_some() && image.source <= at && at <= image.end)
}

/// The caret's left edge, top, and height at `at`, in the widget's coordinates.
fn caret_rect(lines: &[Line], at: usize) -> Option<(f64, f64, f64)> {
    let line = line_at(lines, at)?;
    let (layout, at, left, top) = if let Some(image) = source_image(line, at) {
        let rect = source_rect(line, image)?;
        (
            image.source_layout.as_ref()?,
            at - image.source,
            line.left + f64::from(rect.x()),
            line.y + f64::from(rect.y()),
        )
    } else {
        (&line.layout, line.shown(at), line.left, line.y)
    };
    let (strong, _) = layout.cursor_pos(i32::try_from(at).ok()?);
    Some((left + pixels(strong.x()), top + pixels(strong.y()), pixels(strong.height())))
}

/// Anchor the text baseline, rather than the top of an image or a source font's caret rectangle.
fn paragraph_baseline(lines: &[Line], at: usize) -> Option<f64> {
    let line = line_at(lines, at)?;
    let local = line
        .images
        .iter()
        .find(|image| image.source <= at && at <= image.end)
        .map_or_else(|| line.shown(at), |image| image.index);
    Some(line.y + pixels(baseline_at(&line.layout, local)))
}

fn baseline_at(layout: &pango::Layout, at: usize) -> i32 {
    let (row, _) = layout.index_to_line_x(i32::try_from(at).unwrap_or(0), false);
    let mut rows = layout.iter();
    for _ in 0..row {
        if !rows.next_line() {
            break;
        }
    }
    rows.baseline()
}

fn visual_edge(layout: &pango::Layout, at: usize, end: bool) -> usize {
    let (number, _) = layout.index_to_line_x(i32::try_from(at).unwrap_or(0), false);
    let Some(row) = layout.line_readonly(number) else { return at };
    let start = usize::try_from(row.start_index()).unwrap_or(0);
    let mut edge = start + usize::try_from(row.length()).unwrap_or(0);
    let text = layout.text();
    if edge < text.len() && text[..edge].ends_with(' ') {
        edge -= 1;
    }
    if end { edge } else { start }
}

fn layout_position(layout: &pango::Layout, x: f64, y: f64) -> (bool, usize) {
    let (inside, index, trailing) = layout.xy_to_index(units(x), units(y));
    let index = usize::try_from(index).unwrap_or(0);
    let text = layout.text();
    let after = text[index..]
        .char_indices()
        .nth(usize::try_from(trailing).unwrap_or(0))
        .map_or(text.len(), |(at, _)| index + at);
    (inside, after)
}

fn add_background(backgrounds: &mut Vec<Background>, look: Look, range: Range<usize>) {
    let color = match look {
        Look::Code => SHADE,
        Look::Highlight => HIGHLIGHT,
        _ => return,
    };
    backgrounds.push(Background { range, color });
}

/// The rectangles behind each run's selected bytes, at the run's height. Pango's row extents
/// include neighboring image shapes, which would stretch text backgrounds.
fn run_rects(layout: &pango::Layout, range: &Range<usize>) -> Vec<graphene::Rect> {
    let mut rects = Vec::new();
    let mut runs = layout.iter();
    loop {
        if let Some(run) = runs.run_readonly() {
            let item = run.item();
            let start = usize::try_from(item.offset()).unwrap_or(0).max(range.start);
            let end = usize::try_from(item.offset() + item.length()).unwrap_or(0).min(range.end);
            if start < end {
                let (_, logical) = runs.run_extents();
                if let Some(row) = runs.line_readonly() {
                    let edges = row.x_ranges(
                        i32::try_from(start).unwrap_or(0),
                        i32::try_from(end).unwrap_or(0),
                    );
                    for pair in edges.as_chunks::<2>().0 {
                        #[allow(clippy::cast_possible_truncation)]
                        rects.push(graphene::Rect::new(
                            pixels(pair[0]) as f32,
                            pixels(logical.y()) as f32,
                            pixels(pair[1] - pair[0]) as f32,
                            pixels(logical.height()) as f32,
                        ));
                    }
                }
            }
        }
        if !runs.next_run() {
            break;
        }
    }
    rects
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
            let color = gdk::RGBA::parse(heading_color(level, dark))
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
        Look::Highlight => Vec::new(),
        Look::Code | Look::Monospace => vec![monospace()],
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
        Look::Error => {
            let mut attributes = foreground((0.8, 0.15, 0.15, 1.0));
            attributes.push(pango::AttrString::new_family("monospace").upcast());
            attributes
        }
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

/// Whether a clipboard offers an image without text, as the existing editor prefers text.
pub fn image_only(formats: gdk::ContentFormats) -> bool {
    let formats = formats.union_deserialize_types();
    formats.contains_type(gdk::Texture::static_type()) && !formats.contains_type(glib::Type::STRING)
}

/// Reserves each image's size, scaled down to the text width, above the text's baseline.
struct ImageContext<'a> {
    available: f64,
    display: &'a Display,
    math_scale: f64,
    renders: &'a drawings::Resources,
    text: &'a str,
    textures: &'a HashMap<Id, gdk::Texture>,
}

fn shape_images(
    layout: &pango::Layout,
    attributes: &pango::AttrList,
    pieces: &[Piece],
    context: &ImageContext<'_>,
) -> Vec<InlineImage> {
    let ImageContext { available, display, math_scale, renders, text, textures } = *context;
    let mut images: Vec<InlineImage> = pieces
        .iter()
        .filter_map(|piece| {
            let (texture, width, height, descent) = match piece.content {
                Content::Image(target) => {
                    let texture = textures.get(&target)?.clone();
                    let (width, height) = (f64::from(texture.width()), f64::from(texture.height()));
                    (texture, width, height, 0.0)
                }
                Content::Drawing(index) => {
                    let drawing = &display.drawings[index];
                    let drawings::Resource::Image(texture, size) =
                        renders.get(drawing.preview.as_ref().unwrap_or(&drawing.source))?
                    else {
                        return None;
                    };
                    let descent = pixels(layout.context().metrics(None, None).descent());
                    (
                        texture.clone(),
                        f64::from(size.width) * math_scale,
                        f64::from(size.height) * math_scale,
                        descent,
                    )
                }
                _ => return None,
            };
            let scale = (available.max(1.0) / width.max(1.0)).min(1.0);
            Some(InlineImage {
                descent: descent * scale,
                source: piece.source,
                index: piece.shown,
                width: width * scale,
                height: height * scale,
                end: piece.source + piece.source_len,
                texture,
                source_layout: None,
            })
        })
        .collect();
    for image in &images {
        apply_image_shape(attributes, image);
    }
    // Each image starts where the preceding text and images put it. Resolve sources from left
    // to right, so each one can use the remaining width without moving the next one off the edge.
    layout.set_attributes(attributes.copy().as_ref());
    for image in &mut images {
        if !display.images.iter().any(|found| found.range.start == image.source && found.revealed) {
            continue;
        }
        let (_, x) = layout.index_to_line_x(i32::try_from(image.index).unwrap_or(0), false);
        let remaining = (available - pixels(x)).max(image.width);
        let source = pango::Layout::new(&layout.context());
        source.set_text(&text[image.source..image.end]);
        let source_style = pango::AttrList::new();
        for attribute in attributes_of(Look::Monospace, false)
            .into_iter()
            .chain(attributes_of(Look::Syntax, false))
        {
            source_style.insert(attribute);
        }
        source.set_attributes(Some(&source_style));
        let natural = source.size().0;
        source.set_width(natural.min(units(remaining.max(1.0))));
        source.set_wrap(pango::WrapMode::WordChar);
        image.source_layout = Some(source);
        apply_image_shape(attributes, image);
        layout.set_attributes(attributes.copy().as_ref());
    }
    images
}

fn apply_image_shape(attributes: &pango::AttrList, image: &InlineImage) {
    let (raised, below, reserved) =
        image.source_layout.as_ref().map_or((0.0, 0.0, image.width), |layout| {
            let (width, height) = layout.size();
            let baseline = pixels(layout.baseline());
            (baseline + IMAGE_SOURCE_GAP, pixels(height) - baseline, image.width.max(pixels(width)))
        });
    let rect = pango::Rectangle::new(
        0,
        -units(image.height + raised - image.descent),
        units(reserved),
        units(image.height + raised + below),
    );
    let mut shape = pango::AttrShape::new(&rect, &rect);
    shape.set_start_index(u32::try_from(image.index).unwrap_or(0));
    shape.set_end_index(u32::try_from(image.index + 3).unwrap_or(u32::MAX));
    attributes.change(shape);
}

/// The picture rises above its inline source, whose first baseline follows the paragraph's text.
fn image_rect(line: &Line, image: &InlineImage) -> graphene::Rect {
    let (_, x) = line.layout.index_to_line_x(i32::try_from(image.index).unwrap_or(0), false);
    let raised = image
        .source_layout
        .as_ref()
        .map_or(0.0, |source| pixels(source.baseline()) + IMAGE_SOURCE_GAP);
    #[allow(clippy::cast_possible_truncation)]
    graphene::Rect::new(
        pixels(x) as f32,
        (pixels(baseline_at(&line.layout, image.index)) - image.height - raised + image.descent)
            as f32,
        image.width as f32,
        image.height as f32,
    )
}

fn source_rect(line: &Line, image: &InlineImage) -> Option<graphene::Rect> {
    let source = image.source_layout.as_ref()?;
    let picture = image_rect(line, image);
    let (width, height) = source.size();
    #[allow(clippy::cast_possible_truncation)]
    Some(graphene::Rect::new(
        picture.x(),
        (pixels(baseline_at(&line.layout, image.index)) - pixels(source.baseline())) as f32,
        pixels(width) as f32,
        pixels(height) as f32,
    ))
}

fn draw_image_source(
    snapshot: &gtk::Snapshot,
    line: &Line,
    image: &InlineImage,
    selection: &Range<usize>,
    highlight: &gdk::RGBA,
    color: &gdk::RGBA,
) {
    let (Some(source), Some(rect)) = (&image.source_layout, source_rect(line, image)) else {
        return;
    };
    snapshot.save();
    snapshot.translate(&graphene::Point::new(rect.x(), rect.y()));
    let (start, end) = (selection.start.max(image.source), selection.end.min(image.end));
    if start < end {
        for rect in selection_rects(source, start - image.source, end - image.source, false) {
            snapshot.append_color(highlight, &rect);
        }
    }
    snapshot.append_layout(source, color);
    snapshot.restore();
}

/// Applies source styles to visible runs, including inherited styles on drawn titles.
fn style_source(
    attributes: &pango::AttrList,
    pieces: &[Piece],
    shown: &Display,
    range: Range<usize>,
    dark: bool,
    backgrounds: &mut Vec<Background>,
) -> bool {
    let (start, end) = (range.start, range.end);
    let mut heading = false;
    for styled in &shown.styles {
        if styled.range.start >= end.max(start + 1) || styled.range.end <= start {
            continue;
        }
        heading |= matches!(styled.look, Look::Heading(_));
        for piece in pieces {
            if matches!(piece.content, Content::Image(_) | Content::Drawing(_))
                || (piece.content != Content::Source && styled.look == Look::Syntax)
            {
                continue;
            }
            let from = styled.range.start.max(piece.source);
            let to = styled.range.end.min(piece.source + piece.source_len);
            if from >= to {
                continue;
            }
            let index = |at: usize| {
                let offset = if piece.content == Content::Source {
                    at - piece.source
                } else if at == from {
                    0
                } else {
                    piece.len
                };
                u32::try_from(piece.shown + offset)
            };
            let range = usize::try_from(index(from).unwrap_or(0)).unwrap_or(0)
                ..usize::try_from(index(to).unwrap_or(u32::MAX)).unwrap_or(0);
            add_background(backgrounds, styled.look, range);
            for mut attribute in attributes_of(styled.look, dark) {
                attribute.set_start_index(index(from).unwrap_or(0));
                attribute.set_end_index(index(to).unwrap_or(u32::MAX));
                attributes.insert(attribute);
            }
        }
    }
    heading
}

/// Applies the styles within each title, after the styles inherited from the surrounding source.
fn style_labels(
    attributes: &pango::AttrList,
    pieces: &[Piece],
    display: &Display,
    dark: bool,
    backgrounds: &mut Vec<Background>,
) {
    for piece in pieces.iter().filter(|p| p.content == Content::Label) {
        let Some(label) = display.labels.iter().find(|l| l.range.start == piece.source) else {
            continue;
        };
        for styled in &label.styles {
            add_background(
                backgrounds,
                styled.look,
                piece.shown + styled.range.start..piece.shown + styled.range.end,
            );
            for mut attribute in attributes_of(styled.look, dark) {
                attribute
                    .set_start_index(u32::try_from(piece.shown + styled.range.start).unwrap_or(0));
                attribute.set_end_index(
                    u32::try_from(piece.shown + styled.range.end).unwrap_or(u32::MAX),
                );
                attributes.insert(attribute);
            }
        }
    }
}

/// The parts of a line that show, with link titles in place of their source.
fn pieces(text: &str, start: usize, end: usize, display: &Display) -> (Vec<Piece>, String) {
    let atoms: Vec<_> = display
        .images
        .iter()
        .map(|i| (i.range.clone(), Content::Image(i.target)))
        .chain(
            display
                .drawings
                .iter()
                .enumerate()
                .filter(|(_, d)| !d.block && d.collapsed())
                .map(|(index, d)| (d.range.clone(), Content::Drawing(index))),
        )
        .collect();
    let mut pieces = Vec::new();
    let mut shown_text = String::new();
    let mut at = start;
    while at < end {
        let shown = shown_text.len();
        if let Some((range, content)) = atoms.iter().find(|(range, _)| range.start == at) {
            let atom = "\u{fffc}";
            pieces.push(Piece {
                source: at,
                shown,
                len: atom.len(),
                source_len: range.len(),
                content: *content,
            });
            shown_text.push_str(atom);
            at = range.end;
        } else if let Some(label) = display.labels.iter().find(|l| l.range.start == at) {
            pieces.push(Piece {
                source: at,
                shown,
                len: label.text.len(),
                source_len: label.range.len(),
                content: Content::Label,
            });
            shown_text.push_str(&label.text);
            at = label.range.end;
        } else if let Some(gap) = display.hidden.iter().find(|r| r.contains(&at)) {
            at = display
                .labels
                .iter()
                .map(|l| l.range.start)
                .chain(atoms.iter().map(|(range, _)| range.start))
                .filter(|&p| p > at)
                .min()
                .unwrap_or(end)
                .min(gap.end)
                .min(end);
        } else {
            let next = display
                .hidden
                .iter()
                .map(|r| r.start)
                .chain(display.labels.iter().map(|l| l.range.start))
                .chain(atoms.iter().map(|(range, _)| range.start))
                .filter(|&p| p > at)
                .min()
                .unwrap_or(end)
                .min(end);
            pieces.push(Piece {
                source: at,
                shown,
                len: next - at,
                source_len: next - at,
                content: Content::Source,
            });
            shown_text.push_str(&text[at..next]);
            at = next;
        }
    }
    if pieces.is_empty() {
        pieces.push(Piece {
            source: end,
            shown: 0,
            len: 0,
            source_len: 0,
            content: Content::Source,
        });
    }
    (pieces, shown_text)
}

/// The top and height of the row, as the text wraps, that `at` lies on, in the widget's
/// coordinates.
fn row_extents(lines: &[Line], at: usize) -> Option<(f64, f64)> {
    let line = line_at(lines, at)?;
    let (layout, local, top) = if let Some(image) = source_image(line, at) {
        (
            image.source_layout.as_ref()?,
            at - image.source,
            line.y + f64::from(source_rect(line, image)?.y()),
        )
    } else {
        (&line.layout, line.shown(at), line.y)
    };
    let (number, _) = layout.index_to_line_x(i32::try_from(local).ok()?, false);
    let mut rows = layout.iter();
    for _ in 0..number {
        if !rows.next_line() {
            break;
        }
    }
    let (_, logical) = rows.line_extents();
    Some((top + pixels(logical.y()), pixels(logical.height())))
}

/// Where the text's byte `at` falls in the text that `pieces` show. Hidden syntax falls where the
/// text after it shows.
fn pieces_shown(pieces: &[Piece], at: usize) -> usize {
    for piece in pieces {
        if at <= piece.source + piece.source_len {
            let offset = if piece.content == Content::Source {
                at.saturating_sub(piece.source).min(piece.len)
            } else if at <= piece.source {
                0
            } else {
                piece.len
            };
            return piece.shown + offset;
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
    pub(super) fn widget(text: &str) -> Option<EditorWidget> {
        adw::init().ok()?;
        let at = text.find('|')?;
        let mut editor = Editor::new(Body::detached(&text.replace('|', "")));
        editor.select(at, at);
        let widget = EditorWidget::default();
        widget.set_editor(Some(editor));
        widget.lay_out(600);
        Some(widget)
    }

    pub(super) fn head(widget: &EditorWidget) -> usize {
        widget.head().unwrap_or_default()
    }

    /// Completes the viewport's allocation and the deferred buffer update it schedules.
    pub(super) fn allocate_viewport(scrolled: &gtk::ScrolledWindow, width: i32, height: i32) {
        for _ in 0..2 {
            scrolled.measure(gtk::Orientation::Horizontal, -1);
            scrolled.measure(gtk::Orientation::Vertical, width);
            scrolled.allocate(width, height, -1, None);
            for _ in 0..100 {
                if !glib::MainContext::default().iteration(false) {
                    break;
                }
            }
        }
    }

    #[test]
    fn conversation_messages_allow_selection_but_reject_edits_and_undo() {
        let Some(widget) = widget("A message| with **markdown**") else { return };
        let original = widget.text().unwrap();
        widget.set_read_only(true);
        widget.commit_text("Typing", gdk::ModifierType::empty());
        for key in [gdk::Key::BackSpace, gdk::Key::Delete, gdk::Key::Return] {
            widget.key(key, gdk::ModifierType::empty());
        }
        for key in [gdk::Key::v, gdk::Key::x, gdk::Key::z] {
            widget.key(key, gdk::ModifierType::CONTROL_MASK);
        }
        assert_eq!(widget.text().as_deref(), Some(original.as_str()));
        widget.key(gdk::Key::a, gdk::ModifierType::CONTROL_MASK);
        assert_eq!(widget.imp().editor.borrow().as_ref().unwrap().selected_text(), original);
    }

    #[test]
    fn control_enter_moves_list_items_and_shift_control_enter_moves_them_back() {
        let Some(widget) = widget("- first\n- second|\n- third") else { return };
        let before = widget.text().unwrap();
        widget.key(gdk::Key::Return, gdk::ModifierType::CONTROL_MASK);
        assert_eq!(widget.text().as_deref(), Some("- first\n- third\n- second"));
        widget
            .key(gdk::Key::Return, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        assert_eq!(widget.text().as_deref(), Some(before.as_str()));
    }

    #[test]
    fn chat_messages_do_not_pad_the_shared_history_when_link_titles_refresh() {
        let Some(widget) = widget("|A short agent reply.") else { return };
        widget.set_read_only(true);
        let history = gtk::Box::new(gtk::Orientation::Vertical, 16);
        let earlier = gtk::Box::new(gtk::Orientation::Vertical, 0);
        earlier.set_height_request(1200);
        history.append(&earlier);
        history.append(&widget);
        let scrolled = gtk::ScrolledWindow::builder().child(&history).build();
        let before = widget.measure(gtk::Orientation::Vertical, 360).1;
        let adjustment = scrolled.vadjustment();
        adjustment.configure(1200.0, 0.0, 2000.0, 10.0, 400.0, 480.0);
        for _ in 0..4 {
            // Label resolution runs asynchronously after each streamed message update.
            widget.set_titles(HashMap::new());
            assert_eq!(widget.measure(gtk::Orientation::Vertical, 360).1, before);
            assert_eq!(adjustment.upper(), 2000.0);
        }
    }

    #[test]
    fn a_notes_end_buffer_tracks_the_viewport_and_keeps_text_onscreen() {
        let Some(widget) = widget("# Title\n\nLast line|") else { return };
        let natural = widget.measure(gtk::Orientation::Vertical, 600).1;
        let title_y = widget.imp().lines.borrow()[0].y;
        let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
        for page in [180, 400, 180] {
            allocate_viewport(&scrolled, 600, page);
            let height = widget.measure(gtk::Orientation::Vertical, 600).1;
            assert_eq!(height, natural + page * 4 / 5);
            let adjustment = scrolled.vadjustment();
            assert_eq!(adjustment.upper(), f64::from(height.max(page)));
            adjustment.set_value(adjustment.upper());
            let scroll = adjustment.value();
            let bottom = {
                let lines = widget.imp().lines.borrow();
                let last = lines.last().unwrap();
                last.y + last.height
            };
            assert!(bottom > scroll && bottom < scroll + f64::from(page));
            widget.set_titles(HashMap::new());
            assert_eq!(widget.measure(gtk::Orientation::Vertical, 600).1, height);
            assert_eq!(widget.imp().lines.borrow()[0].y, title_y);
            assert_eq!(adjustment.value(), scroll);
        }
    }

    fn has_image_source(widget: &EditorWidget) -> bool {
        widget
            .imp()
            .lines
            .borrow()
            .iter()
            .any(|line| line.images.iter().any(|image| image.source_layout.is_some()))
    }

    #[test]
    fn deleting_embeds_near_the_title_restores_the_natural_document_extent() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        for embed in [
            format!("![[{id}]]"),
            "| header |\n|---|\n| value |".to_owned(),
            "```table\n- - header\n- - value\n```".to_owned(),
        ] {
            let Some(widget) = widget("|") else { return };
            let text = format!("# Title\n\n{embed}\n\nend");
            let mut editor = Editor::new(Body::detached(&text));
            editor.move_to(text.len(), false);
            widget.set_editor(Some(editor));
            let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
            let (generation, _) = widget.image_requests(false);
            widget.set_image(generation, id, Some(texture(160, 300)));
            let (session, requests) = widget.drawing_requests();
            for drawing in requests {
                widget.set_table(
                    session,
                    widget.table_revision(),
                    drawing,
                    crate::tables::Refs::default(),
                );
            }
            allocate_viewport(&scrolled, 600, 700);
            let height = widget.lay_out(600);
            scrolled.vadjustment().configure(0.0, 0.0, f64::from(height), 10.0, 680.0, 700.0);
            let title_y = widget.imp().lines.borrow()[0].y;
            let start = text.find(&embed).unwrap();
            widget.edit(|e| e.select(start, start + embed.len()));
            widget.edit(Editor::backspace);
            widget.scroll_to_cursor();
            assert_eq!(widget.text().as_deref(), Some("# Title\n\n\n\nend"));
            assert_eq!(widget.imp().lines.borrow()[0].y, title_y, "{embed}");
            let clean = EditorWidget::default();
            let mut editor = Editor::new(Body::detached(&widget.text().unwrap()));
            editor.move_to(start, false);
            clean.set_editor(Some(editor));
            assert_eq!(
                f64::from(widget.lay_out(600)),
                f64::from(clean.lay_out(600)) + scrolled.vadjustment().page_size() * 0.8,
                "{embed}"
            );
            assert!(widget.lay_out(600) < height, "{embed}");
            assert_eq!(scrolled.vadjustment().value(), 0.0);
        }
    }

    /// A colored image with a stripe, so rendering checks show its aspect ratio and placement.
    pub(super) fn texture(width: i32, height: i32) -> gdk::Texture {
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let color = if x < width / 4 || y < height / 4 {
                    [45, 130, 210, 255]
                } else {
                    [220, 145, 45, 255]
                };
                bytes.extend(color);
            }
        }
        gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(bytes),
            usize::try_from(width * 4).unwrap(),
        )
        .upcast()
    }

    #[test]
    fn inline_images_share_the_baseline_and_reveal_their_source_on_a_click() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text =
            format!("before ![[{id}]] after ![[{id}]]\n- [ ] item ![[{id}]] tail\n![[{id}]]\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, targets) = widget.image_requests(false);
        assert_eq!(targets, [id]);
        widget.set_image(generation, id, Some(texture(80, 48)));
        let lines = widget.imp().lines.borrow();
        assert_eq!(lines[0].images.len(), 2);
        for line in &lines[..3] {
            assert!(line.height >= 48.0);
            for image in &line.images {
                let rect = image_rect(line, image);
                assert_eq!(rect.width(), 80.0);
                assert_eq!(rect.height(), 48.0);
                assert!(
                    (f64::from(rect.y() + rect.height()) - pixels(line.layout.baseline())).abs()
                        < 0.01
                );
                let x = line.left + f64::from(rect.x() + rect.width() * 0.9);
                let y = line.y + f64::from(rect.y() + rect.height() * 0.9);
                assert_eq!(widget.position_at(x, y), image.source);
                assert_eq!(widget.link_at(x, y), Some(LinkTarget::Note(id)));
                let after = line.layout.index_to_pos(i32::try_from(image.index + 3).unwrap());
                assert!(pixels(after.x()) >= f64::from(rect.x() + rect.width()) - 0.01);
            }
        }
        let source = lines[0].images[0].source;
        drop(lines);
        widget.edit(|e| e.move_to(source, false));
        assert_eq!(widget.imp().lines.borrow()[0].images.len(), 2);
        assert!(widget.imp().lines.borrow()[0].images[0].source_layout.is_some());
        assert!(widget.text().unwrap().contains(&format!("![[{id}]]")));
    }

    #[test]
    fn revealed_source_occupies_the_image_slot_and_moves_following_text() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("above\nbefore ![[{id}]] after\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(80, 120)));
        let source = widget.text().unwrap().find('!').unwrap();
        let before_x = {
            let lines = widget.imp().lines.borrow();
            let line = &lines[1];
            line.layout.index_to_pos(i32::try_from(line.images[0].index + 3).unwrap()).x()
        };
        widget.edit(|e| e.move_to(source, false));
        let lines = widget.imp().lines.borrow();
        let line = &lines[1];
        let image = &line.images[0];
        let picture = image_rect(line, image);
        let rect = source_rect(line, image).unwrap();
        let layout = image.source_layout.as_ref().unwrap();
        assert_eq!(layout.text(), format!("![[{id}]]"));
        assert_eq!(rect.x(), picture.x());
        assert!(
            (f64::from(rect.y() - picture.y() - picture.height()) - IMAGE_SOURCE_GAP).abs() < 0.01
        );
        assert!(
            (f64::from(rect.y()) + pixels(layout.baseline())
                - pixels(baseline_at(&line.layout, image.index)))
            .abs()
                < 0.01
        );
        let after_x = line.layout.index_to_pos(i32::try_from(image.index + 3).unwrap()).x();
        assert!(after_x > before_x + units(80.0));
        assert!(pixels(after_x) >= f64::from(rect.x() + rect.width()) - 0.01);
        let x = line.left + f64::from(rect.x()) + 1.0;
        let y = line.y + f64::from(rect.y()) + 4.0;
        assert_eq!(widget.position_at(x, y), source);
        assert_eq!(widget.link_at(x, y), Some(LinkTarget::Note(id)));
        assert!(caret_rect(&lines, source).unwrap().2 < 40.0);
        drop(lines);
        widget.edit(|e| e.move_by(Unit::Grapheme, Direction::Forward, false));
        assert_eq!(head(&widget), source + 1);
        let before = row_extents(&widget.imp().lines.borrow(), head(&widget)).unwrap().0;
        widget.move_vertically(false, false, 1);
        assert!(row_extents(&widget.imp().lines.borrow(), head(&widget)).unwrap().0 < before);
    }

    #[test]
    fn wide_images_keep_following_text_in_the_same_horizontal_position() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] after\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(320, 120)));
        let before = {
            let lines = widget.imp().lines.borrow();
            let line = &lines[0];
            line.layout.index_to_pos(i32::try_from(line.images[0].index + 3).unwrap()).x()
        };
        widget.edit(|e| e.move_to(7, false));
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        let image = &line.images[0];
        assert!(source_rect(line, image).unwrap().width() < 320.0);
        assert_eq!(line.layout.index_to_pos(i32::try_from(image.index + 3).unwrap()).x(), before);
        assert_eq!(line.layout.line_count(), 1);
    }

    #[test]
    fn inline_source_wraps_as_one_image_slot_and_keeps_its_rows_editable() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] after\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(80, 80)));
        widget.lay_out(240);
        let before_x = {
            let lines = widget.imp().lines.borrow();
            image_rect(&lines[0], &lines[0].images[0]).x()
        };
        widget.edit(|e| e.move_to(7, false));
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        let image = &line.images[0];
        let source = image.source_layout.as_ref().unwrap();
        let rect = source_rect(line, image).unwrap();
        let picture = image_rect(line, image);
        assert!(source.line_count() > 1);
        assert!(line.layout.line_count() >= 2);
        assert_eq!(rect.x(), picture.x());
        assert_eq!(picture.x(), before_x);
        assert!(rect.x() + rect.width() <= 192.01);
        assert!(picture.x() + picture.width() <= 192.01);
        let suffix = line.layout.index_to_pos(i32::try_from(line.shown(image.end + 1)).unwrap());
        assert!(pixels(suffix.y()) >= f64::from(rect.y() + rect.height()) - 0.01);
        drop(lines);
        let first_end = widget.visual_line_edge(7, true);
        assert!(7 < first_end && first_end < 38);
        widget.move_vertically(true, false, 1);
        assert!(7 < head(&widget) && head(&widget) < 38);
        widget.move_vertically(false, false, 1);
        assert_eq!(head(&widget), 7);
    }

    #[test]
    fn multiple_inline_sources_keep_their_own_offsets_and_room() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] and ![[{id}]]\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(80, 80)));
        widget.edit(Editor::select_all);
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        assert_eq!(line.images.len(), 2);
        let first = source_rect(line, &line.images[0]).unwrap();
        let second = source_rect(line, &line.images[1]).unwrap();
        assert!(first.x() + first.width() <= second.x());
        assert!(second.x() + second.width() <= 552.01);
        for image in &line.images {
            let rect = source_rect(line, image).unwrap();
            let source = image.source_layout.as_ref().unwrap();
            let cell = source.index_to_pos(3);
            let x = line.left + f64::from(rect.x()) + pixels(cell.x()) + 1.0;
            let y = line.y + f64::from(rect.y()) + pixels(cell.y()) + 4.0;
            assert_eq!(widget.position_at(x, y), image.source + 3);
        }
    }

    pub(super) fn render(widget: &EditorWidget, renderer: &gtk::gsk::CairoRenderer) -> Vec<u8> {
        let snapshot = gtk::Snapshot::new();
        let viewport = graphene::Rect::new(0.0, 0.0, 600.0, 330.0);
        snapshot.append_color(&gdk::RGBA::BLACK, &viewport);
        widget.draw(&snapshot);
        let texture = renderer.render_texture(snapshot.to_node().unwrap(), Some(&viewport));
        let mut bytes = vec![0; 600 * 330 * 4];
        texture.download(&mut bytes, 600 * 4);
        bytes
    }

    #[test]
    fn quote_bars_join_across_blank_lines_headings_and_wrapped_text() {
        let text = concat!(
            "> # Quote heading\n>\n> Outer **bold**\n",
            "> > Inner words that wrap onto more rows\n> > more\n\n|end"
        );
        let Some(widget) = widget(text) else { return };
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let mut bottom = MARGIN_TOP;
        for (index, line) in lines.iter().take(5).enumerate() {
            let bars = quote_rects(line);
            let depth = line.quote.as_ref().unwrap().depth;
            assert_eq!(bars.len(), depth);
            if index == 0 {
                assert!(f64::from(bars[0].y()) > bottom);
            } else {
                assert_eq!(f64::from(bars[0].y()), bottom);
            }
            assert_eq!(f64::from(bars[0].x()), MARGIN_SIDES + QUOTE_BAR_OFFSET);
            assert_eq!(line.left, MARGIN_SIDES + f64::from(u32::try_from(depth).unwrap()) * 18.0);
            bottom = f64::from(bars[0].y() + bars[0].height());
            assert_eq!(bottom, line.y + line.height);
            if depth == 2 {
                assert_eq!(bars[1].x() - bars[0].x(), 18.0);
                if index == 3 {
                    assert!(bars[1].y() > bars[0].y());
                } else {
                    assert_eq!(bars[1].y(), bars[0].y());
                }
                assert_eq!(bars[1].y() + bars[1].height(), bars[0].y() + bars[0].height());
            }
        }
        assert_eq!(lines[1].layout.text(), "");
        assert!(lines[3].layout.line_count() > 1);
        assert_eq!(quote_rects(lines.last().unwrap()), Vec::<graphene::Rect>::new());
    }

    #[test]
    fn quotes_inside_list_items_reserve_room_after_the_marker() {
        let text = "- > quoted words that wrap over several rows\n  > > nested\n\n|end";
        let Some(widget) = widget(text) else { return };
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let first = &lines[0];
        let next = &lines[1];
        assert!(first.layout.line_count() > 1);
        assert_eq!(first.left, MARGIN_SIDES);
        let bars = quote_rects(first);
        assert_eq!(bars.len(), 1);
        let first_row = pixels(first.layout.iter().line_extents().1.height());
        assert!(f64::from(bars[0].y()) > first.y);
        assert_eq!(f64::from(bars[0].y() + bars[0].height()), first.y + first_row);
        let x = pixels(first.layout.index_to_pos(2).x());
        assert!(MARGIN_SIDES + x > f64::from(bars[0].x() + bars[0].width()));
        assert_eq!(quote_rects(next).len(), 2);
        assert_eq!(quote_rects(next)[0].x(), bars[0].x());
        let at = text.find("quoted").unwrap();
        let cell = first.layout.index_to_pos(i32::try_from(first.shown(at)).unwrap());
        assert_eq!(widget.position_at(first.left + pixels(cell.x()) + 1.0, first.y + 4.0), at);
    }

    #[test]
    fn quoted_images_fit_the_inset_width_and_keep_their_source_mapping() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("> ![[{id}]]\n\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(800, 400)));
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        let picture = image_rect(line, &line.images[0]);
        assert_eq!(picture.width(), 174.0);
        assert_eq!(picture.height(), 87.0);
        assert_eq!(line.left, MARGIN_SIDES + QUOTE_INDENT);
        assert_eq!(f64::from(picture.width()) + line.left, 216.0);
        let (x, y) = (line.left + 10.0, line.y + 10.0);
        drop(lines);
        assert_eq!(widget.position_at(x, y), 2);
        widget.edit(|e| e.move_to(2, false));
        assert!(has_image_source(&widget));
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        let image = &line.images[0];
        let source = source_rect(line, image).unwrap();
        assert!(source.x() >= 0.0);
        let bar = quote_rects(line)[0];
        assert!((f64::from(bar.y()) - line.y) < 10.0);
        assert_eq!(f64::from(bar.y() + bar.height()), line.y + line.height);
    }

    #[test]
    fn code_blocks_shade_empty_lines_and_quote_bars_render_at_each_depth() {
        let text = "> # Heading\n>\n> Outer\n> > Inner\n> > more\n\n```rust\ncode\n\n```\n\n|end";
        let Some(widget) = widget(text) else { return };
        let _window = gtk::Window::builder().child(&widget).build();
        let provider = gtk::CssProvider::new();
        provider.load_from_string("* { color: #222; }");
        gtk::style_context_add_provider_for_display(
            &widget.display(),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        widget.measure(gtk::Orientation::Horizontal, -1);
        widget.measure(gtk::Orientation::Vertical, 600);
        widget.allocate(600, 330, -1, None);
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer.realize(None::<&gdk::Surface>).unwrap();
        let snapshot = gtk::Snapshot::new();
        let viewport = graphene::Rect::new(0.0, 0.0, 600.0, 330.0);
        snapshot.append_color(&gdk::RGBA::WHITE, &viewport);
        widget.draw(&snapshot);
        let texture = renderer.render_texture(snapshot.to_node().unwrap(), Some(&viewport));
        let mut bytes = vec![0; 600 * 330 * 4];
        texture.download(&mut bytes, 600 * 4);
        renderer.unrealize();
        let pixel = |x: f64, y: f64| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let at = ((y as usize) * 600 + x as usize) * 4;
            &bytes[at..at + 4]
        };
        let lines = widget.imp().lines.borrow();
        for line in lines.iter().take(5) {
            for bar in quote_rects(line) {
                assert_ne!(
                    pixel(f64::from(bar.x()) + 1.0, f64::from(bar.y()) + 2.0),
                    &[255, 255, 255, 255]
                );
            }
        }
        for line in lines.iter().skip(6).take(4) {
            assert!(line.shaded);
            assert_ne!(pixel(400.0, line.y + 2.0), &[255, 255, 255, 255]);
        }
        assert!(!lines[10].shaded);
        assert_eq!(pixel(400.0, lines[10].y + 2.0), &[255, 255, 255, 255]);
    }

    #[test]
    fn quote_and_code_keys_use_core_rules_and_modified_input_stays_literal() {
        let Some(widget) = widget("> text|") else { return };
        widget.key(gdk::Key::Return, gdk::ModifierType::SHIFT_MASK);
        assert_eq!(widget.text().as_deref(), Some("> text\n> "));
        widget.key(gdk::Key::Return, gdk::ModifierType::empty());
        assert_eq!(widget.text().as_deref(), Some("> text\n\n"));
        widget.set_editor(Some(Editor::new(Body::detached("``"))));
        widget.edit(|e| e.move_to(2, false));
        widget.commit_text("`", gdk::ModifierType::empty());
        assert_eq!(widget.text().as_deref(), Some("```\n```"));
        assert_eq!(head(&widget), 3);
        for state in [gdk::ModifierType::ALT_MASK, gdk::ModifierType::CONTROL_MASK] {
            widget.set_editor(Some(Editor::new(Body::detached("selected"))));
            widget.edit(Editor::select_all);
            widget.commit_text("`", state);
            assert_eq!(widget.text().as_deref(), Some("`"));
        }
    }

    #[test]
    fn inline_backgrounds_follow_the_font_height_beside_tall_images() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("![[{id}]] ==highlight== and `code`\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(160, 240)));
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        assert_eq!(line.backgrounds.len(), 2);
        for background in &line.backgrounds {
            let rects = run_rects(&line.layout, &background.range);
            assert_ne!(rects, Vec::<graphene::Rect>::new());
            for rect in rects {
                assert!(rect.height() < 40.0, "{rect:?}");
                assert!(rect.y() > 200.0, "{rect:?}");
                assert!(rect.x() >= 160.0, "{rect:?}");
            }
        }
    }

    #[test]
    fn selecting_text_beside_an_image_uses_the_full_row_height() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] after ==task== **foo** `code`\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(160, 240)));
        let after = widget.text().unwrap().find("after").unwrap();
        let end = widget.text().unwrap().find('\n').unwrap();
        for selection in [0..6, after..end, after..end + 1, 0..end + 1] {
            widget.edit(|e| e.select(selection.start, selection.end));
            let lines = widget.imp().lines.borrow();
            let line = &lines[0];
            let rects = selection_rects(
                &line.layout,
                line.shown(selection.start),
                line.shown(selection.end.min(end)),
                selection.end > end,
            );
            assert_ne!(rects, Vec::<graphene::Rect>::new());
            let mut rows = line.layout.iter();
            let mut bands = Vec::new();
            loop {
                let (_, logical) = rows.line_extents();
                bands.push((pixels(logical.y()), pixels(logical.height())));
                if !rows.next_line() {
                    break;
                }
            }
            for rect in &rects {
                assert!(bands.contains(&(f64::from(rect.y()), f64::from(rect.height()))));
            }
            if selection.end > end {
                assert!(rects.iter().any(|r| (r.x() + r.width() - 552.0).abs() < 0.01));
            }
        }
    }

    #[test]
    fn numbered_list_selections_fill_adjacent_rows_without_gaps() {
        let text = (40..60).map(|n| format!("{n}) a")).collect::<Vec<_>>().join("\n");
        let Some(widget) = widget(&format!("|{text}")) else { return };
        widget.edit(|e| e.select(0, text.len()));
        let lines = widget.imp().lines.borrow();
        let mut bottom = MARGIN_TOP;
        for line in lines.iter() {
            let rects =
                selection_rects(&line.layout, 0, line.layout.text().len(), line.end < text.len());
            assert_eq!(rects.len(), 1);
            let rect = &rects[0];
            assert_eq!(f64::from(rect.y()), 0.0);
            assert_eq!(f64::from(rect.height()), line.height);
            assert_eq!(line.y + f64::from(rect.y()), bottom);
            bottom = line.y + f64::from(rect.y() + rect.height());
        }
    }

    #[test]
    fn wrapped_selection_fills_each_row_including_the_image_row() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("![[{id}]] {}\n|end", "word ".repeat(20));
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(80, 240)));
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        assert!(line.layout.line_count() > 1);
        let rects = selection_rects(&line.layout, line.shown(31), line.shown(line.end), true);
        assert_eq!(rects.len(), usize::try_from(line.layout.line_count()).unwrap());
        assert!(rects[0].height() >= 240.0);
        for pair in rects.windows(2) {
            assert!((pair[0].y() + pair[0].height() - pair[1].y()).abs() < 0.01);
        }
        let tails = rects.iter().filter(|r| (r.x() + r.width() - 192.0).abs() < 0.01);
        assert!(tails.count() >= usize::try_from(line.layout.line_count()).unwrap());
    }

    #[test]
    fn selecting_an_embed_tints_its_picture_but_selecting_source_characters_does_not() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] after\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(160, 240)));
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer.realize(None::<&gdk::Surface>).unwrap();
        let before = render(&widget, &renderer);
        widget.edit(|e| e.select(10, 13));
        let partial = render(&widget, &renderer);
        let pixel = (100 * 600 + 100) * 4;
        assert_eq!(&before[pixel..pixel + 4], &partial[pixel..pixel + 4]);
        widget.edit(|e| e.select(7, 38));
        let selected = render(&widget, &renderer);
        assert_ne!(&partial[pixel..pixel + 4], &selected[pixel..pixel + 4]);
        renderer.unrealize();
    }

    #[test]
    fn revealed_images_still_draw_the_same_pixels() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] ==highlight== and `code`\n|below");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(160, 240)));
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer.realize(None::<&gdk::Surface>).unwrap();
        let before = render(&widget, &renderer);
        widget.edit(|e| e.move_to(7, false));
        let after = render(&widget, &renderer);
        // A point inside the blue stripe keeps the image's pixels, with source now below it.
        let pixel = (100 * 600 + 100) * 4;
        assert_ne!(&before[pixel..pixel + 3], &[0, 0, 0]);
        assert_eq!(&before[pixel..pixel + 4], &after[pixel..pixel + 4]);
        renderer.unrealize();
    }

    #[test]
    fn invalidating_and_restoring_an_image_keeps_the_caret_still_with_room_to_scroll() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!(
            "{}before ![[{id}]] after\n|{}",
            "earlier line\n".repeat(40),
            "later line\n".repeat(40)
        );
        for width in [240, 600] {
            let Some(widget) = widget(&text) else { return };
            let at = widget.text().unwrap().find('!').unwrap() + 13;
            let (generation, _) = widget.image_requests(false);
            widget.set_image(generation, id, Some(texture(160, 400)));
            widget.edit(|e| e.move_to(at, false));
            let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
            scrolled.measure(gtk::Orientation::Horizontal, -1);
            scrolled.measure(gtk::Orientation::Vertical, width);
            scrolled.allocate(width, 500, -1, None);
            let height = widget.lay_out(width);
            let caret_y = caret_rect(&widget.imp().lines.borrow(), at).unwrap().1;
            let page = 500;
            let scroll = (caret_y - 100.0).floor();
            let adjustment = scrolled.vadjustment();
            adjustment.configure(
                scroll,
                0.0,
                f64::from(height),
                10.0,
                f64::from(page - 20),
                f64::from(page),
            );
            let screen_y =
                caret_rect(&widget.imp().lines.borrow(), at).unwrap().1 - adjustment.value();
            assert!(0.0 <= screen_y && screen_y + 20.0 < f64::from(page));
            for _ in 0..3 {
                widget.edit(Editor::backspace);
                widget.scroll_to_cursor();
                scrolled.measure(gtk::Orientation::Horizontal, -1);
                scrolled.measure(gtk::Orientation::Vertical, width);
                scrolled.allocate(width, page, -1, None);
                assert!(!has_image_source(&widget));
                let y = caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
                assert!((y - adjustment.value() - screen_y).abs() < 0.01);
                widget.edit(|e| {
                    assert!(e.undo());
                });
                widget.scroll_to_cursor();
                let y = caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
                assert!((y - adjustment.value() - screen_y).abs() < 0.01);
                assert!(has_image_source(&widget));
                assert_eq!(adjustment.value(), scroll);
            }
            // Ordinary typing still moves to the next line; it must not pin the caret in place.
            let at = widget.text().unwrap().find("later line").unwrap();
            widget.edit(|e| e.move_to(at, false));
            let y = caret_rect(&widget.imp().lines.borrow(), at).unwrap().1;
            widget.edit(|e| e.insert("\n"));
            let next = caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
            assert!(next > y);
            widget.set_editor(Some(Editor::new(Body::detached("another note"))));
            assert_eq!(widget.imp().lines.borrow()[0].y, MARGIN_TOP);
        }
    }

    #[test]
    fn an_image_disappearing_near_the_note_end_keeps_the_caret_still_with_the_end_buffer() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}|{}]] after", "word ".repeat(20));
        let Some(widget) = widget("|") else { return };
        let mut editor = Editor::new(Body::detached(&text));
        editor.move_to(20, false);
        widget.set_editor(Some(editor));
        widget.lay_out(240);
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(80, 120)));
        let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
        allocate_viewport(&scrolled, 240, 200);
        let height = widget.lay_out(240);
        let adjustment = scrolled.vadjustment();
        adjustment.configure(130.0, 0.0, f64::from(height), 10.0, 180.0, 200.0);
        let screen_y = caret_rect(&widget.imp().lines.borrow(), 20).unwrap().1 - adjustment.value();
        assert!((0.0..120.0).contains(&screen_y));
        widget.edit(Editor::backspace);
        widget.scroll_to_cursor();
        scrolled.measure(gtk::Orientation::Horizontal, -1);
        scrolled.measure(gtk::Orientation::Vertical, 240);
        scrolled.allocate(240, 200, -1, None);
        let height = f64::from(widget.lay_out(240));
        assert_eq!(adjustment.upper(), height.max(adjustment.page_size()));
        let y = caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
        assert!((y - adjustment.value() - screen_y).abs() < 0.01);
        assert_eq!(widget.imp().lines.borrow()[0].y, MARGIN_TOP);
        widget.edit(|e| {
            assert!(e.undo());
        });
        widget.scroll_to_cursor();
        let height = f64::from(widget.lay_out(240));
        assert_eq!(adjustment.upper(), height.max(adjustment.page_size()));
        assert_eq!(widget.imp().lines.borrow()[0].y, MARGIN_TOP);
        let y = caret_rect(&widget.imp().lines.borrow(), head(&widget)).unwrap().1;
        assert!((y - adjustment.value() - screen_y).abs() < 0.01);
    }

    #[test]
    fn repeatedly_revealing_inline_source_keeps_the_text_baseline_stable() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] after\n|{}", "more text\n".repeat(60));
        let Some(widget) = widget(&text) else { return };
        let source = 7..38;
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(80, 400)));
        let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
        scrolled.measure(gtk::Orientation::Horizontal, -1);
        scrolled.measure(gtk::Orientation::Vertical, 600);
        scrolled.allocate(600, 180, -1, None);
        for width in [600, 240] {
            let height = widget.lay_out(width);
            let adjustment = scrolled.vadjustment();
            adjustment.configure(300.0, 0.0, f64::from(height), 10.0, 150.0, 180.0);
            // The picture's top is offscreen, but its baseline text and source row both fit.
            for (at, into, out) in [
                (source.start - 1, Direction::Forward, Direction::Back),
                (source.end + 1, Direction::Back, Direction::Forward),
            ] {
                widget.imp().editor.borrow_mut().as_mut().unwrap().move_to(at, false);
                widget.rebuild_layout();
                adjustment.set_value(300.0);
                widget.scroll_to_cursor();
                assert_eq!(adjustment.value(), 300.0, "text beside the image scrolled");
                let baseline = paragraph_baseline(&widget.imp().lines.borrow(), head(&widget))
                    .unwrap()
                    - adjustment.value();
                for _ in 0..8 {
                    widget.edit(|e| e.move_by(Unit::Grapheme, into, false));
                    widget.scroll_to_cursor();
                    let current = paragraph_baseline(&widget.imp().lines.borrow(), head(&widget))
                        .unwrap()
                        - adjustment.value();
                    assert!((current - baseline).abs() < 0.01, "revealing source moved the text");
                    assert!(has_image_source(&widget));
                    widget.edit(|e| e.move_by(Unit::Grapheme, out, false));
                    widget.scroll_to_cursor();
                    assert_eq!(adjustment.value(), 300.0, "repeated reveals drifted");
                    let current = paragraph_baseline(&widget.imp().lines.borrow(), head(&widget))
                        .unwrap()
                        - adjustment.value();
                    assert!((current - baseline).abs() < 0.01, "hiding source moved the text");
                    assert_eq!(head(&widget), at);
                }
            }
        }
    }

    #[test]
    fn hiding_offscreen_source_keeps_the_clicked_caret_at_its_screen_position() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text =
            format!("|![[{id}]]\n{}clicked line\n{}", "line\n".repeat(20), "line\n".repeat(40));
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(160, 240)));
        let scrolled = gtk::ScrolledWindow::builder().child(&widget).build();
        scrolled.measure(gtk::Orientation::Horizontal, -1);
        scrolled.measure(gtk::Orientation::Vertical, 600);
        scrolled.allocate(600, 180, -1, None);
        let height = widget.lay_out(600);
        let adjustment = scrolled.vadjustment();
        adjustment.configure(0.0, 0.0, f64::from(height), 10.0, 150.0, 180.0);
        let at = widget.text().unwrap().find("clicked line").unwrap();
        let (_, before, _) = caret_rect(&widget.imp().lines.borrow(), at).unwrap();
        adjustment.set_value(before - 60.0);
        assert!(adjustment.value() > 240.0);
        assert!(has_image_source(&widget));
        *widget.imp().held.borrow_mut() = Some(0..0);
        widget.edit(|e| e.move_to(at, false));
        assert!(has_image_source(&widget));
        widget.imp().held.take();
        widget.relayout();
        let (_, after, _) = caret_rect(&widget.imp().lines.borrow(), at).unwrap();
        assert!((after - adjustment.value() - 60.0).abs() < 0.01);
        assert!(!has_image_source(&widget));
        widget.scroll_to_cursor();
        assert!((after - adjustment.value() - 60.0).abs() < 0.01);
    }

    #[test]
    fn wide_images_shrink_in_proportion_and_wrap_without_covering_text() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("before ![[{id}]] after\n|end");
        let Some(widget) = widget(&text) else { return };
        let (generation, _) = widget.image_requests(false);
        widget.set_image(generation, id, Some(texture(600, 200)));
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        let rect = image_rect(line, &line.images[0]);
        assert_eq!(rect.width(), 192.0);
        assert_eq!(rect.height(), 64.0);
        assert!(line.layout.line_count() >= 3);
        assert!(rect.y() >= 0.0);
        assert!(rect.x() + rect.width() <= 192.01);
        assert!(f64::from(rect.y() + rect.height()) <= line.height);
        drop(lines);
        widget.set_image(generation, id, None);
        assert_eq!(widget.imp().lines.borrow()[0].images.len(), 0);
        // An old request cannot put a picture into a newly opened note.
        widget.set_editor(Some(Editor::new(Body::detached("other note"))));
        widget.set_image(generation, id, Some(texture(80, 48)));
        assert_eq!(widget.imp().images.borrow().len(), 0);
    }

    #[test]
    fn clipboards_with_text_prefer_it_over_an_image() {
        if adw::init().is_err() {
            return;
        }
        assert!(image_only(gdk::ContentFormats::for_type(gdk::Texture::static_type())));
        assert!(!image_only(gdk::ContentFormats::for_type(glib::Type::STRING)));
        assert!(!image_only(
            gdk::ContentFormats::builder()
                .add_type(gdk::Texture::static_type())
                .add_type(glib::Type::STRING)
                .build()
        ));
    }

    #[test]
    fn a_paste_finishing_after_a_note_switch_leaves_the_new_note_alone() {
        let Some(widget) = widget("before |after") else { return };
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let target = PasteTarget {
            session: widget.imp().session.get(),
            selection: widget.imp().editor.borrow().as_ref().unwrap().edit_target(),
        };
        widget.set_editor(Some(Editor::new(Body::detached("other note"))));
        widget.insert_image(&target, id);
        assert_eq!(widget.text().as_deref(), Some("other note"));
    }

    #[test]
    fn delayed_text_pastes_leave_a_new_note_untouched() {
        let Some(widget) = widget("first note|") else { return };
        let target = PasteTarget {
            session: widget.session(),
            selection: widget.imp().editor.borrow().as_ref().unwrap().edit_target(),
        };
        widget.set_editor(Some(Editor::new(Body::detached("second note"))));
        widget.paste_text(&target, "pasted ");
        assert_eq!(widget.text().as_deref(), Some("second note"));
        assert!(!widget.imp().editor.borrow().as_ref().unwrap().can_undo());
    }

    #[test]
    fn maps_drawn_titles_to_source_and_opens_every_glyph() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("**[[{id}]]** after\n|end");
        let Some(widget) = widget(&text) else { return };
        widget.set_titles(HashMap::from([(id, "**café** and a longer title".to_owned())]));
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        assert_eq!(line.layout.text(), "café and a longer title after");
        let label = line.pieces.iter().find(|p| p.content == Content::Label).unwrap();
        assert_eq!(line.source(3), 2);
        assert_eq!(line.source_after(label.len), 32);
        assert_eq!(line.shown(32), label.len);
        let (x, y) = (line.left, line.y);
        for (index, _) in line.layout.text()[..label.len].char_indices() {
            let cell = line.layout.index_to_pos(i32::try_from(index).unwrap());
            assert_eq!(
                widget.link_at(
                    x + pixels(cell.x()) + pixels(cell.width()) / 2.0,
                    y + pixels(cell.y()) + pixels(cell.height()) / 2.0,
                ),
                Some(LinkTarget::Note(id))
            );
        }
        assert_eq!(widget.link_at(x, y - 2.0), None);
        drop(lines);
        widget.edit(|e| e.move_to(2, false));
        assert!(widget.imp().lines.borrow()[0].layout.text().contains(&id.to_string()));
    }

    #[test]
    fn wrapped_titles_keep_their_source_and_web_links_keep_their_targets() {
        let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
        let text = format!("[[{id}]] [web](https://example.com)\n|end");
        let Some(widget) = widget(&text) else { return };
        let title = "A title long enough to wrap over several visual lines";
        widget.set_titles(HashMap::from([(id, title.to_owned())]));
        widget.lay_out(240);
        let lines = widget.imp().lines.borrow();
        let line = &lines[0];
        assert!(line.layout.line_count() > 1);
        for (index, _) in title.char_indices().filter(|(_, c)| !c.is_whitespace()) {
            let cell = line.layout.index_to_pos(i32::try_from(index).unwrap());
            assert_eq!(
                widget.link_at(
                    line.left + pixels(cell.x()) + pixels(cell.width()) / 2.0,
                    line.y + pixels(cell.y()) + pixels(cell.height()) / 2.0,
                ),
                Some(LinkTarget::Note(id)),
            );
        }
        let web = line.layout.index_to_pos(i32::try_from(title.len() + 1).unwrap());
        assert_eq!(
            widget.link_at(
                line.left + pixels(web.x()) + pixels(web.width()) / 2.0,
                line.y + pixels(web.y()) + pixels(web.height()) / 2.0,
            ),
            Some(LinkTarget::Web("https://example.com".to_owned())),
        );
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
