//! The running instance: it holds the machine's spaces, syncs them, keeps their search indexes,
//! and answers the `zib` command, as the instance specification defines it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use zib_core::channel::Dirs;
use zib_core::instance::{self, Handler, HitInfo, Lock, Reply, Request, SpaceInfo};
use zib_core::net::{Network, Node, Pairing};
use zib_core::pairing::{Paired, Party};
use zib_core::search::Index;
use zib_core::space::Policy;
use zib_core::store::Store;
use zib_core::{Channel, Id, Result, Space};

use crate::tray;

/// How long the instance syncs what it can before it quits.
const QUIT_SYNC: Duration = Duration::from_secs(10);

/// The Linux app keeps every file, as decision ZIB0004 sets for it.
const POLICY: Policy = Policy::All;

/// Everything the instance holds while it runs.
pub struct State {
    channel: Channel,
    dirs: Dirs,
    store: Mutex<Store>,
    node: Arc<Node>,
    indexes: StdMutex<HashMap<Id, Index>>,
    quit: Notify,
}

/// Pairing waits for the Add machine screen, which the app doesn't have yet, so it refuses.
struct NoPairing;

impl Pairing for NoPairing {
    fn accepting(&self) -> bool {
        false
    }

    fn confirm(&self, _: &Party, _: &str) -> impl Future<Output = Option<Vec<Id>>> + Send {
        std::future::ready(None)
    }

    fn paired(&self, _: Paired) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// Runs the instance until the owner quits it. `lock` proves this process is the instance.
pub async fn run(channel: Channel, dirs: Dirs, lock: Lock) -> Result<()> {
    let listener = instance::listen(&dirs, &lock)?;
    let store = Store::open(&dirs, POLICY)?;
    let secret = store.secret_key()?;
    let network = Network::Internet { relay: store.config.relay.clone() };
    let node =
        Arc::new(Node::bind(secret, store.config.id, &store.config.name, POLICY, network).await?);
    let state = Arc::new(State {
        channel,
        dirs,
        store: Mutex::new(store),
        node,
        indexes: StdMutex::new(HashMap::new()),
        quit: Notify::new(),
    });
    state.open_spaces().await?;
    tokio::spawn(state.node.clone().serve(Arc::new(NoPairing)));
    tokio::spawn(state.clone().sync_forever());
    tokio::spawn(instance::serve(listener, state.clone().handler()));
    match tray::show(channel).await {
        Ok(mut events) => {
            let state = state.clone();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    match event {
                        tray::Event::Activate => state.activate(None),
                        tray::Event::Quit => state.quit.notify_one(),
                    }
                }
            });
        }
        Err(error) => eprintln!("{}: no tray icon: {error}", channel.instance()),
    }
    tokio::select! {
        () = state.quit.notified() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    state.finish().await;
    drop(lock);
    Ok(())
}

impl State {
    /// Loads every space the machine holds, serves it to members, and indexes it.
    async fn open_spaces(&self) -> Result<()> {
        let store = self.store.lock().await;
        for id in store.config.spaces.keys().copied().collect::<Vec<_>>() {
            let space = store.load_space(id)?;
            self.index(id, &space)?;
            self.node.spaces().lock().await.insert(id, Arc::new(Mutex::new(space)));
        }
        Ok(())
    }

    /// Rebuilds a space's search index in the cache directory.
    fn index(&self, id: Id, space: &Space) -> Result<()> {
        let dir = self.dirs.cache.join("spaces").join(id.to_string());
        zib_core::store::create_private_dir(&dir)?;
        let mut index = Index::open(&dir.join("index.sqlite"))?;
        index.rebuild(space, &|_| None)?;
        self.indexes.lock().expect("unpoisoned").insert(id, index);
        Ok(())
    }

    /// Syncs every space with every member every 30 seconds, with longer waits for members that
    /// keep failing.
    async fn sync_forever(self: Arc<Self>) {
        let mut failures: HashMap<(Id, Id), u32> = HashMap::new();
        loop {
            self.sync_once(&mut failures).await;
            tokio::time::sleep(zib_core::sync::PERIOD).await;
        }
    }

    async fn sync_once(&self, failures: &mut HashMap<(Id, Id), u32>) {
        let spaces: Vec<(Id, Arc<Mutex<Space>>)> =
            self.node.spaces().lock().await.iter().map(|(id, s)| (*id, s.clone())).collect();
        for (id, space) in spaces {
            let members = {
                let space = space.lock().await;
                space.members().into_iter().filter(|m| m.id != space.machine()).collect::<Vec<_>>()
            };
            let mut changed = false;
            for member in members {
                let count = failures.get(&(id, member.id)).copied().unwrap_or(0);
                if count > 0 && !due(count) {
                    failures.insert((id, member.id), count + 1);
                    continue;
                }
                let address = zib_core::address::Address { key: member.key, hints: Vec::new() };
                match self.node.sync(&space, &address).await {
                    Ok(exchanged) => {
                        failures.remove(&(id, member.id));
                        changed = true;
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
            if changed {
                let space = space.lock().await;
                let _ = self.store.lock().await.save_space(&space);
                let _ = self.index(id, &space);
            }
        }
    }

    /// Opens the window, which the app doesn't have yet, so for now it says so.
    fn activate(&self, uri: Option<&str>) {
        let target = uri.map(|uri| format!(" for {uri}")).unwrap_or_default();
        eprintln!("{}: activate{target}: this build has no window yet", self.channel.instance());
    }

    /// Syncs what it can within 10 seconds, saves every space, and closes the endpoint.
    async fn finish(&self) {
        let mut failures = HashMap::new();
        let _ = tokio::time::timeout(QUIT_SYNC, self.sync_once(&mut failures)).await;
        let spaces: Vec<Arc<Mutex<Space>>> =
            self.node.spaces().lock().await.values().cloned().collect();
        let store = self.store.lock().await;
        for space in spaces {
            let _ = store.save_space(&*space.lock().await);
        }
        self.node.close().await;
    }

    fn handler(self: Arc<Self>) -> Handler {
        Arc::new(move |request| {
            let state = self.clone();
            Box::pin(async move { state.answer(request).await })
        })
    }

    async fn answer(&self, request: Request) -> Reply {
        let id = request.id();
        match self.try_answer(request).await {
            Ok(reply) => reply,
            Err(error) => Reply::Error { id, reason: error.to_string(), code: "failed".into() },
        }
    }

    async fn try_answer(&self, request: Request) -> Result<Reply> {
        Ok(match request {
            Request::Hello { .. } => Reply::Hello { features: Vec::new() },
            Request::Activate { id, uri } => {
                self.activate(uri.as_deref());
                Reply::done(id)
            }
            Request::Stop { id } => {
                self.quit.notify_one();
                Reply::done(id)
            }
            Request::Spaces { id } => {
                let store = self.store.lock().await;
                let spaces = store
                    .config
                    .spaces
                    .iter()
                    .map(|(space, entry)| SpaceInfo { id: *space, name: entry.name.clone() })
                    .collect();
                Reply::Result { id, spaces, hits: Vec::new() }
            }
            Request::CreateSpace { id, name } => {
                let mut store = self.store.lock().await;
                let mut space = store.create_space(&name)?;
                let me = self.node.me();
                space.add_machine(me.machine, me.key, &me.name, POLICY)?;
                store.save_space(&space)?;
                let space_id = space.id()?;
                self.index(space_id, &space)?;
                self.node.spaces().lock().await.insert(space_id, Arc::new(Mutex::new(space)));
                Reply::Result {
                    id,
                    spaces: vec![SpaceInfo { id: space_id, name }],
                    hits: Vec::new(),
                }
            }
            Request::Search { id, query, limit } => {
                let limit = limit.unwrap_or(20) as usize;
                let indexes = self.indexes.lock().expect("unpoisoned");
                let mut hits = Vec::new();
                for (space, index) in indexes.iter() {
                    for hit in index.search(&query, limit)? {
                        hits.push(HitInfo {
                            space: *space,
                            note: hit.id,
                            title: hit.title,
                            snippet: hit.snippet,
                        });
                    }
                }
                hits.truncate(limit);
                Reply::Result { id, spaces: Vec::new(), hits }
            }
        })
    }
}

/// Whether a member that failed `count` times in a row is due another try: the wait doubles from
/// one period up to 10 minutes.
fn due(count: u32) -> bool {
    let period = zib_core::sync::PERIOD.as_secs();
    let wait = (zib_core::sync::FIRST_RETRY.as_secs() << count.min(5))
        .min(zib_core::sync::LAST_RETRY.as_secs());
    u64::from(count) * period >= wait
}
