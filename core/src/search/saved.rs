//! Saved search definitions, as the saved searches specification describes.

use std::collections::HashMap;

use loro::LoroValue;

use crate::{Error, Id, Result, Space};

use super::Sort;

const SEARCHES: &str = "saved-searches";

/// The query, scope, and ordering stored together when someone saves a search.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Definition {
    pub query: String,
    /// `None` searches every space the machine holds. An identifier names exactly one space.
    pub scope: Option<Id>,
    pub sort: Sort,
}

/// A search and the stored value it read, for checking an update before replacing it.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedSearch {
    pub id: Id,
    pub name: String,
    pub definition: Definition,
    original: LoroValue,
}

impl Space {
    /// Lists well-formed saved searches, leaving unknown or malformed entries untouched.
    pub fn saved_searches(&self) -> Vec<SavedSearch> {
        self.doc()
            .get_map(SEARCHES)
            .keys()
            .filter_map(|key| {
                let id = Id::parse(&key).ok()?;
                read(id, self.doc().get_map(SEARCHES).get(&key)?.get_deep_value())
            })
            .collect()
    }

    /// Creates a saved search with its own identifier, in one commit.
    pub fn create_saved_search(
        &mut self,
        name: &str,
        definition: &Definition,
    ) -> Result<SavedSearch> {
        let id = Id::generate();
        self.write_saved_search(id, name, definition, HashMap::new())
    }

    /// Updates the search only if the stored value still matches the one the caller read.
    pub fn update_saved_search(
        &mut self,
        expected: &SavedSearch,
        definition: &Definition,
    ) -> Result<SavedSearch> {
        let map = self.doc().get_map(SEARCHES);
        let current = map.get(&expected.id.to_string()).map(|v| v.get_deep_value());
        if current.as_ref() != Some(&expected.original) {
            return Err(Error::Space(
                "Saved search changed. Save a copy to keep your edits".into(),
            ));
        }
        let LoroValue::Map(original) = &expected.original else { unreachable!() };
        let fields = original.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        self.write_saved_search(expected.id, &expected.name, definition, fields)
    }

    fn write_saved_search(
        &mut self,
        id: Id,
        name: &str,
        definition: &Definition,
        mut fields: HashMap<String, LoroValue>,
    ) -> Result<SavedSearch> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Space("Enter a name for the saved search".into()));
        }
        fields.insert("name".into(), name.into());
        fields.insert("query".into(), definition.query.as_str().into());
        fields.insert(
            "scope".into(),
            definition.scope.map_or(LoroValue::Null, |id| id.to_string().into()),
        );
        fields.insert("sort".into(), definition.sort.name().into());
        let value = LoroValue::Map(fields.into());
        self.doc().get_map(SEARCHES).insert(&id.to_string(), value.clone())?;
        self.commit();
        Ok(read(id, value).expect("wrote a valid search"))
    }
}

fn read(id: Id, original: LoroValue) -> Option<SavedSearch> {
    let LoroValue::Map(fields) = &original else { return None };
    let text = |key| match fields.get(key)? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    };
    let name = text("name")?;
    if name.trim().is_empty() {
        return None;
    }
    let scope = match fields.get("scope")? {
        LoroValue::Null => None,
        LoroValue::String(id) => Some(Id::parse(id).ok()?),
        _ => return None,
    };
    let definition =
        Definition { query: text("query")?, scope, sort: Sort::from_name(&text("sort")?)? };
    Some(SavedSearch { id, name, definition, original })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_searches_sync_as_whole_values_and_reject_stale_updates() {
        let mut first = Space::create(Id::generate()).unwrap();
        let initial = first.create_saved_search("Reading", &Definition::default()).unwrap();
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        let changed = Definition {
            query: "apple OR pear".into(),
            scope: Some(Id::generate()),
            sort: Sort::Title,
        };
        first.update_saved_search(&initial, &changed).unwrap();
        second
            .update_saved_search(
                &initial,
                &Definition { query: "compost".into(), ..Definition::default() },
            )
            .unwrap();
        first.import_checked(&second.snapshot().unwrap()).unwrap();
        second.import_checked(&first.snapshot().unwrap()).unwrap();
        assert_eq!(first.saved_searches(), second.saved_searches());
        assert!(first.update_saved_search(&initial, &changed).is_err());
        let stored = first.saved_searches().pop().unwrap();
        assert!(stored.definition == changed || stored.definition.query == "compost");
    }
}
