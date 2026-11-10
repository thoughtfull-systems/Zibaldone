//! Spell checking for editable prose, with byte ranges in the note's Markdown source.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::ops::Range;

use loro::{LoroDoc, LoroValue, ValueOrContainer};
use pulldown_cmark::{Event, LinkType, Parser, Tag};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::editing::{Style, spans};
use crate::space::{SETTINGS, Space, note_body, note_map};
use crate::{Error, Id, Result, body, schema, space};

const DICTIONARY_SETTING: &str = "zib.spelling-dictionary.";
const DICTIONARY_TITLE: &str = "# Spelling Dictionary";
const IGNORE_SETTING: &str = "zib.spelling-ignore.";
const LEGACY_WORDS: &str = "spelling-words";

impl Space {
    /// Opens the space's editable dictionary, creating it if needed.
    pub fn spelling_dictionary(&mut self) -> Result<Id> {
        edit_dictionary(self, None)
    }

    /// Accepts a word by adding it to the space's dictionary note.
    pub fn add_spelling_word(&mut self, word: &str) -> Result<Id> {
        edit_dictionary(self, Some(word))
    }

    /// Ignores a spelling in one note, without changing its body or metadata.
    pub fn ignore_spelling_word(&mut self, note: Id, word: &str) -> Result<()> {
        let word = word_key(word).ok_or_else(|| Error::invalid("spelling word", word))?;
        self.set_setting(Some(note), &ignore_setting(&word), true)
    }
}

/// The base dictionary, shared by editors. Each check also takes the note's own accepted words.
pub struct Dictionary(spellbook::Dictionary);

impl Dictionary {
    pub fn new(
        affix: &str,
        words: &str,
    ) -> std::result::Result<Self, spellbook::ParseDictionaryError> {
        spellbook::Dictionary::new(affix, words).map(Self)
    }

    /// Misspellings in source order, excluding Markdown syntax and non-prose.
    pub fn misspellings(&self, text: &str, accepted: &HashSet<String>) -> Vec<Range<usize>> {
        word_ranges(text)
            .into_iter()
            .filter(|range| {
                let word = &text[range.clone()];
                !accepted.contains(&key(word)) && !self.0.check(&normalized(word))
            })
            .collect()
    }

    pub fn suggestions(&self, word: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        self.0.suggest(&normalized(word), &mut suggestions);
        suggestions
    }
}

fn normalized(word: &str) -> String {
    word.replace('’', "'").nfc().collect()
}

fn key(word: &str) -> String {
    normalized(word).to_lowercase().nfc().collect()
}

pub(crate) fn accepted_words(doc: &LoroDoc) -> HashSet<String> {
    words_in_dictionaries(doc, &dictionary_notes(doc))
}

pub(crate) fn ignore_setting(word: &str) -> String {
    format!("{IGNORE_SETTING}{word}")
}

pub(crate) fn ignored_words(doc: &LoroDoc, note: Id) -> HashSet<String> {
    let mut words = HashSet::new();
    if let Ok(note) = note_map(doc, note)
        && let Some(ValueOrContainer::Container(settings)) = note.get("settings")
        && let Ok(settings) = settings.into_map()
    {
        settings.for_each(|name, value| {
            if matches!(value, ValueOrContainer::Value(LoroValue::Bool(true)))
                && let Some(word) = name.strip_prefix(IGNORE_SETTING)
            {
                words.insert(word.to_owned());
            }
        });
    }
    words
}

fn dictionary_notes(doc: &LoroDoc) -> Vec<Id> {
    let mut ids = Vec::new();
    doc.get_map(SETTINGS).for_each(|name, value| {
        if matches!(value, ValueOrContainer::Value(LoroValue::Bool(true)))
            && let Some(id) =
                name.strip_prefix(DICTIONARY_SETTING).and_then(|id| Id::parse(id).ok())
        {
            ids.push(id);
        }
    });
    ids.sort_unstable();
    ids
}

fn words_in_dictionaries(doc: &LoroDoc, ids: &[Id]) -> HashSet<String> {
    if ids.is_empty() {
        return legacy_words(doc);
    }
    ids.iter()
        .filter_map(|id| note_body(doc, *id).ok())
        .flat_map(|note| words_in_body(&note.to_string()))
        .collect()
}

fn words_in_body(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .skip(1)
        .filter_map(word_key)
        .collect()
}

fn legacy_words(doc: &LoroDoc) -> HashSet<String> {
    let mut accepted = HashSet::new();
    doc.get_map(LEGACY_WORDS).for_each(|word, value| {
        if matches!(value, ValueOrContainer::Value(LoroValue::Bool(true)))
            && let Some(word) = word_key(word)
        {
            accepted.insert(word);
        }
    });
    accepted
}

fn word_key(word: &str) -> Option<String> {
    (is_word(word) && word.unicode_words().next() == Some(word)).then(|| key(word))
}

fn edit_dictionary(space: &mut Space, word: Option<&str>) -> Result<Id> {
    let candidate = word
        .map(|word| word_key(word).ok_or_else(|| Error::invalid("spelling word", word)))
        .transpose()?;
    let ids = dictionary_notes(space.doc());
    if let Some((id, body)) = ids.iter().find_map(|id| space.body(*id).ok().map(|body| (*id, body)))
    {
        if let Some((word, key)) = word.zip(candidate)
            && !words_in_dictionaries(space.doc(), &ids).contains(&key)
        {
            let text = body.to_string();
            let title = if text.trim().is_empty() {
                format!("\n{DICTIONARY_TITLE}\n")
            } else {
                String::new()
            };
            // The leading line break separates words even when two machines append at once.
            body.insert_utf8(text.len(), &format!("{title}\n{word}"))?;
            space.note_edited(id)?;
        }
        return Ok(id);
    }

    let words = words_in_dictionaries(space.doc(), &ids);
    let new_word =
        word.zip(candidate).filter(|(_, key)| !words.contains(key)).map(|(word, _)| word);
    let mut words: Vec<_> = words.into_iter().collect();
    words.sort_unstable();
    let mut text = format!("{DICTIONARY_TITLE}\n");
    for word in words.iter().map(String::as_str).chain(new_word) {
        let _ = write!(text, "\n{word}");
    }
    if words.is_empty() && new_word.is_none() {
        text.push('\n');
    }
    let id = Id::generate();
    space.insert_note(id, schema::NOTE, &text, &[])?;
    space.doc().get_map(SETTINGS).insert(&format!("{DICTIONARY_SETTING}{id}"), true)?;
    space::commit(space.doc());
    Ok(id)
}

fn is_word(word: &str) -> bool {
    word.chars().any(char::is_alphabetic) && !word.chars().any(|c| c.is_numeric() || c == '_')
}

fn word_ranges(text: &str) -> Vec<Range<usize>> {
    let mut excluded = Vec::new();
    for span in spans(text) {
        if matches!(span.style, Style::Checkbox(_) | Style::Code | Style::CodeBlock | Style::Math)
            || (span.style == Style::Link && span.syntax.is_empty())
        {
            excluded.push(span.range);
        } else {
            excluded.extend(span.syntax);
        }
    }
    let parser = Parser::new_ext(text, body::options());
    excluded.extend(parser.reference_definitions().iter().map(|(_, def)| def.span.clone()));
    excluded.extend(parser.into_offset_iter().filter_map(|(event, range)| match event {
        Event::Start(
            Tag::HtmlBlock
            | Tag::Image { .. }
            | Tag::Link { link_type: LinkType::Autolink | LinkType::Email, .. },
        )
        | Event::InlineHtml(_)
        | Event::TaskListMarker(_) => Some(range),
        _ => None,
    }));
    excluded.extend(body::wikilinks(text).into_iter().filter(|link| link.embed).map(|l| l.range));

    excluded.sort_unstable_by_key(|range| range.start);
    let mut words = Vec::new();
    let mut start = 0;
    for range in excluded.into_iter().chain(std::iter::once(text.len()..text.len())) {
        if range.start > start {
            let mut offset = start;
            for part in text[start..range.start].split_inclusive(char::is_whitespace) {
                // Bare addresses in prose don't always become Markdown links.
                if !is_address(part.trim()) {
                    words.extend(part.unicode_word_indices().filter_map(|(i, word)| {
                        is_word(word).then_some(offset + i..offset + i + word.len())
                    }));
                }
                offset += part.len();
            }
        }
        start = start.max(range.end);
    }
    words
}

fn is_address(part: &str) -> bool {
    let trimmed = part.trim_matches(['(', ')', '[', ']', '<', '>', '"', '\'']);
    trimmed.contains("://")
        || trimmed.starts_with("www.")
        || trimmed.split_once('@').is_some_and(|(name, domain)| {
            !name.is_empty() && domain.trim_end_matches('.').contains('.')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn finds_editable_words_and_leaves_out_markdown_and_addresses() {
        let id = Id::generate();
        let text = format!(
            "# Héading\n- [x] **mistake** `snippet` $equation$\n\n\
            [label](https://address.test) [[{id}|words]] ![[{id}|embed]]\n\
            [reference][id] <me@example.com> https://example.com me@example.com\n\n\
            [id]: /location \"title\"\n\n\
            ```lang\nblock\n```\n\nrock’n’roll and don't"
        );
        let words: Vec<_> = word_ranges(&text).into_iter().map(|range| &text[range]).collect();
        assert_eq!(
            words,
            ["Héading", "mistake", "label", "words", "reference", "rock’n’roll", "and", "don't"]
        );
    }

    #[test]
    fn dictionary_note_merges_edits_and_stays_in_its_space() {
        let mut space = Space::create(Id::generate()).unwrap();
        let note = space.create_note("note", "Zibaldone").unwrap();
        let editor = Editor::new(space.open_body(note).unwrap());
        assert!(editor.spelling_words().is_empty());
        let dictionary = space.spelling_dictionary().unwrap();
        let mut peer = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        assert_eq!(space.add_spelling_word("Zibaldone").unwrap(), dictionary);
        assert!(!editor.can_undo());
        assert_eq!(peer.add_spelling_word("Cafe\u{301}’s").unwrap(), dictionary);
        space.import_checked(&peer.snapshot().unwrap()).unwrap();
        let words = HashSet::from(["zibaldone".into(), "café's".into()]);
        assert_eq!(editor.spelling_words(), words);
        let mut dictionary_editor = Editor::new(space.open_body(dictionary).unwrap());
        dictionary_editor.select_all();
        dictionary_editor.insert("# Renamed vocabulary\n\nCafé's\n\nTwo words");
        assert_eq!(editor.spelling_words(), HashSet::from(["café's".into()]));
        assert_eq!(space.spelling_dictionary().unwrap(), dictionary);
        let mut reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        assert_eq!(
            Editor::new(reopened.open_body(note).unwrap()).spelling_words(),
            HashSet::from(["café's".into()])
        );
        let mut other = Space::create(Id::generate()).unwrap();
        let note = other.create_note("note", "Zibaldone").unwrap();
        assert!(Editor::new(other.open_body(note).unwrap()).spelling_words().is_empty());
    }

    #[test]
    fn dictionary_concurrent_first_creation_keeps_both_notes() {
        let mut space = Space::create(Id::generate()).unwrap();
        let mut peer = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let left = space.add_spelling_word("Zibaldone").unwrap();
        let right = peer.add_spelling_word("Quxley").unwrap();
        space.import_checked(&peer.snapshot().unwrap()).unwrap();
        assert_eq!(
            accepted_words(space.doc()),
            HashSet::from(["zibaldone".into(), "quxley".into()])
        );
        assert_eq!(space.spelling_dictionary().unwrap(), left.min(right));
        space.delete_note(left).unwrap();
        assert_eq!(accepted_words(space.doc()), HashSet::from(["quxley".into()]));
    }

    #[test]
    fn dictionary_migrates_legacy_words_once() {
        let mut space = Space::create(Id::generate()).unwrap();
        space.doc().get_map(LEGACY_WORDS).insert("zibaldone", true).unwrap();
        space.commit();
        assert_eq!(accepted_words(space.doc()), HashSet::from(["zibaldone".into()]));
        let dictionary = space.spelling_dictionary().unwrap();
        assert_eq!(space.note(dictionary).unwrap().body, "# Spelling Dictionary\n\nzibaldone");
        space.set_body(dictionary, "# Spelling Dictionary\n\n").unwrap();
        assert!(accepted_words(space.doc()).is_empty());
        space.delete_note(dictionary).unwrap();
        assert_ne!(space.spelling_dictionary().unwrap(), dictionary);
        assert!(accepted_words(space.doc()).is_empty());
    }

    #[test]
    fn ignored_spellings_sync_independently_and_follow_the_note_on_move() {
        let mut space = Space::create(Id::generate()).unwrap();
        let note = space.create_note("note", "Café’s and Quxley").unwrap();
        let original = space.note(note).unwrap();
        let other = space.create_note("note", &original.body).unwrap();
        space.add_spelling_word("Zibaldone").unwrap();
        let editor = Editor::new(space.open_body(note).unwrap());
        let mut peer = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();

        space.ignore_spelling_word(note, "CAFE\u{301}’S").unwrap();
        peer.ignore_spelling_word(note, "Quxley").unwrap();
        space.import_checked(&peer.snapshot().unwrap()).unwrap();
        let expected = HashSet::from(["café's".into(), "quxley".into(), "zibaldone".into()]);
        assert_eq!(editor.spelling_words(), expected);
        assert_eq!(space.note(note).unwrap(), original);
        assert!(!editor.can_undo());
        assert_eq!(
            Editor::new(space.open_body(other).unwrap()).spelling_words(),
            HashSet::from(["zibaldone".into()])
        );

        let mut reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        assert_eq!(Editor::new(reopened.open_body(note).unwrap()).spelling_words(), expected);
        let mut target = Space::create(Id::generate()).unwrap();
        reopened.move_note_to(note, &mut target).unwrap();
        assert_eq!(
            Editor::new(target.open_body(note).unwrap()).spelling_words(),
            HashSet::from(["café's".into(), "quxley".into()])
        );
    }
}
