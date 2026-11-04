//! Note bodies, as the note bodies specification defines them: wikilinks and embeds, titles,
//! underlines and highlights, list tables, and the labels that raw editors fill in.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::Id;

/// The markdown options of the dialect that the note bodies specification defines. Zib's own
/// additions are found separately.
pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_MATH
}

/// A wikilink or embed in a body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WikiLink {
    /// The byte range of the whole link in the body, from `[[` or `![[` through `]]`.
    pub range: Range<usize>,
    pub embed: bool,
    pub target: Id,
    /// The fragment after `#` in the target, which nothing interprets yet.
    pub fragment: Option<String>,
    /// The label, with its escapes removed, if the link has one.
    pub label: Option<String>,
}

/// The byte ranges of code spans, code blocks, and HTML, where Zib's additions don't apply.
fn code_ranges(body: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut block: Option<usize> = None;
    for (event, range) in Parser::new_ext(body, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => block = Some(range.start),
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock) => {
                if let Some(start) = block.take() {
                    ranges.push(start..range.end);
                }
            }
            Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) => ranges.push(range),
            _ => {}
        }
    }
    ranges
}

/// Every wikilink and embed in `body`, in order, outside code.
pub fn wikilinks(body: &str) -> Vec<WikiLink> {
    let code = code_ranges(body);
    let in_code = |at: usize| code.iter().any(|r| r.contains(&at));
    let bytes = body.as_bytes();
    let mut links = Vec::new();
    let mut at = 0;
    while let Some(found) = body[at..].find("[[") {
        let open = at + found;
        at = open + 2;
        if in_code(open) || (open > 0 && bytes[open - 1] == b'\\') {
            continue;
        }
        let Some((inner, close)) = scan_inner(body, open + 2) else { continue };
        let embed = open > 0 && bytes[open - 1] == b'!' && !(open > 1 && bytes[open - 2] == b'\\');
        let Some(link) = read_inner(&inner) else { continue };
        let start = if embed { open - 1 } else { open };
        links.push(WikiLink { range: start..close, embed, ..link });
        at = close;
    }
    links
}

/// The raw text between `[[` and the `]]` that ends it, and the byte after that `]]`.
fn scan_inner(body: &str, from: usize) -> Option<(String, usize)> {
    let bytes = body.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => return None,
            b'\\' if i + 1 < bytes.len() && matches!(bytes[i + 1], b'\\' | b']' | b'|') => i += 2,
            b']' if bytes.get(i + 1) == Some(&b']') => {
                return Some((body[from..i].to_owned(), i + 2));
            }
            _ => i += 1,
        }
    }
    None
}

/// Splits the inside of a wikilink at its first unescaped `|`, and removes escapes.
fn read_inner(inner: &str) -> Option<WikiLink> {
    let mut parts = [String::new(), String::new()];
    let mut part = 0;
    let mut has_label = false;
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if matches!(chars.peek(), Some('\\' | ']' | '|')) => {
                parts[part].push(chars.next().expect("peeked"));
            }
            '|' if part == 0 => {
                part = 1;
                has_label = true;
            }
            _ => parts[part].push(c),
        }
    }
    let target = parts[0].trim();
    let (id, fragment) = match target.split_once('#') {
        Some((id, fragment)) => (id, Some(fragment.to_owned())),
        None => (target, None),
    };
    let target = Id::parse(id.trim()).ok()?;
    let label = has_label.then(|| parts[1].trim().to_owned());
    Some(WikiLink { range: 0..0, embed: false, target, fragment, label })
}

/// Escapes a label so that a wikilink holds it exactly.
pub fn escape_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        match c {
            '\\' | ']' | '|' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// The wikilink an editor inserts: the target's canonical identifier, and the owner's selection,
/// if any, as its label.
pub fn wikilink(target: Id, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("[[{target}|{}]]", escape_label(label)),
        None => format!("[[{target}]]"),
    }
}

/// A note's title, in both of the forms the core gives apps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Title {
    /// Markdown that keeps the line's span formatting.
    pub markdown: String,
    /// The same text with every mark of formatting removed.
    pub plain: String,
}

impl Title {
    pub fn is_empty(&self) -> bool {
        self.plain.trim().is_empty()
    }
}

/// The source of a note's title: its body's first line that holds anything but white space,
/// without leading white space or heading marks.
pub fn title_line(body: &str) -> &str {
    let line = body.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    let Some(rest) = line.strip_prefix('#') else { return line };
    let hashes = 1 + rest.bytes().take_while(|&b| b == b'#').count();
    if hashes > 6 {
        return line;
    }
    let rest = &line[hashes..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return line;
    }
    let rest = rest.trim();
    // A closing sequence of `#` counts only after white space, or as the whole rest.
    let trimmed = rest.trim_end_matches('#');
    if trimmed.is_empty() || trimmed.ends_with([' ', '\t']) { trimmed.trim_end() } else { rest }
}

/// A note's title, with each link replaced by its text. `resolve` gives the title of a link's
/// target, for links without a label.
pub fn title(body: &str, resolve: &mut dyn FnMut(Id) -> Option<Title>) -> Title {
    let line = title_line(body);
    let mut markdown = String::new();
    let mut last = 0;
    for link in wikilinks(line) {
        markdown.push_str(&line[last..link.range.start]);
        match link.label {
            Some(label) => markdown.push_str(&label),
            None => match resolve(link.target) {
                Some(title) => markdown.push_str(&title.markdown),
                None => markdown.push_str(&link.target.to_string()),
            },
        }
        last = link.range.end;
    }
    markdown.push_str(&line[last..]);
    let markdown = unlink(&markdown);
    let plain = plain_text(&markdown);
    Title { markdown, plain }
}

/// Titles for many notes at once, which shows a target whose title would come back around to a
/// note already on the way as its identifier, so titles that link to each other can't loop.
pub struct Titles<'a> {
    body: &'a dyn Fn(Id) -> Option<String>,
    cache: BTreeMap<Id, Title>,
}

impl<'a> Titles<'a> {
    /// `body` gives the body of a note the machine holds.
    pub fn new(body: &'a dyn Fn(Id) -> Option<String>) -> Self {
        Self { body, cache: BTreeMap::new() }
    }

    pub fn title(&mut self, id: Id) -> Option<Title> {
        self.title_within(id, &mut BTreeSet::new())
    }

    fn title_within(&mut self, id: Id, path: &mut BTreeSet<Id>) -> Option<Title> {
        if let Some(title) = self.cache.get(&id) {
            return Some(title.clone());
        }
        let body = (self.body)(id)?;
        path.insert(id);
        let mut resolve = |target: Id| {
            if path.contains(&target) {
                let text = target.to_string();
                return Some(Title { markdown: text.clone(), plain: text });
            }
            self.title_within(target, path)
        };
        let title = title(&body, &mut resolve);
        path.remove(&id);
        self.cache.insert(id, title.clone());
        Some(title)
    }
}

/// Replaces each standard link, image, and autolink in a line of markdown by its text.
fn unlink(line: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    let mut depth = 0;
    let mut link_text_start = 0;
    for (event, range) in Parser::new_ext(line, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => {
                if depth == 0 {
                    out.push_str(&line[last..range.start]);
                    link_text_start = out.len();
                }
                depth += 1;
                last = range.start;
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                depth -= 1;
                if depth == 0 {
                    last = range.end;
                }
            }
            _ if depth > 0 => {
                if let Event::Text(text) | Event::Code(text) = &event {
                    let _ = link_text_start;
                    out.push_str(text);
                }
            }
            _ => {}
        }
    }
    out.push_str(&line[last..]);
    out
}

/// The text of a line of markdown with every mark of formatting removed.
pub fn plain_text(markdown: &str) -> String {
    let mut out = String::new();
    for event in Parser::new_ext(markdown, options()) {
        match event {
            Event::Text(text) | Event::Code(text) | Event::InlineMath(text) => out.push_str(&text),
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            _ => {}
        }
    }
    remove_marks(&out)
}

/// What a run of `=` marks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mark {
    Underline,
    Highlight,
    Both,
}

impl Mark {
    fn from_len(len: usize) -> Option<Self> {
        match len {
            1 => Some(Self::Underline),
            2 => Some(Self::Highlight),
            3 => Some(Self::Both),
            _ => None,
        }
    }
}

/// A span that a pair of `=` runs marks, with the byte ranges of its opening and closing runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkedSpan {
    pub mark: Mark,
    pub open: Range<usize>,
    pub close: Range<usize>,
}

/// The underlines and highlights in one block's text, paired by the same rules as `*` for
/// emphasis in `CommonMark`, with `\=` as a literal `=`.
pub fn marked_spans(text: &str) -> Vec<MarkedSpan> {
    let bytes = text.as_bytes();
    let mut runs: Vec<(Range<usize>, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] != b'=' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] == b'=' {
            i += 1;
        }
        let before = text[..start].chars().next_back();
        let after = text[i..].chars().next();
        let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
        let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
        let left = !space(after) && (!punct(after) || space(before) || punct(before));
        let right = !space(before) && (!punct(before) || space(after) || punct(after));
        runs.push((start..i, left, right));
    }
    let mut spans = Vec::new();
    let mut openers: Vec<Range<usize>> = Vec::new();
    for (range, can_open, can_close) in runs {
        let len = range.len();
        if Mark::from_len(len).is_none() {
            continue;
        }
        if can_close && let Some(pos) = openers.iter().rposition(|o| o.len() == len) {
            let open = openers.remove(pos);
            openers.truncate(pos);
            spans.push(MarkedSpan {
                mark: Mark::from_len(len).expect("checked"),
                open,
                close: range,
            });
            continue;
        }
        if can_open {
            openers.push(range);
        }
    }
    spans.sort_by_key(|span| span.open.start);
    spans
}

/// Removes the `=` runs that mark underlines and highlights, and the backslashes of `\=`.
fn remove_marks(text: &str) -> String {
    let spans = marked_spans(text);
    let mut skip: Vec<Range<usize>> =
        spans.iter().flat_map(|s| [s.open.clone(), s.close.clone()]).collect();
    skip.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for range in skip {
        out.push_str(&text[at..range.start]);
        at = range.end;
    }
    out.push_str(&text[at..]);
    out.replace("\\=", "=")
}

/// A list table, as the note bodies specification defines it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListTable {
    pub header_rows: usize,
    pub header_columns: usize,
    /// Each row's cells, as markdown, padded with empty cells to the longest row.
    pub rows: Vec<Vec<String>>,
}

/// Reads a fenced `table` block from its info string and content, or `None` when its content
/// isn't a list of lists, so the block shows as code.
pub fn list_table(info: &str, content: &str) -> Option<ListTable> {
    let mut words = info.split_whitespace();
    if words.next() != Some("table") {
        return None;
    }
    let mut table = ListTable { header_rows: 1, header_columns: 0, rows: Vec::new() };
    for word in words {
        if let Some((name, value)) = word.split_once('=') {
            let Ok(value) = value.parse() else { continue };
            match name {
                "header-rows" => table.header_rows = value,
                "header-columns" => table.header_columns = value,
                _ => {}
            }
        }
    }
    let mut depth = 0;
    let mut cell_start: Option<usize> = None;
    for (event, range) in Parser::new_ext(content, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::List(_)) => depth += 1,
            Event::End(TagEnd::List(_)) => depth -= 1,
            Event::Start(Tag::Item) if depth == 1 => table.rows.push(Vec::new()),
            Event::Start(Tag::Item) if depth == 2 => cell_start = Some(range.start),
            Event::End(TagEnd::Item) if depth == 2 => {
                let start = cell_start.take()?;
                let row = table.rows.last_mut()?;
                row.push(cell_text(content, start..range.end));
            }
            Event::Start(_) | Event::Text(_) if depth == 0 => return None,
            _ => {}
        }
    }
    if table.rows.is_empty() || table.rows.iter().any(Vec::is_empty) {
        return None;
    }
    let width = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut table.rows {
        row.resize(width, String::new());
    }
    Some(table)
}

/// A list item's markdown without its marker, with its continuation lines unindented to the
/// column where its content starts.
fn cell_text(content: &str, range: Range<usize>) -> String {
    let line_start = content[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let column = range.start - line_start;
    let item = &content[range];
    let marker = item.chars().next().map_or(0, char::len_utf8);
    let first = item[marker..].strip_prefix(' ').unwrap_or(&item[marker..]);
    let indent = column + marker + 1;
    let mut lines = first.lines();
    let mut out = lines.next().unwrap_or("").to_owned();
    for line in lines {
        let spaces = line.len() - line.trim_start_matches(' ').len();
        out.push('\n');
        out.push_str(&line[spaces.min(indent)..]);
    }
    out.trim_end().to_owned()
}

/// The fenced blocks with a language that Zib gives a meaning, by their byte range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Fenced {
    Math(String),
    Mermaid(String),
    Table(Option<ListTable>),
}

/// The math, diagrams, and list tables in a body, in order.
pub fn fenced_blocks(body: &str) -> Vec<(Range<usize>, Fenced)> {
    let mut out = Vec::new();
    let mut current: Option<(usize, String, String)> = None;
    for (event, range) in Parser::new_ext(body, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                current = Some((range.start, info.to_string(), String::new()));
            }
            Event::Text(text) => {
                if let Some((_, _, content)) = &mut current {
                    content.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let Some((start, info, content)) = current.take() else { continue };
                let language = info.split_whitespace().next().unwrap_or("");
                let block = match language {
                    "math" => Fenced::Math(content),
                    "mermaid" => Fenced::Mermaid(content),
                    "table" => Fenced::Table(list_table(&info, &content)),
                    _ => continue,
                };
                out.push((start..range.end, block));
            }
            _ => {}
        }
    }
    out
}

/// The titles a raw editor showed for each target while a note was open.
#[derive(Clone, Debug, Default)]
pub struct ShownTitles(BTreeMap<Id, BTreeSet<String>>);

impl ShownTitles {
    pub fn remember(&mut self, target: Id, title: &str) {
        self.0.entry(target).or_default().insert(title.to_owned());
    }

    pub fn contains(&self, target: Id, title: &str) -> bool {
        self.0.get(&target).is_some_and(|titles| titles.contains(title))
    }
}

/// Gives each wikilink without a label its target's title, for a raw editor, and remembers each
/// title it gives.
pub fn fill_labels(
    body: &str,
    title: &mut dyn FnMut(Id) -> Option<String>,
    shown: &mut ShownTitles,
) -> String {
    let mut out = String::with_capacity(body.len());
    let mut last = 0;
    for link in wikilinks(body) {
        if link.label.is_some() {
            continue;
        }
        let Some(text) = title(link.target) else { continue };
        let insert_at = link.range.end - 2;
        out.push_str(&body[last..insert_at]);
        out.push('|');
        out.push_str(&escape_label(&text));
        last = insert_at;
        shown.remember(link.target, &text);
    }
    out.push_str(&body[last..]);
    out
}

/// Removes each label that equals its target's current title or a title the editor showed for it,
/// so the stored text keeps only labels the owner chose.
pub fn strip_labels(
    text: &str,
    title: &mut dyn FnMut(Id) -> Option<String>,
    shown: &ShownTitles,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for link in wikilinks(text) {
        let Some(label) = &link.label else { continue };
        let current = title(link.target);
        if current.as_deref() != Some(label.as_str()) && !shown.contains(link.target, label) {
            continue;
        }
        let inner = &text[link.range.clone()];
        let Some(bar) = find_label_bar(inner) else { continue };
        out.push_str(&text[last..link.range.start + bar]);
        out.push_str("]]");
        last = link.range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// The byte offset of the first unescaped `|` in a wikilink's text.
fn find_label_bar(link: &str) -> Option<usize> {
    let bytes = link.as_bytes();
    let mut i = link.find("[[")? + 2;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'|' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// The targets of every wikilink and embed in a body, for backlinks.
pub fn link_targets(body: &str) -> BTreeSet<Id> {
    wikilinks(body).into_iter().map(|link| link.target).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01k97krzh1fcx9rk9fhsgt1de7";
    const B: &str = "01fwhe4ydgfk1shh6w1g60eecf";

    fn id(text: &str) -> Id {
        Id::parse(text).unwrap()
    }

    #[test]
    fn finds_wikilinks_and_embeds() {
        let body = format!(
            "See [[{A}]] and ![[{B}|a \\| b \\] c]].\n\
            `[[{A}]]` \\[[{A}]] [[not an id]] [[{A}#part]]"
        );
        let links = wikilinks(&body);
        assert_eq!(links.len(), 3);
        assert!(!links[0].embed && links[0].label.is_none());
        assert!(links[1].embed);
        assert_eq!(links[1].label.as_deref(), Some("a | b ] c"));
        assert_eq!(links[2].fragment.as_deref(), Some("part"));
        assert_eq!(&body[links[0].range.clone()], format!("[[{A}]]"));
    }

    #[test]
    fn accepts_forgiving_targets() {
        let links = wikilinks("[[01K97-KRZH1-FCX9R-K9FHS-GTIDE7]]");
        assert_eq!(links[0].target, id(A));
    }

    #[test]
    fn reads_titles() {
        let mut none = |_| None;
        assert_eq!(title("\n\n   ## **Bold** idea ##\nbody", &mut none).markdown, "**Bold** idea");
        assert_eq!(title("  ==Lit== and =under=\n", &mut none).plain, "Lit and under");
        assert_eq!(title("#hashtag", &mut none).markdown, "#hashtag");
        assert_eq!(title("[web](https://example.com) page", &mut none).markdown, "web page");
        assert!(title("   \n\t\n", &mut none).is_empty());
        let mut resolve = |target: Id| {
            (target == id(B)).then(|| Title { markdown: "*Other*".into(), plain: "Other".into() })
        };
        assert_eq!(title(&format!("Re: [[{B}]]"), &mut resolve).markdown, "Re: *Other*");
        assert_eq!(title(&format!("[[{B}|mine]]"), &mut resolve).plain, "mine");
    }

    #[test]
    fn titles_that_link_to_each_other_stop() {
        let body =
            |note: Id| Some(if note == id(A) { format!("[[{B}]]") } else { format!("[[{A}]]") });
        let mut titles = Titles::new(&body);
        assert_eq!(titles.title(id(A)).unwrap().plain, A);
    }

    #[test]
    fn pairs_marks_like_emphasis() {
        let spans = marked_spans("=u= ==h== ===b=== a = b \\=no=");
        let marks: Vec<Mark> = spans.iter().map(|s| s.mark).collect();
        assert_eq!(marks, vec![Mark::Underline, Mark::Highlight, Mark::Both]);
        assert_eq!(marked_spans("x=5 and y=6").len(), 1);
        assert_eq!(marked_spans("x\\=5 and y\\=6").len(), 0);
        assert_eq!(marked_spans("====four====").len(), 0);
    }

    #[test]
    fn reads_list_tables() {
        let content =
            "- - Step\n  - Notes\n- - Install\n  - Download.\n\n    Then check.\n- - Only\n";
        let table = list_table("table header-columns=1 width=9", content).unwrap();
        assert_eq!((table.header_rows, table.header_columns), (1, 1));
        assert_eq!(table.rows[1][1], "Download.\n\nThen check.");
        assert_eq!(table.rows[2], vec!["Only".to_owned(), String::new()]);
        assert_eq!(list_table("table", "just text"), None);
        let body = format!("```table\n{content}```\n\n```mermaid\nflowchart LR\n```\n");
        assert_eq!(fenced_blocks(&body).len(), 2);
    }

    #[test]
    fn fills_and_strips_labels() {
        let stored = format!("See [[{A}]] and [[{B}|my label]].");
        let mut current = |target: Id| (target == id(A)).then(|| "Old title".to_owned());
        let mut shown = ShownTitles::default();
        let filled = fill_labels(&stored, &mut current, &mut shown);
        assert_eq!(filled, format!("See [[{A}|Old title]] and [[{B}|my label]]."));
        // The target's title changes while the note is open.
        let mut renamed = |target: Id| (target == id(A)).then(|| "New".to_owned());
        assert_eq!(strip_labels(&filled, &mut renamed, &shown), stored);
        // A label the owner edited stays.
        let edited = filled.replace("Old title", "Chosen");
        assert!(strip_labels(&edited, &mut renamed, &shown).contains("|Chosen]]"));
    }

    #[test]
    fn escapes_labels() {
        assert_eq!(wikilink(id(A), Some("a|b]\\c\nd")), format!("[[{A}|a\\|b\\]\\\\c d]]"));
        let link = &wikilinks(&wikilink(id(A), Some("a|b]\\c")))[0];
        assert_eq!(link.label.as_deref(), Some("a|b]\\c"));
    }
}
