//! Field syntax and validation for the note list's search box.

use crate::schema::{self, FieldDef, Kind, SearchMode};
use crate::{Error, Id, Result};

use super::fts_query;

#[derive(Clone, Copy)]
pub(super) enum Operator {
    Contains,
    Equal,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Missing,
    Present,
}

impl Operator {
    fn take(text: &str) -> Option<(Self, &str)> {
        [
            ("!?", Self::Missing),
            (":", Self::Contains),
            ("<=", Self::LessEqual),
            ("<", Self::Less),
            ("=", Self::Equal),
            (">=", Self::GreaterEqual),
            (">", Self::Greater),
            ("?", Self::Present),
        ]
        .into_iter()
        .find_map(|(prefix, op)| Some((op, text.strip_prefix(prefix)?)))
    }

    pub(super) fn comparison(self) -> Option<&'static str> {
        match self {
            Self::Equal => Some("="),
            Self::Greater => Some(">"),
            Self::GreaterEqual => Some(">="),
            Self::Less => Some("<"),
            Self::LessEqual => Some("<="),
            _ => None,
        }
    }
}

pub(super) struct Clause<'a> {
    pub field: &'a str,
    pub op: Operator,
    pub value: String,
}

#[derive(Default)]
pub(super) struct Query<'a> {
    pub text: String,
    pub fields: Vec<Clause<'a>>,
}

impl<'a> Query<'a> {
    pub fn parse(mut rest: &'a str) -> Result<Self> {
        let mut query = Self::default();
        while !rest.is_empty() {
            rest = rest.trim_start();
            let (word, tail) = split_word(rest);
            if let Some(field) = word.strip_prefix('@') {
                let end = field.find(['!', ':', '<', '=', '>', '?']);
                if let Some(end) = end {
                    let name = &field[..end];
                    if !schema::is_name(name) {
                        return Err(error(format!("Invalid field name {name:?}")));
                    }
                    // Read the value from the full input so a quoted value can contain spaces.
                    let Some((op, value)) = Operator::take(&rest[1 + end..]) else {
                        return Err(error(format!("Invalid operator for {name}")));
                    };
                    let (value, tail) = match op {
                        Operator::Missing | Operator::Present => (String::new(), value),
                        _ => take_value(value, name)?,
                    };
                    if tail.starts_with(|c: char| !c.is_whitespace()) {
                        return Err(error(format!("Expected a space after the {name} condition")));
                    }
                    if query.fields.len() == 16 {
                        return Err(error("Use at most 16 field conditions"));
                    }
                    query.fields.push(Clause { field: name, op, value });
                    rest = tail;
                    continue;
                }
            }
            // Quotes in ordinary text keep a leading @ literal but use the existing word search.
            if let Some(quoted) = rest.strip_prefix('"')
                && let Some((value, tail)) = quoted.split_once('"')
            {
                query.text.push_str(value);
                rest = tail;
            } else {
                query.text.push_str(word);
                rest = tail;
            }
            query.text.push(' ');
        }
        Ok(query)
    }
}

fn split_word(text: &str) -> (&str, &str) {
    text.split_at(text.find(char::is_whitespace).unwrap_or(text.len()))
}

fn take_value<'a>(text: &'a str, field: &str) -> Result<(String, &'a str)> {
    let Some(quoted) = text.strip_prefix('"') else {
        let (value, tail) = split_word(text);
        if value.is_empty() {
            return Err(error(format!("Enter a value for {field}")));
        }
        return Ok((value.into(), tail));
    };
    let mut value = String::new();
    let mut chars = quoted.char_indices();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' => return Ok((value, &quoted[i + 1..])),
            '\\' => match chars.next() {
                Some((_, ch @ ('"' | '\\'))) => value.push(ch),
                _ => return Err(error(format!("Invalid escape in the value for {field}"))),
            },
            _ => value.push(ch),
        }
    }
    Err(error(format!("Close the quoted value for {field}")))
}

fn error(message: impl Into<String>) -> Error {
    Error::SearchQuery(message.into())
}

impl Clause<'_> {
    pub fn note_id(&self) -> Result<Id> {
        if !matches!(self.op, Operator::Equal) {
            return Err(error("Use @note-id= followed by a note ID"));
        }
        Id::parse(&self.value).map_err(|_| error(format!("Invalid note ID {:?}", self.value)))
    }

    pub fn text_query(&self) -> Result<String> {
        fts_query(&self.value)
            .ok_or_else(|| error(format!("Enter search words for {}", self.field)))
    }

    pub fn type_value(&self) -> Result<&str> {
        if !matches!(self.op, Operator::Equal) {
            return Err(error("Use @type= followed by a note type"));
        }
        if !schema::is_name(&self.value) {
            return Err(error(format!("Invalid note type {:?}", self.value)));
        }
        Ok(&self.value)
    }

    pub fn validate(&self, defs: &[&FieldDef]) -> Result<()> {
        let field = self.field;
        if defs.is_empty() {
            return Err(error(format!("Unknown search field {field}")));
        }
        if defs.iter().all(|def| def.search == SearchMode::None) {
            return Err(error(format!("{field} isn't searchable")));
        }
        match self.op {
            Operator::Contains => {
                if !defs.iter().any(|def| def.search == SearchMode::Text) {
                    return Err(error(format!("{field} doesn't support searching for words")));
                }
                self.text_query()?;
            }
            Operator::Missing | Operator::Present => {}
            _ => {
                self.comparison_value(defs)?;
            }
        }
        Ok(())
    }

    pub fn comparison_value(&self, defs: &[&FieldDef]) -> Result<rusqlite::types::Value> {
        let field = self.field;
        let raw = self.value.as_str();
        let mut comparable = defs.iter().copied().filter(|def| comparable(def));
        let Some(def) = comparable.next() else {
            return Err(error(format!("{field} doesn't support value comparisons")));
        };
        let kind = def.value_kind();
        if comparable.any(|def| def.value_kind() != kind) {
            return Err(error(format!("{field} has more than one value kind in these spaces")));
        }
        if !matches!(self.op, Operator::Equal)
            && !matches!(
                kind,
                Kind::Date | Kind::Decimal | Kind::Integer | Kind::Time | Kind::Timestamp
            )
        {
            return Err(error(format!("{field} doesn't support ordered comparisons")));
        }
        let invalid = || {
            let expected = match kind {
                Kind::Boolean => "true or false",
                Kind::Date => "a date in YYYY-MM-DD format",
                Kind::Decimal => "a decimal number",
                Kind::Integer => "an integer",
                Kind::Link => "a note ID",
                Kind::Time => "a time in HH:MM or HH:MM:SS format",
                Kind::Timestamp => "an integer timestamp in milliseconds since the Unix epoch",
                _ => "a valid field value",
            };
            error(format!("Invalid value {raw:?} for {field}. Use {expected}"))
        };
        let value = match kind {
            Kind::Boolean => i64::from(raw.parse::<bool>().map_err(|_| invalid())?).into(),
            Kind::Date if schema::is_date(raw) => raw.to_owned().into(),
            Kind::Decimal if schema::is_decimal(raw) => decimal_key(raw).into(),
            Kind::Integer | Kind::Timestamp => raw.parse::<i64>().map_err(|_| invalid())?.into(),
            Kind::Link => Id::parse(raw).map_err(|_| invalid())?.to_string().into(),
            Kind::Time if schema::is_time(raw) => time_key(raw).into(),
            Kind::Choice if defs.iter().any(|d| comparable_choice(d, raw)) => raw.to_owned().into(),
            Kind::Text => raw.to_owned().into(),
            _ => return Err(invalid()),
        };
        Ok(value)
    }
}

fn comparable_choice(def: &FieldDef, value: &str) -> bool {
    comparable(def) && def.choices.iter().any(|choice| choice == value)
}

pub(super) fn comparable(def: &FieldDef) -> bool {
    def.search != SearchMode::None && def.value_kind() != Kind::File
}

pub(super) fn time_key(time: &str) -> String {
    if time.len() == 5 { format!("{time}:00") } else { time.into() }
}

/// Sorts decimal strings by numeric value without converting them to floating point. Encode the
/// whole part's length before its digits, and reverse the magnitude ordering for negatives.
/// The final slash sorts before digits, including after reversal for prefix comparisons.
pub(super) fn decimal_key(value: &str) -> String {
    let magnitude = value.strip_prefix('-').unwrap_or(value);
    let (whole, fraction) = magnitude.split_once('.').unwrap_or((magnitude, ""));
    let whole = whole.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    let key = format!("{:016x}{whole}.{fraction}/", whole.len());
    if value.starts_with('-') && !(whole.is_empty() && fraction.is_empty()) {
        format!("0{}", key.bytes().map(|b| char::from(b'~' - b)).collect::<String>())
    } else {
        format!("1{key}")
    }
}
