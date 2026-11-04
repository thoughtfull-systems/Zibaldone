//! The preview of a note: the core's tree of its body, drawn in a read-only text view, with math,
//! diagrams, and embedded files as images, and tables as grids.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::Id;
use zib_core::markup::{self, Block, Inline};
use zib_core::render::Kind;

use crate::app::core;
use crate::state::State;
use crate::window::Window;

/// Where a click on linked text goes.
#[derive(Clone, Debug)]
enum Target {
    Note(Id),
    Web(String),
}

struct Builder {
    state: Arc<State>,
    space: Id,
    view: gtk::TextView,
    buffer: gtk::TextBuffer,
    titles: HashMap<Id, String>,
    links: Rc<RefCell<Vec<(gtk::TextTag, Target)>>>,
    window: Window,
}

/// Builds the preview of `body`, whose wikilinks show `titles`.
pub fn build(
    state: &Arc<State>,
    space: Id,
    body: &str,
    titles: &HashMap<Id, String>,
    window: Window,
) -> gtk::TextView {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(16)
        .bottom_margin(16)
        .left_margin(24)
        .right_margin(24)
        .pixels_below_lines(4)
        .build();
    let buffer = view.buffer();
    for (name, properties) in tag_styles() {
        let tag = gtk::TextTag::new(Some(name));
        for (key, value) in properties {
            tag.set_property_from_value(key, &value);
        }
        buffer.tag_table().add(&tag);
    }
    let builder = Builder {
        state: state.clone(),
        space,
        view: view.clone(),
        buffer,
        titles: titles.clone(),
        links: Rc::default(),
        window,
    };
    let tree = markup::parse(body);
    builder.blocks(&tree, 0);
    builder.connect_links();
    view
}

fn tag_styles() -> Vec<(&'static str, Vec<(&'static str, glib::Value)>)> {
    let weight = |w: i32| ("weight", w.to_value());
    let scale = |s: f64| ("scale", s.to_value());
    vec![
        ("h1", vec![weight(800), scale(1.8), ("pixels-above-lines", 12.to_value())]),
        ("h2", vec![weight(800), scale(1.5), ("pixels-above-lines", 10.to_value())]),
        ("h3", vec![weight(700), scale(1.3), ("pixels-above-lines", 8.to_value())]),
        ("h4", vec![weight(700), scale(1.15)]),
        ("h5", vec![weight(700), scale(1.05)]),
        ("h6", vec![weight(700)]),
        ("strong", vec![weight(700)]),
        ("emphasis", vec![("style", gtk::pango::Style::Italic.to_value())]),
        ("strike", vec![("strikethrough", true.to_value())]),
        ("underline", vec![("underline", gtk::pango::Underline::Single.to_value())]),
        (
            "highlight",
            vec![("background", "#fff3a0".to_value()), ("foreground", "#1e1e1e".to_value())],
        ),
        ("code", vec![("family", "monospace".to_value())]),
        (
            "codeblock",
            vec![("family", "monospace".to_value()), ("paragraph-background", "#8882".to_value())],
        ),
        (
            "quote",
            vec![("left-margin", 48.to_value()), ("style", gtk::pango::Style::Italic.to_value())],
        ),
        (
            "link",
            vec![
                ("underline", gtk::pango::Underline::Single.to_value()),
                ("foreground", "#3584e4".to_value()),
            ],
        ),
        ("missing", vec![("strikethrough", true.to_value()), ("foreground", "#c01c28".to_value())]),
    ]
}

impl Builder {
    fn end(&self) -> gtk::TextIter {
        self.buffer.end_iter()
    }

    fn insert(&self, text: &str, tags: &[gtk::TextTag]) {
        let mut iter = self.end();
        let refs: Vec<&gtk::TextTag> = tags.iter().collect();
        self.buffer.insert_with_tags(&mut iter, text, &refs);
    }

    fn named(&self, name: &str) -> gtk::TextTag {
        self.buffer.tag_table().lookup(name).expect("defined in tag_styles")
    }

    fn indent(&self, level: usize) -> Option<gtk::TextTag> {
        (level > 0).then(|| {
            let name = format!("indent-{level}");
            self.buffer.tag_table().lookup(&name).unwrap_or_else(|| {
                let tag = gtk::TextTag::new(Some(&name));
                tag.set_left_margin(24 + 28 * i32::try_from(level).unwrap_or(0));
                self.buffer.tag_table().add(&tag);
                tag
            })
        })
    }

    fn newline(&self) {
        if !self.end().starts_line() {
            self.insert("\n", &[]);
        }
    }

    fn blocks(&self, blocks: &[Block], level: usize) {
        for block in blocks {
            self.block(block, level);
        }
    }

    fn block(&self, block: &Block, level: usize) {
        let base: Vec<gtk::TextTag> = self.indent(level).into_iter().collect();
        match block {
            Block::Heading { level: h, content } => {
                let mut tags = base.clone();
                tags.push(self.named(&format!("h{}", (*h).clamp(1, 6))));
                self.inlines(content, &tags);
                self.insert("\n", &[]);
            }
            Block::Paragraph(content) => {
                self.inlines(content, &base);
                self.insert("\n", &[]);
            }
            Block::Quote(blocks) => {
                let start = self.buffer.create_mark(None, &self.end(), true);
                self.blocks(blocks, level);
                let quote = self.named("quote");
                self.buffer.apply_tag(&quote, &self.buffer.iter_at_mark(&start), &self.end());
            }
            Block::List { start, items } => {
                for (i, item) in items.iter().enumerate() {
                    let marker = match (item.checked, start) {
                        (Some(true), _) => "☑ ".to_owned(),
                        (Some(false), _) => "☐ ".to_owned(),
                        (None, Some(first)) => format!("{}. ", first + i as u64),
                        (None, None) => "• ".to_owned(),
                    };
                    let tags: Vec<gtk::TextTag> = self.indent(level + 1).into_iter().collect();
                    self.insert(&marker, &tags);
                    let mut rest = item.blocks.as_slice();
                    if let Some(Block::Paragraph(first)) = rest.first() {
                        self.inlines(first, &tags);
                        self.insert("\n", &[]);
                        rest = &rest[1..];
                    } else {
                        self.insert("\n", &[]);
                    }
                    self.blocks(rest, level + 1);
                }
            }
            Block::Code { text, .. } | Block::Html(text) => {
                let mut tags = base.clone();
                tags.push(self.named("codeblock"));
                self.insert(text.trim_end_matches('\n'), &tags);
                self.insert("\n", &[]);
            }
            Block::Math(source) => {
                self.image(Kind::DisplayMath, source);
                self.insert("\n", &[]);
            }
            Block::Diagram(source) => {
                self.image(Kind::Mermaid, source);
                self.insert("\n", &[]);
            }
            Block::Table { header_rows, header_columns, rows } => {
                self.table(*header_rows, *header_columns, rows);
                self.insert("\n", &[]);
            }
            Block::Rule => {
                self.newline();
                self.child(&gtk::Separator::new(gtk::Orientation::Horizontal));
                self.insert("\n", &[]);
            }
        }
    }

    fn inlines(&self, inlines: &[Inline], tags: &[gtk::TextTag]) {
        for inline in inlines {
            self.inline(inline, tags);
        }
    }

    fn inline(&self, inline: &Inline, tags: &[gtk::TextTag]) {
        let with = |name: &str| {
            let mut more = tags.to_vec();
            more.push(self.named(name));
            more
        };
        match inline {
            Inline::Text(text) => self.insert(text, tags),
            Inline::Code(code) => self.insert(code, &with("code")),
            Inline::Math { source, display } => {
                self.image(if *display { Kind::DisplayMath } else { Kind::InlineMath }, source);
            }
            Inline::Emphasis(c) => self.inlines(c, &with("emphasis")),
            Inline::Strong(c) => self.inlines(c, &with("strong")),
            Inline::Strikethrough(c) => self.inlines(c, &with("strike")),
            Inline::Underline(c) => self.inlines(c, &with("underline")),
            Inline::Highlight(c) => self.inlines(c, &with("highlight")),
            Inline::Link { url, content } => {
                let mut linked = with("link");
                linked.push(self.link_tag(Target::Web(url.clone())));
                self.inlines(content, &linked);
            }
            Inline::Image { url, alt } => self.insert(&format!("[{alt}]({url})"), tags),
            Inline::WikiLink { target, label } => {
                let mut linked = tags.to_vec();
                match (label, self.titles.get(target)) {
                    (Some(label), _) => {
                        linked.push(self.named("link"));
                        linked.push(self.link_tag(Target::Note(*target)));
                        self.inlines(label, &linked);
                    }
                    (None, Some(title)) => {
                        linked.push(self.named("link"));
                        linked.push(self.link_tag(Target::Note(*target)));
                        self.inlines(&markup::inline_markdown(title), &linked);
                    }
                    (None, None) => {
                        linked.push(self.named("missing"));
                        self.insert(&format!("missing note {target}"), &linked);
                    }
                }
            }
            Inline::Embed { target, label } => self.embed(*target, label.as_deref()),
            Inline::SoftBreak => self.insert(" ", tags),
            Inline::HardBreak => self.insert("\n", tags),
        }
    }

    fn link_tag(&self, target: Target) -> gtk::TextTag {
        let tag = gtk::TextTag::new(None);
        self.buffer.tag_table().add(&tag);
        self.links.borrow_mut().push((tag.clone(), target));
        tag
    }

    fn child(&self, widget: &impl IsA<gtk::Widget>) {
        let mut iter = self.end();
        let anchor = self.buffer.create_child_anchor(&mut iter);
        self.view.add_child_at_anchor(widget, &anchor);
    }

    /// Math or a diagram, drawn by the core and shown once it's ready, or its source and the error.
    fn image(&self, kind: Kind, source: &str) {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.child(&holder);
        let state = self.state.clone();
        let source = source.to_owned();
        glib::spawn_future_local(async move {
            let shown = source.clone();
            match core(async move { state.render(kind, source).await }).await {
                Ok(image) => {
                    let texture = gdk::Texture::from_bytes(&glib::Bytes::from(&image.png));
                    if let Ok(texture) = texture {
                        let picture = gtk::Picture::for_paintable(&texture);
                        picture.set_can_shrink(true);
                        picture.set_size_request(
                            i32::try_from(image.width).unwrap_or(1),
                            i32::try_from(image.height).unwrap_or(1),
                        );
                        holder.append(&picture);
                    }
                }
                Err(error) => {
                    let label = gtk::Label::builder()
                        .label(format!("{shown}\n{error}"))
                        .xalign(0.0)
                        .selectable(true)
                        .build();
                    label.add_css_class("error");
                    label.add_css_class("monospace");
                    holder.append(&label);
                }
            }
        });
    }

    /// An embedded note: an image note's picture, or a note's title, with its thumbnail if it has
    /// one, linking to it.
    fn embed(&self, target: Id, label: Option<&str>) {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 4);
        self.child(&holder);
        let state = self.state.clone();
        let window = self.window.clone();
        let fallback = self.titles.get(&target).cloned();
        let label = label.map(ToOwned::to_owned);
        glib::spawn_future_local(async move {
            let found = core({
                let state = state.clone();
                async move {
                    let space = state.find_note(target).await?;
                    let note = state.note(space, target).await.ok()?;
                    let (field, preview) = match note.type_name.as_str() {
                        "image" => ("image", true),
                        _ => ("thumbnail", false),
                    };
                    let file = state.note_file(space, target, field).await.ok().flatten();
                    let path = match file {
                        Some((hash, _)) => state.blob_path(space, hash).await.ok(),
                        None => None,
                    };
                    Some((space, path, preview))
                }
            })
            .await;
            let title = label.or(fallback).unwrap_or_else(|| target.to_string());
            match found {
                Some((_, Some(path), true)) => {
                    let picture = gtk::Picture::for_filename(&path);
                    picture.set_can_shrink(true);
                    picture.set_size_request(480, 320);
                    picture.set_alternative_text(Some(&title));
                    holder.append(&picture);
                }
                Some((space, thumbnail, false)) => {
                    let button = gtk::Button::new();
                    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    if let Some(path) = thumbnail {
                        let picture = gtk::Picture::for_filename(&path);
                        picture.set_size_request(64, 64);
                        content.append(&picture);
                    }
                    content.append(&gtk::Label::new(Some(&zib_core::body::plain_text(&title))));
                    button.set_child(Some(&content));
                    button.connect_clicked(move |_| window.open_note(space, target));
                    holder.append(&button);
                }
                _ => {
                    let missing = gtk::Label::new(Some(&format!("missing note {target}")));
                    missing.add_css_class("error");
                    holder.append(&missing);
                }
            }
        });
    }

    /// A table as a grid, with header rows and columns in bold.
    fn table(&self, header_rows: usize, header_columns: usize, rows: &[Vec<Vec<Block>>]) {
        let grid = gtk::Grid::builder()
            .column_spacing(16)
            .row_spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        grid.add_css_class("card");
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let header = r < header_rows || c < header_columns;
                let text = cell_markup(cell, &self.titles);
                let label = gtk::Label::builder()
                    .use_markup(true)
                    .label(if header { format!("<b>{text}</b>") } else { text })
                    .xalign(0.0)
                    .yalign(0.0)
                    .wrap(true)
                    .selectable(true)
                    .margin_start(8)
                    .margin_end(8)
                    .build();
                grid.attach(
                    &label,
                    i32::try_from(c).unwrap_or(0),
                    i32::try_from(r).unwrap_or(0),
                    1,
                    1,
                );
            }
        }
        self.newline();
        self.child(&grid);
    }

    fn connect_links(self) {
        let click = gtk::GestureClick::new();
        let view = self.view.clone();
        let links = self.links.clone();
        let window = self.window.clone();
        let space = self.space;
        click.connect_released(move |_, _, x, y| {
            #[allow(clippy::cast_possible_truncation)]
            let (bx, by) =
                view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let Some(iter) = view.iter_at_location(bx, by) else { return };
            let tags = iter.tags();
            let target =
                links.borrow().iter().find(|(tag, _)| tags.contains(tag)).map(|(_, t)| t.clone());
            match target {
                Some(Target::Note(note)) => {
                    let _ = space;
                    window.open_uri(&format!("zib:note/{note}"));
                }
                Some(Target::Web(url)) => {
                    let _ = gtk::gio::AppInfo::launch_default_for_uri(
                        &url,
                        None::<&gtk::gio::AppLaunchContext>,
                    );
                }
                None => {}
            }
        });
        self.view.add_controller(click);
    }
}

/// A table cell as Pango markup, with its blocks run together.
fn cell_markup(blocks: &[Block], titles: &HashMap<Id, String>) -> String {
    let mut out = String::new();
    for block in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match block {
            Block::Paragraph(content) | Block::Heading { content, .. } => {
                out.push_str(&inline_markup(content, titles));
            }
            Block::List { items, .. } => {
                for item in items {
                    out.push_str("• ");
                    out.push_str(&cell_markup(&item.blocks, titles));
                    out.push('\n');
                }
            }
            Block::Code { text, .. } => {
                let _ = write!(out, "<tt>{}</tt>", glib::markup_escape_text(text));
            }
            Block::Math(source) | Block::Diagram(source) | Block::Html(source) => {
                out.push_str(&glib::markup_escape_text(source));
            }
            Block::Quote(blocks) => {
                let _ = write!(out, "<i>{}</i>", cell_markup(blocks, titles));
            }
            Block::Table { .. } | Block::Rule => {}
        }
    }
    out
}

/// Inlines as Pango markup.
pub fn inline_markup(inlines: &[Inline], titles: &HashMap<Id, String>) -> String {
    let mut out = String::new();
    for inline in inlines {
        let wrapped = |open: &str, close: &str, content: &[Inline]| {
            format!("{open}{}{close}", inline_markup(content, titles))
        };
        match inline {
            Inline::Text(t) => out.push_str(&glib::markup_escape_text(t)),
            Inline::Code(t) | Inline::Math { source: t, .. } => {
                let _ = write!(out, "<tt>{}</tt>", glib::markup_escape_text(t));
            }
            Inline::Emphasis(c) => out.push_str(&wrapped("<i>", "</i>", c)),
            Inline::Strong(c) => out.push_str(&wrapped("<b>", "</b>", c)),
            Inline::Strikethrough(c) => out.push_str(&wrapped("<s>", "</s>", c)),
            Inline::Underline(c) | Inline::Link { content: c, .. } => {
                out.push_str(&wrapped("<u>", "</u>", c));
            }
            Inline::Highlight(c) => out.push_str(&wrapped(
                "<span background=\"#fff3a0\" foreground=\"#1e1e1e\">",
                "</span>",
                c,
            )),
            Inline::Image { alt, .. } => out.push_str(&glib::markup_escape_text(alt)),
            Inline::WikiLink { target, label } => {
                let text = match (label, titles.get(target)) {
                    (Some(label), _) => inline_markup(label, titles),
                    (None, Some(title)) => inline_markup(&markup::inline_markdown(title), titles),
                    (None, None) => glib::markup_escape_text(&target.to_string()).to_string(),
                };
                let _ = write!(out, "<u>{text}</u>");
            }
            Inline::Embed { target, label } => {
                let text = label
                    .clone()
                    .or_else(|| titles.get(target).cloned())
                    .unwrap_or_else(|| target.to_string());
                out.push_str(&glib::markup_escape_text(&text));
            }
            Inline::SoftBreak => out.push(' '),
            Inline::HardBreak => out.push('\n'),
        }
    }
    out
}
