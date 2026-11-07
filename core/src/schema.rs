//! Note types, their schemas, and field values, as the schemas and built-in types specifications
//! define them.

use std::collections::BTreeMap;

use loro::{LoroDoc, LoroMap, LoroValue, ValueOrContainer};

use crate::{Id, Result, space::FileHash};

/// The root container that holds schemas.
pub const SCHEMAS: &str = "schemas";

/// The kind of a field's values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
    Time,
    Timestamp,
    Link,
    File,
    Choice,
    List,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Time => "time",
            Self::Timestamp => "timestamp",
            Self::Link => "link",
            Self::File => "file",
            Self::Choice => "choice",
            Self::List => "list",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Text,
            Self::Integer,
            Self::Decimal,
            Self::Boolean,
            Self::Date,
            Self::Time,
            Self::Timestamp,
            Self::Link,
            Self::File,
            Self::Choice,
            Self::List,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// How the search index treats a field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchMode {
    Text,
    Filter,
    None,
}

impl SearchMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Filter => "filter",
            Self::None => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Text, Self::Filter, Self::None].into_iter().find(|mode| mode.name() == name)
    }
}

/// A field's definition, as a space stores it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldDef {
    pub kind: Kind,
    pub item: Option<Kind>,
    pub search: SearchMode,
    pub choices: Vec<String>,
    pub target: Option<String>,
    pub accepts: Vec<String>,
    pub derivation: Option<String>,
    pub derived_from: Option<String>,
    pub preview: bool,
    pub deprecated: bool,
}

impl FieldDef {
    pub const fn new(kind: Kind, search: SearchMode) -> Self {
        Self {
            kind,
            item: None,
            search,
            choices: Vec::new(),
            target: None,
            accepts: Vec::new(),
            derivation: None,
            derived_from: None,
            preview: false,
            deprecated: false,
        }
    }

    /// A derived `file` field, filled by `derivation` from the field named `from`.
    pub fn derived(derivation: &str, from: &str, search: SearchMode) -> Self {
        Self {
            derivation: Some(derivation.into()),
            derived_from: Some(from.into()),
            ..Self::new(Kind::File, search)
        }
    }

    #[must_use]
    pub const fn preview(mut self) -> Self {
        self.preview = true;
        self
    }

    #[must_use]
    pub fn accepting(mut self, media_type: &str) -> Self {
        self.accepts.push(media_type.into());
        self
    }

    /// The kind of each value the field holds: a list's item kind, or the field's own kind.
    pub fn value_kind(&self) -> Kind {
        if self.kind == Kind::List { self.item.unwrap_or(Kind::Text) } else { self.kind }
    }
}

/// A type's name and its fields, by name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub fields: BTreeMap<String, FieldDef>,
}

/// The names of the built-in types.
pub const AGENT_TRANSCRIPT: &str = "agent-transcript";
pub const FILE: &str = "file";
pub const IMAGE: &str = "image";
pub const NOTE: &str = "note";

/// The types this release defines, as the built-in types specification lists them.
pub fn built_in() -> Vec<TypeDef> {
    let fields = |list: Vec<(&str, FieldDef)>| -> BTreeMap<String, FieldDef> {
        list.into_iter().map(|(name, def)| (name.to_owned(), def)).collect()
    };
    vec![
        TypeDef { name: AGENT_TRANSCRIPT.into(), fields: BTreeMap::new() },
        TypeDef {
            name: FILE.into(),
            fields: fields(vec![
                ("file", FieldDef::new(Kind::File, SearchMode::None)),
                ("text", FieldDef::derived("text", "file", SearchMode::Text)),
                ("thumbnail", FieldDef::derived("thumbnail", "file", SearchMode::None).preview()),
            ]),
        },
        TypeDef {
            name: IMAGE.into(),
            fields: fields(vec![
                ("image", FieldDef::new(Kind::File, SearchMode::None).accepting("image/*")),
                ("thumbnail", FieldDef::derived("thumbnail", "image", SearchMode::None).preview()),
            ]),
        },
        TypeDef { name: NOTE.into(), fields: BTreeMap::new() },
    ]
}

/// Whether `name` is a valid type or field name: lowercase words of letters and digits, joined by
/// single hyphens, starting with a letter.
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|word| {
            word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && word.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Adds to the space's schemas whatever `types` define that the space lacks, as the schemas
/// specification's writing rules require. It never removes or changes what's there, apart from
/// setting `deprecated`. It returns whether it wrote anything.
pub fn write_schemas(doc: &LoroDoc, types: &[TypeDef]) -> Result<bool> {
    let schemas = doc.get_map(SCHEMAS);
    let mut wrote = false;
    for ty in types {
        wrote |= schemas.get(&ty.name).is_none();
        let fields = child_map(&schemas, &ty.name)?;
        for (name, def) in &ty.fields {
            match fields.get(name) {
                Some(ValueOrContainer::Container(container)) => {
                    let Ok(existing) = container.into_map() else { continue };
                    wrote |= add_missing(&existing, def)?;
                }
                Some(ValueOrContainer::Value(_)) => {}
                None => {
                    let map = fields.insert_container(name, LoroMap::new())?;
                    write_field(&map, def)?;
                    wrote = true;
                }
            }
        }
    }
    Ok(wrote)
}

/// The map under `key`, created when it's missing.
pub(crate) fn child_map(parent: &LoroMap, key: &str) -> Result<LoroMap> {
    match parent.get(key) {
        Some(ValueOrContainer::Container(container)) => {
            container.into_map().map_err(|_| crate::Error::Space(format!("{key} isn't a map")))
        }
        _ => Ok(parent.insert_container(key, LoroMap::new())?),
    }
}

fn write_field(map: &LoroMap, def: &FieldDef) -> Result<()> {
    map.insert("kind", def.kind.name())?;
    if let Some(item) = def.item {
        map.insert("item", item.name())?;
    }
    map.insert("search", def.search.name())?;
    if let Some(target) = &def.target {
        map.insert("target", target.as_str())?;
    }
    if let Some(derivation) = &def.derivation {
        map.insert("derivation", derivation.as_str())?;
    }
    if let Some(from) = &def.derived_from {
        map.insert("derived-from", from.as_str())?;
    }
    if def.preview {
        map.insert("preview", true)?;
    }
    add_missing(map, def)?;
    Ok(())
}

/// Adds the choices and accepted media types the stored definition lacks, and its deprecation.
fn add_missing(map: &LoroMap, def: &FieldDef) -> Result<bool> {
    let mut wrote = false;
    for (key, values) in [("choices", &def.choices), ("accepts", &def.accepts)] {
        if values.is_empty() {
            continue;
        }
        let set = child_map(map, key)?;
        for value in values {
            if set.get(value).is_none() {
                set.insert(value, true)?;
                wrote = true;
            }
        }
    }
    if def.deprecated && map.get("deprecated").is_none() {
        map.insert("deprecated", true)?;
        wrote = true;
    }
    Ok(wrote)
}

/// Reads the space's schema for `type_name`, which holds every field any release defined.
pub fn read_type(doc: &LoroDoc, type_name: &str) -> Option<TypeDef> {
    let LoroValue::Map(fields) = doc.get_map(SCHEMAS).get(type_name)?.get_deep_value() else {
        return None;
    };
    let fields = fields
        .iter()
        .filter_map(|(name, value)| Some((name.clone(), read_field(value)?)))
        .collect();
    Some(TypeDef { name: type_name.into(), fields })
}

fn read_field(value: &LoroValue) -> Option<FieldDef> {
    let LoroValue::Map(map) = value else { return None };
    let text = |key: &str| match map.get(key) {
        Some(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    };
    let set = |key: &str| match map.get(key) {
        Some(LoroValue::Map(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let flag = |key: &str| matches!(map.get(key), Some(LoroValue::Bool(true)));
    Some(FieldDef {
        kind: Kind::from_name(&text("kind")?)?,
        item: text("item").and_then(|k| Kind::from_name(&k)),
        search: text("search").and_then(|s| SearchMode::from_name(&s)).unwrap_or(SearchMode::None),
        choices: set("choices"),
        target: text("target"),
        accepts: set("accepts"),
        derivation: text("derivation"),
        derived_from: text("derived-from"),
        preview: flag("preview"),
        deprecated: flag("deprecated"),
    })
}

/// A field's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Integer(i64),
    Decimal(String),
    Boolean(bool),
    Date(String),
    Time(String),
    Timestamp(i64),
    Link(Id),
    File(FileHash),
    Choice(String),
    List(Vec<Value>),
}

impl Value {
    /// The value as the space document stores it.
    pub fn to_loro(&self) -> LoroValue {
        match self {
            Self::Text(s) | Self::Decimal(s) | Self::Date(s) | Self::Time(s) | Self::Choice(s) => {
                s.as_str().into()
            }
            Self::Integer(n) | Self::Timestamp(n) => (*n).into(),
            Self::Boolean(b) => (*b).into(),
            Self::Link(id) => id.to_string().into(),
            Self::File(hash) => hash.to_string().into(),
            Self::List(items) => {
                LoroValue::from(items.iter().map(Self::to_loro).collect::<Vec<_>>())
            }
        }
    }

    /// Reads a stored value against its field's definition, or `None` for a value that doesn't
    /// match, which apps show and index as nothing.
    pub fn from_loro(def: &FieldDef, value: &LoroValue) -> Option<Self> {
        if def.kind == Kind::List {
            let LoroValue::List(items) = value else { return None };
            let item = FieldDef { kind: def.value_kind(), ..def.clone() };
            return items
                .iter()
                .map(|v| Self::from_loro(&item, v))
                .collect::<Option<_>>()
                .map(Self::List);
        }
        match (def.kind, value) {
            (Kind::Text, LoroValue::String(s)) => Some(Self::Text(s.to_string())),
            (Kind::Integer, LoroValue::I64(n)) => Some(Self::Integer(*n)),
            (Kind::Decimal, LoroValue::String(s)) if is_decimal(s) => {
                Some(Self::Decimal(s.to_string()))
            }
            (Kind::Boolean, LoroValue::Bool(b)) => Some(Self::Boolean(*b)),
            (Kind::Date, LoroValue::String(s)) if is_date(s) => Some(Self::Date(s.to_string())),
            (Kind::Time, LoroValue::String(s)) if is_time(s) => Some(Self::Time(s.to_string())),
            (Kind::Timestamp, LoroValue::I64(n)) => Some(Self::Timestamp(*n)),
            (Kind::Link, LoroValue::String(s)) => Id::parse(s).ok().map(Self::Link),
            (Kind::File, LoroValue::String(s)) => s.parse().ok().map(Self::File),
            (Kind::Choice, LoroValue::String(s)) if def.choices.iter().any(|c| c == s.as_str()) => {
                Some(Self::Choice(s.to_string()))
            }
            _ => None,
        }
    }
}

/// An optional `-`, digits, and an optional `.` with more digits.
pub fn is_decimal(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = text.split_once('.').map_or((text, None), |(w, f)| (w, Some(f)));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits)
}

/// `YYYY-MM-DD`, with a month from 1 to 12 and a day that the month has.
pub fn is_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let [year, month, day] = parts.as_slice() else { return false };
    let number = |s: &str, len: usize| {
        (s.len() == len && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day)) = (number(year, 4), number(month, 2), number(day, 2))
    else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// `HH:MM` or `HH:MM:SS` on a 24-hour clock.
pub fn is_time(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    let two = |s: &str, max: u32| s.len() == 2 && s.parse::<u32>().is_ok_and(|n| n <= max);
    match parts.as_slice() {
        [h, m] => two(h, 23) && two(m, 59),
        [h, m, s] => two(h, 23) && two(m, 59) && two(s, 59),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names() {
        assert!(is_name("note") && is_name("derived-from") && is_name("isbn13"));
        assert!(
            !is_name("")
                && !is_name("Note")
                && !is_name("a--b")
                && !is_name("1a")
                && !is_name("-a")
        );
    }

    #[test]
    fn validates_dates_times_and_decimals() {
        assert!(is_date("2024-02-29") && !is_date("2023-02-29") && !is_date("2024-13-01"));
        assert!(is_time("07:00") && is_time("23:59:59") && !is_time("24:00") && !is_time("7:00"));
        assert!(is_decimal("12.50") && is_decimal("-3") && !is_decimal("1.") && !is_decimal(".5"));
    }

    #[test]
    fn writes_schemas_once_and_only_adds() {
        let doc = LoroDoc::new();
        assert!(write_schemas(&doc, &built_in()).unwrap());
        doc.commit();
        assert!(!write_schemas(&doc, &built_in()).unwrap());
        let image = read_type(&doc, IMAGE).unwrap();
        assert_eq!(image.fields["image"].accepts, vec!["image/*".to_owned()]);
        assert!(image.fields["thumbnail"].preview);
        assert!(read_type(&doc, NOTE).is_some());

        let mut newer = built_in();
        newer[1].fields.get_mut("text").unwrap().deprecated = true;
        newer[1].fields.insert("pages".into(), FieldDef::new(Kind::Integer, SearchMode::Filter));
        assert!(write_schemas(&doc, &newer).unwrap());
        let file = read_type(&doc, FILE).unwrap();
        assert!(file.fields["text"].deprecated);
        assert!(file.fields.contains_key("pages"));
        // An older release neither removes the field nor clears the deprecation.
        assert!(!write_schemas(&doc, &built_in()).unwrap());
        assert!(read_type(&doc, FILE).unwrap().fields["text"].deprecated);
    }

    #[test]
    fn reads_values_against_their_kind() {
        let date = FieldDef::new(Kind::Date, SearchMode::Filter);
        assert_eq!(
            Value::from_loro(&date, &"2024-01-02".into()),
            Some(Value::Date("2024-01-02".into()))
        );
        assert_eq!(Value::from_loro(&date, &"soon".into()), None);
        let list =
            FieldDef { item: Some(Kind::Integer), ..FieldDef::new(Kind::List, SearchMode::Filter) };
        let stored = Value::List(vec![Value::Integer(1), Value::Integer(2)]).to_loro();
        assert_eq!(
            Value::from_loro(&list, &stored),
            Some(Value::List(vec![Value::Integer(1), Value::Integer(2)]))
        );
    }
}
