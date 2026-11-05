//! Everything the running instance holds, and every operation its window, tray, and the `zib`
//! command ask of it. The core runs on Tokio, as decision ZIB0011 describes; the window reaches it
//! through `State` and hears back through `UiEvent`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify, oneshot};
use zib_core::address::{Address, MachineKey};
use zib_core::channel::Dirs;
use zib_core::files::{self, Blobs, GarbageRecords};
use zib_core::net::{Nearby, Network, Node, OpenNote, Pairing};
use zib_core::pairing::{Invite, Paired, Party};
use zib_core::render::{Image, Kind, Renderer};
use zib_core::schema::Value;
use zib_core::search::{Hit, Index};
use zib_core::space::{FileHash, Policy};
use zib_core::store::Store;
use zib_core::sync::SyncState;
use zib_core::{Channel, Error, Id, Result, Space};

use crate::derive;

/// The Linux app keeps every file, as decision ZIB0004 sets for it.
pub const POLICY: Policy = Policy::All;
/// The pet name of the space that a new machine starts with.
const DEFAULT_SPACE: &str = "Default";
/// How many times a machine tries to receive a space that another shared with it while pairing.
const RECEIVE_TRIES: u32 = 6;
/// How long the instance syncs what it can before it quits.
const QUIT_SYNC: Duration = Duration::from_secs(10);
/// How often changed spaces go to disk.
const SAVE_PERIOD: Duration = Duration::from_secs(2);
/// How often the instance looks for derivations to run and garbage to collect.
const UPKEEP_PERIOD: Duration = Duration::from_secs(60);

/// What the instance tells the window.
#[derive(Debug)]
pub enum UiEvent {
    /// Open the window, and a note if a URI names one.
    Activate(Option<String>),
    /// The owner asked to quit.
    Quit,
    /// A space's content changed through sync or a derivation.
    Changed(Id),
    /// Another machine asks to pair: show the code and ask which spaces to share.
    ConfirmPairing { peer: Party, code: String, reply: oneshot::Sender<Option<Vec<Id>>> },
    /// Pairing or joining finished, with a message for the owner.
    Notice(String),
    /// A space arrived from another machine with the name `arrived`: ask the owner for one.
    NameSpace { space: Id, from: String, arrived: String },
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
}

/// Pairing as the window decides it: requests wait for the owner on the Add machine screen.
pub struct UiPairing {
    accepting: std::sync::atomic::AtomicBool,
    /// The spaces the owner chose to share in the last confirmation.
    last: StdMutex<Vec<Id>>,
    state: std::sync::OnceLock<std::sync::Weak<State>>,
}

impl Pairing for UiPairing {
    fn accepting(&self) -> bool {
        self.accepting.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn confirm(&self, peer: &Party, code: &str) -> impl Future<Output = Option<Vec<Id>>> + Send {
        let state = self.state.get().and_then(std::sync::Weak::upgrade);
        let (peer, code) = (peer.clone(), code.to_owned());
        async move {
            let state = state?;
            let (reply, answer) = oneshot::channel();
            state.send_ui(UiEvent::ConfirmPairing { peer, code, reply });
            let shared = answer.await.ok().flatten();
            *state.pairing.last.lock().expect("unpoisoned") = shared.clone().unwrap_or_default();
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

    fn offered(&self, space: Id, from: MachineKey) -> impl Future<Output = bool> + Send {
        let state = self.state.get().and_then(std::sync::Weak::upgrade);
        async move {
            let Some(state) = state else { return false };
            let party = state.store.lock().await.paired().into_iter().find(|p| p.key == from);
            let Some(party) = party else { return false };
            tokio::spawn(async move {
                let address = Address { key: from, hints: Vec::new() };
                if let Err(error) = state.receive_space(space, &address, &party.name).await {
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
        let secret = store.secret_key()?;
        let network = Network::Internet { relay: store.config.relay.clone() };
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
        });
        let _ = pairing.state.set(Arc::downgrade(&state));
        state.open_spaces().await?;
        // A new machine starts with a space, so the owner can write a note straight away.
        if state.store.lock().await.created {
            let space = state.create_space(DEFAULT_SPACE).await?;
            state.set_last_space(space).await?;
        }
        tokio::spawn(state.node.clone().serve(pairing));
        tokio::spawn(state.clone().sync_forever());
        tokio::spawn(state.clone().save_forever());
        tokio::spawn(state.clone().upkeep_forever());
        Ok(state)
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

    /// The space the owner last put a note into.
    pub async fn last_space(&self) -> Option<Id> {
        self.store.lock().await.config.last_space
    }

    pub async fn set_last_space(&self, space: Id) -> Result<()> {
        self.store.lock().await.set_last_space(space)
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
        self.mark_dirty(space_id);
        Ok(id)
    }

    /// Stores a note's body as the editor holds it, with filled-in labels already removed.
    pub async fn save_body(&self, space_id: Id, id: Id, body: &str) -> Result<()> {
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.set_body(id, body)?;
        space.commit();
        self.reindex_note(space_id, &space, id);
        self.mark_dirty(space_id);
        Ok(())
    }

    pub async fn delete_note(&self, space_id: Id, id: Id) -> Result<()> {
        let space = self.space(space_id).await?;
        let mut space = space.lock().await;
        space.delete_note(id)?;
        self.reindex_note(space_id, &space, id);
        self.mark_dirty(space_id);
        Ok(())
    }

    /// Searches every space, best matches first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(Id, Hit)> {
        let indexes = self.indexes.lock().expect("unpoisoned");
        let mut hits = Vec::new();
        for (space, index) in indexes.iter() {
            if let Ok(found) = index.search(query, limit) {
                hits.extend(found.into_iter().map(|hit| (*space, hit)));
            }
        }
        hits.truncate(limit);
        hits
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
        let space = self.space(space_id).await?;
        let id = {
            let mut space = space.lock().await;
            let id = files::add_file_note(&mut space, hash, size, &name, &media_type)?;
            self.reindex_note(space_id, &space, id);
            id
        };
        self.mark_dirty(space_id);
        let _ = self.derive(space_id).await;
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

    /// The sync status across every space, the least synced first.
    pub async fn status(&self) -> SyncState {
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        let store = self.store.lock().await;
        let mut state = SyncState::Synced;
        for (id, space) in spaces {
            let peers = store.config.peers.get(&id).cloned().unwrap_or_default();
            let recorded = store.config.versions.get(&id).cloned().unwrap_or_default();
            state = state.min(zib_core::sync::status(&*space.lock().await, &peers, &recorded));
        }
        state
    }

    /// Opens a note for editing, which syncs its space and starts live sessions.
    pub async fn open_note(&self, space: Id, note: Id) -> Result<OpenNote> {
        self.node.open_note(space, note).await
    }

    pub fn close_note(&self, space: Id, note: Id) {
        self.node.close_note(space, note);
    }

    /// Encodes a position in a note's body as a Loro cursor.
    pub async fn cursor_at(&self, space_id: Id, note: Id, position: usize) -> Option<Vec<u8>> {
        let space = self.space(space_id).await.ok()?;
        space.lock().await.cursor_at(note, position)
    }

    /// The position in a note's body that an encoded Loro cursor points to now.
    pub async fn cursor_position(&self, space_id: Id, bytes: &[u8]) -> Option<usize> {
        let space = self.space(space_id).await.ok()?;
        space.lock().await.cursor_position(bytes)
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
        share: Vec<Id>,
    ) -> Result<()> {
        let shared = share.clone();
        let paired = self
            .node
            .pair(&address, async move |_: &Party, shown: &str| {
                let _ = code.send(shown.to_owned());
                confirmed.await.unwrap_or(false).then_some(shared)
            })
            .await?;
        self.finish_pairing(paired, Some(address), share).await;
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
            if let Err(error) = self.receive_space(*space_id, &address, &paired.peer.name).await {
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
        self.node.offer(space_id, &Address { key: party.key, hints: Vec::new() }).await
    }

    /// Moves a note to another space with its files, as decision ZIB0005 describes moving one.
    pub async fn move_note(&self, from: Id, to: Id, note: Id) -> Result<()> {
        if from == to {
            return Ok(());
        }
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
        self.mark_dirty(from);
        self.mark_dirty(to);
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

    async fn receive_space(&self, space_id: Id, address: &Address, from: &str) -> Result<()> {
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
        let arrived = format!("Shared by {from}");
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
        )
        .await?;
        Ok(space)
    }

    /// Syncs every space with every member every 30 seconds, with longer waits for members that
    /// keep failing, and fetches the files this machine lacks.
    async fn sync_forever(self: Arc<Self>) {
        let mut failures: HashMap<(Id, Id), u32> = HashMap::new();
        loop {
            self.sync_once(&mut failures).await;
            tokio::time::sleep(zib_core::sync::PERIOD).await;
        }
    }

    async fn sync_once(&self, failures: &mut HashMap<(Id, Id), u32>) {
        let paired: HashSet<MachineKey> =
            self.store.lock().await.paired().into_iter().map(|p| p.key).collect();
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        for (id, space) in spaces {
            let (members, before) = {
                let space = space.lock().await;
                let members = space
                    .members()
                    .into_iter()
                    .filter(|m| m.id != space.machine())
                    .collect::<Vec<_>>();
                (members, space.version())
            };
            for member in members {
                let count = failures.get(&(id, member.id)).copied().unwrap_or(0);
                if count > 0 && !due(count) {
                    failures.insert((id, member.id), count + 1);
                    continue;
                }
                // A paired machine that failed may lack the space, so it hears the offer again.
                if count > 0 && paired.contains(&member.key) {
                    let address = Address { key: member.key, hints: Vec::new() };
                    let _ = self.node.offer(id, &address).await;
                }
                match self.node.sync(&space, &Address { key: member.key, hints: Vec::new() }).await
                {
                    Ok(exchanged) => {
                        failures.remove(&(id, member.id));
                        let mut store = self.store.lock().await;
                        store
                            .config
                            .versions
                            .entry(id)
                            .or_default()
                            .insert(exchanged.machine, exchanged.version);
                        let _ = store.save();
                    }
                    Err(_) => {
                        failures.insert((id, member.id), count + 1);
                    }
                }
            }
            if self.node.fetch_missing(id).await > 0 {
                self.mark_dirty(id);
            }
            let changed = space.lock().await.version() != before;
            if changed {
                self.mark_dirty(id);
                let guard = space.lock().await;
                let _ = self.index(id, &guard);
                drop(guard);
                self.send_ui(UiEvent::Changed(id));
            }
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
                let space = space.lock().await;
                let _ = self.store.lock().await.save_space(&space);
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
            self.mark_dirty(space_id);
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
            self.mark_dirty(space_id);
        }
        Ok(())
    }

    /// Syncs what it can within 10 seconds, saves every space, and closes the endpoint.
    pub async fn finish(&self) {
        let mut failures = HashMap::new();
        let _ = tokio::time::timeout(QUIT_SYNC, self.sync_once(&mut failures)).await;
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

/// Whether a member that failed `count` times in a row is due another try: the wait doubles from
/// one period up to 10 minutes.
fn due(count: u32) -> bool {
    let period = zib_core::sync::PERIOD.as_secs();
    let wait = (zib_core::sync::FIRST_RETRY.as_secs() << count.min(5))
        .min(zib_core::sync::LAST_RETRY.as_secs());
    u64::from(count) * period >= wait
}

/// The directory where a path lives, for messages.
pub fn display_path(path: &Path) -> String {
    path.display().to_string()
}
