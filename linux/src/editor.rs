//! The note editor, which styles markdown as it's written and shows the syntax of whatever the
//! cursor touches.

use std::cell::RefCell;
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use sourceview5::prelude::*;
use zib_core::editing::{Drawn, Span, Style, drawings as find_drawings, spans, toggle_checkbox};
use zib_core::lists::{ListKey, list_edit};

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
const HIDDEN: &str = "markup-hidden";
/// Covers the whole text, so GTK repaints every run of text that moves. GTK's GL renderer
/// repaints too little around text that moves or goes, such as syntax as it hides, and leaves faint
/// ghosts of its antialiased edges. A background makes it repaint each run's whole cell. A fully
/// transparent one renders black.
const REPAINT: &str = "markup-repaint";
const QUOTE_MARGIN: &str = "markup-quote-margin";
/// How far a block quote sits inside the editor's left margin.
const QUOTE_INDENT: i32 = 16;
/// Another machine's cursor or selection.
pub const REMOTE: &str = "remote";
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

    let view = sourceview5::View::with_buffer(&buffer);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(16);
    view.set_bottom_margin(24);
    view.set_left_margin(24);
    view.set_right_margin(24);
    view.set_vexpand(true);
    attach_preview(&buffer, Drawings::new(view.upcast_ref(), draw, HIDDEN));
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
    view
}

/// Returns the buffer's whole text, including the syntax that the editor hides.
pub fn text(buffer: &gtk::TextBuffer) -> String {
    let (start, end) = buffer.bounds();
    buffer.text(&start, &end, true).into()
}

/// Wraps the selection in markdown delimiters when the owner types one, rather than replacing it,
/// and keeps the text inside selected, so typing the delimiter again doubles it.
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
                return glib::Propagation::Proceed;
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
    if edit.range.is_empty() && edit.text.is_empty() {
        return true;
    }
    let range = char_range(&body, edit.range.clone());
    let mut changed = body;
    changed.replace_range(edit.range, &edit.text);
    let new_cursor = changed[..edit.cursor].chars().count();
    replace(buffer, range, &edit.text);
    buffer.place_cursor(&buffer.iter_at_offset(offset(new_cursor)));
    true
}

/// Toggles a task box when the owner clicks it.
fn attach_checkbox_clicks(view: &sourceview5::View) {
    let click = gtk::GestureClick::new();
    click.connect_released(glib::clone!(
        #[weak]
        view,
        move |gesture, presses, x, y| {
            if presses != 1 || !view.is_editable() {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let (bx, by) =
                view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let Some(iter) = view.iter_at_location(bx, by) else { return };
            let buffer = view.buffer();
            let body = text(&buffer);
            let byte = byte_at(&body, iter.offset());
            let on_box = spans(&body).iter().any(|span| {
                matches!(span.style, Style::Checkbox(_))
                    && span.range.start <= byte
                    && byte < span.range.end
            });
            if on_box && toggle_checkbox_at(&buffer, iter.offset()) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    ));
    view.add_controller(click);
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
        gtk::TextTag::builder().name(HIDDEN).invisible(true).build(),
        gtk::TextTag::builder()
            .name(REMOTE)
            .background_rgba(&gdk::RGBA::new(1.0, 0.82, 0.5, 0.5))
            .build(),
    ] {
        table.add(&added);
    }
    // A tag added later wins, so syntax would take the color of a heading it sits in.
    for name in HEADING_COLORS {
        table.add(&gtk::TextTag::builder().name(name).build());
    }
    if let Some(syntax) = table.lookup(SYNTAX) {
        syntax.set_priority(table.size() - 1);
    }
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
        Style::ListPrefix => "markup-list-prefix",
        Style::Math => "markup-math",
        Style::Strikethrough => "markup-strikethrough",
        Style::Strong => "markup-strong",
        Style::Underline => "markup-underline",
    }
}

/// Every style the editor applies, for clearing them before a restyle.
const STYLES: [Style; 18] = [
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
    Style::ListPrefix,
    Style::Math,
    Style::Strikethrough,
    Style::Strong,
    Style::Underline,
];

fn attach_preview(buffer: &sourceview5::Buffer, drawings: Rc<Drawings>) {
    let preview = Rc::new(RefCell::new(Preview::default()));
    buffer.connect_changed({
        let (preview, drawings) = (preview.clone(), drawings.clone());
        move |buffer| {
            contain_panic(|| {
                restyle(buffer.upcast_ref(), &mut preview.borrow_mut(), &drawings);
            });
        }
    });
    buffer.connect_mark_set(move |buffer, _, mark| {
        if *mark == buffer.get_insert() || *mark == buffer.selection_bound() {
            contain_panic(|| reveal(buffer.upcast_ref(), &mut preview.borrow_mut(), &drawings));
        }
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
    let names = [CODE_BLOCK_SHADE, HIDDEN, QUOTE_MARGIN, SYNTAX]
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
        for syntax in &span.syntax {
            if !matches!(span.style, Style::Heading(_)) {
                apply(buffer, SYNTAX, preview.chars(syntax));
            }
            if !revealed {
                apply(buffer, HIDDEN, preview.chars(syntax));
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
        for syntax in &preview.spans[i].syntax {
            let range = preview.chars(syntax);
            let start = buffer.iter_at_offset(offset(range.start));
            let end = buffer.iter_at_offset(offset(range.end));
            if revealed {
                buffer.remove_tag_by_name(HIDDEN, &start, &end);
            } else {
                buffer.apply_tag_by_name(HIDDEN, &start, &end);
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
