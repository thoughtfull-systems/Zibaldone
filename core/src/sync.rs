//! The `zib/sync` protocol, as the sync specification defines it: exchanges of a space's changes,
//! confirmation before a drop, live editing, and sync status.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use loro::VersionVector;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, broadcast, mpsc};

use crate::address::MachineKey;
use crate::frame::{self, MAX_FRAME};
use crate::space::{FileHash, Policy};
use crate::{Error, Id, Result, Space};

/// The protocol's name, which both sides agree on when a connection opens.
pub const ALPN: &[u8] = b"zib/sync";

/// The features this release supports, which `hello` lists.
pub const FEATURES: &[&str] = &[];

/// How often a machine syncs each space with every member in the background.
pub const PERIOD: Duration = Duration::from_secs(30);
/// How long a machine first waits before it tries a member again after a failure.
pub const FIRST_RETRY: Duration = Duration::from_secs(30);
/// The longest it waits between tries.
pub const LAST_RETRY: Duration = Duration::from_secs(10 * 60);
/// How long live editing gathers changes or cursor moves before it sends them.
pub const LIVE_GATHER: Duration = Duration::from_millis(50);
/// How often live editing repeats a cursor that hasn't moved.
pub const CURSOR_REPEAT: Duration = Duration::from_secs(20);
/// How long a cursor stays without a message.
pub const CURSOR_TIMEOUT: Duration = Duration::from_secs(60);

/// A message on a `zib/sync` stream.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Message {
    Hello {
        space: Id,
        machine: Id,
        #[serde(with = "serde_bytes")]
        version: Vec<u8>,
        #[serde(default)]
        features: Vec<String>,
    },
    Updates {
        #[serde(with = "serde_bytes")]
        updates: Vec<u8>,
    },
    Done,
    Error {
        reason: String,
    },
    Hold {
        space: Id,
        machine: Id,
        #[serde(with = "serde_bytes")]
        hash: Vec<u8>,
    },
    Holding {
        #[serde(with = "serde_bytes")]
        hash: Vec<u8>,
        holds: bool,
    },
    Watch {
        space: Id,
        machine: Id,
        note: Id,
        #[serde(with = "serde_bytes")]
        version: Vec<u8>,
    },
    Watching {
        note: Id,
        #[serde(with = "serde_bytes")]
        version: Vec<u8>,
    },
    Unwatch,
    Cursor {
        note: Id,
        selections: Vec<Selection>,
    },
    Fetch {
        space: Id,
        machine: Id,
        #[serde(with = "serde_bytes")]
        hash: Vec<u8>,
    },
    Blob {
        size: u64,
    },
    Chunk {
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
}

/// The most a `chunk` message holds.
pub const CHUNK: usize = 1 << 18;

/// A cursor or selection in a note's body, as Loro encodes positions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Selection {
    #[serde(with = "serde_bytes")]
    pub anchor: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub head: Vec<u8>,
}

pub(crate) async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: &Message) -> Result<()> {
    frame::write(writer, message, MAX_FRAME).await
}

pub(crate) async fn receive<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Message> {
    match frame::read(reader, MAX_FRAME).await? {
        Some(Message::Error { reason }) => {
            Err(Error::Protocol(format!("the other machine refused: {reason}")))
        }
        Some(message) => Ok(message),
        None => Err(Error::Protocol("the stream ended early".into())),
    }
}

/// Checks that the space lists `machine` as a member, not marked removed, with `key`.
pub fn is_member(space: &Space, machine: Id, key: &MachineKey) -> bool {
    space.members().iter().any(|m| m.id == machine && &m.key == key)
}

/// What an exchange learned about the other machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exchanged {
    pub machine: Id,
    /// The version vector that the other machine holds now, which the caller records.
    pub version: Vec<u8>,
}

/// Runs one exchange of a space as the machine that opened the stream. `peer_key` is the key that
/// the connection authenticated.
pub async fn dial<S>(stream: S, space: &Mutex<Space>, peer_key: MachineKey) -> Result<Exchanged>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    send(&mut writer, &hello(&*space.lock().await)?).await?;
    let Message::Hello { space: space_id, machine, version, .. } = receive(&mut reader).await?
    else {
        return Err(Error::Protocol("expected hello".into()));
    };
    let checked = {
        let space = space.lock().await;
        // A blank copy, which pairing just made, trusts the machine it paired with.
        if space.is_blank() { Ok(()) } else { check_peer(&space, space_id, machine, &peer_key) }
    };
    if let Err(error) = checked {
        let _ = send(&mut writer, &Message::Error { reason: error.to_string() }).await;
        return Err(error);
    }
    finish(&mut reader, &mut writer, space, machine, &version).await
}

/// Answers an exchange on a stream another member opened, after its `hello` has arrived.
/// `find` gives the space with that identifier, if the machine holds it.
pub async fn listen<S, F>(stream: S, peer_key: MachineKey, find: F) -> Result<(Id, Exchanged)>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(Id) -> Option<Arc<Mutex<Space>>>,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let first = receive(&mut reader).await?;
    respond(&mut reader, &mut writer, first, peer_key, find).await
}

/// Answers an exchange whose first message, `hello`, the caller already read.
pub async fn respond<R, W, F>(
    reader: &mut R,
    writer: &mut W,
    first: Message,
    peer_key: MachineKey,
    find: F,
) -> Result<(Id, Exchanged)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnOnce(Id) -> Option<Arc<Mutex<Space>>>,
{
    let Message::Hello { space: space_id, machine, version, .. } = first else {
        return Err(Error::Protocol("expected hello".into()));
    };
    let Some(space) = find(space_id) else {
        let _ = send(writer, &Message::Error { reason: "no such space".into() }).await;
        return Err(Error::NotFound(format!("space {space_id}")));
    };
    let checked = check_peer(&*space.lock().await, space_id, machine, &peer_key);
    if let Err(error) = checked {
        let _ = send(writer, &Message::Error { reason: error.to_string() }).await;
        return Err(error);
    }
    send(writer, &hello(&*space.lock().await)?).await?;
    let exchanged = finish(reader, writer, &space, machine, &version).await?;
    Ok((space_id, exchanged))
}

fn hello(space: &Space) -> Result<Message> {
    space.commit();
    Ok(Message::Hello {
        space: space.id()?,
        machine: space.machine(),
        version: space.version(),
        features: FEATURES.iter().map(|&f| f.to_owned()).collect(),
    })
}

fn check_peer(space: &Space, space_id: Id, machine: Id, key: &MachineKey) -> Result<()> {
    if space.id()? != space_id {
        return Err(Error::Protocol(format!("expected space {}", space.id()?)));
    }
    if !is_member(space, machine, key) {
        return Err(Error::Protocol(format!("{machine} isn't a member of this space")));
    }
    Ok(())
}

/// Sends what the other side lacks and receives what this side lacks, at the same time, so
/// neither side waits on the other's buffers.
async fn finish<R, W>(
    reader: &mut R,
    writer: &mut W,
    space: &Mutex<Space>,
    machine: Id,
    their_version: &[u8],
) -> Result<Exchanged>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let outgoing = space.lock().await.updates_since(their_version)?;
    let sending = async {
        if let Some(updates) = outgoing {
            send(writer, &Message::Updates { updates }).await?;
        }
        send(writer, &Message::Done).await
    };
    let receiving = async {
        let mut incoming = Vec::new();
        loop {
            match receive(reader).await? {
                Message::Updates { updates } => incoming.push(updates),
                Message::Done => return Ok::<_, Error>(incoming),
                other => return Err(Error::Protocol(format!("unexpected {other:?}"))),
            }
        }
    };
    let (sent, received) = tokio::join!(sending, receiving);
    sent?;
    let mut space = space.lock().await;
    for updates in received? {
        space.import_checked(&updates)?;
    }
    Ok(Exchanged { machine, version: space.version() })
}

/// Asks a holder with the `all` policy whether it holds a blob, before this machine drops it.
pub async fn ask_hold<S>(stream: S, space: Id, machine: Id, hash: FileHash) -> Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    send(&mut writer, &Message::Hold { space, machine, hash: hash.0.to_vec() }).await?;
    match receive(&mut reader).await? {
        Message::Holding { hash: answered, holds } if answered == hash.0 => Ok(holds),
        other => Err(Error::Protocol(format!("unexpected {other:?}"))),
    }
}

/// Answers `hold`: `true` only for a machine with the `all` policy that holds the blob whole.
pub async fn answer_hold<W: AsyncWrite + Unpin>(
    writer: &mut W,
    hash: &[u8],
    policy: Policy,
    holds_whole: bool,
) -> Result<()> {
    let holds = policy == Policy::All && holds_whole;
    send(writer, &Message::Holding { hash: hash.to_vec(), holds }).await
}

/// What a live editing session reports to the app.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveEvent {
    /// The session with `machine` started.
    Started { machine: Id },
    /// The other machine's changes arrived and the space imported them.
    Changed { machine: Id },
    /// The other machine's cursors and selections in the note.
    Cursor { machine: Id, selections: Vec<Selection> },
    /// The session with `machine` ended.
    Ended { machine: Id },
}

/// Runs a live editing session on a stream where both sides have the note open: it catches the
/// other side up, then sends each local change as it comes and imports each remote one. It ends
/// when `local` closes, when the other side sends `unwatch`, or when the stream breaks.
pub async fn live<S>(
    stream: S,
    space: &Mutex<Space>,
    peer: Id,
    peer_version: &[u8],
    mut local: broadcast::Receiver<Vec<u8>>,
    mut cursors: broadcast::Receiver<(Id, Vec<Selection>)>,
    events: mpsc::Sender<LiveEvent>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let _ = events.send(LiveEvent::Started { machine: peer }).await;
    if let Some(updates) = space.lock().await.updates_since(peer_version)? {
        send(&mut writer, &Message::Updates { updates }).await?;
    }
    let result = loop {
        tokio::select! {
            update = local.recv() => match update {
                Ok(updates) => send(&mut writer, &Message::Updates { updates }).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Too many changes queued: send everything at once instead.
                    let all = space.lock().await.updates_since(&VersionVector::new().encode())?;
                    if let Some(updates) = all {
                        send(&mut writer, &Message::Updates { updates }).await?;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = send(&mut writer, &Message::Unwatch).await;
                    break Ok(());
                }
            },
            cursor = cursors.recv() => match cursor {
                Ok((note, selections)) => {
                    send(&mut writer, &Message::Cursor { note, selections }).await?;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = send(&mut writer, &Message::Unwatch).await;
                    break Ok(());
                }
            },
            message = frame::read::<_, Message>(&mut reader, MAX_FRAME) => match message? {
                Some(Message::Updates { updates }) => {
                    space.lock().await.import_checked(&updates)?;
                    let _ = events.send(LiveEvent::Changed { machine: peer }).await;
                }
                Some(Message::Cursor { selections, .. }) => {
                    let _ = events.send(LiveEvent::Cursor { machine: peer, selections }).await;
                }
                Some(Message::Unwatch) | None => break Ok(()),
                Some(other) => break Err(Error::Protocol(format!("unexpected {other:?}"))),
            },
        }
    };
    let _ = events.send(LiveEvent::Ended { machine: peer }).await;
    result
}

/// Sends a blob in answer to `fetch`, after the caller checked membership.
pub async fn send_blob<W: AsyncWrite + Unpin>(
    writer: &mut W,
    path: &std::path::Path,
) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await.map_err(|e| Error::io(path, e))?;
    let size = file.metadata().await.map_err(|e| Error::io(path, e))?.len();
    send(writer, &Message::Blob { size }).await?;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buffer).await.map_err(|e| Error::io(path, e))?;
        if read == 0 {
            break;
        }
        send(writer, &Message::Chunk { data: buffer[..read].to_vec() }).await?;
    }
    send(writer, &Message::Done).await
}

/// Fetches a blob from a holder into `blobs`, and keeps it only if its bytes match the hash.
pub async fn fetch_blob<S>(
    stream: S,
    space: Id,
    machine: Id,
    hash: FileHash,
    blobs: &crate::files::Blobs,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use std::io::Write;
    let (mut reader, mut writer) = tokio::io::split(stream);
    send(&mut writer, &Message::Fetch { space, machine, hash: hash.0.to_vec() }).await?;
    let Message::Blob { size } = receive(&mut reader).await? else {
        return Err(Error::Protocol("expected blob".into()));
    };
    let (path, mut file) = blobs.incoming()?;
    let mut received = 0u64;
    loop {
        match receive(&mut reader).await? {
            Message::Chunk { data } => {
                received += data.len() as u64;
                if received > size {
                    let _ = std::fs::remove_file(&path);
                    return Err(Error::Protocol("the blob is longer than announced".into()));
                }
                file.write_all(&data).map_err(|e| Error::io(&path, e))?;
            }
            Message::Done => break,
            other => return Err(Error::Protocol(format!("unexpected {other:?}"))),
        }
    }
    file.sync_all().map_err(|e| Error::io(&path, e))?;
    drop(file);
    if blobs.finish_incoming(&path, hash)? {
        Ok(())
    } else {
        Err(Error::Protocol("the blob doesn't match its hash".into()))
    }
}

/// How far this machine's changes and files have reached the other members.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SyncState {
    NotSynced,
    PartlySynced,
    Synced,
}

/// The sync status of a space, as the sync specification defines it. `own_peers` are the Loro
/// peers this machine has written with, and `recorded` each member's recorded version vector.
pub fn status(space: &Space, own_peers: &[u64], recorded: &BTreeMap<Id, Vec<u8>>) -> SyncState {
    let current = space.doc().oplog_vv();
    let others: Vec<_> = space.members().into_iter().filter(|m| m.id != space.machine()).collect();
    let versions: BTreeMap<Id, VersionVector> = recorded
        .iter()
        .filter_map(|(id, bytes)| Some((*id, VersionVector::decode(bytes).ok()?)))
        .collect();
    let has_all = |member: Id| {
        versions.get(&member).is_some_and(|vv| {
            own_peers.iter().all(|peer| {
                vv.get(peer).copied().unwrap_or(0) >= current.get(peer).copied().unwrap_or(0)
            })
        })
    };
    let wrote = own_peers.iter().any(|peer| current.get(peer).copied().unwrap_or(0) > 0);
    let mut state = SyncState::Synced;
    if wrote {
        let reached: Vec<bool> = others.iter().map(|m| has_all(m.id)).collect();
        if !reached.iter().any(|&r| r) {
            return SyncState::NotSynced;
        }
        if reached.iter().any(|&r| !r) {
            state = SyncState::PartlySynced;
        }
    }
    let me = space.machine();
    for hash in space.files_in_use() {
        let Some(entry) = space.file(hash) else { continue };
        if !entry.holders.contains_key(&me) {
            continue;
        }
        if !entry.holders.keys().any(|holder| *holder != me) {
            return SyncState::NotSynced;
        }
        let missing =
            others.iter().any(|m| m.policy == Policy::All && !entry.holders.contains_key(&m.id));
        if missing {
            state = state.min(SyncState::PartlySynced);
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    struct Pair {
        a: Arc<Mutex<Space>>,
        b: Arc<Mutex<Space>>,
        a_key: MachineKey,
        b_key: MachineKey,
    }

    fn pair() -> Pair {
        let (a_id, b_id) = (Id::generate(), Id::generate());
        let (a_key, b_key) = (MachineKey([1; 32]), MachineKey([2; 32]));
        let mut a = Space::create(a_id).unwrap();
        a.add_machine(a_id, a_key, "A", Policy::All).unwrap();
        a.add_machine(b_id, b_key, "B", Policy::OnDemand).unwrap();
        let b = Space::open(&a.snapshot().unwrap(), b_id).unwrap();
        Pair { a: Arc::new(Mutex::new(a)), b: Arc::new(Mutex::new(b)), a_key, b_key }
    }

    #[tokio::test]
    async fn exchanges_changes_both_ways() {
        let p = pair();
        let from_a = p.a.lock().await.create_note(schema::NOTE, "from a").unwrap();
        let from_b = p.b.lock().await.create_note(schema::NOTE, "from b").unwrap();
        let (one, two) = tokio::io::duplex(64);
        let b = p.b.clone();
        let listener =
            tokio::spawn(async move { listen(two, MachineKey([1; 32]), |_| Some(b)).await });
        let dialed = dial(one, &p.a, p.b_key).await.unwrap();
        let (_, from_listener) = listener.await.unwrap().unwrap();
        assert_eq!(p.a.lock().await.note(from_b).unwrap().body, "from b");
        assert_eq!(p.b.lock().await.note(from_a).unwrap().body, "from a");
        assert_eq!(dialed.version, from_listener.version);
    }

    #[tokio::test]
    async fn refuses_strangers() {
        let p = pair();
        let (one, two) = tokio::io::duplex(1024);
        let b = p.b.clone();
        let listener =
            tokio::spawn(async move { listen(two, MachineKey([9; 32]), |_| Some(b)).await });
        assert!(dial(one, &p.a, p.b_key).await.is_err());
        assert!(listener.await.unwrap().is_err());
        let _ = p.a_key;
    }

    #[tokio::test]
    async fn confirms_holding() {
        let (one, two) = tokio::io::duplex(1024);
        let hash = FileHash::of(b"x");
        let answer = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(two);
            let Some(Message::Hold { hash, .. }) =
                frame::read(&mut reader, MAX_FRAME).await.unwrap()
            else {
                panic!("expected hold")
            };
            answer_hold(&mut writer, &hash, Policy::All, true).await.unwrap();
        });
        assert!(ask_hold(one, Id::generate(), Id::generate(), hash).await.unwrap());
        answer.await.unwrap();
    }

    #[tokio::test]
    async fn streams_live_changes() {
        let p = pair();
        let note = p.a.lock().await.create_note(schema::NOTE, "shared").unwrap();
        let version_b = p.b.lock().await.version();
        let version_a = p.a.lock().await.version();
        let (one, two) = tokio::io::duplex(1 << 16);
        let (a_tx, a_rx) = broadcast::channel(16);
        let (b_tx, b_rx) = broadcast::channel::<Vec<u8>>(16);
        let (_a_cursor_tx, a_cursor_rx) = broadcast::channel(4);
        let (b_cursor_tx, b_cursor_rx) = broadcast::channel(4);
        let (a_events_tx, mut a_events) = mpsc::channel(16);
        let (b_events_tx, mut b_events) = mpsc::channel(16);
        let (a, b) = (p.a.clone(), p.b.clone());
        let b_id = p.b.lock().await.machine();
        let a_id = p.a.lock().await.machine();
        let a_session = tokio::spawn(async move {
            live(one, &a, b_id, &version_b, a_rx, a_cursor_rx, a_events_tx).await
        });
        let b_session = tokio::spawn(async move {
            live(two, &b, a_id, &version_a, b_rx, b_cursor_rx, b_events_tx).await
        });
        // Each side hears that the session started, then B catches up with A's note through it.
        assert_eq!(b_events.recv().await, Some(LiveEvent::Started { machine: a_id }));
        assert_eq!(a_events.recv().await, Some(LiveEvent::Started { machine: b_id }));
        assert_eq!(b_events.recv().await, Some(LiveEvent::Changed { machine: a_id }));
        assert_eq!(p.b.lock().await.note(note).unwrap().body, "shared");
        // A types; its local update reaches B.
        let before = p.a.lock().await.version();
        {
            let mut a = p.a.lock().await;
            a.insert_text(note, 6, "!").unwrap();
            a.commit();
        }
        let update = p.a.lock().await.updates_since(&before).unwrap().unwrap();
        a_tx.send(update).unwrap();
        assert_eq!(b_events.recv().await, Some(LiveEvent::Changed { machine: a_id }));
        assert_eq!(p.b.lock().await.note(note).unwrap().body, "shared!");
        // B's cursor reaches A.
        let selection = Selection { anchor: vec![1], head: vec![1] };
        b_cursor_tx.send((note, vec![selection.clone()])).unwrap();
        loop {
            match a_events.recv().await.unwrap() {
                LiveEvent::Cursor { selections, .. } => {
                    assert_eq!(selections, vec![selection]);
                    break;
                }
                LiveEvent::Started { .. } | LiveEvent::Changed { .. } => {}
                LiveEvent::Ended { .. } => panic!("ended early"),
            }
        }
        // B closes the note.
        drop(b_tx);
        drop(b_cursor_tx);
        b_session.await.unwrap().unwrap();
        a_session.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn reports_sync_status() {
        let p = pair();
        let mut a = p.a.lock().await;
        let own = vec![a.doc().peer_id()];
        a.create_note(schema::NOTE, "new").unwrap();
        let b_id = p.b.lock().await.machine();
        assert_eq!(status(&a, &own, &BTreeMap::new()), SyncState::NotSynced);
        let recorded = BTreeMap::from([(b_id, a.version())]);
        assert_eq!(status(&a, &own, &recorded), SyncState::Synced);
        let third = Id::generate();
        a.add_machine(third, MachineKey([3; 32]), "C", Policy::All).unwrap();
        let recorded = BTreeMap::from([(b_id, a.version())]);
        assert_eq!(status(&a, &own, &recorded), SyncState::PartlySynced);
    }
}
