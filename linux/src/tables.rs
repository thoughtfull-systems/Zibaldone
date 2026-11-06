//! List tables, drawn in the editor over their source as a grid of cells, as the note bodies
//! specification's list tables define them. Each cell's markdown renders from the core's markup
//! tree, as decision ZIB0009 describes. Math and diagrams inside a cell draw as they do in the
//! editor, except math inside a line of text, which shows its source.

use std::collections::HashMap;
use std::fmt::Write as _;

use adw::prelude::*;
use gtk::{gdk, glib, graphene};
use zib_core::Id;
use zib_core::body::{Alignment, ListTable};
use zib_core::markup::{self, Block, Inline, Item};
use zib_core::render::{Image, Kind};

/// The style of tables, which the editor's theme colors.
const CSS: &str = "
.zib-table { border-top: 1px solid alpha(currentColor, 0.25);
  border-left: 1px solid alpha(currentColor, 0.25); }
.zib-table-cell { border-right: 1px solid alpha(currentColor, 0.25);
  border-bottom: 1px solid alpha(currentColor, 0.25); padding: 6px 10px; }
.zib-table-header { font-weight: bold; background-color: alpha(currentColor, 0.06); }
.zib-table-code { background-color: alpha(currentColor, 0.08); padding: 4px 6px;
  border-radius: 3px; }
.zib-table-quote { border-left: 3px solid alpha(currentColor, 0.25); padding-left: 8px; }
";

/// The widest a column's text grows before it wraps, in characters.
const COLUMN_CHARS: i32 = 40;
/// The widest a diagram in a cell draws, in pixels.
const MAX_DIAGRAM_WIDTH: i32 = 480;

/// What a table's cells show from outside their text: the titles of the notes they link to, and the
/// math and diagrams the core drew, by `drawing_key`.
#[derive(Default)]
pub struct Refs {
    pub titles: HashMap<Id, String>,
    pub images: HashMap<String, Image>,
}

/// The key of a drawing in `Refs::images`.
pub fn drawing_key(kind: Kind, source: &str) -> String {
    format!("{kind:?}\n{source}")
}

/// The math and diagrams in `table`'s cells, nested tables included, which the core draws. Math
/// draws when it's a paragraph of its own; inside a line of text it shows its source.
pub fn drawings(table: &ListTable) -> Vec<(Kind, String)> {
    let mut found = Vec::new();
    for cell in table.rows.iter().flatten() {
        collect(&markup::parse(cell), &mut found);
    }
    found
}

fn collect(blocks: &[Block], found: &mut Vec<(Kind, String)>) {
    for block in blocks {
        match block {
            Block::Math(source) => found.push((Kind::DisplayMath, source.clone())),
            Block::Diagram(source) => found.push((Kind::Mermaid, source.clone())),
            Block::Paragraph(inlines) => found.extend(lone_math(inlines)),
            Block::Quote(blocks) => collect(blocks, found),
            Block::List { items, .. } => {
                for item in items {
                    collect(&item.blocks, found);
                }
            }
            Block::Table { rows, .. } => {
                for cell in rows.iter().flatten() {
                    collect(cell, found);
                }
            }
            _ => {}
        }
    }
}

/// The math that a paragraph holds and nothing else.
fn lone_math(inlines: &[Inline]) -> Option<(Kind, String)> {
    let [Inline::Math { source, display }] = inlines else { return None };
    Some((if *display { Kind::DisplayMath } else { Kind::InlineMath }, source.clone()))
}

/// A drawing the core made, or `None` when it couldn't, so the source shows instead.
fn picture(kind: Kind, source: &str, refs: &Refs) -> Option<gtk::Widget> {
    let image = refs.images.get(&drawing_key(kind, source))?;
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from(&image.png)).ok()?;
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_can_shrink(true);
    picture.set_halign(gtk::Align::Start);
    let (width, height) = (f64::from(image.width), f64::from(image.height));
    let scale = match kind {
        Kind::Mermaid => (f64::from(MAX_DIAGRAM_WIDTH) / width).min(1.0),
        Kind::DisplayMath | Kind::InlineMath => crate::drawings::math_scale(&picture),
    };
    #[allow(clippy::cast_possible_truncation)]
    let (width, height) = ((width * scale).round() as i32, (height * scale).round() as i32);
    picture.set_size_request(width, height);
    // The texture is twice the size the picture shows at, for sharp text, and a picture's natural
    // size is its texture's. Held to its width, the picture takes the height that fits it.
    let clamp = adw::Clamp::builder()
        .maximum_size(width)
        .tightening_threshold(width)
        .halign(gtk::Align::Start)
        .child(&picture)
        .build();
    Some(clamp.upcast())
}

/// The targets of the wikilinks in `table`, whose titles show in place of links without labels.
pub fn link_targets(table: &ListTable) -> Vec<Id> {
    let cells = table.rows.iter().flatten();
    cells.flat_map(|cell| zib_core::body::wikilinks(cell)).map(|link| link.target).collect()
}

/// A grid of `table`'s cells, showing what they link to and the drawings from `refs`.
pub fn widget(table: &ListTable, refs: &Refs) -> gtk::Widget {
    let rows = table.rows.iter().map(|row| row.iter().map(|cell| markup::parse(cell)).collect());
    let rows = rows.collect::<Vec<_>>();
    grid(&rows, table.header_rows, table.header_columns, &table.alignments, refs)
}

/// A cell of a table: its row and column, then, for a cell of a table inside that cell, which table
/// it is among those directly in the cell, and its row and column, and so on, as
/// `body::table_cell_source` takes them.
pub type CellPath = (usize, usize, Vec<(usize, usize, usize)>);

/// The cell of the table `table` that holds `point`, in `relative_to`'s coordinates.
pub fn cell_at(
    table: &gtk::Widget,
    relative_to: &gtk::Widget,
    point: &graphene::Point,
) -> Option<CellPath> {
    let grid = table.downcast_ref::<gtk::Grid>()?;
    let contains = |widget: &gtk::Widget| {
        widget.compute_bounds(relative_to).is_some_and(|bounds| bounds.contains_point(point))
    };
    let mut child = grid.first_child();
    while let Some(cell) = child {
        child = cell.next_sibling();
        if !contains(&cell) {
            continue;
        }
        let (column, row, _, _) = grid.query_child(&cell);
        let (row, column) = (usize::try_from(row).ok()?, usize::try_from(column).ok()?);
        let mut nested = Vec::new();
        let mut tables = (0..).zip(nested_tables(&cell));
        if let Some((index, inner)) = tables.find(|(_, inner)| contains(inner))
            && let Some((inner_row, inner_column, deeper)) = cell_at(&inner, relative_to, point)
        {
            nested.push((index, inner_row, inner_column));
            nested.extend(deeper);
        }
        return Some((row, column, nested));
    }
    None
}

fn children(widget: &gtk::Widget) -> impl Iterator<Item = gtk::Widget> {
    std::iter::successors(widget.first_child(), gtk::Widget::next_sibling)
}

/// Collect direct table descendants through quote and list containers, stopping at each grid.
fn nested_tables(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut tables = Vec::new();
    for child in children(widget) {
        if child.has_css_class("zib-table") {
            tables.push(child);
        } else {
            tables.extend(nested_tables(&child));
        }
    }
    tables
}

fn grid(
    rows: &[Vec<Vec<Block>>],
    header_rows: usize,
    header_columns: usize,
    alignments: &[Alignment],
    refs: &Refs,
) -> gtk::Widget {
    style();
    let grid = gtk::Grid::builder().halign(gtk::Align::Start).build();
    grid.add_css_class("zib-table");
    for (row, cells) in (0..).zip(rows) {
        for (column, blocks) in (0..).zip(cells) {
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 6);
            cell.add_css_class("zib-table-cell");
            cell.set_hexpand(true);
            let header = usize::try_from(row).is_ok_and(|r| r < header_rows)
                || usize::try_from(column).is_ok_and(|c| c < header_columns);
            if header {
                cell.add_css_class("zib-table-header");
            }
            append_blocks(&cell, blocks, refs);
            let alignment = usize::try_from(column).ok().and_then(|c| alignments.get(c));
            align(&cell, alignment.copied().unwrap_or_default());
            grid.attach(&cell, column, row, 1, 1);
        }
    }
    grid.upcast()
}

/// Places the blocks of a cell to follow its column's alignment, with each line of their text too.
fn align(cell: &gtk::Box, alignment: Alignment) {
    let (halign, xalign, justify) = match alignment {
        Alignment::None | Alignment::Left => return,
        Alignment::Center => (gtk::Align::Center, 0.5, gtk::Justification::Center),
        Alignment::Right => (gtk::Align::End, 1.0, gtk::Justification::Right),
    };
    for child in children(cell.upcast_ref()) {
        child.set_halign(halign);
        if let Some(label) = child.downcast_ref::<gtk::Label>() {
            label.set_xalign(xalign);
            label.set_justify(justify);
        }
    }
}

fn append_blocks(container: &gtk::Box, blocks: &[Block], refs: &Refs) {
    for block in blocks {
        container.append(&block_widget(block, refs));
    }
}

fn block_widget(block: &Block, refs: &Refs) -> gtk::Widget {
    match block {
        Block::Paragraph(inlines) => lone_math(inlines)
            .and_then(|(kind, source)| picture(kind, &source, refs))
            .unwrap_or_else(|| text(&markup_of(inlines, refs)).upcast()),
        Block::Heading { content, .. } => {
            text(&format!("<b>{}</b>", markup_of(content, refs))).upcast()
        }
        Block::Quote(blocks) => {
            let quote = gtk::Box::new(gtk::Orientation::Vertical, 6);
            quote.add_css_class("zib-table-quote");
            append_blocks(&quote, blocks, refs);
            quote.upcast()
        }
        Block::List { start, items } => list(*start, items, refs),
        Block::Diagram(source) if let Some(drawn) = picture(Kind::Mermaid, source, refs) => drawn,
        Block::Math(source) if let Some(drawn) = picture(Kind::DisplayMath, source, refs) => drawn,
        Block::Code { text: source, .. } | Block::Diagram(source) | Block::Math(source) => {
            let label = text(&glib::markup_escape_text(source.trim_end()));
            label.add_css_class("monospace");
            label.add_css_class("zib-table-code");
            label.upcast()
        }
        Block::Table { header_rows, header_columns, alignments, rows } => {
            grid(rows, *header_rows, *header_columns, alignments, refs)
        }
        Block::Rule => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
        Block::Html(html) => text(&glib::markup_escape_text(html)).upcast(),
    }
}

fn list(start: Option<u64>, items: &[Item], refs: &Refs) -> gtk::Widget {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for (number, item) in (start.unwrap_or(1)..).zip(items) {
        let marker = match (item.checked, start) {
            (Some(true), _) => "☑".to_owned(),
            (Some(false), _) => "☐".to_owned(),
            (None, Some(_)) => format!("{number}."),
            (None, None) => "•".to_owned(),
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&gtk::Label::builder().label(marker).valign(gtk::Align::Start).build());
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        append_blocks(&content, &item.blocks, refs);
        row.append(&content);
        list.append(&row);
    }
    list.upcast()
}

/// A label for Pango markup that wraps within its column.
fn text(markup: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(markup)
        .use_markup(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(COLUMN_CHARS)
        .xalign(0.0)
        .build()
}

/// Inline markdown as Pango markup.
fn markup_of(inlines: &[Inline], refs: &Refs) -> String {
    let mut out = String::new();
    for inline in inlines {
        write_inline(&mut out, inline, refs);
    }
    out
}

fn write_inline(out: &mut String, inline: &Inline, refs: &Refs) {
    let mut wrapped = |open: &str, close: &str, inlines: &[Inline]| {
        let _ = write!(out, "{open}{}{close}", markup_of(inlines, refs));
    };
    match inline {
        Inline::Text(text) => out.push_str(&glib::markup_escape_text(text)),
        Inline::Code(code) => {
            let _ = write!(out, "{CODE}{}</span>", glib::markup_escape_text(code));
        }
        Inline::Math { source, .. } => {
            let _ = write!(out, "<i>{}</i>", glib::markup_escape_text(source));
        }
        Inline::Emphasis(inlines) => wrapped("<i>", "</i>", inlines),
        Inline::Strong(inlines) => wrapped("<b>", "</b>", inlines),
        Inline::Strikethrough(inlines) => wrapped("<s>", "</s>", inlines),
        Inline::Underline(inlines) => wrapped("<u>", "</u>", inlines),
        Inline::Highlight(inlines) => {
            wrapped("<span background=\"#f5d42e\" bgalpha=\"45%\">", "</span>", inlines);
        }
        Inline::Link { content, .. } => wrapped(LINK, "</span>", content),
        Inline::Image { alt, .. } => out.push_str(&glib::markup_escape_text(alt)),
        Inline::WikiLink { label: Some(label), .. } => wrapped(LINK, "</span>", label),
        Inline::WikiLink { target, label: None } | Inline::Embed { target, label: None } => {
            let title = refs.titles.get(target).map_or_else(|| target.to_string(), Clone::clone);
            let _ = write!(out, "{LINK}{}</span>", glib::markup_escape_text(&title));
        }
        Inline::Embed { label: Some(label), .. } => {
            let _ = write!(out, "{LINK}{}</span>", glib::markup_escape_text(label));
        }
        Inline::SoftBreak => out.push(' '),
        Inline::HardBreak => out.push('\n'),
    }
}

/// The look of inline code, as in the editor.
const CODE: &str = "<span font_family=\"monospace\" background=\"#808080\" bgalpha=\"15%\">";
/// The look of links, as in the editor.
const LINK: &str = "<span foreground=\"#3584e4\" underline=\"single\">";

/// Adds the tables' style to the display once.
fn style() {
    thread_local! {
        static ADDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if ADDED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
