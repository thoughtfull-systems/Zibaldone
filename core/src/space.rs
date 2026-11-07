//! A space's Loro document, as the space document specification defines it.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::str::FromStr;

use loro::{
    CommitOptions, ExportMode, LoroDoc, LoroMap, LoroText, LoroValue, UpdateOptions,
    ValueOrContainer, VersionVector,
};

use crate::address::MachineKey;
use crate::body::{self, Title};
use crate::schema::{self, Value, child_map};
use crate::time::{self, Timestamp};
use crate::{Error, Id, Result};

/// The origin of every commit the core makes for itself, which an editor's undo leaves out, so undo
/// only reverses the owner's edits in the editor.
pub const ORIGIN: &str = "zib";

const META: &str = "meta";
const NOTES: &str = "notes";
const TOMBSTONES: &str = "tombstones";
const SETTINGS: &str = "settings";
const FILES: &str = "files";
const MACHINES: &str = "machines";

/// While an editor has a note open, how long a writer waits before it sets `updated` again.
const UPDATED_INTERVAL: Timestamp = 60_000;

/// A blob's BLAKE3 hash.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileHash(pub [u8; 32]);

impl FileHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }
}

impl fmt::Display for FileHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&MachineKey(self.0).to_hex())
    }
}

impl fmt::Debug for FileHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileHash({self})")
    }
}

impl FromStr for FileHash {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        if text.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(Error::invalid("file hash", text));
        }
        MachineKey::from_hex(text)
            .map(|key| Self(key.0))
            .map_err(|_| Error::invalid("file hash", text))
    }
}

/// A machine's file policy, as decision ZIB0004 defines it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Policy {
    All,
    OnDemand,
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::OnDemand => "on-demand",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "all" => Some(Self::All),
            "on-demand" => Some(Self::OnDemand),
            _ => None,
        }
    }
}

/// A note's attributes from its `meta` map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteMeta {
    pub created: Timestamp,
    pub created_by: Option<Id>,
    pub updated: Timestamp,
    pub updated_by: Option<Id>,
}

/// A note as the core reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: Id,
    pub type_name: String,
    pub body: String,
    /// Each field's stored value, before checking it against the schema.
    pub fields: BTreeMap<String, LoroValue>,
    pub meta: NoteMeta,
}

impl Note {
    /// The field's value, read against the space's schema, or `None` for no value or a value
    /// that doesn't match its kind.
    pub fn field(&self, space: &Space, name: &str) -> Option<Value> {
        let def = schema::read_type(&space.doc, &self.type_name)?.fields.remove(name)?;
        Value::from_loro(&def, self.fields.get(name)?)
    }
}

/// A note deleted for good.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tombstone {
    pub title: String,
    pub deleted: Timestamp,
    pub deleted_by: Option<Id>,
}

/// An entry in the `files` map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub size: u64,
    pub media_type: String,
    pub holders: BTreeMap<Id, Timestamp>,
}

/// A machine that shares the space.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Machine {
    pub id: Id,
    pub key: MachineKey,
    pub name: String,
    pub policy: Policy,
    pub removed: Option<Timestamp>,
}

/// A note that an editor has open.
#[derive(Clone, Copy, Debug, Default)]
struct OpenNote {
    /// When this process last set `updated`.
    written: Option<Timestamp>,
    /// Whether edits came after that, which closing the note records.
    pending: bool,
}

/// A space: one Loro document, edited by this machine.
pub struct Space {
    doc: LoroDoc,
    machine: Id,
    /// The identifier of a space this machine is joining, before its first sync brings `meta.id`.
    joining: Option<Id>,
    /// The notes an editor has open, with when this process last set each one's `updated`.
    open: HashMap<Id, OpenNote>,
}

impl Space {
    /// Creates a space with a new identifier and the built-in schemas, in its first commit.
    pub fn create(machine: Id) -> Result<Self> {
        let space = Self::empty(machine);
        space.doc.get_map(META).insert("id", Id::generate().to_string())?;
        schema::write_schemas(&space.doc, &schema::built_in())?;
        commit(&space.doc);
        Ok(space)
    }

    /// Opens a space from a snapshot or updates, and adds any built-in schemas it lacks.
    pub fn open(bytes: &[u8], machine: Id) -> Result<Self> {
        let space = Self::empty(machine);
        space.doc.import(bytes)?;
        space.id()?;
        if schema::write_schemas(&space.doc, &schema::built_in())? {
            commit(&space.doc);
        }
        Ok(space)
    }

    /// An empty copy of a space this machine is joining, which its first sync fills.
    pub fn joining(id: Id, machine: Id) -> Self {
        let mut space = Self::empty(machine);
        space.joining = Some(id);
        space
    }

    /// Whether this copy holds nothing yet.
    pub fn is_blank(&self) -> bool {
        self.doc.get_map(META).get("id").is_none()
    }

    /// A document with no content, and a fresh random Loro peer, as every process opens a space.
    fn empty(machine: Id) -> Self {
        let doc = LoroDoc::new();
        doc.set_record_timestamp(false);
        Self { doc, machine, joining: None, open: HashMap::new() }
    }

    /// The space's Loro document, for the parts of the core that read it directly.
    pub fn doc(&self) -> &LoroDoc {
        &self.doc
    }

    /// The identifier of the machine that edits this copy.
    pub fn machine(&self) -> Id {
        self.machine
    }

    /// The space's identifier, from `meta.id`.
    pub fn id(&self) -> Result<Id> {
        match self.doc.get_map(META).get("id") {
            Some(ValueOrContainer::Value(LoroValue::String(id))) => Id::parse(&id),
            _ => self.joining.ok_or_else(|| Error::Space("no identifier in meta".into())),
        }
    }

    /// Commits pending edits, such as typing, in one Loro commit.
    pub fn commit(&self) {
        commit(&self.doc);
    }

    /// The whole space as a snapshot, for saving.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.doc.export(ExportMode::Snapshot)?)
    }

    /// The space's version vector, in Loro's encoding.
    pub fn version(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    /// The updates that a machine with `version` lacks, or `None` when it lacks nothing.
    pub fn updates_since(&self, version: &[u8]) -> Result<Option<Vec<u8>>> {
        let version = VersionVector::decode(version)?;
        if version.includes_vv(&self.doc.oplog_vv()) {
            return Ok(None);
        }
        Ok(Some(self.doc.export(ExportMode::updates(&version))?))
    }

    /// Imports updates from another member after checking them in a copy, as the sync
    /// specification requires. It imports nothing if the check fails.
    pub fn import_checked(&mut self, updates: &[u8]) -> Result<()> {
        commit(&self.doc);
        // Updates from another space carry its own `meta.id`, which a merge might hide, so read
        // them alone first.
        let alone = LoroDoc::new();
        let _ = alone.import(updates);
        if let Some(ValueOrContainer::Value(LoroValue::String(id))) = alone.get_map(META).get("id")
            && Id::parse(&id).ok() != Some(self.id()?)
        {
            return Err(Error::Space("the updates belong to another space".into()));
        }
        let trial = self.doc.fork();
        trial.import(updates)?;
        check_import(&self.doc, &trial)?;
        self.doc.import(updates)?;
        Ok(())
    }

    fn notes(&self) -> LoroMap {
        self.doc.get_map(NOTES)
    }

    fn note_map(&self, id: Id) -> Result<LoroMap> {
        match self.notes().get(&id.to_string()) {
            Some(ValueOrContainer::Container(container)) => {
                container.into_map().map_err(|_| Error::Space(format!("note {id} isn't a map")))
            }
            _ => Err(Error::NotFound(format!("note {id}"))),
        }
    }

    /// Creates a note of `type_name` with `body`, in one commit.
    pub fn create_note(&mut self, type_name: &str, body: &str) -> Result<Id> {
        let id = Id::generate();
        self.create_note_with_id(id, type_name, body)?;
        Ok(id)
    }

    /// Finishes a durable agent reservation without replacing an existing or deleted note.
    pub fn create_reserved_note(&mut self, id: Id, type_name: &str, body: &str) -> Result<()> {
        if self.note(id).is_ok() {
            return Ok(());
        }
        if self.tombstone(id).is_some() {
            return Err(crate::Error::NotFound(format!("reserved note {id} was deleted")));
        }
        self.create_note_with_id(id, type_name, body)
    }

    fn create_note_with_id(&mut self, id: Id, type_name: &str, body: &str) -> Result<()> {
        if !schema::is_name(type_name) {
            return Err(Error::invalid("type name", type_name));
        }
        let note = self.notes().insert_container(&id.to_string(), LoroMap::new())?;
        note.insert("type", type_name)?;
        let text = note.insert_container("body", LoroText::new())?;
        text.insert(0, body)?;
        note.insert_container("fields", LoroMap::new())?;
        let meta = note.insert_container("meta", LoroMap::new())?;
        let now = time::now();
        let machine = self.machine.to_string();
        meta.insert("created", now)?;
        meta.insert("created-by", machine.as_str())?;
        meta.insert("updated", now)?;
        meta.insert("updated-by", machine.as_str())?;
        note.insert_container("settings", LoroMap::new())?;
        commit(&self.doc);
        Ok(())
    }

    /// The identifiers of every note in the space.
    pub fn note_ids(&self) -> Vec<Id> {
        self.notes().keys().filter_map(|key| Id::parse(&key).ok()).collect()
    }

    /// Reads a note.
    pub fn note(&self, id: Id) -> Result<Note> {
        let note = self.note_map(id)?;
        let LoroValue::Map(value) = note.get_deep_value() else {
            return Err(Error::Space(format!("note {id} isn't a map")));
        };
        let text = |key: &str| match value.get(key) {
            Some(LoroValue::String(s)) => s.to_string(),
            _ => String::new(),
        };
        let LoroValue::Map(meta) = value.get("meta").cloned().unwrap_or(LoroValue::Null) else {
            return Err(Error::Space(format!("note {id} has no meta")));
        };
        let number = |key: &str| match meta.get(key) {
            Some(LoroValue::I64(n)) => *n,
            _ => 0,
        };
        let machine = |key: &str| match meta.get(key) {
            Some(LoroValue::String(s)) => Id::parse(s).ok(),
            _ => None,
        };
        let fields = match value.get("fields") {
            Some(LoroValue::Map(fields)) => {
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            _ => BTreeMap::new(),
        };
        Ok(Note {
            id,
            type_name: text("type"),
            body: text("body"),
            fields,
            meta: NoteMeta {
                created: number("created"),
                created_by: machine("created-by"),
                updated: number("updated"),
                updated_by: machine("updated-by"),
            },
        })
    }

    /// The body of a note, for an editor to change directly.
    pub fn body(&self, id: Id) -> Result<LoroText> {
        match self.note_map(id)?.get("body") {
            Some(ValueOrContainer::Container(container)) => {
                container.into_text().map_err(|_| Error::Space(format!("body of {id}")))
            }
            _ => Err(Error::Space(format!("note {id} has no body"))),
        }
    }

    /// A note's body for an editor to change directly, which records the note as open.
    pub fn open_body(&mut self, id: Id) -> Result<crate::editor::Body> {
        self.open_note(id);
        Ok(crate::editor::Body { doc: self.doc.clone(), text: self.body(id)? })
    }

    /// Inserts `text` at a position in Unicode characters, without committing.
    pub fn insert_text(&mut self, id: Id, at: usize, text: &str) -> Result<()> {
        self.body(id)?.insert(at, text)?;
        self.touch(id)
    }

    /// Deletes `len` Unicode characters at `at`, without committing.
    pub fn delete_text(&mut self, id: Id, at: usize, len: usize) -> Result<()> {
        self.body(id)?.delete(at, len)?;
        self.touch(id)
    }

    /// Replaces a note's body with `text` by applying only the differences, without committing.
    pub fn set_body(&mut self, id: Id, text: &str) -> Result<()> {
        let body = self.body(id)?;
        if body.to_string() == text {
            return Ok(());
        }
        body.update(text, UpdateOptions::default()).map_err(|e| Error::Loro(format!("{e:?}")))?;
        self.touch(id)
    }

    /// Sets a field's value, in one commit.
    pub fn set_field(&mut self, id: Id, name: &str, value: &Value) -> Result<()> {
        if !schema::is_name(name) {
            return Err(Error::invalid("field name", name));
        }
        let fields = child_map(&self.note_map(id)?, "fields")?;
        fields.insert(name, value.to_loro())?;
        self.clear_derived(id, name)?;
        self.touch(id)?;
        commit(&self.doc);
        Ok(())
    }

    /// Clears a field, in one commit.
    pub fn clear_field(&mut self, id: Id, name: &str) -> Result<()> {
        let fields = child_map(&self.note_map(id)?, "fields")?;
        fields.delete(name)?;
        self.clear_derived(id, name)?;
        self.touch(id)?;
        commit(&self.doc);
        Ok(())
    }

    /// Clears every field derived from `source`, as a change to a source field requires.
    fn clear_derived(&self, id: Id, source: &str) -> Result<()> {
        let type_name = self.note(id)?.type_name;
        let Some(ty) = schema::read_type(&self.doc, &type_name) else { return Ok(()) };
        let fields = child_map(&self.note_map(id)?, "fields")?;
        for (name, def) in ty.fields {
            if def.derived_from.as_deref() == Some(source) && fields.get(&name).is_some() {
                fields.delete(&name)?;
            }
        }
        Ok(())
    }

    /// Records that an editor opened a note, so `updated` changes at most once a minute.
    pub fn open_note(&mut self, id: Id) {
        self.open.entry(id).or_default();
    }

    /// Records that an editor closed a note, and sets `updated` if it held unrecorded edits.
    pub fn close_note(&mut self, id: Id) -> Result<()> {
        if self.open.remove(&id).is_some_and(|open| open.pending) {
            self.write_updated(id)?;
            commit(&self.doc);
        }
        Ok(())
    }

    /// Sets `updated` and `updated-by`, but while an editor has the note open, at most once a
    /// minute.
    /// Records that an editor changed a note's body, as `insert_text` does, and commits.
    pub fn note_edited(&mut self, id: Id) -> Result<()> {
        self.touch(id)?;
        commit(&self.doc);
        Ok(())
    }

    fn touch(&mut self, id: Id) -> Result<()> {
        let now = time::now();
        if let Some(open) = self.open.get_mut(&id) {
            if open.written.is_some_and(|written| now - written < UPDATED_INTERVAL) {
                open.pending = true;
                return Ok(());
            }
            *open = OpenNote { written: Some(now), pending: false };
        }
        self.write_updated(id)
    }

    fn write_updated(&self, id: Id) -> Result<()> {
        let now = time::now();
        let meta = child_map(&self.note_map(id)?, "meta")?;
        meta.insert("updated", now)?;
        meta.insert("updated-by", self.machine.to_string())?;
        Ok(())
    }

    /// Encodes a position in a note's body, in Unicode characters, as a Loro cursor that stays with
    /// the same characters as the text around them changes.
    pub fn cursor_at(&self, id: Id, position: usize) -> Option<Vec<u8>> {
        let text = self.body(id).ok()?;
        text.get_cursor(position, loro::cursor::Side::Middle).map(|cursor| cursor.encode())
    }

    /// The position in Unicode characters that an encoded Loro cursor points to now.
    pub fn cursor_position(&self, bytes: &[u8]) -> Option<usize> {
        let cursor = loro::cursor::Cursor::decode(bytes).ok()?;
        self.doc.get_cursor_pos(&cursor).ok().map(|found| found.current.pos)
    }

    /// Deletes a note for good, leaving a tombstone with its title, in one commit.
    pub fn delete_note(&mut self, id: Id) -> Result<()> {
        let note = self.note(id)?;
        let title = self.title_of(&note.body);
        let tombstone =
            self.doc.get_map(TOMBSTONES).insert_container(&id.to_string(), LoroMap::new())?;
        tombstone.insert("title", title.markdown)?;
        tombstone.insert("deleted", time::now())?;
        tombstone.insert("deleted-by", self.machine.to_string())?;
        self.notes().delete(&id.to_string())?;
        self.open.remove(&id);
        commit(&self.doc);
        Ok(())
    }

    /// The tombstone of a note deleted for good.
    pub fn tombstone(&self, id: Id) -> Option<Tombstone> {
        let LoroValue::Map(map) =
            self.doc.get_map(TOMBSTONES).get(&id.to_string())?.get_deep_value()
        else {
            return None;
        };
        Some(Tombstone {
            title: match map.get("title") {
                Some(LoroValue::String(s)) => s.to_string(),
                _ => String::new(),
            },
            deleted: match map.get("deleted") {
                Some(LoroValue::I64(n)) => *n,
                _ => 0,
            },
            deleted_by: match map.get("deleted-by") {
                Some(LoroValue::String(s)) => Id::parse(s).ok(),
                _ => None,
            },
        })
    }

    /// Copies a note, with the same identifier, into `target`, then deletes it here without a
    /// tombstone, as decision ZIB0005 describes moving a note. Files go with it separately.
    pub fn move_note_to(&mut self, id: Id, target: &mut Self) -> Result<()> {
        let note = self.note(id)?;
        target.create_note_with_id(id, &note.type_name, &note.body)?;
        let fields = child_map(&target.note_map(id)?, "fields")?;
        for (name, value) in &note.fields {
            fields.insert(name, value.clone())?;
        }
        commit(&target.doc);
        self.notes().delete(&id.to_string())?;
        commit(&self.doc);
        Ok(())
    }

    /// A note's title, with each link resolved against this space.
    pub fn title(&self, id: Id) -> Result<Title> {
        let note = self.note(id)?;
        Ok(self.title_of(&note.body))
    }

    fn title_of(&self, body_text: &str) -> Title {
        let body = |id: Id| self.note(id).ok().map(|note| note.body);
        let mut titles = body::Titles::new(&body);
        let mut resolve = |target: Id| {
            titles.title(target).or_else(|| {
                self.tombstone(target)
                    .map(|t| Title { plain: body::plain_text(&t.title), markdown: t.title })
            })
        };
        body::title(body_text, &mut resolve)
    }

    /// A synced setting of the space, or of a note when `note` is given.
    pub fn setting(&self, note: Option<Id>, key: &str) -> Result<Option<LoroValue>> {
        let map = match note {
            Some(id) => child_map(&self.note_map(id)?, "settings")?,
            None => self.doc.get_map(SETTINGS),
        };
        Ok(match map.get(key) {
            Some(ValueOrContainer::Value(value)) => Some(value),
            _ => None,
        })
    }

    /// Sets a synced setting, in one commit. Keys start with an implementation's name and a
    /// period, or `zib.`.
    pub fn set_setting(
        &mut self,
        note: Option<Id>,
        key: &str,
        value: impl Into<LoroValue>,
    ) -> Result<()> {
        if !key.contains('.') {
            return Err(Error::invalid("setting key", key));
        }
        let map = match note {
            Some(id) => child_map(&self.note_map(id)?, "settings")?,
            None => self.doc.get_map(SETTINGS),
        };
        map.insert(key, value.into())?;
        commit(&self.doc);
        Ok(())
    }

    /// Records a blob that this machine added and holds whole, in the same commit as the note
    /// that first refers to it, so it doesn't commit.
    pub fn add_file(&mut self, hash: FileHash, size: u64, media_type: &str) -> Result<()> {
        let entry = child_map(&self.doc.get_map(FILES), &hash.to_string())?;
        entry.insert("size", i64::try_from(size).unwrap_or(i64::MAX))?;
        entry.insert("media-type", media_type)?;
        child_map(&entry, "holders")?.insert(&self.machine.to_string(), time::now())?;
        Ok(())
    }

    /// Adds or removes this machine as a holder of a blob, in one commit.
    pub fn set_holding(&mut self, hash: FileHash, holds: bool) -> Result<()> {
        let entry = child_map(&self.doc.get_map(FILES), &hash.to_string())?;
        let holders = child_map(&entry, "holders")?;
        if holds {
            holders.insert(&self.machine.to_string(), time::now())?;
        } else {
            holders.delete(&self.machine.to_string())?;
        }
        commit(&self.doc);
        Ok(())
    }

    /// The entry for a blob.
    pub fn file(&self, hash: FileHash) -> Option<FileEntry> {
        let LoroValue::Map(map) = self.doc.get_map(FILES).get(&hash.to_string())?.get_deep_value()
        else {
            return None;
        };
        let holders = match map.get("holders") {
            Some(LoroValue::Map(h)) => h
                .iter()
                .filter_map(|(k, v)| match v {
                    LoroValue::I64(t) => Some((Id::parse(k).ok()?, *t)),
                    _ => None,
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        Some(FileEntry {
            size: match map.get("size") {
                Some(LoroValue::I64(n)) => u64::try_from(*n).unwrap_or(0),
                _ => 0,
            },
            media_type: match map.get("media-type") {
                Some(LoroValue::String(s)) => s.to_string(),
                _ => String::new(),
            },
            holders,
        })
    }

    /// Every blob that a note's file field holds, which are the blobs in use.
    pub fn files_in_use(&self) -> Vec<FileHash> {
        let mut hashes = Vec::new();
        for id in self.note_ids() {
            hashes.extend(self.note_files(id));
        }
        hashes.sort();
        hashes.dedup();
        hashes
    }

    /// The blobs that a note's file fields hold.
    pub fn note_files(&self, id: Id) -> Vec<FileHash> {
        let mut hashes = Vec::new();
        let Ok(note) = self.note(id) else { return hashes };
        let Some(ty) = schema::read_type(&self.doc, &note.type_name) else { return hashes };
        for (name, def) in &ty.fields {
            if let Some(value) = note.fields.get(name).and_then(|v| Value::from_loro(def, v)) {
                collect_files(&value, &mut hashes);
            }
        }
        hashes
    }

    /// Adds a machine as a member, in one commit.
    pub fn add_machine(
        &mut self,
        id: Id,
        key: MachineKey,
        name: &str,
        policy: Policy,
    ) -> Result<()> {
        let entry = child_map(&self.doc.get_map(MACHINES), &id.to_string())?;
        entry.insert("key", key.to_hex())?;
        entry.insert("name", name)?;
        entry.insert("policy", policy.name())?;
        commit(&self.doc);
        Ok(())
    }

    /// Renames a member, in one commit.
    pub fn rename_machine(&mut self, id: Id, name: &str) -> Result<()> {
        child_map(&self.doc.get_map(MACHINES), &id.to_string())?.insert("name", name)?;
        commit(&self.doc);
        Ok(())
    }

    /// Marks a member removed, and removes its holder entries, in one commit. Nothing clears the
    /// mark.
    pub fn remove_machine(&mut self, id: Id) -> Result<()> {
        let key = id.to_string();
        child_map(&self.doc.get_map(MACHINES), &key)?.insert("removed", time::now())?;
        let files = self.doc.get_map(FILES);
        for hash in files.keys() {
            if let Some(ValueOrContainer::Container(entry)) = files.get(&hash)
                && let Ok(entry) = entry.into_map()
                && let Some(ValueOrContainer::Container(h)) = entry.get("holders")
                && let Ok(holders) = h.into_map()
            {
                holders.delete(&key)?;
            }
        }
        commit(&self.doc);
        Ok(())
    }

    /// Every machine in `machines`, removed ones included.
    pub fn machines(&self) -> Vec<Machine> {
        machines_of(&self.doc)
    }

    /// The members that aren't marked removed.
    pub fn members(&self) -> Vec<Machine> {
        self.machines().into_iter().filter(|m| m.removed.is_none()).collect()
    }
}

fn collect_files(value: &Value, out: &mut Vec<FileHash>) {
    match value {
        Value::File(hash) => out.push(*hash),
        Value::List(items) => items.iter().for_each(|v| collect_files(v, out)),
        _ => {}
    }
}

fn machines_of(doc: &LoroDoc) -> Vec<Machine> {
    let LoroValue::Map(map) = doc.get_map(MACHINES).get_deep_value() else { return Vec::new() };
    map.iter()
        .filter_map(|(id, value)| {
            let LoroValue::Map(entry) = value else { return None };
            let text = |key: &str| match entry.get(key) {
                Some(LoroValue::String(s)) => Some(s.to_string()),
                _ => None,
            };
            Some(Machine {
                id: Id::parse(id).ok()?,
                key: MachineKey::from_hex(&text("key")?).ok()?,
                name: text("name").unwrap_or_default(),
                policy: text("policy")
                    .and_then(|p| Policy::from_name(&p))
                    .unwrap_or(Policy::OnDemand),
                removed: match entry.get("removed") {
                    Some(LoroValue::I64(t)) => Some(*t),
                    _ => None,
                },
            })
        })
        .collect()
}

/// Checks a copy with incoming updates against the space before them: the identifier stays the
/// same, and no removed machine comes back.
fn check_import(before: &LoroDoc, after: &LoroDoc) -> Result<()> {
    let id = |doc: &LoroDoc| match doc.get_map(META).get("id") {
        Some(ValueOrContainer::Value(LoroValue::String(id))) => Some(id.to_string()),
        _ => None,
    };
    let expected = id(before);
    if expected.is_some() && id(after) != expected {
        return Err(Error::Space("the updates change the space's identifier".into()));
    }
    let removed_after: Vec<Id> =
        machines_of(after).into_iter().filter(|m| m.removed.is_some()).map(|m| m.id).collect();
    for machine in machines_of(before) {
        if machine.removed.is_some() && !removed_after.contains(&machine.id) {
            return Err(Error::Space(format!(
                "the updates bring back removed machine {}",
                machine.id
            )));
        }
    }
    Ok(())
}

/// Commits `doc`'s pending changes as the core's own, with the `ORIGIN` that undo leaves out.
pub(crate) fn commit(doc: &LoroDoc) {
    doc.commit_with(CommitOptions::new().origin(ORIGIN));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> Space {
        Space::create(Id::generate()).unwrap()
    }

    #[test]
    fn creates_spaces_with_identity_and_schemas() {
        let space = space();
        let reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        assert_eq!(reopened.id().unwrap(), space.id().unwrap());
        assert!(schema::read_type(reopened.doc(), schema::IMAGE).is_some());
    }

    #[test]
    fn creates_and_edits_notes() {
        let mut space = space();
        let id = space.create_note(schema::NOTE, "# Hello\nworld").unwrap();
        space.insert_text(id, 7, ", there").unwrap();
        space.commit();
        let note = space.note(id).unwrap();
        assert_eq!(note.body, "# Hello, there\nworld");
        assert_eq!(note.type_name, "note");
        assert_eq!(note.meta.created_by, Some(space.machine()));
        assert_eq!(space.title(id).unwrap().plain, "Hello, there");
        space.set_body(id, "Rewritten").unwrap();
        space.commit();
        assert_eq!(space.note(id).unwrap().body, "Rewritten");
    }

    #[test]
    fn deletes_with_tombstones_and_moves_without() {
        let mut space = space();
        let other = space.create_note(schema::NOTE, "*Other*").unwrap();
        let id = space.create_note(schema::NOTE, &format!("About [[{other}]]")).unwrap();
        space.delete_note(id).unwrap();
        assert_eq!(space.tombstone(id).unwrap().title, "About *Other*");
        assert!(space.note(id).is_err());

        let mut target = Space::create(space.machine()).unwrap();
        space.move_note_to(other, &mut target).unwrap();
        assert!(space.tombstone(other).is_none());
        assert_eq!(target.note(other).unwrap().body, "*Other*");
    }

    #[test]
    fn changing_a_source_clears_derived_fields() {
        let mut space = space();
        let id = space.create_note(schema::FILE, "report.pdf").unwrap();
        let pdf = FileHash::of(b"pdf");
        let text = FileHash::of(b"text");
        space.add_file(pdf, 3, "application/pdf").unwrap();
        space.set_field(id, "file", &Value::File(pdf)).unwrap();
        space.add_file(text, 4, "text/plain").unwrap();
        space.set_field(id, "text", &Value::File(text)).unwrap();
        assert_eq!(space.files_in_use(), {
            let mut v = vec![pdf, text];
            v.sort();
            v
        });
        space.set_field(id, "file", &Value::File(FileHash::of(b"pdf 2"))).unwrap();
        let note = space.note(id).unwrap();
        assert!(note.field(&space, "text").is_none());
        assert!(note.field(&space, "file").is_some());
    }

    #[test]
    fn updated_changes_at_most_once_a_minute_while_open() {
        let mut space = space();
        let id = space.create_note(schema::NOTE, "a").unwrap();
        space.open_note(id);
        space.insert_text(id, 1, "b").unwrap();
        let first = space.note(id).unwrap().meta.updated;
        space.insert_text(id, 2, "c").unwrap();
        assert_eq!(space.note(id).unwrap().meta.updated, first);
    }

    #[test]
    fn syncs_and_refuses_bad_updates() {
        let mut a = space();
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();
        let id = a.create_note(schema::NOTE, "from a").unwrap();
        let updates = a.updates_since(&b.version()).unwrap().unwrap();
        b.import_checked(&updates).unwrap();
        assert_eq!(b.note(id).unwrap().body, "from a");
        assert!(a.updates_since(&b.version()).unwrap().is_none());

        let stranger = Space::create(Id::generate()).unwrap();
        let foreign = stranger.snapshot().unwrap();
        assert!(b.import_checked(&foreign).is_err());
        assert_eq!(b.id().unwrap(), a.id().unwrap());
    }

    #[test]
    fn removed_machines_stay_removed() {
        let mut a = space();
        let phone = Id::generate();
        a.add_machine(phone, MachineKey([7; 32]), "Phone", Policy::OnDemand).unwrap();
        let mut stale = Space::open(&a.snapshot().unwrap(), phone).unwrap();
        a.remove_machine(phone).unwrap();
        // The removed phone rewrites its entry from a copy that hasn't heard of the removal.
        stale.add_machine(phone, MachineKey([7; 32]), "Phone again", Policy::OnDemand).unwrap();
        let updates = stale.updates_since(&a.version()).unwrap().unwrap();
        a.import_checked(&updates).unwrap();
        assert!(a.members().iter().all(|m| m.id != phone));
    }

    #[test]
    fn parses_file_hashes() {
        let hash = FileHash::of(b"x");
        assert_eq!(hash.to_string().parse::<FileHash>().unwrap(), hash);
        assert!(hash.to_string().to_uppercase().parse::<FileHash>().is_err());
    }
}
