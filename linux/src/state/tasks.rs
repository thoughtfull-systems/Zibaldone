//! Persistent project and task operations, with derived hierarchy snapshots for the desktop.

use std::collections::BTreeSet;

use zib_core::recurrence::{self, Replacement, Rule};
use zib_core::schema::Value;
use zib_core::search::Sort;
use zib_core::search::saved::Definition;
use zib_core::work::{ALL_TASKS_QUERY, Tree, WORK_QUERY};
use zib_core::{Id, Result};

use super::{State, UiEvent};

const SHOW_COMPLETED: &str = "zib.show-completed";

impl State {
    pub(crate) async fn show_completed(&self, space: Id, note: Id) -> Result<bool> {
        let space = self.space(space).await?;
        let space = space.lock().await;
        Ok(space.setting(Some(note), SHOW_COMPLETED)? == Some(true.into()))
    }

    pub(crate) async fn set_show_completed(
        &self,
        space_id: Id,
        note: Id,
        completed: bool,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.set_setting(Some(note), SHOW_COMPLETED, completed)?;
        let saved = store.save_space(&space);
        self.changed_here(space_id);
        self.send_ui(UiEvent::Changed(space_id));
        saved
    }

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
        let line = space.rename_work_title(note, expected, title)?;
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
                ("All tasks", ALL_TASKS_QUERY, Sort::Due),
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

impl State {
    pub(crate) async fn recurrence_rule(&self, space: Id, note: Id) -> Result<Option<Rule>> {
        Ok(self.space(space).await?.lock().await.recurrence_rule(note))
    }

    pub(crate) async fn recurrence_replacement(
        &self,
        space: Id,
        note: Id,
        rule: Option<Rule>,
    ) -> Result<Option<Replacement>> {
        let space = self.space(space).await?;
        let space = space.lock().await;
        Ok(space.recurrence_replacement(note, rule.as_ref(), recurrence::today()))
    }

    pub(crate) async fn set_recurrence(
        &self,
        space_id: Id,
        note: Id,
        rule: Option<Rule>,
        replacement: Option<Replacement>,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        let today = recurrence::today();
        if space.recurrence_replacement(note, rule.as_ref(), today) != replacement {
            return Err(zib_core::Error::Space(
                "The series changed during review. Open the recurrence editor and try again."
                    .into(),
            ));
        }
        space.set_recurrence(note, rule, today)?;
        self.save_recurrences(&store, space_id, &space)
    }

    pub(crate) async fn record_occurrence(
        &self,
        space_id: Id,
        note: Id,
        outcome: zib_core::recurrence::Outcome,
        comment: String,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.record_occurrence(note, outcome, zib_core::recurrence::today(), &comment)?;
        self.save_recurrences(&store, space_id, &space)
    }

    pub(crate) async fn add_occurrence_comment(
        &self,
        id: Id,
        note: Id,
        comment: String,
    ) -> Result<()> {
        let store = self.store.lock().await;
        let document = self.space(id).await?;
        let mut space = document.lock().await;
        space.add_occurrence_comment(note, zib_core::recurrence::today(), &comment)?;
        self.save_recurrences(&store, id, &space)
    }

    pub(crate) async fn catch_up_recurrences(&self, id: Id) -> Result<()> {
        let store = self.store.lock().await;
        let space = self.space(id).await?;
        let mut space = space.lock().await;
        if space.realize_recurrences(zib_core::recurrence::today())? {
            self.save_recurrences(&store, id, &space)?;
        }
        Ok(())
    }

    fn save_recurrences(
        &self,
        store: &zib_core::store::Store,
        id: Id,
        space: &zib_core::Space,
    ) -> Result<()> {
        let saved = store.save_space(space);
        let indexed = self.index(id, space);
        self.changed_here(id);
        self.send_ui(UiEvent::Changed(id));
        saved.and(indexed)
    }
}
