//! Everything the running instance holds, and every operation its window, tray, and the `zib`
//! command ask of it. The core runs on Tokio, as decision ZIB0011 describes; the window reaches it
//! through `State` and hears back through `UiEvent`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify, oneshot};
use zib_core::address::{Address, MachineKey};
use zib_core::agent::Agent;
use zib_core::channel::Dirs;
use zib_core::files::{self, Blobs, GarbageRecords};
use zib_core::instance::NoteRead;
use zib_core::net::{Nearby, Network, Node, OpenNote, Pairing};
use zib_core::pairing::{Invite, Paired, Party};
use zib_core::render::{Image, Kind, Renderer};
use zib_core::schema::{self, Value};
use zib_core::search::{Hit, Index};
use zib_core::space::{FileHash, Machine, Note, Policy};
use zib_core::store::Store;
use zib_core::sync::{Exchanged, SyncState};
use zib_core::{Channel, Error, Id, Result, Space};

use crate::derive;

mod agent;
mod web_clip;

/// The Linux app keeps every file, as decision ZIB0004 sets for it.
pub const POLICY: Policy = Policy::All;
/// The pet name of the space that a new machine starts with.
const DEFAULT_SPACE: &str = "Default";
/// How many times a machine tries to receive a space that another shared with it while pairing.
const RECEIVE_TRIES: u32 = 6;
/// How long the instance gathers changes made on the machine before it syncs them.
const LOCAL_GATHER: Duration = Duration::from_secs(2);
/// How long the instance syncs what it can before it quits.
const QUIT_SYNC: Duration = Duration::from_secs(10);
/// How often changed spaces go to disk.
const SAVE_PERIOD: Duration = Duration::from_secs(2);
/// How often the instance looks for derivations to run and garbage to collect.
const UPKEEP_PERIOD: Duration = Duration::from_secs(60);

/// What the instance tells the window.
#[derive(Debug)]
pub enum UiEvent {
    /// An agent tool waits for the owner to choose a note's destination.
    AgentDestination { title: String, reply: oneshot::Sender<Option<Id>> },
    /// Open the window, and a note if a URI names one.
    Activate(Option<String>),
    /// The owner asked to quit.
    Quit,
    /// A space's content changed outside the window's editor.
    Changed(Id),
    /// Another machine asks to pair: show the code and ask which spaces to share.
    /// The reply holds the spaces to share, each with its pet name.
    ConfirmPairing { peer: Party, code: String, reply: oneshot::Sender<Option<Vec<(Id, String)>>> },
    /// Pairing or joining finished, with a message for the owner.
    Notice(String),
    /// A space arrived from another machine with the name `arrived`: ask the owner for one.
    NameSpace { space: Id, from: String, arrived: String },
}

/// A space's sync state, as the sync details show it.
#[derive(Clone, Debug)]
pub struct SpaceSync {
    pub name: String,
    /// `None` when the space is shared with no other machine.
    pub state: Option<SyncState>,
    pub members: Vec<MemberSync>,
}

/// Another member of a space, as the sync details show it.
#[derive(Clone, Debug)]
pub struct MemberSync {
    pub name: String,
    pub key: MachineKey,
    /// When it last finished an exchange with this machine.
    pub last: Option<zib_core::time::Timestamp>,
    /// Whether it lacks a change made on this machine.
    pub lacks: bool,
}

/// A note, as the note list shows it.
#[derive(Clone, Debug)]
pub struct NoteRow {
    pub id: Id,
    pub title: String,
    pub plain: String,
    pub updated: i64,
    pub type_name: String,
}

/// The running instance.
pub struct State {
    pub agent: Agent,
    pub channel: Channel,
    pub dirs: Dirs,
    pub store: Mutex<Store>,
    pub node: Arc<Node>,
    indexes: StdMutex<HashMap<Id, Index>>,
    pub renderer: Renderer,
    pub quit: Notify,
    /// Sends events to the app once it listens, and until then holds them, such as the
    /// `activate` that the `zib` command sends right after starting the instance.
    ui: StdMutex<Result<async_channel::Sender<UiEvent>, Vec<UiEvent>>>,
    dirty: StdMutex<HashSet<Id>>,
    records: Mutex<HashMap<Id, GarbageRecords>>,
    pairing: Arc<UiPairing>,
    nearby: Mutex<Option<Nearby>>,
    syncing: StdMutex<Syncing>,
    /// Wakes the sync loop after a change made on this machine.
    soon: Notify,
}

/// The exchanges with members that run now, and how many times in a row each failed, by space
/// and member.
#[derive(Default)]
struct Syncing {
    running: HashSet<(Id, Id)>,
    /// How many exchanges in a row failed, and when the last one did.
    failures: HashMap<(Id, Id), (u32, std::time::Instant)>,
}

/// Pairing as the window decides it: requests wait for the owner on the Add machine screen.
pub struct UiPairing {
    accepting: std::sync::atomic::AtomicBool,
    /// The spaces the owner chose to share in the last confirmation.
    last: StdMutex<Vec<Id>>,
    state: std::sync::OnceLock<std::sync::Weak<State>>,
}

impl Pairing for UiPairing {
    fn exchanged(
        &self,
        space: Id,
        exchanged: Exchanged,
        changed: bool,
    ) -> impl Future<Output = ()> + Send {
        let state = self.state.get().and_then(std::sync::Weak::upgrade);
        async move {
            let Some(state) = state else { return };
            state.record_exchange(space, exchanged).await;
            if changed {
                state.arrived(space).await;
            }
        }
    }

    fn accepting(&self) -> bool {
        self.accepting.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn confirm(
        &self,
        peer: &Party,
        code: &str,
    ) -> impl Future<Output = Option<Vec<(Id, String)>>> + Send {
        let state = self.state.get().and_then(std::sync::Weak::upgrade);
        let (peer, code) = (peer.clone(), code.to_owned());
        async move {
            let state = state?;
            let (reply, answer) = oneshot::channel();
            state.send_ui(UiEvent::ConfirmPairing { peer, code, reply });
            let shared = answer.await.ok().flatten();
            *state.pairing.last.lock().expect("unpoisoned") =
                shared.iter().flatten().map(|(id, _)| *id).collect();
            shared
        }
    }

    fn paired(&self, paired: Paired) -> impl Future<Output = ()> + Send {
        let state = self.state.get().and_then(std::sync::Weak::upgrade);
        async move {
            if let Some(state) = state {
                state.finish_pairing(paired, None, Vec::new()).await;
            }
        }
    }

    fn offered(
        &self,
        space: Id,
        from: MachineKey,
        name: Option<String>,
    ) -> impl Future<Output = bool> + Send {
        let state = self.state.get().and_then(std::sync::Weak::upgrade);
        async move {
            let Some(state) = state else { return false };
            let party = state.store.lock().await.paired().into_iter().find(|p| p.key == from);
            let Some(party) = party else { return false };
            tokio::spawn(async move {
                let address = Address { key: from, hints: Vec::new() };
                if let Err(error) = state.receive_space(space, &address, &party.name, name).await {
                    state.send_ui(UiEvent::Notice(format!(
                        "Couldn't receive a space from {}: {error}",
                        party.name
                    )));
                }
            });
            true
        }
    }
}

impl State {
    /// Opens the machine store and every space, binds the endpoint, and starts the background
    /// work.
    pub async fn start(channel: Channel, dirs: Dirs) -> Result<Arc<Self>> {
        let store = Store::open(&dirs, POLICY)?;
        let network = Network::Internet { relay: store.config.relay.clone() };
        let state = Self::open(channel, dirs, store, network).await?;
        tokio::spawn(state.node.clone().serve(state.pairing.clone()));
        tokio::spawn(state.clone().sync_forever());
        tokio::spawn(state.clone().save_forever());
        tokio::spawn(state.clone().upkeep_forever());
        Ok(state)
    }

    async fn open(
        channel: Channel,
        dirs: Dirs,
        store: Store,
        network: Network,
    ) -> Result<Arc<Self>> {
        let secret = store.secret_key()?;
        let node = Arc::new(
            Node::bind(secret, store.config.id, &store.config.name, POLICY, network).await?,
        );
        node.set_spaces_dir(store.spaces_dir());
        let pairing = Arc::new(UiPairing {
            accepting: false.into(),
            last: StdMutex::new(Vec::new()),
            state: std::sync::OnceLock::new(),
        });
        let state = Arc::new(Self {
            agent: Agent::open(&dirs.data)?,
            channel,
            renderer: Renderer::new(dirs.cache.join("rendered")),
            dirs,
            store: Mutex::new(store),
            node,
            indexes: StdMutex::new(HashMap::new()),
            quit: Notify::new(),
            ui: StdMutex::new(Err(Vec::new())),
            dirty: StdMutex::new(HashSet::new()),
            records: Mutex::new(HashMap::new()),
            pairing: pairing.clone(),
            nearby: Mutex::new(None),
            syncing: StdMutex::default(),
            soon: Notify::new(),
        });
        let _ = pairing.state.set(Arc::downgrade(&state));
        state.open_spaces().await?;
        // A new machine starts with a space, so the owner can write a note straight away.
        if state.store.lock().await.created {
            let space = state.create_space(DEFAULT_SPACE).await?;
            state.set_last_space(space).await?;
        }
        Ok(state)
    }

    #[cfg(test)]
    pub(crate) async fn for_test(dirs: Dirs) -> Result<Arc<Self>> {
        let store = Store::open(&dirs, POLICY)?;
        Self::open(Channel::Development, dirs, store, Network::Loopback).await
    }

    /// Sets where events for the window go.
    pub fn set_ui(&self, sender: async_channel::Sender<UiEvent>) {
        let mut ui = self.ui.lock().expect("unpoisoned");
        if let Err(waiting) = &mut *ui {
            for event in waiting.drain(..) {
                let _ = sender.try_send(event);
            }
        }
        *ui = Ok(sender);
    }

    pub fn send_ui(&self, event: UiEvent) {
        match &mut *self.ui.lock().expect("unpoisoned") {
            Ok(sender) => {
                let _ = sender.try_send(event);
            }
            // Without a display, no app ever listens, so only the first few wait.
            Err(waiting) if waiting.len() < 16 => waiting.push(event),
            Err(_) => {}
        }
    }

    async fn open_spaces(&self) -> Result<()> {
        let mut store = self.store.lock().await;
        for id in store.config.spaces.keys().copied().collect::<Vec<_>>() {
            let space = store.load_space(id)?;
            store.record_peer(&space)?;
            self.index(id, &space)?;
            self.node.spaces().lock().await.insert(id, Arc::new(Mutex::new(space)));
        }
        Ok(())
    }

    fn blobs(&self, space: Id) -> Blobs {
        self.node.blobs(space).expect("set at start")
    }

    /// Rebuilds a space's search index in the cache directory.
    fn index(&self, id: Id, space: &Space) -> Result<()> {
        let dir = self.dirs.cache.join("spaces").join(id.to_string());
        zib_core::store::create_private_dir(&dir)?;
        let mut index = Index::open(&dir.join("index.sqlite"))?;
        let blobs = self.blobs(id);
        index.rebuild(space, &|hash| fs_text(&blobs, hash))?;
        self.indexes.lock().expect("unpoisoned").insert(id, index);
        Ok(())
    }

    fn reindex_note(&self, space_id: Id, space: &Space, note: Id) {
        let blobs = self.blobs(space_id);
        if let Some(index) = self.indexes.lock().expect("unpoisoned").get_mut(&space_id) {
            let _ = index.update_note(space, note, &|hash| fs_text(&blobs, hash));
        }
    }

    async fn space(&self, id: Id) -> Result<Arc<Mutex<Space>>> {
        self.node
            .spaces()
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("space {id}")))
    }

    fn mark_dirty(&self, space: Id) {
        self.dirty.lock().expect("unpoisoned").insert(space);
    }

    /// Marks a space that changed on this machine, which syncs with its members soon after.
    fn changed_here(&self, space: Id) {
        self.mark_dirty(space);
        self.soon.notify_one();
    }

    /// The machine's spaces, by pet name.
    pub async fn spaces(&self) -> Vec<(Id, String)> {
        let store = self.store.lock().await;
        let mut spaces: Vec<(Id, String)> =
            store.config.spaces.iter().map(|(id, e)| (*id, e.name.clone())).collect();
        spaces.sort_by_key(|a| a.1.to_lowercase());
        spaces
    }

    /// The machine's spaces, by pet name, with whether each one's notes are hidden.
    pub async fn space_list(&self) -> Vec<(Id, String, bool)> {
        let store = self.store.lock().await;
        let mut spaces: Vec<(Id, String, bool)> =
            store.config.spaces.iter().map(|(id, e)| (*id, e.name.clone(), e.hidden)).collect();
        spaces.sort_by_key(|a| a.1.to_lowercase());
        spaces
    }

    pub async fn set_hidden(&self, space: Id, hidden: bool) -> Result<()> {
        self.store.lock().await.set_hidden(space, hidden)
    }

    /// The width of the window's AI chat that the owner last chose on this machine.
    pub async fn chat_width(&self) -> Option<u32> {
        self.store.lock().await.config.chat_width
    }

    pub async fn set_chat_width(&self, width: u32) -> Result<()> {
        self.store.lock().await.set_chat_width(width)
    }

    /// The space the owner last put a note into.
    pub async fn last_space(&self) -> Option<Id> {
        self.store.lock().await.config.last_space
    }

    pub async fn set_last_space(&self, space: Id) -> Result<()> {
        self.store.lock().await.set_last_space(space)
    }

    /// A deleted note or a space no longer held cannot reopen.
    pub async fn last_note(&self) -> Option<(Id, Id)> {
        let last = self.store.lock().await.config.last_note?;
        self.note(last.space, last.note).await.ok()?;
        Some((last.space, last.note))
    }

    pub async fn remember_note(&self, space: Id, note: Id) -> Result<()> {
        self.store.lock().await.set_last_note(space, note)
    }

    pub async fn create_space(&self, name: &str) -> Result<Id> {
        let mut store = self.store.lock().await;
        let mut space = store.create_space(name)?;
        let me = self.node.me();
        space.add_machine(me.machine, me.key, &me.name, POLICY)?;
        store.save_space(&space)?;
        store.record_peer(&space)?;
        let id = space.id()?;
        self.index(id, &space)?;
        self.node.spaces().lock().await.insert(id, Arc::new(Mutex::new(space)));
        Ok(id)
    }

    pub async fn rename_space(&self, id: Id, name: &str) -> Result<()> {
        self.store.lock().await.rename_space(id, name)
    }

    /// The name of a machine in a space's `machines`.
    pub async fn machine_name(&self, space: Id, machine: Id) -> Option<String> {
        let space = self.space(space).await.ok()?;
        let space = space.lock().await;
        space.machines().into_iter().find(|m| m.id == machine).map(|m| m.name)
    }

    /// A space's notes, most recently changed first.
    pub async fn notes(&self, space_id: Id) -> Result<Vec<NoteRow>> {
        let space = self.space(space_id).await?;
        let space = space.lock().await;
        let mut rows: Vec<NoteRow> = space
            .note_ids()
            .into_iter()
            .filter_map(|id| {
                let note = space.note(id).ok()?;
                let title = space.title(id).ok()?;
                Some(NoteRow {
                    id,
                    title: title.markdown,
                    plain: title.plain,
                    updated: note.meta.updated,
                    type_name: note.type_name,
                })
            })
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.updated));
        Ok(rows)
    }

    pub async fn note(&self, space_id: Id, id: Id) -> Result<zib_core::space::Note> {
        self.space(space_id).await?.lock().await.note(id)
    }

    /// Reads a note and its populated, schema-validated fields from the same space snapshot.
    pub(crate) async fn note_with_fields(
        &self,
        space_id: Id,
        id: Id,
    ) -> Result<(Note, Vec<(String, Value)>)> {
        let space = self.space(space_id).await?;
        let space = space.lock().await;
        let note = space.note(id)?;
        let mut fields = Vec::new();
        if !note.fields.is_empty()
            && let Some(schema) = schema::read_type(space.doc(), &note.type_name)
        {
            fields.extend(schema.fields.into_iter().filter_map(|(name, def)| {
                let value = Value::from_loro(&def, note.fields.get(&name)?)?;
                Some((name, value))
            }));
        }
        Ok((note, fields))
    }

    pub(crate) async fn read_note(&self, note: Id) -> Result<NoteRead> {
        let space =
            self.find_note(note).await.ok_or_else(|| Error::NotFound(format!("note {note}")))?;
        let doc = self.space(space).await?;
        let doc = doc.lock().await;
        Ok(NoteRead::Found {
            body: doc.note(note)?.body,
            note,
            space,
            title: doc.title(note)?.plain,
        })
    }

    /// The markdown titles of notes in any space this machine holds.
    pub async fn titles(&self, targets: &[Id]) -> HashMap<Id, String> {
        let spaces: Vec<Arc<Mutex<Space>>> =
            self.node.spaces().lock().await.values().cloned().collect();
        let mut out = HashMap::new();
        for space in spaces {
            let space = space.lock().await;
            for target in targets {
                if out.contains_key(target) {
                    continue;
                }
                if let Ok(title) = space.title(*target) {
                    out.insert(*target, title.markdown);
                } else if let Some(tombstone) = space.tombstone(*target) {
                    out.insert(*target, tombstone.title);
                }
            }
        }
        out
    }

    /// Which space holds a note, for links that name only the note.
    pub async fn find_note(&self, note: Id) -> Option<Id> {
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        for (id, space) in spaces {
            if space.lock().await.note(note).is_ok() {
                return Some(id);
            }
        }
        None
    }

    pub async fn create_note(&self, space_id: Id, body: &str) -> Result<Id> {
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        let id = space.create_note(zib_core::schema::NOTE, body)?;
        self.reindex_note(space_id, &space, id);
        self.changed_here(space_id);
        Ok(id)
    }

    /// A note's body, for an editor to change directly.
    pub async fn open_body(&self, space_id: Id, id: Id) -> Result<zib_core::editor::Body> {
        let space = self.space(space_id).await?;
        space.lock().await.open_body(id)
    }

    pub(crate) async fn spelling_dictionary(&self, space_id: Id, word: Option<&str>) -> Result<Id> {
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        let before = space.version();
        let note = match word {
            Some(word) => space.add_spelling_word(word)?,
            None => space.spelling_dictionary()?,
        };
        if space.version() != before {
            self.reindex_note(space_id, &space, note);
            self.changed_here(space_id);
        }
        Ok(note)
    }

    pub(crate) async fn ignore_spelling_word(&self, space: Id, note: Id, word: &str) -> Result<()> {
        self.space(space).await?.lock().await.ignore_spelling_word(note, word)?;
        self.changed_here(space);
        Ok(())
    }

    /// Records that an editor changed a note's body directly: updates its metadata and its place
    /// in the search index, and has the space saved and synced.
    pub async fn body_edited(&self, space_id: Id, id: Id) -> Result<()> {
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.note_edited(id)?;
        self.reindex_note(space_id, &space, id);
        self.changed_here(space_id);
        Ok(())
    }

    pub(crate) async fn replace_note_body(
        &self,
        note: Id,
        expected: &str,
        body: &str,
        shown_only: bool,
    ) -> Result<String> {
        // Store before space keeps visibility stable for an agent write.
        let store = self.store.lock().await;
        for (space_id, entry) in &store.config.spaces {
            if shown_only && entry.hidden {
                continue;
            }
            let space = self.space(*space_id).await?;
            let mut space = space.lock().await;
            let Ok(current) = space.note(note) else { continue };
            if current.body != body && current.body != expected {
                return Err(Error::NoteChanged(note));
            }
            space.set_body(note, body)?;
            let saved = store.save_space(&space);
            self.reindex_note(*space_id, &space, note);
            self.changed_here(*space_id);
            self.send_ui(UiEvent::Changed(*space_id));
            saved?;
            return Ok(space.title(note)?.plain);
        }
        let scope = if shown_only { " in shown spaces" } else { "" };
        Err(Error::NotFound(format!("note {note}{scope}")))
    }

    pub async fn delete_note(&self, space_id: Id, id: Id) -> Result<()> {
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.delete_note(id)?;
        self.reindex_note(space_id, &space, id);
        self.changed_here(space_id);
        Ok(())
    }

    /// Searches the selected space or every space, best matches first within each space.
    pub(crate) fn search_in(&self, query: &str, limit: usize, scope: Option<Id>) -> Vec<(Id, Hit)> {
        let indexes = self.indexes.lock().expect("unpoisoned");
        let mut hits = Vec::new();
        for (space, index) in indexes.iter().filter(|(id, _)| scope.is_none_or(|s| s == **id)) {
            if let Ok(found) = index.search(query, limit) {
                hits.extend(found.into_iter().map(|hit| (*space, hit)));
            }
        }
        hits.truncate(limit);
        hits
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(Id, Hit)> {
        self.search_in(query, limit, None)
    }

    /// The notes in any space that link to `note`, with their spaces.
    pub fn backlinks(&self, note: Id) -> Vec<(Id, Id)> {
        let indexes = self.indexes.lock().expect("unpoisoned");
        let mut found = Vec::new();
        for (space, index) in indexes.iter() {
            let sources = index.backlinks(note).unwrap_or_default();
            found.extend(sources.into_iter().filter(|source| *source != note).map(|s| (*space, s)));
        }
        found
    }

    /// Adds a file to a space as a note, and returns the note.
    pub async fn add_file(&self, space_id: Id, path: PathBuf, media_type: String) -> Result<Id> {
        let blobs = self.blobs(space_id);
        let source = path.clone();
        let (hash, size) = tokio::task::spawn_blocking(move || blobs.add_file(&source))
            .await
            .map_err(|e| Error::Protocol(e.to_string()))??;
        let name = path
            .file_name()
            .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
        let id = self.add_file_note(space_id, hash, size, &name, &media_type).await?;
        let _ = self.derive(space_id).await;
        Ok(id)
    }

    /// Adds bytes that aren't in a file, such as a pasted image, to a space as a note named `name`,
    /// and returns the note. It leaves the note's derived fields to `derive`, which changes the
    /// space again, so the caller can use the note first.
    pub async fn add_bytes(
        &self,
        space_id: Id,
        bytes: Vec<u8>,
        name: String,
        media_type: String,
    ) -> Result<Id> {
        let blobs = self.blobs(space_id);
        let size = bytes.len() as u64;
        let hash = tokio::task::spawn_blocking(move || blobs.add_bytes(&bytes))
            .await
            .map_err(|e| Error::Protocol(e.to_string()))??;
        self.add_file_note(space_id, hash, size, &name, &media_type).await
    }

    /// Creates the note for a blob already among the space's blobs.
    async fn add_file_note(
        &self,
        space_id: Id,
        hash: FileHash,
        size: u64,
        name: &str,
        media_type: &str,
    ) -> Result<Id> {
        let space = self.space(space_id).await?;
        let id = {
            let mut space = space.lock().await;
            let id = files::add_file_note(&mut space, hash, size, name, media_type)?;
            self.reindex_note(space_id, &space, id);
            id
        };
        self.changed_here(space_id);
        Ok(id)
    }

    /// The path of a blob on this machine, fetching it from a holder first if needed.
    pub async fn blob_path(&self, space_id: Id, hash: FileHash) -> Result<PathBuf> {
        let blobs = self.blobs(space_id);
        if !blobs.has(hash) {
            self.node.fetch_blob(space_id, hash).await?;
        }
        Ok(blobs.path(hash))
    }

    /// The file a `file` or `image` note holds.
    pub async fn note_file(
        &self,
        space_id: Id,
        note: Id,
        field: &str,
    ) -> Result<Option<(FileHash, String)>> {
        let space = self.space(space_id).await?;
        let space = space.lock().await;
        let note = space.note(note)?;
        let Some(Value::File(hash)) = note.field(&space, field) else { return Ok(None) };
        let media = space.file(hash).map(|f| f.media_type).unwrap_or_default();
        Ok(Some((hash, media)))
    }

    /// An image already on this machine: its original, or its thumbnail while the original arrives.
    pub async fn image_preview(&self, target: Id) -> Option<PathBuf> {
        let space = self.find_note(target).await?;
        if self.note(space, target).await.ok()?.type_name != "image" {
            return None;
        }
        let blobs = self.blobs(space);
        // The original takes precedence over the thumbnail, regardless of their names.
        for field in ["image", "thumbnail"] {
            if let Some((hash, _)) = self.note_file(space, target, field).await.ok()?
                && blobs.has(hash)
            {
                return Some(blobs.path(hash));
            }
        }
        None
    }

    /// Prints notes to a PDF, as the instance specification's `print` request describes: each note
    /// in order, and with `linked`, the notes each one links to, unless the PDF already holds them.
    pub async fn print(&self, notes: &[Id], linked: bool) -> Result<Vec<u8>> {
        let mut order: Vec<Id> = Vec::new();
        let mut bodies = Vec::new();
        for &note in notes {
            let space = self
                .find_note(note)
                .await
                .ok_or_else(|| Error::NotFound(format!("note {note}")))?;
            let body = self.note(space, note).await?.body;
            let mut more = Vec::new();
            if linked {
                for link in zib_core::body::wikilinks(&body).into_iter().filter(|l| !l.embed) {
                    if !more.contains(&link.target) {
                        more.push(link.target);
                    }
                }
            }
            if !order.contains(&note) {
                order.push(note);
                bodies.push(body);
            }
            for target in more {
                if order.contains(&target) {
                    continue;
                }
                let Some(space) = self.find_note(target).await else { continue };
                if let Ok(note) = self.note(space, target).await {
                    order.push(target);
                    bodies.push(note.body);
                }
            }
        }
        let mut refs = zib_core::print::Refs::default();
        let targets: Vec<Id> =
            bodies.iter().flat_map(|body| zib_core::body::link_targets(body)).collect();
        refs.titles = self.titles(&targets).await;
        for body in &bodies {
            for link in zib_core::body::wikilinks(body).into_iter().filter(|l| l.embed) {
                if refs.images.contains_key(&link.target) {
                    continue;
                }
                let Some(space) = self.find_note(link.target).await else { continue };
                let Ok(Some((hash, media))) = self.note_file(space, link.target, "image").await
                else {
                    continue;
                };
                if let Ok(data) = self
                    .blob_path(space, hash)
                    .await
                    .and_then(|path| std::fs::read(&path).map_err(|e| Error::io(path, e)))
                {
                    refs.images.insert(link.target, (data, media));
                }
            }
        }
        let page = self.store.lock().await.config.page.clone();
        tokio::task::spawn_blocking(move || zib_core::print::pdf(&bodies, &refs, &page))
            .await
            .map_err(|e| Error::Render(e.to_string()))?
    }

    /// Draws math or a diagram, from the cache when it's there.
    pub async fn render(&self, kind: Kind, source: String) -> Result<Image> {
        let renderer = self.renderer.clone();
        tokio::task::spawn_blocking(move || renderer.image(kind, &source))
            .await
            .map_err(|e| Error::Render(e.to_string()))?
    }

    /// Each space's sync state, and each other member with when it last synced and whether it
    /// lacks a change made here, for the sync details. A space shared with no other machine has no
    /// state.
    pub async fn sync_details(&self) -> Vec<SpaceSync> {
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        let store = self.store.lock().await;
        let mut details = Vec::new();
        for (id, space) in spaces {
            let space = space.lock().await;
            let peers = store.config.peers.get(&id).cloned().unwrap_or_default();
            let recorded = store.config.versions.get(&id).cloned().unwrap_or_default();
            let last = store.config.last_synced.get(&id);
            let members: Vec<MemberSync> = space
                .members()
                .into_iter()
                .filter(|m| m.id != space.machine())
                .map(|m| MemberSync {
                    name: m.name.clone(),
                    key: m.key,
                    last: last.and_then(|l| l.get(&m.id)).copied(),
                    lacks: zib_core::sync::lacks_changes(&space, &peers, &recorded, m.id),
                })
                .collect();
            let state =
                (!members.is_empty()).then(|| zib_core::sync::status(&space, &peers, &recorded));
            let name = store.config.spaces.get(&id).map(|e| e.name.clone()).unwrap_or_default();
            details.push(SpaceSync { name, state, members });
        }
        details.sort_by_key(|d| d.name.to_lowercase());
        details
    }

    /// The sync status across every space, the least synced first.
    pub async fn status(&self) -> SyncState {
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        let store = self.store.lock().await;
        let mut state = SyncState::Synced;
        for (id, space) in spaces {
            let space = space.lock().await;
            // A space this machine shares with no other has nothing to sync.
            if space.members().iter().all(|m| m.id == space.machine()) {
                continue;
            }
            let peers = store.config.peers.get(&id).cloned().unwrap_or_default();
            let recorded = store.config.versions.get(&id).cloned().unwrap_or_default();
            state = state.min(zib_core::sync::status(&space, &peers, &recorded));
        }
        state
    }

    /// Opens a note for editing, which syncs its space and starts live sessions.
    pub async fn open_note(&self, space: Id, note: Id) -> Result<OpenNote> {
        self.node.open_note(space, note).await
    }

    pub async fn close_note(&self, space_id: Id, note: Id) {
        self.node.close_note(space_id, note);
        if let Ok(space) = self.space(space_id).await {
            let mut space = space.lock().await;
            if space.close_note(note).is_ok() {
                self.reindex_note(space_id, &space, note);
                self.changed_here(space_id);
            }
        }
    }

    /// This machine's address, as the Add machine screen shows it.
    pub fn address(&self) -> Address {
        self.node.address()
    }

    /// Starts or stops answering pairing requests and announcing this machine nearby.
    pub async fn set_adding(
        &self,
        adding: bool,
    ) -> Option<tokio::sync::mpsc::Receiver<zib_core::net::NearbyEvent>> {
        self.pairing.accepting.store(adding, std::sync::atomic::Ordering::SeqCst);
        let mut nearby = self.nearby.lock().await;
        if !adding {
            *nearby = None;
            return None;
        }
        let name = self.store.lock().await.config.name.clone();
        let mut announced = self.node.announce_nearby(&name).await.ok()?;
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let mut events = std::mem::replace(&mut announced.events, tokio::sync::mpsc::channel(1).1);
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        *nearby = Some(announced);
        Some(receiver)
    }

    /// Pairs with the machine at `address`, sharing `share` once its owner confirms.
    pub async fn pair(
        &self,
        address: Address,
        code: oneshot::Sender<String>,
        confirmed: oneshot::Receiver<bool>,
        share: Vec<(Id, String)>,
    ) -> Result<()> {
        let ids = share.iter().map(|(id, _)| *id).collect();
        let shared = share;
        let paired = self
            .node
            .pair(&address, async move |_: &Party, shown: &str| {
                let _ = code.send(shown.to_owned());
                confirmed.await.unwrap_or(false).then_some(shared)
            })
            .await?;
        self.finish_pairing(paired, Some(address), ids).await;
        Ok(())
    }

    /// Adds the other machine to the spaces this one shared, and receives the spaces it shared.
    async fn finish_pairing(&self, paired: Paired, address: Option<Address>, shared: Vec<Id>) {
        // A paired machine can offer this one spaces later, as decision ZIB0016 describes.
        if let Err(error) = self.store.lock().await.add_paired(&paired.peer) {
            self.send_ui(UiEvent::Notice(format!(
                "Couldn't remember {}: {error}",
                paired.peer.name
            )));
        }
        let address = address.unwrap_or(Address { key: paired.peer.key, hints: Vec::new() });
        let shared = if shared.is_empty() { self.last_shared() } else { shared };
        for space_id in &shared {
            if let Ok(space) = self.space(*space_id).await {
                let added = space.lock().await.add_machine(
                    paired.peer.machine,
                    paired.peer.key,
                    &paired.peer.name,
                    paired.peer.policy,
                );
                if added.is_ok() {
                    self.mark_dirty(*space_id);
                    let _ = self.node.sync(&space, &address).await;
                }
            }
        }
        for space_id in &paired.received {
            let suggested = paired.names.get(space_id).cloned();
            let received = self.receive_space(*space_id, &address, &paired.peer.name, suggested);
            if let Err(error) = received.await {
                self.send_ui(UiEvent::Notice(format!(
                    "Couldn't receive a space from {}: {error}",
                    paired.peer.name
                )));
            }
        }
        self.send_ui(UiEvent::Notice(format!("Paired with {}", paired.peer.name)));
    }

    /// The machines this one paired with, and whether each is a member of `space`.
    pub async fn sharing(&self, space_id: Id) -> Result<Vec<(Party, bool)>> {
        let space = self.space(space_id).await?;
        let members: HashSet<Id> = space.lock().await.members().into_iter().map(|m| m.id).collect();
        let paired = self.store.lock().await.paired();
        Ok(paired
            .into_iter()
            .map(|p| {
                let member = members.contains(&p.machine);
                (p, member)
            })
            .collect())
    }

    /// Shares a space with a paired machine: adds it to the space's members and offers it the
    /// space, as decision ZIB0016 describes.
    pub async fn share_space(&self, space_id: Id, machine: Id) -> Result<()> {
        let party = self.store.lock().await.paired().into_iter().find(|p| p.machine == machine);
        let party = party.ok_or_else(|| Error::NotFound(format!("paired machine {machine}")))?;
        let space = self.space(space_id).await?;
        space.lock().await.add_machine(party.machine, party.key, &party.name, party.policy)?;
        self.mark_dirty(space_id);
        self.save_dirty().await;
        let name = self.store.lock().await.config.spaces.get(&space_id).map(|e| e.name.clone());
        self.node.offer(space_id, name, &Address { key: party.key, hints: Vec::new() }).await
    }

    /// Moves a note to another space with its files, as decision ZIB0005 describes moving one.
    pub async fn move_note(&self, from: Id, to: Id, note: Id) -> Result<()> {
        if from == to {
            return Ok(());
        }
        // A clip retry locates its original note across spaces under this same lock.
        let _store = self.store.lock().await;
        let (source, target) = (self.space(from).await?, self.space(to).await?);
        let (source_blobs, target_blobs) = (self.blobs(from), self.blobs(to));
        {
            let mut source = source.lock().await;
            let mut target = target.lock().await;
            for hash in source.note_files(note) {
                let Some(entry) = source.file(hash) else { continue };
                if source_blobs.has(hash) && !target_blobs.has(hash) {
                    target_blobs.add_file(&source_blobs.path(hash))?;
                }
                target.add_file(hash, entry.size, &entry.media_type)?;
                if !target_blobs.has(hash) {
                    target.set_holding(hash, false)?;
                }
            }
            source.move_note_to(note, &mut target)?;
            self.reindex_note(from, &source, note);
            self.reindex_note(to, &target, note);
        }
        self.changed_here(from);
        self.changed_here(to);
        self.send_ui(UiEvent::Changed(from));
        self.send_ui(UiEvent::Changed(to));
        Ok(())
    }

    /// Moves every note of one space to another, and returns the notes it moved.
    pub async fn move_notes(&self, from: Id, to: Id) -> Result<Vec<Id>> {
        let notes = self.space(from).await?.lock().await.note_ids();
        for note in &notes {
            self.move_note(from, to, *note).await?;
        }
        Ok(notes)
    }

    /// The spaces the owner chose in the last confirmation.
    fn last_shared(&self) -> Vec<Id> {
        std::mem::take(&mut *self.pairing.last.lock().expect("unpoisoned"))
    }

    /// Receives a space that `from` shared, and names it `suggested`, the pet name it has there,
    /// until the owner names it.
    async fn receive_space(
        &self,
        space_id: Id,
        address: &Address,
        from: &str,
        suggested: Option<String>,
    ) -> Result<()> {
        if self.node.spaces().lock().await.contains_key(&space_id) {
            return Ok(());
        }
        // The other machine adds this one as a member only once its side of pairing finishes, which
        // can come after this side's first try, so later tries wait longer, up to half a minute.
        let mut tries = 0;
        let space = loop {
            match self.node.fetch(space_id, address).await {
                Ok(space) => break space,
                Err(error) if tries + 1 >= RECEIVE_TRIES => return Err(error),
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(1 << tries)).await;
                    tries += 1;
                }
            }
        };
        let mut store = self.store.lock().await;
        let guard = space.lock().await;
        let arrived = suggested
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Shared by {from}"));
        store.add_space(&guard, &arrived)?;
        store.record_peer(&guard)?;
        self.index(space_id, &guard)?;
        drop(guard);
        drop(store);
        self.send_ui(UiEvent::Changed(space_id));
        self.send_ui(UiEvent::NameSpace { space: space_id, from: from.to_owned(), arrived });
        Ok(())
    }

    /// Makes an invite link for a space.
    pub fn invite(&self, space: Id) -> Result<Invite> {
        self.node.invite(space)
    }

    /// Joins a space by invite link.
    pub async fn join(&self, uri: &str) -> Result<Id> {
        let invite = Invite::parse(uri)?;
        let space = self.node.join(&invite).await?;
        self.receive_space(
            space,
            &Address { key: invite.key, hints: invite.hints.clone() },
            "an invite",
            None,
        )
        .await?;
        Ok(space)
    }

    /// Syncs every space with every member every 30 seconds, with longer waits for members that
    /// keep failing, and fetches the files this machine lacks.
    async fn sync_forever(self: Arc<Self>) {
        loop {
            self.start_exchanges(false).await;
            tokio::select! {
                () = tokio::time::sleep(zib_core::sync::PERIOD) => {}
                () = self.soon.notified() => tokio::time::sleep(LOCAL_GATHER).await,
            }
        }
    }

    /// Starts an exchange of each space with each member that's due one and has none running, each
    /// on its own task, so a member that's away never holds up the others. With `quitting`, it
    /// starts one only with each member that lacks a change made here and answered last time, and
    /// waits for them, so the instance quits as soon as what it made has gone where it can go.
    async fn start_exchanges(self: &Arc<Self>, quitting: bool) {
        let (paired, own_peers, recorded) = {
            let store = self.store.lock().await;
            let paired: HashSet<MachineKey> = store.paired().into_iter().map(|p| p.key).collect();
            (paired, store.config.peers.clone(), store.config.versions.clone())
        };
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        let mut started = tokio::task::JoinSet::new();
        for (id, space) in spaces {
            let members = {
                let space = space.lock().await;
                let me = space.machine();
                let peers = own_peers.get(&id).map_or(&[][..], Vec::as_slice);
                let recorded = recorded.get(&id).cloned().unwrap_or_default();
                space
                    .members()
                    .into_iter()
                    .filter(|m| m.id != me)
                    .filter(|m| {
                        !quitting || zib_core::sync::lacks_changes(&space, peers, &recorded, m.id)
                    })
                    .collect::<Vec<_>>()
            };
            for member in members {
                let pair = (id, member.id);
                let count = {
                    let mut syncing = self.syncing.lock().expect("unpoisoned");
                    if syncing.running.contains(&pair) {
                        continue;
                    }
                    let (count, last) = syncing
                        .failures
                        .get(&pair)
                        .copied()
                        .map_or((0, None), |(c, t)| (c, Some(t)));
                    if quitting && count > 0 {
                        continue;
                    }
                    if !quitting && last.is_some_and(|last| last.elapsed() < retry_wait(count)) {
                        continue;
                    }
                    syncing.running.insert(pair);
                    count
                };
                // A paired machine that failed may lack the space, so it hears the offer again.
                let offer = count > 0 && paired.contains(&member.key);
                let (state, space) = (self.clone(), space.clone());
                started.spawn(async move {
                    state.exchange(id, &space, member, offer, !quitting).await;
                });
            }
        }
        if quitting {
            started.join_all().await;
        } else {
            started.detach_all();
        }
    }

    /// Exchanges a space's changes with one member, then, with `fetch`, fetches the files this
    /// machine lacks, and tells the window when the space changed.
    async fn exchange(
        &self,
        id: Id,
        space: &Arc<Mutex<Space>>,
        member: Machine,
        offer: bool,
        fetch: bool,
    ) {
        let before = space.lock().await.version();
        let address = Address { key: member.key, hints: Vec::new() };
        if offer {
            let name = self.store.lock().await.config.spaces.get(&id).map(|e| e.name.clone());
            let _ = self.node.offer(id, name, &address).await;
        }
        let result = self.node.sync(space, &address).await;
        let pair = (id, member.id);
        let succeeded = {
            let mut syncing = self.syncing.lock().expect("unpoisoned");
            syncing.running.remove(&pair);
            if result.is_ok() {
                syncing.failures.remove(&pair);
            } else {
                let count = syncing.failures.get(&pair).map_or(0, |(c, _)| *c);
                syncing.failures.insert(pair, (count + 1, std::time::Instant::now()));
            }
            result.is_ok()
        };
        if let Ok(exchanged) = result {
            self.record_exchange(id, exchanged).await;
        }
        if fetch && succeeded && self.node.fetch_missing(id).await > 0 {
            self.mark_dirty(id);
        }
        if space.lock().await.version() != before {
            self.arrived(id).await;
        }
    }

    /// Records the version vector a member holds after an exchange, in either direction.
    async fn record_exchange(&self, id: Id, exchanged: Exchanged) {
        let mut store = self.store.lock().await;
        let now = zib_core::time::now();
        store.config.last_synced.entry(id).or_default().insert(exchanged.machine, now);
        store.config.versions.entry(id).or_default().insert(exchanged.machine, exchanged.version);
        let _ = store.save();
    }

    /// Saves, indexes, and shows changes that arrived from another member.
    async fn arrived(&self, id: Id) {
        self.mark_dirty(id);
        if let Ok(space) = self.space(id).await {
            let _ = self.index(id, &*space.lock().await);
        }
        self.send_ui(UiEvent::Changed(id));
    }

    /// Saves and indexes a space after a live edit from another machine arrived, which the
    /// editor shows by itself.
    pub async fn live_changed(&self, id: Id) {
        self.mark_dirty(id);
        if let Ok(space) = self.space(id).await {
            let _ = self.index(id, &*space.lock().await);
        }
    }

    /// Saves changed spaces every two seconds.
    async fn save_forever(self: Arc<Self>) {
        loop {
            tokio::time::sleep(SAVE_PERIOD).await;
            self.save_dirty().await;
        }
    }

    async fn save_dirty(&self) {
        let dirty: Vec<Id> = self.dirty.lock().expect("unpoisoned").drain().collect();
        for id in dirty {
            if let Ok(space) = self.space(id).await {
                // The store before the space, as everywhere else, or two tasks could each hold
                // one and wait for the other forever.
                let store = self.store.lock().await;
                let _ = store.save_space(&*space.lock().await);
            }
        }
    }

    /// Runs derivations and collects garbage every minute.
    async fn upkeep_forever(self: Arc<Self>) {
        loop {
            let spaces: Vec<Id> = self.node.spaces().lock().await.keys().copied().collect();
            for id in spaces {
                let _ = self.derive(id).await;
                let _ = self.collect_garbage(id).await;
            }
            tokio::time::sleep(UPKEEP_PERIOD).await;
        }
    }

    fn records_path(&self, space: Id) -> PathBuf {
        self.dirs.cache.join("spaces").join(space.to_string()).join("records.json")
    }

    /// Fills the derived fields this machine can, as the schemas specification describes.
    pub async fn derive(&self, space_id: Id) -> Result<()> {
        let space = self.space(space_id).await?;
        let blobs = self.blobs(space_id);
        let pending = files::pending_derivations(&*space.lock().await, &blobs);
        let mut records = self.records.lock().await;
        let record = records
            .entry(space_id)
            .or_insert_with(|| GarbageRecords::load(&self.records_path(space_id)));
        let mut changed = false;
        for item in pending {
            if record.has_failed(&item) {
                continue;
            }
            let path = blobs.path(item.source);
            let (derivation, media) = (item.derivation.clone(), item.media_type.clone());
            let made = tokio::task::spawn_blocking(move || derive::run(&derivation, &path, &media))
                .await
                .ok()
                .flatten();
            match made {
                Some((bytes, media_type)) => {
                    let mut space = space.lock().await;
                    if files::complete_derivation(&mut space, &blobs, &item, &bytes, &media_type)? {
                        self.reindex_note(space_id, &space, item.note);
                        changed = true;
                    }
                }
                None => record.record_failure(&item),
            }
        }
        record.save(&self.records_path(space_id))?;
        if changed {
            self.changed_here(space_id);
            self.send_ui(UiEvent::Changed(space_id));
        }
        Ok(())
    }

    async fn collect_garbage(&self, space_id: Id) -> Result<()> {
        let space = self.space(space_id).await?;
        let blobs = self.blobs(space_id);
        let mut records = self.records.lock().await;
        let record = records
            .entry(space_id)
            .or_insert_with(|| GarbageRecords::load(&self.records_path(space_id)));
        let deleted = files::collect_garbage(
            &mut *space.lock().await,
            &blobs,
            record,
            zib_core::time::now(),
        )?;
        record.save(&self.records_path(space_id))?;
        if !deleted.is_empty() {
            self.changed_here(space_id);
        }
        Ok(())
    }

    /// Syncs what it can within 10 seconds, saves every space, and closes the endpoint.
    pub async fn finish(self: &Arc<Self>) {
        self.agent.stop();
        let _ = self.agent.save();
        // Everything goes to disk first, so nothing waits on the network to be safe.
        self.save_dirty().await;
        let _ = tokio::time::timeout(QUIT_SYNC, self.start_exchanges(true)).await;
        self.save_dirty().await;
        let spaces: Vec<Arc<Mutex<Space>>> =
            self.node.spaces().lock().await.values().cloned().collect();
        let store = self.store.lock().await;
        for space in spaces {
            let _ = store.save_space(&*space.lock().await);
        }
        drop(store);
        self.node.close().await;
    }
}

/// The text of a derived text blob, for the search index.
fn fs_text(blobs: &Blobs, hash: FileHash) -> Option<String> {
    let bytes = std::fs::read(blobs.path(hash)).ok()?;
    if bytes.len() > 8 << 20 {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// How long to wait before trying a member again after `count` failures in a row: 30 seconds,
/// doubling with each failure, up to 10 minutes, as the sync specification sets.
fn retry_wait(count: u32) -> Duration {
    let first = zib_core::sync::FIRST_RETRY;
    first.saturating_mul(1 << count.saturating_sub(1).min(5)).min(zib_core::sync::LAST_RETRY)
}

/// The directory where a path lives, for messages.
pub fn display_path(path: &Path) -> String {
    path.display().to_string()
}
