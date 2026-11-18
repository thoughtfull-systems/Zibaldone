//! Saved searches live in spaces and use the same persistence and sync as their notes.

use zib_core::search::saved::{Definition, SavedSearch};
use zib_core::{Error, Id, Result};

use super::{State, UiEvent};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SearchEntry {
    pub space: Id,
    pub search: SavedSearch,
}

impl State {
    pub(crate) async fn saved_searches(&self) -> Vec<SearchEntry> {
        let mut entries = Vec::new();
        for (id, _) in self.spaces().await {
            if let Ok(space) = self.space(id).await {
                entries.extend(
                    space
                        .lock()
                        .await
                        .saved_searches()
                        .into_iter()
                        .map(|search| SearchEntry { space: id, search }),
                );
            }
        }
        entries.sort_by(|a, b| {
            a.search
                .name
                .to_lowercase()
                .cmp(&b.search.name.to_lowercase())
                .then_with(|| a.space.cmp(&b.space))
                .then_with(|| a.search.id.cmp(&b.search.id))
        });
        entries
    }

    pub(crate) async fn save_search(
        &self,
        space: Id,
        expected: Option<SavedSearch>,
        name: String,
        definition: Definition,
    ) -> Result<SearchEntry> {
        let spaces = self.spaces().await;
        let scope: Vec<_> = spaces
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| definition.scope.is_none_or(|scope| scope == *id))
            .collect();
        if definition.scope.is_some() && scope.is_empty() {
            return Err(Error::Space("This machine doesn't hold the searched space".into()));
        }
        self.search_shown(&scope, &definition.query, definition.sort, 0)?;
        let store = self.store.lock().await;
        let document = self.space(space).await?;
        let mut document = document.lock().await;
        let search = if let Some(expected) = expected {
            document.update_saved_search(&expected, &definition)?
        } else {
            document.create_saved_search(&name, &definition)?
        };
        store.save_space(&document)?;
        self.changed_here(space);
        self.send_ui(UiEvent::Changed(space));
        Ok(SearchEntry { space, search })
    }
}
