//! Agent tools use the same space operations as the rest of the instance.

use serde_json::{Value, json};
use zib_core::agent::{Creation, Host, Settings};
use zib_core::search::{MATCH_END, MATCH_START};

use super::{Id, Result, State, UiEvent, oneshot};
use zib_core::Error;

impl State {
    pub async fn agent_settings(&self) -> Settings {
        self.store.lock().await.config.agent.clone()
    }

    pub async fn set_agent_settings(&self, settings: Settings) -> Result<()> {
        let mut store = self.store.lock().await;
        let previous = store.config.agent.clone();
        store.config.agent = settings;
        if let Err(error) = store.save() {
            store.config.agent = previous;
            return Err(error);
        }
        Ok(())
    }

    pub async fn set_agent_model(&self, model: String) -> Result<()> {
        let mut store = self.store.lock().await;
        let previous = std::mem::replace(&mut store.config.agent.model, model);
        if let Err(error) = store.save() {
            store.config.agent.model = previous;
            return Err(error);
        }
        Ok(())
    }
}

impl Host for State {
    async fn prompt(&self, note: Id) -> Result<String> {
        let space = self.find_note(note).await.ok_or_else(|| Error::NotFound(note.to_string()))?;
        Ok(self.note(space, note).await?.body)
    }

    async fn get_note(&self, note: Id) -> Result<Value> {
        // Store before space, including while checking visibility, follows the instance's lock
        // order and prevents hiding a space halfway through a read.
        let store = self.store.lock().await;
        for (space, entry) in &store.config.spaces {
            if entry.hidden {
                continue;
            }
            let space = self.space(*space).await?;
            let space = space.lock().await;
            if let Ok(found) = space.note(note) {
                return Ok(json!({
                    "body": found.body, "id": note, "title": space.title(note)?.plain
                }));
            }
        }
        Err(Error::NotFound(format!("note {note} in shown spaces")))
    }

    async fn search_notes(&self, query: String) -> Result<Value> {
        let store = self.store.lock().await;
        let indexes = self.indexes.lock().expect("unpoisoned");
        let mut results = Vec::new();
        for (id, entry) in &store.config.spaces {
            if entry.hidden {
                continue;
            }
            if let Some(index) = indexes.get(id) {
                for hit in index.search(&query, 20)? {
                    results.push(json!({
                        "id": hit.id,
                        "snippet": hit.snippet.replace([MATCH_END, MATCH_START], ""),
                        "title": hit.title
                    }));
                }
            }
        }
        results.truncate(20);
        Ok(json!(results))
    }

    async fn write_note(&self, note: Id, expected: String, body: String) -> Result<Value> {
        let store = self.store.lock().await;
        for (space_id, entry) in &store.config.spaces {
            if entry.hidden {
                continue;
            }
            let space = self.space(*space_id).await?;
            let mut space = space.lock().await;
            let Ok(current) = space.note(note) else { continue };
            if current.body != body && current.body != expected {
                return Err(Error::Agent(
                    concat!(
                        "The note changed after it was read. ",
                        "Read it again with get_note before writing it."
                    )
                    .into(),
                ));
            }
            space.set_body(note, &body)?;
            // Save commits with the core's excluded origin, so the editor's undo leaves it alone.
            store.save_space(&space)?;
            self.reindex_note(*space_id, &space, note);
            self.changed_here(*space_id);
            self.send_ui(UiEvent::Changed(*space_id));
            return Ok(json!({"id": note, "title": space.title(note)?.plain}));
        }
        Err(Error::NotFound(format!("note {note} in shown spaces")))
    }

    async fn choose_space(&self, title: String) -> Option<Id> {
        let (reply, answer) = oneshot::channel();
        self.send_ui(UiEvent::AgentDestination { title, reply });
        answer.await.ok().flatten()
    }

    async fn create_note(
        &self,
        creation: Creation,
        type_name: String,
        body: String,
    ) -> Result<Value> {
        let id = creation.id;
        // A completed creation may have been moved after its result was saved. Preserve it.
        let space_id = self
            .find_note(id)
            .await
            .or(creation.space)
            .ok_or_else(|| Error::Agent("Choose a destination first".into()))?;
        let store = self.store.lock().await;
        for id_of_space in store.config.spaces.keys() {
            let space = self.space(*id_of_space).await?;
            if space.lock().await.tombstone(id).is_some() {
                return Err(Error::NotFound(format!("reserved note {id} was deleted")));
            }
        }
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.create_reserved_note(id, &type_name, &body)?;
        self.reindex_note(space_id, &space, id);
        store.save_space(&space)?;
        self.changed_here(space_id);
        Ok(json!({"id": id, "space": space_id, "title": space.title(id)?.plain}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zib_core::channel::Dirs;

    #[tokio::test]
    async fn writes_are_durable_searchable_visible_and_preserve_concurrent_edits() {
        let root = tempfile::tempdir().unwrap();
        let state = State::for_test(Dirs::under(root.path())).await.unwrap();
        let space = state.spaces().await[0].0;
        let id = state.create_note(space, "# Original\n\nBefore").await.unwrap();
        let original = state.note(space, id).await.unwrap();
        let body = "# Updated\n\nPersistedneedle";
        let write = |expected: &str, body: &str| {
            Host::write_note(&*state, id, expected.into(), body.into())
        };
        let result = write(&original.body, body).await.unwrap();
        assert_eq!(result["id"], id.to_string());
        assert_eq!(result["title"], "Updated");
        assert_eq!(state.note(space, id).await.unwrap().type_name, original.type_name);
        let results = Host::search_notes(&*state, "Persistedneedle".into()).await.unwrap();
        assert_eq!(results[0]["id"], id.to_string());
        // The write saves before returning, rather than waiting for the instance's save timer.
        assert_eq!(
            state.store.lock().await.load_space(space).unwrap().note(id).unwrap().body,
            body
        );
        // Replaying a completed write is harmless; a subsequent different edit wins over retry.
        write(&original.body, body).await.unwrap();
        state.set_hidden(space, true).await.unwrap();
        assert!(write(body, "Hidden edit").await.is_err());
        state.set_hidden(space, false).await.unwrap();
        {
            let doc = state.space(space).await.unwrap();
            doc.lock().await.set_body(id, "An edit after the write").unwrap();
        }
        assert!(write(&original.body, body).await.is_err());
        assert_eq!(state.note(space, id).await.unwrap().body, "An edit after the write");
        assert!(
            Host::write_note(&*state, Id::generate(), String::new(), body.into()).await.is_err()
        );
        state.delete_note(space, id).await.unwrap();
        assert!(write("An edit after the write", body).await.is_err());
        assert!(state.note(space, id).await.is_err());
        state.finish().await;
    }

    #[tokio::test]
    async fn tools_check_visibility_and_creation_survives_retry_restart_and_deletion() {
        let root = tempfile::tempdir().unwrap();
        let state = State::for_test(Dirs::under(root.path())).await.unwrap();
        let first = state.spaces().await[0].0;
        let hidden = state.create_space("Hidden").await.unwrap();
        let note = state.create_note(hidden, "# Secret topic\n\nNeedle").await.unwrap();
        assert!(Host::get_note(&*state, note).await.is_ok());
        state.set_hidden(hidden, true).await.unwrap();
        assert!(Host::get_note(&*state, note).await.is_err());
        assert_eq!(Host::search_notes(&*state, "Needle".into()).await.unwrap(), json!([]));
        // Explicitly configured system prompts aren't tool reads.
        assert!(Host::prompt(&*state, note).await.unwrap().contains("Needle"));
        state.set_hidden(hidden, false).await.unwrap();
        let found = Host::search_notes(&*state, "Needle".into()).await.unwrap();
        assert_eq!(found[0]["id"], note.to_string());

        let creation = Creation { id: Id::generate(), space: Some(first) };
        let create =
            |body: &str| Host::create_note(&*state, creation.clone(), "note".into(), body.into());
        create("Original").await.unwrap();
        {
            let space = state.space(first).await.unwrap();
            space.lock().await.set_body(creation.id, "Edited afterward").unwrap();
        }
        create("Original").await.unwrap();
        assert_eq!(state.note(first, creation.id).await.unwrap().body, "Edited afterward");
        state.delete_note(first, creation.id).await.unwrap();
        assert!(create("Original").await.is_err());
        state.finish().await;
    }
}
