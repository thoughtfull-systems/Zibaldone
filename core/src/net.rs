//! Connections between machines over iroh, as the sync and pairing specifications define them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::address::{Address, MachineKey};
use crate::pairing::{self, Invite, Invites, Paired, Party};
use crate::space::{FileHash, Policy};
use crate::sync::{self, Exchanged, Message};
use crate::{Error, Id, Result, Space};

/// How long a machine waits for another to answer a connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one exchange may take.
pub const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(300);

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
        Ok(Self { endpoint, me, spaces: Arc::default(), invites: Arc::default() })
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
        let result =
            timeout(EXCHANGE_TIMEOUT, sync::dial(tokio::io::join(recv, send), space, address.key))
                .await
                .map_err(|_| Error::Protocol("the exchange took too long".into()))?;
        connection.close(0u32.into(), b"done");
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
        let paired =
            pairing::initiate(tokio::io::join(recv, send), &self.me, address.key, confirm).await;
        connection.close(0u32.into(), b"done");
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
