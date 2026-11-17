//! Plans a structured search once for the spaces in scope, then applies one sort across them.

use std::cmp::{Ordering, Reverse};
use std::fmt::Write as _;

use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::{Id, Result};

use super::query::{Operator, Query};
use super::{Hit, Index, MATCH_END, MATCH_START, NOTE_ID_FIELD, TYPE_FIELD, fts_query};

/// How to order search results. Field-only searches use last modified by default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Sort {
    #[default]
    Relevance,
    Modified,
    Title,
}

impl Sort {
    const fn sql(self) -> &'static str {
        match self {
            Self::Relevance => "score, n.updated DESC, n.id",
            Self::Modified => "n.updated DESC, n.id",
            Self::Title => "n.title COLLATE NOCASE, n.id",
        }
    }

    pub fn compare_titles(a: &str, b: &str) -> Ordering {
        a.bytes().map(|b| b.to_ascii_lowercase()).cmp(b.bytes().map(|b| b.to_ascii_lowercase()))
    }
}

/// A result with the values needed to merge ordered lists from different spaces.
struct RankedHit {
    hit: Hit,
    modified: i64,
    score: f64,
}

impl RankedHit {
    fn compare(&self, other: &Self, sort: Sort) -> Ordering {
        let modified = || Reverse(self.modified).cmp(&Reverse(other.modified));
        match sort {
            Sort::Relevance => self.score.total_cmp(&other.score).then_with(modified),
            Sort::Modified => modified(),
            Sort::Title => Sort::compare_titles(&self.hit.title, &other.hit.title),
        }
        .then_with(|| self.hit.id.cmp(&other.hit.id))
    }
}

/// Searches the supplied spaces. Invalid expressions return an error even if no note matches.
pub fn search_indexes<'a>(
    indexes: impl IntoIterator<Item = (Id, &'a Index)>,
    text: &str,
    sort: Sort,
    limit: usize,
) -> Result<Vec<(Id, Hit)>> {
    let indexes: Vec<_> = indexes.into_iter().collect();
    let query = Query::parse(text)?;
    let Some(statement) = Statement::new(&indexes, &query, sort, limit)? else {
        return Ok(Vec::new());
    };
    let mut hits = Vec::new();
    for (space, index) in indexes {
        hits.extend(statement.run(index)?.into_iter().map(|hit| (space, hit)));
    }
    hits.sort_by(|(a_space, a), (b_space, b)| {
        a.compare(b, sort).then_with(|| a_space.cmp(b_space))
    });
    hits.truncate(limit);
    Ok(hits.into_iter().map(|(space, hit)| (space, hit.hit)).collect())
}

/// SQL assembled from fixed fragments, with every query value bound separately.
#[derive(Default)]
struct Statement {
    args: Vec<Value>,
    matches: Vec<String>,
    predicates: Vec<String>,
    sql: String,
}

impl Statement {
    fn bind(&mut self, value: Value) -> String {
        self.args.push(value);
        format!("?{}", self.args.len())
    }

    fn text_match(&mut self, query: String, field: Option<&str>) {
        let text = self.bind(query.into());
        let start = self.bind(MATCH_START.to_string().into());
        let end = self.bind(MATCH_END.to_string().into());
        let n = self.matches.len();
        let (from, snippet, score) = if let Some(field) = field {
            let field = self.bind(field.to_owned().into());
            (
                format!("field_text WHERE field_text MATCH {text} AND field = {field}"),
                format!("note, snippet(field_text, 2, {start}, {end}, '…', 12)"),
                "rank",
            )
        } else {
            (
                format!("text WHERE text MATCH {text}"),
                format!("id, snippet(text, -1, {start}, {end}, '…', 12)"),
                "bm25(text, 0.0, 10.0, 1.0, 1.0)",
            )
        };
        self.matches
            .push(format!("m{n}(note, excerpt, score) AS (SELECT {snippet}, {score} FROM {from})"));
    }

    fn new(
        indexes: &[(Id, &Index)],
        query: &Query<'_>,
        sort: Sort,
        limit: usize,
    ) -> Result<Option<Self>> {
        let mut statement = Self::default();
        if let Some(text) = fts_query(&query.text) {
            statement.text_match(text, None);
        }
        for clause in &query.fields {
            if clause.field == NOTE_ID_FIELD {
                let id = statement.bind(clause.note_id()?.to_string().into());
                statement.predicates.push(format!("n.id = {id}"));
                continue;
            }
            if clause.field == TYPE_FIELD {
                let kind = statement.bind(clause.type_value()?.to_owned().into());
                statement.predicates.push(format!("n.type = {kind}"));
                continue;
            }
            let defs: Vec<_> = indexes
                .iter()
                .flat_map(|(_, index)| &index.types)
                .filter_map(|ty| ty.fields.get(clause.field))
                .collect();
            clause.validate(&defs)?;
            if matches!(clause.op, Operator::Contains) {
                statement.text_match(clause.text_query()?, Some(clause.field));
                continue;
            }
            let field = statement.bind(clause.field.to_owned().into());
            let comparison = if let Some(op) = clause.op.comparison() {
                let value = clause.comparison_value(&defs)?;
                let column = if matches!(value, Value::Integer(_)) { "number" } else { "text" };
                let arg = statement.bind(value);
                format!(" AND f.{column} {op} {arg}")
            } else {
                String::new()
            };
            let not = if matches!(clause.op, Operator::Missing) { "NOT " } else { "" };
            statement.predicates.push(format!(
                "{not}EXISTS (SELECT 1 FROM filters AS f \
                WHERE f.note = n.id AND f.field = {field}{comparison})"
            ));
        }
        if statement.matches.is_empty() && statement.predicates.is_empty() {
            return Ok(None);
        }
        statement.finish(sort, limit);
        Ok(Some(statement))
    }

    fn finish(&mut self, sort: Sort, limit: usize) {
        let sql = &mut self.sql;
        let (excerpt, score) = if self.matches.is_empty() {
            ("''", "0.0".to_owned())
        } else {
            write!(sql, "WITH {} ", self.matches.join(", ")).unwrap();
            let score = (0..self.matches.len()).map(|n| format!("m{n}.score")).collect::<Vec<_>>();
            ("m0.excerpt", score.join(" + "))
        };
        write!(sql, "SELECT n.id, n.title, {excerpt}, n.updated, {score} AS score FROM notes n")
            .unwrap();
        for n in 0..self.matches.len() {
            write!(sql, " JOIN m{n} ON m{n}.note = n.id").unwrap();
        }
        if !self.predicates.is_empty() {
            write!(sql, " WHERE {}", self.predicates.join(" AND ")).unwrap();
        }
        let order = sort.sql();
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        write!(sql, " ORDER BY {order} LIMIT {limit}").unwrap();
    }

    fn run(&self, index: &Index) -> Result<Vec<RankedHit>> {
        let mut statement = index.db.prepare(&self.sql)?;
        let rows = statement.query_map(params_from_iter(&self.args), |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
        let mut hits = Vec::new();
        for row in rows {
            let (id, title, snippet, modified, score) = row?;
            hits.push(RankedHit {
                hit: Hit { id: Id::parse(&id)?, title, snippet },
                modified,
                score,
            });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Error, Space, schema};

    use super::*;

    #[test]
    fn finds_a_note_by_its_metadata_id() {
        let mut space = Space::create(Id::generate()).unwrap();
        let note = space.create_note(schema::NOTE, "Garden notes").unwrap();
        space.create_note(schema::NOTE, "Garden plans").unwrap();
        let mut index = Index::in_memory().unwrap();
        index.rebuild(&space, &|_| None).unwrap();
        let search =
            |query: &str| search_indexes([(space.id().unwrap(), &index)], query, Sort::Title, 10);

        let upper_id = note.to_string().to_ascii_uppercase();
        let hits = search(&format!("garden @note-id=\"{upper_id}\"")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.id, note);
        assert!(search("@note-id=\"01m7mpqsqre0b8y1y7gff6bv88\"").unwrap().is_empty());
        assert!(matches!(search("@note-id=invalid"), Err(Error::SearchQuery(_))));
    }

    #[test]
    fn combines_text_and_fields_and_distinguishes_invalid_values_from_missing_fields() {
        let mut space = Space::create(Id::generate()).unwrap();
        let article = space.create_note(schema::ARTICLE, "Tomato growing").unwrap();
        let other = space.create_note(schema::ARTICLE, "Tomato tips by Jane Doe").unwrap();
        let plain = space.create_note(schema::NOTE, "Tomato planting").unwrap();
        space
            .set_field(
                article,
                "authors",
                &schema::Value::List(vec![
                    schema::Value::Text("Jane Doe".into()),
                    schema::Value::Text("Alex Jones".into()),
                ]),
            )
            .unwrap();
        space
            .set_field(article, "published-on", &schema::Value::Date("2025-04-01".into()))
            .unwrap();
        space.set_field(other, "published-on", &schema::Value::Date("2024-12-31".into())).unwrap();
        let mut index = Index::in_memory().unwrap();
        index.rebuild(&space, &|_| None).unwrap();
        let query =
            |index, text| search_indexes([(space.id().unwrap(), index)], text, Sort::Relevance, 10);
        let hits =
            query(&index, r#"tomato @authors:"Jane Doe" @published-on>=2025-01-01"#).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.id, article);
        let scoped = query(&index, "@authors:jan").unwrap();
        assert_eq!(scoped.len(), 1);
        assert!(scoped[0].1.snippet.contains(&format!("{MATCH_START}Jane{MATCH_END}")));
        assert!(query(&index, "@authors=Jane").unwrap().is_empty());
        assert_eq!(query(&index, r#"@authors="Jane Doe""#).unwrap()[0].1.id, article);
        assert_eq!(query(&index, "tomato @published-on!?").unwrap()[0].1.id, plain);
        assert_eq!(query(&index, "@type=article @authors!?").unwrap()[0].1.id, other);
        assert_eq!(query(&index, "@published-on?").unwrap().len(), 2);
        for invalid in ["@published-on>=2025-02-30", "@unknown=1", "@authors>Jane", "@file?"] {
            assert!(matches!(query(&index, invalid), Err(Error::SearchQuery(_))), "{invalid}");
        }

        // On changing a text field, update both its scoped index and its whole-value entries.
        space.set_field(article, "authors", &schema::Value::List(vec![])).unwrap();
        index.update_note(&space, article, &|_| None).unwrap();
        assert!(
            search_indexes([(space.id().unwrap(), &index)], "@authors:jan", Sort::Title, 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            search_indexes([(space.id().unwrap(), &index)], "@authors?", Sort::Title, 1)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn compares_decimals_exactly_and_sorts_across_spaces_before_limiting() {
        let make_index = |title, amount: &str| {
            let mut space = Space::create(Id::generate()).unwrap();
            let ty = schema::TypeDef {
                name: "expense".into(),
                fields: [(
                    "amount".into(),
                    schema::FieldDef::new(schema::Kind::Decimal, schema::SearchMode::Filter),
                )]
                .into(),
            };
            schema::write_schemas(space.doc(), &[ty]).unwrap();
            let id = space.create_note("expense", title).unwrap();
            space.set_field(id, "amount", &schema::Value::Decimal(amount.into())).unwrap();
            let mut index = Index::in_memory().unwrap();
            index.rebuild(&space, &|_| None).unwrap();
            (space.id().unwrap(), index, id)
        };
        let (a, first, zulu) = make_index("Zulu", "-1000000000000000.0000001");
        let (b, second, alpha) = make_index("alpha", "-1000000000000000.0");
        let run = |query, limit| {
            search_indexes([(a, &first), (b, &second)], query, Sort::Title, limit).unwrap()[0].1.id
        };
        assert_eq!(run("@amount<=0", 1), alpha);
        assert_eq!(run("@amount=-1000000000000000", 2), alpha);
        assert_eq!(run("@amount<-1000000000000000", 2), zulu);
    }
}
