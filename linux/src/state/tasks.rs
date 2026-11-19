//! Persistent project and task operations, with derived hierarchy snapshots for the desktop.

use std::collections::BTreeSet;

use zib_core::schema::Value;
use zib_core::search::Sort;
use zib_core::search::saved::Definition;
use zib_core::work::{Tree, WORK_QUERY};
use zib_core::{Id, Result};

use super::{State, UiEvent};

impl State {
    pub(crate) async fn set_work_status(
        &self,
        space_id: Id,
        note: Id,
        status: Option<String>,
        reason: String,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.set_work_status(note, status.as_deref(), &reason)?;
        let saved = store.save_space(&space);
        self.reindex_note(space_id, &space, note);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved?;
        Ok(())
    }

    pub(crate) async fn waiting_values(&self) -> Vec<String> {
        let spaces: Vec<_> = self.node.spaces().lock().await.values().cloned().collect();
        let mut values = BTreeSet::new();
        for space in spaces {
            values.extend(space.lock().await.waiting_values());
        }
        let mut values: Vec<_> = values.into_iter().collect();
        values.sort_by_key(|value| (value.to_lowercase(), value.clone()));
        values
    }

    pub(crate) async fn reorder_project(
        &self,
        space_id: Id,
        note: Id,
        before: Option<Id>,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.reorder_project(note, before)?;
        let saved = store.save_space(&space);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved?;
        Ok(())
    }

    pub(crate) async fn move_task(
        &self,
        space_id: Id,
        note: Id,
        parent: Option<Id>,
        before: Option<Id>,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.move_task(note, parent, before)?;
        let saved = store.save_space(&space);
        self.reindex_note(space_id, &space, note);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved?;
        Ok(())
    }

    pub(crate) async fn rename_task(
        &self,
        space_id: Id,
        note: Id,
        expected: &str,
        title: &str,
    ) -> Result<String> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        let line = space.rename_task(note, expected, title)?;
        let saved = store.save_space(&space);
        self.reindex_note(space_id, &space, note);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved?;
        Ok(line)
    }

    pub(crate) async fn work_tree(&self, space: Id) -> Result<Tree> {
        Ok(Tree::read(&*self.space(space).await?.lock().await))
    }

    pub(crate) async fn create_work(
        &self,
        space_id: Id,
        type_name: &str,
        body: &str,
        parent: Option<Id>,
    ) -> Result<Id> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        let id = space.create_work(type_name, body, parent)?;
        if space.setting(None, "zib.work-searches")?.is_none() {
            let searches = space.saved_searches();
            for (name, query, sort) in [
                ("All tasks", "@type=task", Sort::Due),
                ("Projects", "@type=project", Sort::Title),
                ("What should I work on?", WORK_QUERY, Sort::Due),
            ] {
                if !searches.iter().any(|s| s.name == name) {
                    space.create_saved_search(
                        name,
                        &Definition { query: query.into(), scope: Some(space_id), sort },
                    )?;
                }
            }
            space.set_setting(None, "zib.work-searches", true)?;
        }
        let saved = store.save_space(&space);
        self.reindex_note(space_id, &space, id);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved?;
        Ok(id)
    }

    pub(crate) async fn set_work_field(
        &self,
        space_id: Id,
        note: Id,
        field: &str,
        value: Option<Value>,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        if let Some(value) = value {
            space.set_field(note, field, &value)?;
        } else {
            space.clear_field(note, field)?;
        }
        let saved = store.save_space(&space);
        self.reindex_note(space_id, &space, note);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved?;
        Ok(())
    }
}
