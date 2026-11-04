//! The search index of one space, as decision ZIB0008 describes: SQLite with FTS5 for text, plain
//! tables for filter fields and backlinks.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::body;
use crate::schema::{self, Kind, SearchMode, Value};
use crate::space::FileHash;
use crate::{Id, Result, Space};

/// Marks the start of a match in a snippet, for the app to highlight.
pub const MATCH_START: char = '\u{2}';
/// Marks the end of a match in a snippet.
pub const MATCH_END: char = '\u{3}';

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS notes (
        id TEXT PRIMARY KEY,
        type TEXT NOT NULL,
        title TEXT NOT NULL,
        updated INTEGER NOT NULL
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS text USING fts5(
        id UNINDEXED, title, body, fields, tokenize = 'unicode61 remove_diacritics 2'
    );
    CREATE TABLE IF NOT EXISTS filters (
        note TEXT NOT NULL,
        field TEXT NOT NULL,
        text TEXT,
        number INTEGER
    );
    CREATE INDEX IF NOT EXISTS filters_text ON filters (field, text);
    CREATE INDEX IF NOT EXISTS filters_number ON filters (field, number);
    CREATE INDEX IF NOT EXISTS filters_note ON filters (note);
    CREATE TABLE IF NOT EXISTS links (source TEXT NOT NULL, target TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS links_target ON links (target);
    CREATE INDEX IF NOT EXISTS links_source ON links (source);
";

const INSERT_FILTER: &str =
    "INSERT INTO filters (note, field, text, number) VALUES (?1, ?2, ?3, ?4)";

/// One result of a search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hit {
    pub id: Id,
    /// The note's title as plain text.
    pub title: String,
    /// Words around the matches, with each match between `MATCH_START` and `MATCH_END`.
    pub snippet: String,
}

/// A space's search index.
pub struct Index {
    db: Connection,
}

impl Index {
    /// Opens the index at `path`, creating it if it's missing.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "wal")?;
        db.pragma_update(None, "synchronous", "normal")?;
        Self::with(db)
    }

    /// An index in memory, for tests and for spaces opened only briefly.
    pub fn in_memory() -> Result<Self> {
        Self::with(Connection::open_in_memory()?)
    }

    fn with(db: Connection) -> Result<Self> {
        db.execute_batch(SCHEMA)?;
        Ok(Self { db })
    }

    /// Rebuilds the whole index from the space. `file_text` gives the text of a blob the machine
    /// holds, for file fields with the `text` search mode.
    pub fn rebuild(
        &mut self,
        space: &Space,
        file_text: &dyn Fn(FileHash) -> Option<String>,
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute_batch(
            "DELETE FROM notes; DELETE FROM text; DELETE FROM filters; DELETE FROM links;",
        )?;
        tx.commit()?;
        for id in space.note_ids() {
            self.update_note(space, id, file_text)?;
        }
        Ok(())
    }

    /// Updates one note's rows, or removes them if the space no longer holds the note.
    pub fn update_note(
        &mut self,
        space: &Space,
        id: Id,
        file_text: &dyn Fn(FileHash) -> Option<String>,
    ) -> Result<()> {
        let key = id.to_string();
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM notes WHERE id = ?1", [&key])?;
        tx.execute("DELETE FROM text WHERE id = ?1", [&key])?;
        tx.execute("DELETE FROM filters WHERE note = ?1", [&key])?;
        tx.execute("DELETE FROM links WHERE source = ?1", [&key])?;
        if let Ok(note) = space.note(id) {
            let title = space.title(id)?.plain;
            let mut texts = Vec::new();
            if let Some(ty) = schema::read_type(space.doc(), &note.type_name) {
                for (name, def) in &ty.fields {
                    let Some(value) = note.fields.get(name).and_then(|v| Value::from_loro(def, v))
                    else {
                        continue;
                    };
                    for item in flatten(value) {
                        match def.search {
                            SearchMode::Text => match &item {
                                Value::File(hash) => texts.extend(file_text(*hash)),
                                other => texts.extend(text_of(other)),
                            },
                            SearchMode::Filter => {
                                let (text, number) = filter_of(def.value_kind(), &item);
                                tx.execute(INSERT_FILTER, params![key, name, text, number])?;
                            }
                            SearchMode::None => {}
                        }
                    }
                }
            }
            tx.execute(
                "INSERT INTO notes (id, type, title, updated) VALUES (?1, ?2, ?3, ?4)",
                params![key, note.type_name, title, note.meta.updated],
            )?;
            tx.execute(
                "INSERT INTO text (id, title, body, fields) VALUES (?1, ?2, ?3, ?4)",
                params![key, title, note.body, texts.join("\n\n")],
            )?;
            for target in body::link_targets(&note.body) {
                tx.execute(
                    "INSERT INTO links (source, target) VALUES (?1, ?2)",
                    [&key, &target.to_string()],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Notes that match every word of `query`, each as a prefix, best matches first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        let Some(query) = fts_query(query) else { return Ok(Vec::new()) };
        let mut statement = self.db.prepare(
            "SELECT id, title, snippet(text, -1, ?2, ?3, '…', 12) FROM text \
            WHERE text MATCH ?1 ORDER BY bm25(text, 0.0, 10.0, 1.0, 1.0) LIMIT ?4",
        )?;
        let hits = statement
            .query_map(
                params![
                    query,
                    MATCH_START.to_string(),
                    MATCH_END.to_string(),
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .filter_map(|row| {
                let (id, title, snippet) = row.ok()?;
                Some(Hit { id: Id::parse(&id).ok()?, title, snippet })
            })
            .collect();
        Ok(hits)
    }

    /// The notes whose bodies link to `target`.
    pub fn backlinks(&self, target: Id) -> Result<Vec<Id>> {
        let mut statement =
            self.db.prepare("SELECT DISTINCT source FROM links WHERE target = ?1")?;
        let ids = statement
            .query_map([target.to_string()], |row| row.get::<_, String>(0))?
            .filter_map(|id| Id::parse(&id.ok()?).ok())
            .collect();
        Ok(ids)
    }

    /// The notes whose filter field `field` holds `value` exactly.
    pub fn filter_equals(&self, field: &str, value: &str) -> Result<Vec<Id>> {
        let mut statement =
            self.db.prepare("SELECT DISTINCT note FROM filters WHERE field = ?1 AND text = ?2")?;
        let ids = statement
            .query_map([field, value], |row| row.get::<_, String>(0))?
            .filter_map(|id| Id::parse(&id.ok()?).ok())
            .collect();
        Ok(ids)
    }

    /// The plain title the index holds for a note.
    pub fn title(&self, id: Id) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT title FROM notes WHERE id = ?1", [id.to_string()], |row| row.get(0))
            .optional()?)
    }
}

fn flatten(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items,
        other => vec![other],
    }
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::Text(s) | Value::Decimal(s) | Value::Date(s) | Value::Time(s) | Value::Choice(s) => {
            Some(s.clone())
        }
        Value::Integer(n) | Value::Timestamp(n) => Some(n.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::Link(_) | Value::File(_) | Value::List(_) => None,
    }
}

/// A filter value as text for exact matches and sorting, or as a number for ranges.
fn filter_of(kind: Kind, value: &Value) -> (Option<String>, Option<i64>) {
    match (kind, value) {
        (_, Value::Integer(n) | Value::Timestamp(n)) => (None, Some(*n)),
        (_, Value::Boolean(b)) => (None, Some(i64::from(*b))),
        (_, Value::Link(id)) => (Some(id.to_string()), None),
        (_, Value::File(hash)) => (Some(hash.to_string()), None),
        (_, other) => (text_of(other), None),
    }
}

/// Turns what the owner typed into an FTS5 query that matches every word as a prefix.
fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_titles_bodies_and_file_text() {
        let mut space = Space::create(Id::generate()).unwrap();
        let ideas =
            space.create_note(schema::NOTE, "# Garden ideas\nPlant tomatoes in spring.").unwrap();
        let other =
            space.create_note(schema::NOTE, &format!("Groceries\nSee [[{ideas}]].")).unwrap();
        let file = space.create_note(schema::FILE, "manual.pdf").unwrap();
        let text_hash = FileHash::of(b"text");
        space.add_file(text_hash, 4, "text/plain").unwrap();
        space.set_field(file, "text", &Value::File(text_hash)).unwrap();

        let mut index = Index::in_memory().unwrap();
        let text =
            |hash: FileHash| (hash == text_hash).then(|| "Torque the bolts carefully".to_owned());
        index.rebuild(&space, &text).unwrap();

        let hits = index.search("tomat", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, ideas);
        assert_eq!(hits[0].title, "Garden ideas");
        assert!(hits[0].snippet.contains(&format!("{MATCH_START}tomatoes{MATCH_END}")));
        assert_eq!(index.search("torque", 10).unwrap()[0].id, file);
        assert_eq!(index.backlinks(ideas).unwrap(), vec![other]);

        space.delete_note(ideas).unwrap();
        index.update_note(&space, ideas, &text).unwrap();
        assert_eq!(index.search("tomatoes", 10).unwrap(), Vec::new());
        assert_eq!(index.search("  ", 10).unwrap(), Vec::new());
    }
}
