//! List tables, drawn in the editor over their source as a grid of cells, as the note bodies
//! specification's list tables define them. Each cell's markdown renders from the core's markup
//! tree, as decision ZIB0009 describes. Math and diagrams inside a cell show their source.

use std::collections::HashMap;
use std::fmt::Write as _;

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::Id;
use zib_core::body::ListTable;
use zib_core::markup::{self, Block, Inline, Item};

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

/// The targets of the wikilinks in `table`, whose titles show in place of links without labels.
pub fn link_targets(table: &ListTable) -> Vec<Id> {
    let cells = table.rows.iter().flatten();
    cells.flat_map(|cell| zib_core::body::wikilinks(cell)).map(|link| link.target).collect()
}

/// A grid of `table`'s cells, with each wikilink that has no label showing its target's title from
/// `titles`.
pub fn widget(table: &ListTable, titles: &HashMap<Id, String>) -> gtk::Widget {
    let rows = table.rows.iter().map(|row| row.iter().map(|cell| markup::parse(cell)).collect());
    grid(&rows.collect::<Vec<_>>(), table.header_rows, table.header_columns, titles)
}

fn grid(
    rows: &[Vec<Vec<Block>>],
    header_rows: usize,
    header_columns: usize,
    titles: &HashMap<Id, String>,
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
            append_blocks(&cell, blocks, titles);
            grid.attach(&cell, column, row, 1, 1);
        }
    }
    grid.upcast()
}

fn append_blocks(container: &gtk::Box, blocks: &[Block], titles: &HashMap<Id, String>) {
    for block in blocks {
        container.append(&block_widget(block, titles));
    }
}

fn block_widget(block: &Block, titles: &HashMap<Id, String>) -> gtk::Widget {
    match block {
        Block::Paragraph(inlines) => text(&markup_of(inlines, titles)).upcast(),
        Block::Heading { content, .. } => {
            text(&format!("<b>{}</b>", markup_of(content, titles))).upcast()
        }
        Block::Quote(blocks) => {
            let quote = gtk::Box::new(gtk::Orientation::Vertical, 6);
            quote.add_css_class("zib-table-quote");
            append_blocks(&quote, blocks, titles);
            quote.upcast()
        }
        Block::List { start, items } => list(*start, items, titles),
        Block::Code { text: source, .. } | Block::Diagram(source) | Block::Math(source) => {
            let label = text(&glib::markup_escape_text(source.trim_end()));
            label.add_css_class("monospace");
            label.add_css_class("zib-table-code");
            label.upcast()
        }
        Block::Table { header_rows, header_columns, rows } => {
            grid(rows, *header_rows, *header_columns, titles)
        }
        Block::Rule => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
        Block::Html(html) => text(&glib::markup_escape_text(html)).upcast(),
    }
}

fn list(start: Option<u64>, items: &[Item], titles: &HashMap<Id, String>) -> gtk::Widget {
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
        append_blocks(&content, &item.blocks, titles);
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
fn markup_of(inlines: &[Inline], titles: &HashMap<Id, String>) -> String {
    let mut out = String::new();
    for inline in inlines {
        write_inline(&mut out, inline, titles);
    }
    out
}

fn write_inline(out: &mut String, inline: &Inline, titles: &HashMap<Id, String>) {
    let mut wrapped = |open: &str, close: &str, inlines: &[Inline]| {
        let _ = write!(out, "{open}{}{close}", markup_of(inlines, titles));
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
            let title = titles.get(target).map_or_else(|| target.to_string(), Clone::clone);
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
