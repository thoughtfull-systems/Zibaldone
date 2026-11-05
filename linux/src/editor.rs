//! The note editor, which styles markdown as it's written and shows the syntax of whatever the
//! cursor touches.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib, graphene, pango};
use sourceview5::prelude::*;
use zib_core::editing::{
    Drawn, Span, Style, complete_block_opening, drawings as find_drawings, fence_selection, spans,
    table_cell_in_body, toggle_checkbox,
};
use zib_core::lists::{Edit, ListKey, list_edit};

use crate::drawings::{Draw, Drawings};

const CODE_BLOCK_SHADE: &str = "markup-code-block-shade";
/// The colors of headings' text, kept apart from the heading tags so a heading's markers keep the
/// editor's text color.
const HEADING_COLORS: [&str; 4] = [
    "markup-heading-color-1",
    "markup-heading-color-2",
    "markup-heading-color-3",
    "markup-heading-color-4",
];
/// Leaves list markers and task boxes unseen, but keeping their room, for the view to draw bullets
/// and boxes over. GTK draws a fully transparent color as black.
const DRAWN_OVER: &str = "markup-drawn-over";
const HIDDEN: &str = "markup-hidden";
/// Covers the whole text, so GTK repaints every run of text that moves. GTK's GL renderer
/// repaints too little around text that moves or goes, such as syntax as it hides, and leaves faint
/// ghosts of its antialiased edges. A background makes it repaint each run's whole cell. A fully
/// transparent one renders black.
const REPAINT: &str = "markup-repaint";
const QUOTE_MARGIN: &str = "markup-quote-margin";
/// How far a block quote sits inside the editor's left margin.
const QUOTE_INDENT: i32 = 16;
const SYNTAX: &str = "markup-syntax";

/// Creates the editor view. Its colors follow the system's light or dark preference.
pub fn new_view(draw: Draw) -> sourceview5::View {
    let buffer = sourceview5::Buffer::new(None);
    buffer.set_highlight_matching_brackets(false);
    let style_manager = adw::StyleManager::default();
    let apply_scheme = {
        let buffer = buffer.clone();
        move |style_manager: &adw::StyleManager| {
            let dark = style_manager.is_dark();
            let name = if dark { "Adwaita-dark" } else { "Adwaita" };
            buffer
                .set_style_scheme(sourceview5::StyleSchemeManager::default().scheme(name).as_ref());
            color_headings(&buffer.tag_table(), dark);
        }
    };
    let table = buffer.tag_table();
    add_tags(&table);
    apply_scheme(&style_manager);
    style_manager.connect_dark_notify(apply_scheme);

    let view = crate::view::new(&buffer, CODE_BLOCK_SHADE, DRAWN_OVER);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(16);
    view.set_bottom_margin(24);
    view.set_left_margin(24);
    view.set_right_margin(24);
    view.set_vexpand(true);
    attach_preview(&view, Drawings::new(view.upcast_ref(), draw, HIDDEN));
    // A tag's margin replaces the view's margin rather than adding to it, so the quote margin
    // follows the view's.
    let indent_quotes = |view: &sourceview5::View| {
        if let Some(tag) = view.buffer().tag_table().lookup(QUOTE_MARGIN) {
            tag.set_left_margin(view.left_margin() + QUOTE_INDENT);
        }
    };
    indent_quotes(&view);
    view.connect_left_margin_notify(indent_quotes);
    attach_list_keys(&view);
    attach_wrap_keys(&view);
    attach_checkbox_clicks(&view);
    attach_primary_selection(&view);
    view
}

/// Offers the selected text as the primary selection, which a middle click pastes, without tying
/// the selection to it. GTK's text buffer drops its selection whenever another window or program
/// takes the primary selection, so with several windows open, selecting in one would deselect the
/// rest.
fn attach_primary_selection(view: &sourceview5::View) {
    view.connect_realize(|view| {
        view.buffer().remove_selection_clipboard(&view.primary_clipboard());
    });
    let weak = view.downgrade();
    view.buffer().connect_mark_set(move |buffer, _, mark| {
        let Some(view) = weak.upgrade() else { return };
        if mark != &buffer.get_insert() && mark != &buffer.selection_bound() {
            return;
        }
        if let Some((start, end)) = buffer.selection_bounds() {
            view.primary_clipboard().set_text(&buffer.text(&start, &end, false));
        }
    });
}

/// Returns the buffer's whole text, including the syntax that the editor hides.
pub fn text(buffer: &gtk::TextBuffer) -> String {
    let (start, end) = buffer.bounds();
    buffer.text(&start, &end, true).into()
}

/// Wraps the selection in markdown delimiters when the owner types one, rather than replacing it,
/// and keeps the text inside selected, so typing the delimiter again doubles it. Around lines of
/// text, a third backtick or a second `$` makes a code block or display math instead. Without a
/// selection, the same keys at the start of a line open the block, and close it below.
fn attach_wrap_keys(view: &sourceview5::View) {
    // The capture phase sees the key before the view replaces the selection with it.
    let keys = gtk::EventControllerKey::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        view,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
            let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
            if !(modifiers - gdk::ModifierType::SHIFT_MASK).is_empty() || !view.is_editable() {
                return glib::Propagation::Proceed;
            }
            let (open, close) = match key.to_unicode() {
                Some(delimiter @ ('*' | '_' | '`' | '~' | '=' | '$')) => (delimiter, delimiter),
                Some('[') => ('[', ']'),
                _ => return glib::Propagation::Proceed,
            };
            let buffer = view.buffer();
            let Some((mut start, mut end)) = buffer.selection_bounds() else {
                // A third backtick or a second `$` at the start of a line opens a block.
                let body = text(&buffer);
                let cursor = byte_at(&body, buffer.cursor_position());
                return match complete_block_opening(&body, cursor, open) {
                    Some(edit) => {
                        apply_edit(&buffer, &body, edit, None);
                        glib::Propagation::Stop
                    }
                    None => glib::Propagation::Proceed,
                };
            };
            // Line breaks at either end of the selection stay outside the delimiters, which can't
            // pair across lines.
            while start < end && start.char() == '\n' {
                start.forward_char();
            }
            while start < end {
                let mut last = end;
                last.backward_char();
                if last.char() != '\n' {
                    break;
                }
                end = last;
            }
            if start == end {
                return glib::Propagation::Proceed;
            }
            // A third backtick or a second `$` around text that fills its lines makes a block.
            let body = text(&buffer);
            let selection = byte_at(&body, start.offset())..byte_at(&body, end.offset());
            if let Some((edit, selected)) = fence_selection(&body, selection, open) {
                apply_edit(&buffer, &body, edit, Some(selected));
                return glib::Propagation::Stop;
            }
            let (start_offset, end_offset) = (start.offset(), end.offset());
            let selected = buffer.text(&start, &end, true);
            buffer.begin_user_action();
            buffer.delete(&mut start, &mut end);
            buffer.insert(&mut start, &format!("{open}{selected}{close}"));
            buffer.select_range(
                &buffer.iter_at_offset(start_offset + 1),
                &buffer.iter_at_offset(end_offset + 1),
            );
            buffer.end_user_action();
            glib::Propagation::Stop
        }
    ));
    view.add_controller(keys);
}

/// Gives Enter, Shift+Enter, Tab, and Shift+Tab their outliner meanings inside list items.
fn attach_list_keys(view: &sourceview5::View) {
    // The capture phase sees the keys before the view inserts a line break or a tab.
    let keys = gtk::EventControllerKey::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        view,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
            let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
            let shift = match modifiers {
                gdk::ModifierType::SHIFT_MASK => true,
                modifiers if modifiers.is_empty() => false,
                _ => return glib::Propagation::Proceed,
            };
            let list_key = match key {
                gdk::Key::Return | gdk::Key::KP_Enter if shift => ListKey::ShiftEnter,
                gdk::Key::Return | gdk::Key::KP_Enter => ListKey::Enter,
                gdk::Key::ISO_Left_Tab | gdk::Key::Tab if shift => ListKey::ShiftTab,
                gdk::Key::Tab => ListKey::Tab,
                _ => return glib::Propagation::Proceed,
            };
            let buffer = view.buffer();
            if view.is_editable() && !buffer.has_selection() && apply_list_edit(&buffer, list_key) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    ));
    view.add_controller(keys);
}

/// Makes the edit that `key` makes at the cursor, as one undoable step. Returns whether the
/// cursor was in a list item.
fn apply_list_edit(buffer: &gtk::TextBuffer, key: ListKey) -> bool {
    let body = text(buffer);
    let cursor = byte_at(&body, buffer.cursor_position());
    let Some(edit) = list_edit(&body, cursor, key) else {
        return false;
    };
    if !edit.range.is_empty() || !edit.text.is_empty() {
        apply_edit(buffer, &body, edit, None);
    }
    true
}

/// Makes `edit` to `body`, the buffer's text, as one undoable step. Then it selects `selected`, a
/// byte range of the changed text, or puts the cursor where the edit says.
fn apply_edit(buffer: &gtk::TextBuffer, body: &str, edit: Edit, selected: Option<Range<usize>>) {
    let range = char_range(body, edit.range.clone());
    let mut changed = body.to_owned();
    changed.replace_range(edit.range, &edit.text);
    let chars = |byte: usize| buffer.iter_at_offset(offset(changed[..byte].chars().count()));
    replace(buffer, range, &edit.text);
    match selected {
        Some(selected) => buffer.select_range(&chars(selected.end), &chars(selected.start)),
        None => buffer.place_cursor(&chars(edit.cursor)),
    }
}

/// Toggles a task box that the view draws when the owner clicks it, without moving the cursor, and
/// shows the pointer as an arrow over it, as over a button. A box showing its source edits as text.
fn attach_checkbox_clicks(view: &sourceview5::View) {
    let box_at = |view: &sourceview5::View, x: f64, y: f64| {
        #[allow(clippy::cast_possible_truncation)]
        let (x, y) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        crate::view::box_at(view.upcast_ref(), x, y)
    };
    // The capture phase claims the press before the view's own gestures move the cursor.
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_PRIMARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    let claimed = Rc::new(Cell::new(false));
    click.connect_pressed(glib::clone!(
        #[weak]
        view,
        #[strong]
        claimed,
        move |gesture, _, x, y| {
            claimed.set(view.is_editable() && box_at(&view, x, y).is_some());
            if claimed.get() {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    ));
    click.connect_released(glib::clone!(
        #[weak]
        view,
        move |_, _, x, y| {
            if let Some(at) = box_at(&view, x, y).filter(|_| claimed.replace(false)) {
                toggle_checkbox_at(&view.buffer(), at.offset() + 1);
            }
        }
    ));
    view.add_controller(click);
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion(glib::clone!(
        #[weak]
        view,
        move |_, x, y| {
            let name = if box_at(&view, x, y).is_some() { "default" } else { "text" };
            if view.cursor().and_then(|cursor| cursor.name()).as_deref() != Some(name) {
                view.set_cursor_from_name(Some(name));
            }
        }
    ));
    view.add_controller(motion);
}

/// Checks or unchecks the list item at the character `offset`, or gives it a box if it has none,
/// as one undoable edit. Returns whether there was an item to change.
pub fn toggle_checkbox_at(buffer: &gtk::TextBuffer, at: i32) -> bool {
    let body = text(buffer);
    let Some((mark, replacement)) = toggle_checkbox(&body, byte_at(&body, at)) else {
        return false;
    };
    replace(buffer, char_range(&body, mark), replacement);
    true
}

/// What the editor last applied to a buffer.
#[derive(Default)]
struct Preview {
    /// Character offsets of each byte boundary of the text, for converting spans to offsets.
    byte_chars: Vec<usize>,
    /// Byte offsets of each character boundary, for converting the cursor to bytes.
    char_bytes: Vec<usize>,
    /// Whether each span's syntax is showing.
    revealed: Vec<bool>,
    spans: Vec<Span>,
    /// The math, diagrams, and embeds drawn over their source, and whether each source shows.
    drawn: Vec<Drawn>,
    drawn_revealed: Vec<bool>,
}

impl Preview {
    fn chars(&self, bytes: &Range<usize>) -> Range<usize> {
        self.byte_chars[bytes.start]..self.byte_chars[bytes.end]
    }

    /// The cursor or selection as a byte range.
    fn selection(&self, buffer: &gtk::TextBuffer) -> Range<usize> {
        let (start, end) = buffer.selection_bounds().unwrap_or_else(|| {
            let cursor = buffer.iter_at_mark(&buffer.get_insert());
            (cursor, cursor)
        });
        let byte = |offset: i32| {
            let offset = usize::try_from(offset).unwrap_or(0);
            self.char_bytes[offset.min(self.char_bytes.len() - 1)]
        };
        byte(start.offset())..byte(end.offset())
    }

    /// What a move by `count` `step`s from the character offset `cursor` needs to correct where
    /// GTK puts the cursor.
    fn before_move(&self, cursor: usize, step: gtk::MovementStep, count: i32) -> BeforeMove {
        let hidden = self
            .drawn
            .iter()
            .zip(&self.drawn_revealed)
            .filter(|(drawn, revealed)| drawn.block && !**revealed)
            .map(|(drawn, _)| self.chars(&drawn.range))
            .collect();
        let positions = matches!(
            step,
            gtk::MovementStep::LogicalPositions | gtk::MovementStep::VisualPositions
        );
        let shown = || self.spans.iter().zip(&self.revealed).filter(|(_, revealed)| !**revealed);
        let syntax = if count > 0 && positions {
            shown()
                // Math hides behind its drawing, which a move from either side lands outside, and
                // bullets and boxes keep their room, so a move steps through them.
                .filter(|(span, _)| span.style != Style::Math && !drawn_over(span.style))
                .flat_map(|(span, _)| span.syntax.iter().map(|syntax| self.chars(syntax)))
                .collect()
        } else {
            Vec::new()
        };
        let boxes = shown()
            .filter(|(span, _)| matches!(span.style, Style::Checkbox(_)))
            .filter_map(|(span, _)| {
                Some((self.chars(&span.range), self.chars(span.syntax.get(1)?)))
            })
            .collect();
        let left = count < 0 && positions;
        BeforeMove { cursor, hidden, syntax, boxes, left }
    }
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

fn color_headings(table: &gtk::TextTagTable, dark: bool) {
    for (level, name) in (1..).zip(HEADING_COLORS) {
        if let Some(tag) = table.lookup(name) {
            tag.set_foreground(Some(heading_color(level, dark)));
        }
    }
}

fn add_tags(table: &gtk::TextTagTable) {
    // Added first, so every other background shows over it.
    table.add(
        &gtk::TextTag::builder()
            .name(REPAINT)
            .background_rgba(&gdk::RGBA::new(0.5, 0.5, 0.5, 0.01))
            .build(),
    );
    let dim = gdk::RGBA::new(0.5, 0.5, 0.5, 0.9);
    let shade = gdk::RGBA::new(0.5, 0.5, 0.5, 0.15);
    let link = gdk::RGBA::new(0.21, 0.52, 0.89, 1.0);
    let heading = |name: &str, scale: f64| {
        gtk::TextTag::builder().name(name).weight(700).scale(scale).pixels_above_lines(6).build()
    };
    let tag = style_tag;
    for added in [
        gtk::TextTag::builder().name(SYNTAX).foreground_rgba(&dim).build(),
        heading(tag(Style::Heading(1)), 1.6),
        heading(tag(Style::Heading(2)), 1.4),
        heading(tag(Style::Heading(3)), 1.2),
        heading(tag(Style::Heading(4)), 1.0),
        gtk::TextTag::builder()
            .name(tag(Style::BlockQuote))
            .style(pango::Style::Italic)
            .foreground_rgba(&dim)
            .build(),
        gtk::TextTag::builder().name(QUOTE_MARGIN).build(),
        gtk::TextTag::builder()
            .name(tag(Style::Code))
            .family("monospace")
            .background_rgba(&shade)
            .build(),
        gtk::TextTag::builder().name(tag(Style::CodeBlock)).family("monospace").build(),
        gtk::TextTag::builder().name(CODE_BLOCK_SHADE).paragraph_background_rgba(&shade).build(),
        gtk::TextTag::builder().name(tag(Style::Emphasis)).style(pango::Style::Italic).build(),
        gtk::TextTag::builder().name(tag(Style::Strikethrough)).strikethrough(true).build(),
        gtk::TextTag::builder().name(tag(Style::Strong)).weight(700).build(),
        gtk::TextTag::builder()
            .name(tag(Style::Highlight))
            .background_rgba(&gdk::RGBA::new(0.96, 0.83, 0.18, 0.45))
            .build(),
        gtk::TextTag::builder()
            .name(tag(Style::Underline))
            .underline(pango::Underline::Single)
            .build(),
        gtk::TextTag::builder()
            .name(tag(Style::Link))
            .underline(pango::Underline::Single)
            .foreground_rgba(&link)
            .build(),
        gtk::TextTag::builder()
            .name(tag(Style::Math))
            .family("serif")
            .style(pango::Style::Italic)
            .build(),
        gtk::TextTag::builder().name(tag(Style::Completed)).foreground_rgba(&dim).build(),
        gtk::TextTag::builder()
            .name(tag(Style::Checkbox(false)))
            .family("monospace")
            .weight(700)
            .build(),
        gtk::TextTag::builder()
            .name(tag(Style::Checkbox(true)))
            .family("monospace")
            .weight(700)
            .foreground_rgba(&dim)
            .build(),
        gtk::TextTag::builder().name(tag(Style::ListPrefix)).family("monospace").build(),
        gtk::TextTag::builder().name(tag(Style::ListMarker)).build(),
        gtk::TextTag::builder().name(HIDDEN).invisible(true).build(),
    ] {
        table.add(&added);
    }
    for added in crate::remote::tags() {
        table.add(&added);
    }
    // A tag added later wins, so syntax would take the color of a heading it sits in.
    for name in HEADING_COLORS {
        table.add(&gtk::TextTag::builder().name(name).build());
    }
    if let Some(syntax) = table.lookup(SYNTAX) {
        syntax.set_priority(table.size() - 1);
    }
    // Above every color, the syntax's included.
    table.add(
        &gtk::TextTag::builder()
            .name(DRAWN_OVER)
            .foreground_rgba(&gdk::RGBA::new(0.5, 0.5, 0.5, 0.004))
            .build(),
    );
}

/// The tag that styles a span.
fn style_tag(style: Style) -> &'static str {
    match style {
        Style::BlockQuote => "markup-quote",
        Style::Checkbox(false) => "markup-checkbox",
        Style::Checkbox(true) => "markup-checkbox-checked",
        Style::Code => "markup-code",
        Style::CodeBlock => "markup-code-block",
        Style::Completed => "markup-completed",
        Style::Emphasis => "markup-emphasis",
        Style::Heading(1) => "markup-heading-1",
        Style::Heading(2) => "markup-heading-2",
        Style::Heading(3) => "markup-heading-3",
        Style::Heading(_) => "markup-heading-4",
        Style::Highlight => "markup-highlight",
        Style::Link => "markup-link",
        Style::ListMarker => "markup-list-marker",
        Style::ListPrefix => "markup-list-prefix",
        Style::Math => "markup-math",
        Style::Strikethrough => "markup-strikethrough",
        Style::Strong => "markup-strong",
        Style::Underline => "markup-underline",
    }
}

/// The tag that covers the `index`th piece of a span's syntax while the cursor is away: one that
/// hides it, or for a list marker and the first piece of a task box, one that leaves it unseen for
/// the view to draw over.
fn cover(style: Style, index: usize) -> &'static str {
    if drawn_over(style) && index == 0 { DRAWN_OVER } else { HIDDEN }
}

fn drawn_over(style: Style) -> bool {
    matches!(style, Style::Checkbox(_) | Style::ListMarker)
}

/// Every style the editor applies, for clearing them before a restyle.
const STYLES: [Style; 19] = [
    Style::BlockQuote,
    Style::Checkbox(false),
    Style::Checkbox(true),
    Style::Code,
    Style::CodeBlock,
    Style::Completed,
    Style::Emphasis,
    Style::Heading(1),
    Style::Heading(2),
    Style::Heading(3),
    Style::Heading(4),
    Style::Highlight,
    Style::Link,
    Style::ListMarker,
    Style::ListPrefix,
    Style::Math,
    Style::Strikethrough,
    Style::Strong,
    Style::Underline,
];

/// Restyles the buffer as its text changes, and shows or hides syntax as the cursor moves. While a
/// mouse button is down in `view`, syntax stays as it is until the button comes up: text that
/// moved under the pointer between press and release would turn a click into a selection.
fn attach_preview(view: &sourceview5::View, drawings: Rc<Drawings>) {
    let buffer = view.buffer();
    let preview = Rc::new(RefCell::new(Preview::default()));
    let pressed = Rc::new(Cell::new(false));
    attach_cursor_moves(view, &preview, &drawings);
    buffer.connect_changed({
        let (preview, drawings) = (preview.clone(), drawings.clone());
        move |buffer| {
            contain_panic(|| {
                restyle(buffer, &mut preview.borrow_mut(), &drawings);
            });
        }
    });
    buffer.connect_mark_set({
        let (preview, drawings, pressed) = (preview.clone(), drawings.clone(), pressed.clone());
        move |buffer, _, mark| {
            let moved = *mark == buffer.get_insert() || *mark == buffer.selection_bound();
            if moved && !pressed.get() {
                contain_panic(|| reveal(buffer, &mut preview.borrow_mut(), &drawings));
            }
        }
    });
    // A legacy controller sees every press and release without taking part in the view's own
    // gestures, which would otherwise cancel it.
    let buttons = gtk::EventControllerLegacy::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    buttons.connect_event(glib::clone!(
        #[weak]
        buffer,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |controller, event| {
            match event.event_type() {
                gdk::EventType::ButtonPress => pressed.set(true),
                gdk::EventType::ButtonRelease if pressed.replace(false) => {
                    let point = controller.widget().and_then(|view| view_point(&view, event));
                    place_click(&buffer, &preview.borrow(), &drawings, point.as_ref());
                    contain_panic(|| reveal(&buffer, &mut preview.borrow_mut(), &drawings));
                }
                _ => {}
            }
            glib::Propagation::Proceed
        }
    ));
    view.add_controller(buttons);
}

/// Where a pointer event happened, in `view`'s coordinates. Events carry their position on the
/// window's surface.
fn view_point(view: &gtk::Widget, event: &gdk::Event) -> Option<graphene::Point> {
    let native = view.native()?;
    let (x, y) = event.position()?;
    let (left, top) = native.surface_transform();
    #[allow(clippy::cast_possible_truncation)]
    let point = graphene::Point::new((x - left) as f32, (y - top) as f32);
    native.compute_point(view, &point)
}

/// Puts a cursor that a click left at hidden syntax beside the shown character the owner clicked.
/// GTK finds that character, but then counts the side of it the click fell on in the buffer's
/// characters, hidden ones included. A click before the first character after hidden syntax lands
/// at the start of the syntax, and a click after that character lands one character into the
/// syntax. Once the syntax shows, the cursor would sit before or inside it. A click on a drawing
/// lands in the source it covers, and goes to the start of that source, or for a block, such as
/// display math or a diagram, to the start of its second line, just inside its opening `$$` or
/// fence. A click on a block lands on its last line, where its room is, so at the end of its range.
fn place_click(
    buffer: &gtk::TextBuffer,
    preview: &Preview,
    drawings: &Drawings,
    point: Option<&graphene::Point>,
) {
    let Some(hidden) = buffer.tag_table().lookup(HIDDEN) else { return };
    if buffer.has_selection() {
        return;
    }
    let at = buffer.iter_at_mark(&buffer.get_insert());
    // The drawings are stale while the text changes before the buffer reports it.
    if preview.char_bytes.len() == usize::try_from(buffer.char_count()).unwrap_or(0) + 1 {
        let clicked = usize::try_from(at.offset()).unwrap_or(0);
        let drawn = preview.drawn.iter().zip(&preview.drawn_revealed).enumerate();
        for (index, (drawn, _)) in drawn.filter(|(_, (_, revealed))| !**revealed) {
            let chars = preview.chars(&drawn.range);
            let on = if drawn.block { chars.end >= clicked } else { chars.end > clicked };
            if chars.start <= clicked && on {
                // A click on a table's cell selects the cell's source, to type over it.
                let cell = point.and_then(|point| drawings.table_cell_at(index, point));
                let source = cell.and_then(|(row, column, nested)| {
                    table_cell_in_body(&text(buffer), drawn.range.clone(), row, column, &nested)
                });
                if let Some(source) = source {
                    let at = |byte: usize| buffer.iter_at_offset(offset(preview.byte_chars[byte]));
                    buffer.select_range(&at(source.end), &at(source.start));
                    return;
                }
                let mut start = buffer.iter_at_offset(offset(chars.start));
                let mut inside = start;
                if drawn.block && inside.forward_line() && inside.offset() < offset(chars.end) {
                    start = inside;
                }
                buffer.place_cursor(&start);
                return;
            }
        }
    }
    if let Some(placed) = clicked_place(&at, &hidden) {
        buffer.place_cursor(&placed);
    }
}

/// Where a click that GTK placed at `at` belongs, when that differs.
fn clicked_place(at: &gtk::TextIter, hidden: &gtk::TextTag) -> Option<gtk::TextIter> {
    if !at.has_tag(hidden) {
        return None;
    }
    let mut before = *at;
    let starts_run = !before.backward_char() || !before.has_tag(hidden);
    // Syntax at the start of a line or after white space opens a span, such as a heading's `# `
    // or the `**` before a strong word. Syntax after a word closes one, and a click at the end of
    // the word belongs before it.
    if starts_run && before != *at && !before.char().is_whitespace() {
        return None;
    }
    let mut placed = *at;
    while placed.has_tag(hidden) && !placed.is_end() && placed.char() != '\n' {
        placed.forward_char();
    }
    if !starts_run && !placed.is_end() && placed.char() != '\n' {
        placed.forward_char();
    }
    Some(placed)
}

/// Where the cursor was before a move, the character ranges of the blocks that were hidden, and,
/// for a move right, those of the hidden syntax. Then the character ranges of the task boxes that
/// take their markers' places, with those of each one's hidden source, and whether the move is
/// left.
struct BeforeMove {
    cursor: usize,
    hidden: Vec<Range<usize>>,
    syntax: Vec<Range<usize>>,
    boxes: Vec<(Range<usize>, Range<usize>)>,
    left: bool,
}

/// Corrects where GTK puts the cursor when a key moves it into hidden source.
///
/// A key that moves the cursor into a hidden block, such as a table or a diagram, from above puts
/// it at the block's start. GTK skips hidden lines, so Down would land on the block's last line,
/// the one that stays to hold the drawing's room. A move that extends the selection shows a block
/// it heads into first.
///
/// GTK puts the cursor at the start of the hidden syntax it moves onto, which is inside a span's
/// closing syntax but outside its opening syntax. A move right onto hidden syntax goes past it,
/// so the cursor lands inside the span from either side, as a click does. Math is the exception,
/// and the cursor lands outside its drawing from either side.
///
/// A move left onto the hidden box of a task that takes its marker's place lands before the item's
/// text, with the box still drawn, rather than past the box, where GTK puts it. The next move left
/// lands just after the box's `]`, which shows its source. Any other move onto the hidden box, such
/// as Up or Down from the start of another item's text, lands before the text too.
fn attach_cursor_moves(
    view: &sourceview5::View,
    preview: &Rc<RefCell<Preview>>,
    drawings: &Rc<Drawings>,
) {
    let before: Rc<RefCell<Option<BeforeMove>>> = Rc::default();
    view.connect_move_cursor({
        let (before, preview, drawings) = (before.clone(), preview.clone(), drawings.clone());
        move |view, step, count, extend| {
            let mut preview = preview.borrow_mut();
            let buffer = view.buffer();
            let fresh =
                preview.char_bytes.len() == usize::try_from(buffer.char_count()).unwrap_or(0) + 1;
            if !fresh {
                return;
            }
            let cursor = buffer.iter_at_mark(&buffer.get_insert());
            if extend {
                // A selection that grows into a hidden block shows the block first, so it grows
                // through the source a character or a line at a time, rather than across all the
                // hidden lines GTK skips.
                let (mut near, from) = (cursor, usize::try_from(cursor.offset()).unwrap_or(0));
                if count > 0 {
                    near.forward_line();
                    near.forward_to_line_end();
                } else {
                    near.backward_line();
                }
                let near = usize::try_from(near.offset()).unwrap_or(0);
                let mut ahead = Vec::new();
                for index in 0..preview.drawn.len() {
                    let chars = preview.chars(&preview.drawn[index].range);
                    let heading = if count > 0 {
                        from <= chars.start && chars.start <= near
                    } else {
                        near <= chars.end && chars.end <= from
                    };
                    if heading && !preview.drawn_revealed[index] {
                        preview.drawn_revealed[index] = true;
                        ahead.push(index);
                    }
                }
                // Showing a block can move the cursor, which reads the preview again.
                drop(preview);
                for index in ahead {
                    drawings.reveal(&buffer, index, true);
                }
                return;
            }
            let cursor = usize::try_from(cursor.offset()).unwrap_or(0);
            *before.borrow_mut() = Some(preview.before_move(cursor, step, count));
        }
    });
    view.connect_local("move-cursor", true, move |values| {
        let BeforeMove { cursor: from, hidden, syntax, boxes, left } =
            before.borrow_mut().take()?;
        let view = values.first()?.get::<sourceview5::View>().ok()?;
        let buffer = view.buffer();
        let mut cursor = usize::try_from(buffer.cursor_position()).unwrap_or(0);
        let onto_box = boxes.iter().find(|(chars, source)| {
            let start = if left { chars.start } else { source.start };
            start <= cursor && cursor < source.end && (!left || source.end <= from)
        });
        if let Some((chars, source)) = onto_box {
            let to = if left && from == source.end { chars.end } else { source.end };
            buffer.place_cursor(&buffer.iter_at_offset(offset(to)));
            return None;
        }
        let entered = hidden
            .iter()
            .find(|chars| from < chars.start && chars.start <= cursor && cursor <= chars.end);
        if let Some(chars) = entered {
            buffer.place_cursor(&buffer.iter_at_offset(offset(chars.start)));
            return None;
        }
        // Syntax can follow syntax, as `*` follows `**` in `***both***`.
        let landed = cursor;
        while let Some(chars) = syntax.iter().find(|chars| from < cursor && chars.start == cursor) {
            cursor = chars.end;
        }
        if cursor != landed {
            buffer.place_cursor(&buffer.iter_at_offset(offset(cursor)));
        }
        None
    });
}

/// Runs `f`, and stops a panic in it from going further. A panic can't unwind through a GTK signal
/// handler, so it would abort the app and lose edits not yet saved, all over a missing style. The
/// panic hook still reports it, and the next change restyles the buffer from scratch.
fn contain_panic(f: impl FnOnce()) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(f));
}

/// Styles every span from scratch, after the text changes.
fn restyle(buffer: &gtk::TextBuffer, preview: &mut Preview, drawings: &Drawings) {
    let (start, end) = buffer.bounds();
    let names = [CODE_BLOCK_SHADE, DRAWN_OVER, HIDDEN, QUOTE_MARGIN, SYNTAX]
        .into_iter()
        .chain(HEADING_COLORS)
        .chain(STYLES.into_iter().map(style_tag));
    for name in names {
        buffer.remove_tag_by_name(name, &start, &end);
    }
    buffer.apply_tag_by_name(REPAINT, &start, &end);
    let body = text(buffer);
    let mut byte_chars = vec![0; body.len() + 1];
    let mut char_bytes = Vec::with_capacity(body.len() + 1);
    for (char_index, (byte, c)) in body.char_indices().enumerate() {
        char_bytes.push(byte);
        for offset in 0..c.len_utf8() {
            byte_chars[byte + offset] = char_index;
        }
    }
    char_bytes.push(body.len());
    byte_chars[body.len()] = char_bytes.len() - 1;
    preview.byte_chars = byte_chars;
    preview.char_bytes = char_bytes;
    preview.spans = spans(&body);
    let selection = preview.selection(buffer);
    preview.revealed = preview.spans.iter().map(|span| span.touches(&selection)).collect();
    for (span, &revealed) in preview.spans.iter().zip(&preview.revealed) {
        apply(buffer, style_tag(span.style), preview.chars(&span.range));
        // GTK takes a line's paragraph properties, such as its margin and background, from the
        // tags at the line's first character. A block nested in a list item starts partway into
        // its first line, after the item's marker or indentation.
        let line_start = body[..span.range.start].rfind('\n').map_or(0, |i| i + 1);
        match span.style {
            Style::CodeBlock => {
                apply(buffer, CODE_BLOCK_SHADE, preview.chars(&(line_start..span.range.end)));
            }
            Style::BlockQuote if line_start == span.range.start => {
                apply(buffer, QUOTE_MARGIN, preview.chars(&span.range));
            }
            _ => {}
        }
        if let Style::Heading(level) = span.style {
            let color = HEADING_COLORS[usize::from(level.clamp(1, 4)) - 1];
            let mut start = span.range.start;
            for syntax in span.syntax.iter().chain([&(span.range.end..span.range.end)]) {
                if start < syntax.start {
                    apply(buffer, color, preview.chars(&(start..syntax.start)));
                }
                start = start.max(syntax.end);
            }
        }
        for (index, syntax) in span.syntax.iter().enumerate() {
            if !matches!(span.style, Style::Heading(_)) && !drawn_over(span.style) {
                apply(buffer, SYNTAX, preview.chars(syntax));
            }
            if !revealed {
                apply(buffer, cover(span.style, index), preview.chars(syntax));
            }
        }
    }
    // After the spans, whose hidden syntax a drawing's source can hold.
    preview.drawn = find_drawings(&body);
    preview.drawn_revealed = preview.drawn.iter().map(|drawn| drawn.touches(&selection)).collect();
    let found = preview
        .drawn
        .iter()
        .zip(&preview.drawn_revealed)
        .map(|(drawn, &revealed)| (drawn.clone(), preview.chars(&drawn.range), revealed))
        .collect();
    drawings.update(buffer, found);
}

/// Shows the syntax of spans the cursor now touches, and hides it for spans it left, and does the
/// same for the source of drawings.
fn reveal(buffer: &gtk::TextBuffer, preview: &mut Preview, drawings: &Drawings) {
    // Marks can move before the buffer reports a change, while the spans are stale.
    if preview.char_bytes.len() != usize::try_from(buffer.char_count()).unwrap_or(0) + 1 {
        return;
    }
    let selection = preview.selection(buffer);
    for i in 0..preview.spans.len() {
        let revealed = preview.spans[i].touches(&selection);
        if revealed == preview.revealed[i] {
            continue;
        }
        preview.revealed[i] = revealed;
        let style = preview.spans[i].style;
        for (index, syntax) in preview.spans[i].syntax.iter().enumerate() {
            let cover = cover(style, index);
            let range = preview.chars(syntax);
            let start = buffer.iter_at_offset(offset(range.start));
            let end = buffer.iter_at_offset(offset(range.end));
            if revealed {
                buffer.remove_tag_by_name(cover, &start, &end);
            } else {
                buffer.apply_tag_by_name(cover, &start, &end);
            }
        }
    }
    for i in 0..preview.drawn.len() {
        let revealed = preview.drawn[i].touches(&selection);
        if revealed != preview.drawn_revealed[i] {
            preview.drawn_revealed[i] = revealed;
            drawings.reveal(buffer, i, revealed);
        }
    }
}

fn apply(buffer: &gtk::TextBuffer, name: &str, range: Range<usize>) {
    let start = buffer.iter_at_offset(offset(range.start));
    let end = buffer.iter_at_offset(offset(range.end));
    buffer.apply_tag_by_name(name, &start, &end);
}

/// Replaces a character range of the buffer, as one undoable step.
fn replace(buffer: &gtk::TextBuffer, range: Range<usize>, text: &str) {
    let mut start = buffer.iter_at_offset(offset(range.start));
    let mut end = buffer.iter_at_offset(offset(range.end));
    buffer.begin_user_action();
    buffer.delete(&mut start, &mut end);
    buffer.insert(&mut start, text);
    buffer.end_user_action();
}

fn offset(chars: usize) -> i32 {
    i32::try_from(chars).unwrap_or(i32::MAX)
}

/// The byte offset in `text` of the character offset `at`.
pub fn byte_at(text: &str, at: i32) -> usize {
    let at = usize::try_from(at).unwrap_or(0);
    text.char_indices().nth(at).map_or(text.len(), |(byte, _)| byte)
}

/// Converts a byte range in `text` to the character range that `GtkTextBuffer` offsets use.
fn char_range(text: &str, bytes: Range<usize>) -> Range<usize> {
    let start = text[..bytes.start].chars().count();
    start..start + text[bytes].chars().count()
}
