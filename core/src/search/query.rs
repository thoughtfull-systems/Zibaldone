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
    range: Option<Range>,
}

struct Range {
    start: String,
    end: String,
    inclusive: bool,
}

impl Range {
    fn parse(value: &str) -> Result<Option<Self>> {
        let Some((start, end)) = value.split_once("..") else { return Ok(None) };
        let inclusive = end.starts_with('.');
        let end = end.strip_prefix('.').unwrap_or(end);
        if start.is_empty() || end.is_empty() || end.contains("..") {
            return Err(error("Enter two range bounds separated by .. or ..."));
        }
        Ok(Some(Self { start: start.into(), end: end.into(), inclusive }))
    }
}

pub(super) enum Query<'a> {
    And(Vec<Self>),
    Field(Clause<'a>),
    Not(Box<Self>),
    Or(Vec<Self>),
    Text(String),
}

impl<'a> Query<'a> {
    pub fn parse(text: &'a str) -> Result<Option<Self>> {
        let mut tokens = tokenize(text)?.into_iter().peekable();
        if tokens.peek().is_none() {
            return Ok(None);
        }
        let query = parse_or(&mut tokens, 0)?;
        if tokens.next().is_some() {
            return Err(error("Unexpected closing parenthesis"));
        }
        Ok(Some(query))
    }
}

enum Token<'a> {
    And,
    Close,
    Not,
    Open,
    Or,
    Term(Query<'a>),
}

type Tokens<'a> = std::iter::Peekable<std::vec::IntoIter<Token<'a>>>;

fn parse_or<'a>(tokens: &mut Tokens<'a>, depth: usize) -> Result<Query<'a>> {
    let first = parse_and(tokens, depth)?;
    if !matches!(tokens.peek(), Some(Token::Or)) {
        return Ok(first);
    }
    let mut terms = vec![first];
    while matches!(tokens.peek(), Some(Token::Or)) {
        tokens.next();
        terms.push(parse_and(tokens, depth)?);
    }
    Ok(Query::Or(terms))
}

fn parse_and<'a>(tokens: &mut Tokens<'a>, depth: usize) -> Result<Query<'a>> {
    let mut terms = Vec::new();
    let mut text = String::new();
    loop {
        match parse_unary(tokens, depth)? {
            // Keep the existing combined full-text ranking for a conjunction of search words.
            Query::Text(word) => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&word);
            }
            term => terms.push(term),
        }
        match tokens.peek() {
            None | Some(Token::Close | Token::Or) => break,
            Some(Token::And) => {
                tokens.next();
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        terms.insert(0, Query::Text(text));
    }
    Ok(if terms.len() == 1 { terms.pop().unwrap() } else { Query::And(terms) })
}

fn parse_unary<'a>(tokens: &mut Tokens<'a>, depth: usize) -> Result<Query<'a>> {
    if depth > 32 {
        return Err(error("Search groups and NOT can nest at most 32 levels"));
    }
    match tokens.next() {
        Some(Token::Not) => Ok(Query::Not(Box::new(parse_unary(tokens, depth + 1)?))),
        Some(Token::Open) => {
            let query = parse_or(tokens, depth + 1)?;
            if !matches!(tokens.next(), Some(Token::Close)) {
                return Err(error("Close the search group with a parenthesis"));
            }
            Ok(query)
        }
        Some(Token::Term(term)) => Ok(term),
        _ => Err(error("Expected a search term, field condition, or group")),
    }
}

fn tokenize(mut rest: &str) -> Result<Vec<Token<'_>>> {
    let mut tokens = Vec::new();
    let mut fields = 0;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(tokens);
        }
        if tokens.len() == 512 {
            return Err(error("Use at most 512 search terms and operators"));
        }
        let (token, tail) = match rest.as_bytes()[0] {
            b'(' => (Some(Token::Open), &rest[1..]),
            b')' => (Some(Token::Close), &rest[1..]),
            b'"' => {
                let (value, tail) = take_quoted(&rest[1..], "search text")?;
                if tail.starts_with(|c: char| !separator(c)) {
                    return Err(error("Expected a space or parenthesis after quoted search text"));
                }
                (text_token(value), tail)
            }
            _ => {
                let (word, tail) = split_word(rest);
                if let Some((clause, tail)) = take_clause(rest, word)? {
                    fields += 1;
                    if fields > 16 {
                        return Err(error("Use at most 16 field conditions"));
                    }
                    (Some(Token::Term(Query::Field(clause))), tail)
                } else {
                    let token = match word {
                        w if w.eq_ignore_ascii_case("and") => Some(Token::And),
                        w if w.eq_ignore_ascii_case("not") => Some(Token::Not),
                        w if w.eq_ignore_ascii_case("or") => Some(Token::Or),
                        _ => text_token(word.to_owned()),
                    };
                    (token, tail)
                }
            }
        };
        tokens.extend(token);
        rest = tail;
    }
}

fn text_token(text: String) -> Option<Token<'static>> {
    text.chars().any(char::is_alphanumeric).then_some(Token::Term(Query::Text(text)))
}

fn take_clause<'a>(text: &'a str, word: &'a str) -> Result<Option<(Clause<'a>, &'a str)>> {
    let Some(field) = word.strip_prefix('@') else { return Ok(None) };
    let Some(end) = field.find(['!', ':', '<', '=', '>', '?']) else { return Ok(None) };
    let name = &field[..end];
    if !schema::is_name(name) {
        return Err(error(format!("Invalid field name {name:?}")));
    }
    // Read from the full input so a quoted value can contain spaces or parentheses.
    let Some((op, value)) = Operator::take(&text[1 + end..]) else {
        return Err(error(format!("Invalid operator for {name}")));
    };
    let quoted = value.starts_with('"');
    let (value, tail) = match op {
        Operator::Missing | Operator::Present => (String::new(), value),
        _ => take_value(value, name)?,
    };
    if tail.starts_with(|c: char| !separator(c)) {
        return Err(error(format!("Expected a space or parenthesis after the {name} condition")));
    }
    let range = if matches!(op, Operator::Equal) && !quoted { Range::parse(&value)? } else { None };
    Ok(Some((Clause { field: name, op, value, range }, tail)))
}

fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')')
}

fn split_word(text: &str) -> (&str, &str) {
    text.split_at(text.find(separator).unwrap_or(text.len()))
}

fn take_value<'a>(text: &'a str, field: &str) -> Result<(String, &'a str)> {
    if let Some(quoted) = text.strip_prefix('"') {
        return take_quoted(quoted, field);
    }
    let (value, tail) = split_word(text);
    if value.is_empty() {
        return Err(error(format!("Enter a value for {field}")));
    }
    Ok((value.into(), tail))
}

fn take_quoted<'a>(quoted: &'a str, label: &str) -> Result<(String, &'a str)> {
    let mut value = String::new();
    let mut chars = quoted.char_indices();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' => return Ok((value, &quoted[i + 1..])),
            '\\' => match chars.next() {
                Some((_, ch @ ('"' | '\\'))) => value.push(ch),
                _ => return Err(error(format!("Invalid escape in the quoted value for {label}"))),
            },
            _ => value.push(ch),
        }
    }
    Err(error(format!("Close the quoted value for {label}")))
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
                if self.range.is_some() {
                    self.range_bounds(defs)?;
                } else {
                    self.comparison_value(defs)?;
                }
            }
        }
        Ok(())
    }

    pub fn comparison_value(&self, defs: &[&FieldDef]) -> Result<rusqlite::types::Value> {
        self.typed_value(
            &self.value,
            defs,
            matches!(self.op, Operator::Greater | Operator::LessEqual),
        )
    }

    pub fn range_bounds(
        &self,
        defs: &[&FieldDef],
    ) -> Result<Option<(rusqlite::types::Value, rusqlite::types::Value, bool)>> {
        use rusqlite::types::Value;

        let (start, end, inclusive) = if let Some(range) = &self.range {
            (range.start.as_str(), range.end.as_str(), range.inclusive)
        } else if matches!(self.op, Operator::Equal)
            && self.comparison_kind(defs)? == Kind::Timestamp
        {
            (self.value.as_str(), self.value.as_str(), true)
        } else {
            return Ok(None);
        };
        let start = self.typed_value(start, defs, false)?;
        let end = self.typed_value(end, defs, inclusive)?;
        let reversed = match (&start, &end) {
            (Value::Integer(a), Value::Integer(b)) => a > b,
            (Value::Text(a), Value::Text(b)) => a > b,
            _ => unreachable!("ordered values have one integer or text kind"),
        };
        if reversed {
            return Err(error(format!(
                "The range start must not exceed its end for {}",
                self.field
            )));
        }
        Ok(Some((start, end, inclusive)))
    }

    fn comparison_kind(&self, defs: &[&FieldDef]) -> Result<Kind> {
        let field = self.field;
        let mut comparable = defs.iter().copied().filter(|def| comparable(def));
        let Some(def) = comparable.next() else {
            return Err(error(format!("{field} doesn't support value comparisons")));
        };
        let kind = def.value_kind();
        if comparable.any(|def| def.value_kind() != kind) {
            return Err(error(format!("{field} has more than one value kind in these spaces")));
        }
        if (!matches!(self.op, Operator::Equal) || self.range.is_some())
            && !matches!(
                kind,
                Kind::Date | Kind::Decimal | Kind::Integer | Kind::Time | Kind::Timestamp
            )
        {
            return Err(error(format!("{field} doesn't support ordered comparisons")));
        }
        Ok(kind)
    }

    fn typed_value(
        &self,
        raw: &str,
        defs: &[&FieldDef],
        end: bool,
    ) -> Result<rusqlite::types::Value> {
        let field = self.field;
        let kind = self.comparison_kind(defs)?;
        let invalid = || {
            let expected = match kind {
                Kind::Boolean => "true or false",
                Kind::Date => "a date in YYYY-MM-DD format",
                Kind::Decimal => "a decimal number",
                Kind::Integer => "an integer",
                Kind::Link => "a note ID",
                Kind::Time => "a time in HH:MM or HH:MM:SS format",
                Kind::Timestamp => "a timestamp such as 2026-01-01 or 2026-02-12T05",
                _ => "a valid field value",
            };
            error(format!("Invalid value {raw:?} for {field}. Use {expected}"))
        };
        let value = match kind {
            Kind::Boolean => i64::from(raw.parse::<bool>().map_err(|_| invalid())?).into(),
            Kind::Date if schema::is_date(raw) => raw.to_owned().into(),
            Kind::Decimal if schema::is_decimal(raw) => decimal_key(raw).into(),
            Kind::Integer => raw.parse::<i64>().map_err(|_| invalid())?.into(),
            Kind::Link => Id::parse(raw).map_err(|_| invalid())?.to_string().into(),
            Kind::Time if schema::is_time(raw) => time_key(raw).into(),
            Kind::Timestamp => {
                let bounds = super::timestamp::parse(raw, field)?;
                if end { bounds.end.into() } else { bounds.start.into() }
            }
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
