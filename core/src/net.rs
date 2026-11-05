//! Connections between machines over iroh, as the sync and pairing specifications define them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::time::timeout;

use crate::address::{Address, MachineKey};
use crate::files::Blobs;
use crate::pairing::{self, Invite, Invites, Paired, Party};
use crate::space::{FileHash, Policy};
use crate::sync::{self, Exchanged, LiveEvent, Message, Selection};
use crate::{Error, Id, Result, Space};

/// How long a machine waits for another to answer a connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one exchange may take.
pub const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(300);

/// Finishes a stream and closes its connection once the other machine acknowledges everything
/// sent on it, since closing a connection discards what's still on the way, such as a last message
/// that crossed the other machine's own last message.
async fn close_after_sent(connection: &Connection, mut send: SendStream) {
    if send.finish().is_ok() {
        let _ = timeout(CONNECT_TIMEOUT, send.stopped()).await;
    }
    connection.close(0u32.into(), b"done");
}

/// Where an endpoint looks for other machines.
#[derive(Clone, Debug)]
pub enum Network {
    /// The internet and local networks, through n0's relays or the owner's own.
    Internet { relay: Option<String> },
    /// Only this machine's loopback interface, for tests.
    Loopback,
}

/// The spaces a machine holds, by identifier, which other members sync with.
pub type Spaces = Arc<Mutex<HashMap<Id, Arc<Mutex<Space>>>>>;

/// What the owner decides while pairing, which the app asks for on the Add machine screen.
pub trait Pairing: Send + Sync + 'static {
    /// Whether the Add machine screen is open, so the machine answers requests.
    fn accepting(&self) -> bool;
    /// Shows the code and the other machine's name, and waits for the owner: the spaces the owner
    /// chose to share once the codes match, or `None` when the owner declines.
    fn confirm(&self, peer: &Party, code: &str) -> impl Future<Output = Option<Vec<Id>>> + Send;
    /// Records that pairing finished, so the app can add the machine to the shared spaces.
    fn paired(&self, paired: Paired) -> impl Future<Output = ()> + Send;
}

/// A running machine's endpoint and the spaces it serves.
pub struct Node {
    endpoint: Endpoint,
    me: Party,
    spaces: Spaces,
    invites: Arc<std::sync::Mutex<Invites>>,
    /// The directory that holds each space's directory, with its blobs.
    spaces_dir: std::sync::Mutex<Option<PathBuf>>,
    /// The notes open for editing on this machine, by space and note.
    live: std::sync::Mutex<HashMap<(Id, Id), Live>>,
}

/// What a live session needs from an open note: its local changes, its cursors, and where its
/// events go.
type LiveParts = (
    broadcast::Receiver<Vec<u8>>,
    broadcast::Receiver<(Id, Vec<Selection>)>,
    mpsc::Sender<LiveEvent>,
);

/// A note open for editing, which live sessions with other machines share.
struct Live {
    local: broadcast::Sender<Vec<u8>>,
    cursors: broadcast::Sender<(Id, Vec<Selection>)>,
    events: mpsc::Sender<LiveEvent>,
    _subscription: loro::Subscription,
}

/// A note this machine has open for editing: its cursor goes out through `cursors`, and the other
/// machines' changes and cursors come in through `events`.
pub struct OpenNote {
    pub cursors: broadcast::Sender<(Id, Vec<Selection>)>,
    pub events: mpsc::Receiver<LiveEvent>,
}

/// A machine that announces itself nearby while its owner adds a machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NearbyMachine {
    pub address: Address,
    pub name: String,
}

/// What the nearby listing reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NearbyEvent {
    Found(NearbyMachine),
    Lost(crate::address::MachineKey),
}

/// Announces this machine nearby, until it's dropped.
pub struct Nearby {
    _lookup: MdnsAddressLookup,
    pub events: mpsc::Receiver<NearbyEvent>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Nearby {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Node {
    /// Binds the machine's endpoint with its secret key.
    pub async fn bind(
        secret: [u8; 32],
        me_id: Id,
        name: &str,
        policy: Policy,
        network: Network,
    ) -> Result<Self> {
        let key = SecretKey::from_bytes(&secret);
        let builder = match network {
            Network::Internet { relay } => {
                let builder =
                    Endpoint::builder(presets::N0).address_lookup(MdnsAddressLookup::builder());
                match relay {
                    Some(relay) => {
                        let url: RelayUrl =
                            relay.parse().map_err(|_| Error::invalid("relay", relay))?;
                        builder.relay_mode(RelayMode::Custom(url.into()))
                    }
                    None => builder,
                }
            }
            Network::Loopback => Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(|e| Error::Protocol(e.to_string()))?,
        };
        let endpoint = builder
            .alpns(vec![sync::ALPN.to_vec(), pairing::ALPN.to_vec()])
            .secret_key(key)
            .bind()
            .await
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let me = Party {
            machine: me_id,
            name: name.to_owned(),
            policy,
            key: MachineKey(*endpoint.id().as_bytes()),
        };
        Ok(Self {
            endpoint,
            me,
            spaces: Arc::default(),
            invites: Arc::default(),
            spaces_dir: std::sync::Mutex::new(None),
            live: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Sets the directory that holds each space's directory, as the machine store lays it out.
    pub fn set_spaces_dir(&self, dir: PathBuf) {
        *self.spaces_dir.lock().expect("unpoisoned") = Some(dir);
    }

    /// The blobs of a space on this machine.
    pub fn blobs(&self, space: Id) -> Option<Blobs> {
        let dir = self.spaces_dir.lock().expect("unpoisoned").clone()?;
        Some(Blobs::new(dir.join(space.to_string()).join("blobs")))
    }

    /// This machine, as pairing describes it.
    pub fn me(&self) -> &Party {
        &self.me
    }

    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    /// This machine's address, with the network addresses it's bound to as hints.
    pub fn address(&self) -> Address {
        Address { key: self.me.key, hints: self.endpoint.bound_sockets() }
    }

    /// Accepts connections from members and pairing machines until the endpoint closes.
    pub async fn serve<P: Pairing>(self: Arc<Self>, pairing: Arc<P>) {
        while let Some(incoming) = self.endpoint.accept().await {
            let node = self.clone();
            let pairing = pairing.clone();
            tokio::spawn(async move {
                let Ok(connection) = incoming.await else { return };
                let peer = MachineKey(*connection.remote_id().as_bytes());
                if connection.alpn() == pairing::ALPN {
                    let _ = node.answer_pairing(&connection, peer, pairing.as_ref()).await;
                    // The other machine closes the connection once it reads the last reply.
                    let _ = timeout(CONNECT_TIMEOUT, connection.closed()).await;
                } else {
                    while let Ok((send, recv)) = connection.accept_bi().await {
                        let node = node.clone();
                        tokio::spawn(async move {
                            let _ =
                                timeout(EXCHANGE_TIMEOUT, node.answer_sync(send, recv, peer)).await;
                        });
                    }
                }
            });
        }
    }

    async fn answer_sync(
        &self,
        mut send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
        peer: MachineKey,
    ) -> Result<()> {
        match sync::receive(&mut recv).await? {
            first @ Message::Hello { .. } => {
                let spaces = self.spaces.lock().await.clone();
                sync::respond(&mut recv, &mut send, first, peer, |id| spaces.get(&id).cloned())
                    .await?;
            }
            Message::Hold { space, machine, hash } => {
                let found = self.spaces.lock().await.get(&space).cloned();
                let Some(found) = found else {
                    return Err(Error::NotFound(format!("space {space}")));
                };
                let space = found.lock().await;
                let member = sync::is_member(&space, machine, &peer);
                let holds = member
                    && <[u8; 32]>::try_from(hash.as_slice()).is_ok_and(|h| {
                        space
                            .file(FileHash(h))
                            .is_some_and(|f| f.holders.contains_key(&space.machine()))
                    });
                sync::answer_hold(&mut send, &hash, self.me.policy, holds).await?;
            }
            Message::Fetch { space, machine, hash } => {
                let found = self.spaces.lock().await.get(&space).cloned();
                let member = match &found {
                    Some(found) => sync::is_member(&*found.lock().await, machine, &peer),
                    None => false,
                };
                let path = <[u8; 32]>::try_from(hash.as_slice())
                    .ok()
                    .and_then(|h| self.blobs(space).map(|b| b.path(FileHash(h))))
                    .filter(|p| p.is_file());
                match path {
                    Some(path) if member => sync::send_blob(&mut send, &path).await?,
                    _ => {
                        sync::send(&mut send, &Message::Error { reason: "no such blob".into() })
                            .await?;
                    }
                }
            }
            Message::Watch { space, machine, note, version } => {
                let found = self.spaces.lock().await.get(&space).cloned();
                let Some(found) = found else {
                    return Err(Error::NotFound(format!("space {space}")));
                };
                let member = sync::is_member(&*found.lock().await, machine, &peer);
                let open = self.live_receivers(space, note);
                let Some((local, cursors, events)) = open.filter(|_| member) else {
                    sync::send(&mut send, &Message::Error { reason: "not open here".into() })
                        .await?;
                    return Ok(());
                };
                let ours = found.lock().await.version();
                sync::send(&mut send, &Message::Watching { note, version: ours }).await?;
                let stream = tokio::io::join(recv, send);
                return sync::live(stream, &found, machine, &version, local, cursors, events).await;
            }
            other => {
                let reason = format!("unexpected {other:?}");
                sync::send(&mut send, &Message::Error { reason: reason.clone() }).await?;
                return Err(Error::Protocol(reason));
            }
        }
        let _ = send.finish();
        Ok(())
    }

    async fn answer_pairing<P: Pairing>(
        &self,
        connection: &Connection,
        peer: MachineKey,
        pairing: &P,
    ) -> Result<()> {
        let (send, recv) =
            connection.accept_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
        let mut stream = tokio::io::join(recv, send);
        // An invite needs no screen open; a request does.
        let first =
            crate::frame::read::<_, pairing::Message>(&mut stream, crate::frame::MAX_PAIR_FRAME)
                .await?;
        match first {
            Some(message @ pairing::Message::Invite { .. }) => {
                let replay = Replay::new(&message, stream);
                let spaces = self.spaces.clone();
                pairing::admit(replay, peer, &self.invites, async |space: Id, party: &Party| {
                    let found = spaces.lock().await.get(&space).cloned();
                    let found = found.ok_or_else(|| Error::NotFound(format!("space {space}")))?;
                    found.lock().await.add_machine(
                        party.machine,
                        party.key,
                        &party.name,
                        party.policy,
                    )
                })
                .await?;
            }
            Some(message @ pairing::Message::Request { .. }) if pairing.accepting() => {
                let replay = Replay::new(&message, stream);
                let me = self.me.clone();
                let paired =
                    pairing::respond(replay, &me, peer, async |party: &Party, code: &str| {
                        pairing.confirm(party, code).await
                    })
                    .await?;
                pairing.paired(paired).await;
            }
            _ => return Err(Error::Protocol("pairing isn't open".into())),
        }
        Ok(())
    }

    async fn connect(&self, address: &Address, alpn: &[u8]) -> Result<Connection> {
        let id =
            PublicKey::from_bytes(&address.key.0).map_err(|e| Error::Protocol(e.to_string()))?;
        let mut addr = EndpointAddr::new(id);
        for hint in &address.hints {
            addr = addr.with_ip_addr(*hint);
        }
        timeout(CONNECT_TIMEOUT, self.endpoint.connect(addr, alpn))
            .await
            .map_err(|_| Error::Protocol("the other machine didn't answer".into()))?
            .map_err(|e| Error::Protocol(e.to_string()))
    }

    /// Syncs `space` with the member at `address`.
    pub async fn sync(&self, space: &Arc<Mutex<Space>>, address: &Address) -> Result<Exchanged> {
        let connection = self.connect(address, sync::ALPN).await?;
        let (send, recv) =
            connection.open_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
        let mut stream = tokio::io::join(recv, send);
        let result = timeout(EXCHANGE_TIMEOUT, sync::dial(&mut stream, space, address.key))
            .await
            .map_err(|_| Error::Protocol("the exchange took too long".into()))?;
        close_after_sent(&connection, stream.into_inner().1).await;
        result
    }

    /// Syncs `space` with every member that answers, and returns what each exchange learned.
    pub async fn sync_all(&self, space: &Arc<Mutex<Space>>) -> Vec<Result<Exchanged>> {
        let members = {
            let space = space.lock().await;
            space.members().into_iter().filter(|m| m.id != space.machine()).collect::<Vec<_>>()
        };
        let mut results = Vec::new();
        for member in members {
            results.push(self.sync(space, &Address { key: member.key, hints: Vec::new() }).await);
        }
        results
    }

    /// Receives a space that the machine at `address` just shared with this one, through pairing or
    /// an invite, and starts serving it.
    pub async fn fetch(&self, space: Id, address: &Address) -> Result<Arc<Mutex<Space>>> {
        let copy = Arc::new(Mutex::new(Space::joining(space, self.me.machine)));
        self.sync(&copy, address).await?;
        if copy.lock().await.is_blank() {
            return Err(Error::Protocol("the space didn't arrive".into()));
        }
        self.spaces.lock().await.insert(space, copy.clone());
        Ok(copy)
    }

    fn live_receivers(&self, space: Id, note: Id) -> Option<LiveParts> {
        let live = self.live.lock().expect("unpoisoned");
        let open = live.get(&(space, note))?;
        Some((open.local.subscribe(), open.cursors.subscribe(), open.events.clone()))
    }

    /// Opens a note for editing: syncs its space with every member at once, and starts live
    /// sessions with each member that has the same note open.
    pub async fn open_note(self: &Arc<Self>, space_id: Id, note: Id) -> Result<OpenNote> {
        let space = self.spaces.lock().await.get(&space_id).cloned();
        let space = space.ok_or_else(|| Error::NotFound(format!("space {space_id}")))?;
        let (local, _) = broadcast::channel(256);
        let (cursors, _) = broadcast::channel(64);
        let (events, receiver) = mpsc::channel(256);
        let sender = local.clone();
        let subscription =
            space.lock().await.doc().subscribe_local_update(Box::new(move |bytes| {
                let _ = sender.send(bytes.clone());
                true
            }));
        self.live.lock().expect("unpoisoned").insert(
            (space_id, note),
            Live { local, cursors: cursors.clone(), events, _subscription: subscription },
        );
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this.sync_all(&space).await;
            let members = {
                let space = space.lock().await;
                space.members().into_iter().filter(|m| m.id != space.machine()).collect::<Vec<_>>()
            };
            for member in members {
                let this = this.clone();
                let space = space.clone();
                tokio::spawn(async move {
                    let _ = this
                        .watch(
                            &space,
                            space_id,
                            note,
                            member.id,
                            &Address { key: member.key, hints: Vec::new() },
                        )
                        .await;
                });
            }
        });
        Ok(OpenNote { cursors, events: receiver })
    }

    async fn watch(
        &self,
        space: &Arc<Mutex<Space>>,
        space_id: Id,
        note: Id,
        peer: Id,
        address: &Address,
    ) -> Result<()> {
        let connection = self.connect(address, sync::ALPN).await?;
        let (mut send, mut recv) =
            connection.open_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
        let version = space.lock().await.version();
        sync::send(
            &mut send,
            &Message::Watch { space: space_id, machine: self.me.machine, note, version },
        )
        .await?;
        let Message::Watching { version: theirs, .. } = sync::receive(&mut recv).await? else {
            return Err(Error::Protocol("expected watching".into()));
        };
        let Some((local, cursors, events)) = self.live_receivers(space_id, note) else {
            return Ok(());
        };
        let result =
            sync::live(tokio::io::join(recv, send), space, peer, &theirs, local, cursors, events)
                .await;
        connection.close(0u32.into(), b"done");
        result
    }

    /// Closes a note, which ends its live sessions.
    pub fn close_note(&self, space: Id, note: Id) {
        self.live.lock().expect("unpoisoned").remove(&(space, note));
    }

    /// Fetches a blob of a space from any holder that answers.
    pub async fn fetch_blob(&self, space_id: Id, hash: FileHash) -> Result<()> {
        let blobs =
            self.blobs(space_id).ok_or_else(|| Error::NotFound("the spaces directory".into()))?;
        if blobs.has(hash) {
            return Ok(());
        }
        let space = self.spaces.lock().await.get(&space_id).cloned();
        let space = space.ok_or_else(|| Error::NotFound(format!("space {space_id}")))?;
        let holders = {
            let space = space.lock().await;
            let holders = space.file(hash).map(|f| f.holders).unwrap_or_default();
            space
                .members()
                .into_iter()
                .filter(|m| holders.contains_key(&m.id) && m.id != space.machine())
                .collect::<Vec<_>>()
        };
        let mut last = Error::NotFound(format!("a holder of {hash}"));
        for holder in holders {
            let address = Address { key: holder.key, hints: Vec::new() };
            let fetched = async {
                let connection = self.connect(&address, sync::ALPN).await?;
                let (send, recv) =
                    connection.open_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
                let result = sync::fetch_blob(
                    tokio::io::join(recv, send),
                    space_id,
                    self.me.machine,
                    hash,
                    &blobs,
                )
                .await;
                connection.close(0u32.into(), b"done");
                result
            };
            match fetched.await {
                Ok(()) => {
                    space.lock().await.set_holding(hash, true)?;
                    return Ok(());
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// Fetches every blob in use that this machine lacks, as the `all` policy asks, and returns
    /// how many arrived.
    pub async fn fetch_missing(&self, space_id: Id) -> usize {
        let Some(blobs) = self.blobs(space_id) else { return 0 };
        let space = self.spaces.lock().await.get(&space_id).cloned();
        let Some(space) = space else { return 0 };
        let missing: Vec<FileHash> =
            space.lock().await.files_in_use().into_iter().filter(|h| !blobs.has(*h)).collect();
        let mut fetched = 0;
        for hash in missing {
            if self.fetch_blob(space_id, hash).await.is_ok() {
                fetched += 1;
            }
        }
        fetched
    }

    /// Announces this machine nearby under the pairing service, with its name, and lists the
    /// other machines that do, until the result is dropped.
    pub async fn announce_nearby(&self, name: &str) -> Result<Nearby> {
        use futures_util::StreamExt;
        use iroh::address_lookup::AddressLookup;
        use iroh::endpoint_info::{EndpointData, UserData};
        let lookup = MdnsAddressLookup::builder()
            .service_name(pairing::SERVICE)
            .build(self.endpoint.id())
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let own = self.endpoint.addr();
        let mut data =
            EndpointData::new(own.ip_addrs().copied().map(iroh::TransportAddr::Ip).collect());
        if let Ok(user) = name.parse::<UserData>() {
            data = data.with_user_data(user);
        }
        lookup.publish(&data);
        let mut stream = lookup.subscribe().await;
        let (sender, events) = mpsc::channel(32);
        let own_key = self.endpoint.id();
        let task = tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                let message = match event {
                    iroh_mdns_address_lookup::DiscoveryEvent::Discovered {
                        endpoint_info, ..
                    } => {
                        if endpoint_info.endpoint_id == own_key {
                            continue;
                        }
                        let key = crate::address::MachineKey(*endpoint_info.endpoint_id.as_bytes());
                        let name = endpoint_info
                            .data
                            .user_data()
                            .map(ToString::to_string)
                            .unwrap_or_default();
                        let hints = endpoint_info.data.ip_addrs().copied().collect();
                        NearbyEvent::Found(NearbyMachine { address: Address { key, hints }, name })
                    }
                    iroh_mdns_address_lookup::DiscoveryEvent::Expired { endpoint_id } => {
                        NearbyEvent::Lost(crate::address::MachineKey(*endpoint_id.as_bytes()))
                    }
                    _ => continue,
                };
                if sender.send(message).await.is_err() {
                    break;
                }
            }
        });
        Ok(Nearby { _lookup: lookup, events, task })
    }

    /// Asks the holder at `address` whether it holds a blob, before this machine drops it.
    pub async fn confirm_hold(&self, space: Id, hash: FileHash, address: &Address) -> Result<bool> {
        let connection = self.connect(address, sync::ALPN).await?;
        let (send, recv) =
            connection.open_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
        let holds = sync::ask_hold(tokio::io::join(recv, send), space, self.me.machine, hash).await;
        connection.close(0u32.into(), b"done");
        holds
    }

    /// Pairs with the machine at `address`, as the machine that picked it.
    pub async fn pair<C>(&self, address: &Address, confirm: C) -> Result<Paired>
    where
        C: AsyncFnOnce(&Party, &str) -> Option<Vec<Id>>,
    {
        let connection = self.connect(address, pairing::ALPN).await?;
        let (send, recv) =
            connection.open_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
        let mut stream = tokio::io::join(recv, send);
        let paired = pairing::initiate(&mut stream, &self.me, address.key, confirm).await;
        close_after_sent(&connection, stream.into_inner().1).await;
        paired
    }

    /// Makes an invite to `space`, which works once within 15 minutes.
    pub fn invite(&self, space: Id) -> Result<Invite> {
        let invite = Invite::new(self.me.key, space, self.endpoint.bound_sockets())?;
        self.invites.lock().expect("unpoisoned").add(invite.clone());
        Ok(invite)
    }

    /// Joins a space by invite.
    pub async fn join(&self, invite: &Invite) -> Result<Id> {
        let address = Address { key: invite.key, hints: invite.hints.clone() };
        let connection = self.connect(&address, pairing::ALPN).await?;
        let (send, recv) =
            connection.open_bi().await.map_err(|e| Error::Protocol(e.to_string()))?;
        let joined = pairing::join(tokio::io::join(recv, send), &self.me, invite).await;
        connection.close(0u32.into(), b"done");
        joined
    }

    /// The network addresses the endpoint is bound to.
    pub fn sockets(&self) -> Vec<SocketAddr> {
        self.endpoint.bound_sockets()
    }

    /// Closes the endpoint.
    pub async fn close(&self) {
        let _ = timeout(Duration::from_secs(2), self.endpoint.close()).await;
    }
}

/// A stream that yields one already-read message before the rest of the stream, so a handler can
/// read the message it was dispatched on.
struct Replay<S> {
    prefix: std::io::Cursor<Vec<u8>>,
    stream: S,
}

impl<S> Replay<S> {
    fn new(message: &pairing::Message, stream: S) -> Self {
        let mut bytes = Vec::new();
        ciborium::into_writer(message, &mut bytes).expect("serializable");
        let mut framed = u32::try_from(bytes.len()).expect("small").to_be_bytes().to_vec();
        framed.extend(bytes);
        Self { prefix: std::io::Cursor::new(framed), stream }
    }
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Replay<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let position = usize::try_from(self.prefix.position()).unwrap_or(usize::MAX);
        let remaining = self.prefix.get_ref().len().saturating_sub(position);
        if remaining > 0 {
            let n = remaining.min(buf.remaining());
            let start = position;
            buf.put_slice(&self.prefix.get_ref()[start..start + n]);
            self.prefix.set_position((start + n) as u64);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Replay<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    struct Accepting;

    impl Pairing for Accepting {
        fn accepting(&self) -> bool {
            true
        }
        fn confirm(&self, _: &Party, _: &str) -> impl Future<Output = Option<Vec<Id>>> + Send {
            std::future::ready(Some(Vec::new()))
        }
        fn paired(&self, _: Paired) -> impl Future<Output = ()> + Send {
            std::future::ready(())
        }
    }

    async fn node(policy: Policy) -> Arc<Node> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).unwrap();
        let node = Arc::new(
            Node::bind(secret, Id::generate(), "test", policy, Network::Loopback).await.unwrap(),
        );
        tokio::spawn(node.clone().serve(Arc::new(Accepting)));
        node
    }

    #[tokio::test]
    async fn syncs_over_iroh_and_joins_by_invite() {
        let (a, b) = (node(Policy::All).await, node(Policy::OnDemand).await);
        let mut space = Space::create(a.me().machine).unwrap();
        space.add_machine(a.me().machine, a.me().key, "A", Policy::All).unwrap();
        let space_id = space.id().unwrap();
        let note = space.create_note(schema::NOTE, "over the wire").unwrap();
        let space = Arc::new(Mutex::new(space));
        a.spaces().lock().await.insert(space_id, space.clone());

        // B joins by invite, then syncs to receive the space.
        let invite = a.invite(space_id).unwrap();
        assert_eq!(b.join(&invite).await.unwrap(), space_id);
        let a_address = a.address();
        let copy = b.fetch(space_id, &a_address).await.unwrap();
        // From then on, B syncs as an ordinary member.
        b.sync(&copy, &a_address).await.unwrap();
        assert!(copy.lock().await.members().iter().any(|m| m.id == b.me().machine));
        assert_eq!(copy.lock().await.note(note).unwrap().body, "over the wire");

        // A holds a blob, and confirms it to B.
        let hash = FileHash::of(b"blob");
        space.lock().await.add_file(hash, 4, "text/plain").unwrap();
        assert!(b.confirm_hold(space_id, hash, &a_address).await.unwrap());
        assert!(!b.confirm_hold(space_id, FileHash::of(b"other"), &a_address).await.unwrap());
        a.close().await;
        b.close().await;
    }

    #[tokio::test]
    async fn pairs_by_address() {
        let (a, b) = (node(Policy::All).await, node(Policy::OnDemand).await);
        let paired =
            a.pair(&b.address(), async |_: &Party, _: &str| Some(Vec::new())).await.unwrap();
        assert_eq!(paired.peer.machine, b.me().machine);
        a.close().await;
        b.close().await;
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::schema;

    struct Refusing;

    impl Pairing for Refusing {
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

    async fn node(dir: &std::path::Path) -> Arc<Node> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).unwrap();
        let node = Arc::new(
            Node::bind(secret, Id::generate(), "test", Policy::All, Network::Loopback)
                .await
                .unwrap(),
        );
        node.set_spaces_dir(dir.to_path_buf());
        tokio::spawn(node.clone().serve(Arc::new(Refusing)));
        node
    }

    /// Two nodes that share a space, each knowing the other's loopback address as a hint.
    async fn shared(
        dir: &std::path::Path,
    ) -> (Arc<Node>, Arc<Node>, Id, Arc<Mutex<Space>>, Arc<Mutex<Space>>) {
        let (a, b) = (node(&dir.join("a")).await, node(&dir.join("b")).await);
        let mut space = Space::create(a.me().machine).unwrap();
        space.add_machine(a.me().machine, a.me().key, "A", Policy::All).unwrap();
        space.add_machine(b.me().machine, b.me().key, "B", Policy::All).unwrap();
        let id = space.id().unwrap();
        let copy = Space::open(&space.snapshot().unwrap(), b.me().machine).unwrap();
        let (space, copy) = (Arc::new(Mutex::new(space)), Arc::new(Mutex::new(copy)));
        a.spaces().lock().await.insert(id, space.clone());
        b.spaces().lock().await.insert(id, copy.clone());
        (a, b, id, space, copy)
    }

    #[tokio::test]
    async fn fetches_blobs_from_holders() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, id, space, copy) = shared(dir.path()).await;
        let blobs = a.blobs(id).unwrap();
        let hash = blobs.add_bytes(b"a file's bytes").unwrap();
        let note =
            crate::files::add_file_note(&mut *space.lock().await, hash, 14, "f.txt", "text/plain")
                .unwrap();
        b.sync(&copy, &a.address()).await.unwrap();
        assert!(copy.lock().await.note(note).is_ok());
        // B finds A through the hint its address carries; a real network looks the key up.
        let holder_hint = a.address();
        let connection = b.connect(&holder_hint, sync::ALPN).await.unwrap();
        let (send, recv) = connection.open_bi().await.unwrap();
        sync::fetch_blob(
            tokio::io::join(recv, send),
            id,
            b.me().machine,
            hash,
            &b.blobs(id).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(b.blobs(id).unwrap().read(hash).unwrap(), b"a file's bytes");
        a.close().await;
        b.close().await;
    }

    #[tokio::test]
    async fn edits_live_between_open_notes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, id, space, copy) = shared(dir.path()).await;
        let note = space.lock().await.create_note(schema::NOTE, "shared").unwrap();
        b.sync(&copy, &a.address()).await.unwrap();
        let mut b_open = b.open_note(id, note).await.unwrap();
        let _a_open = a.open_note(id, note).await.unwrap();
        // A opens second, so it watches B directly, through B's loopback hint.
        let watch_space = space.clone();
        let (a2, b_address, b_machine) = (a.clone(), b.address(), b.me().machine);
        tokio::spawn(async move { a2.watch(&watch_space, id, note, b_machine, &b_address).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        {
            let mut space = space.lock().await;
            space.insert_text(note, 6, " live").unwrap();
            space.commit();
        }
        let changed = timeout(Duration::from_secs(5), async {
            loop {
                match b_open.events.recv().await {
                    Some(LiveEvent::Changed { .. })
                        if copy.lock().await.note(note).unwrap().body == "shared live" =>
                    {
                        return true;
                    }
                    Some(_) => {}
                    None => return false,
                }
            }
        })
        .await;
        assert_eq!(changed, Ok(true));
        a.close().await;
        b.close().await;
    }
}
