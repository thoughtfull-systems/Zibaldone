//! Pairing and invites, as the pairing specification defines them.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::address::MachineKey;
use crate::frame::{self, MAX_PAIR_FRAME};
use crate::space::Policy;
use crate::{Error, Id, Result, base32};

/// The protocol's name, which both sides agree on when a connection opens.
pub const ALPN: &[u8] = b"zib/pair";
/// The service name under which a machine announces itself while adding a machine.
pub const SERVICE: &str = "zib-pair";
/// How long a machine waits for its owner to compare codes.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);
/// How long an invite works.
pub const INVITE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// A message on a `zib/pair` stream.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Message {
    Request {
        machine: Id,
        name: String,
        policy: String,
        #[serde(with = "serde_bytes")]
        commitment: Vec<u8>,
    },
    Nonce {
        machine: Id,
        name: String,
        policy: String,
        #[serde(with = "serde_bytes")]
        nonce: Vec<u8>,
    },
    Reveal {
        #[serde(with = "serde_bytes")]
        nonce: Vec<u8>,
    },
    Confirm,
    Cancel,
    Share {
        spaces: Vec<Id>,
    },
    Invite {
        #[serde(with = "serde_bytes")]
        secret: Vec<u8>,
        space: Id,
        machine: Id,
        name: String,
        policy: String,
    },
    Joined {
        space: Id,
    },
    Error {
        reason: String,
    },
}

/// One side of a pairing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Party {
    pub machine: Id,
    pub name: String,
    pub policy: Policy,
    pub key: MachineKey,
}

/// What pairing settled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paired {
    /// The other machine, to add to each space this side shares.
    pub peer: Party,
    /// The spaces the other machine shared with this one, which arrive through sync.
    pub received: Vec<Id>,
}

/// The six-digit code that both machines show, from both keys and both nonces.
pub fn code(
    initiator: &MachineKey,
    responder: &MachineKey,
    initiator_nonce: &[u8],
    responder_nonce: &[u8],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"zib pairing code");
    hasher.update(&initiator.0);
    hasher.update(&responder.0);
    hasher.update(initiator_nonce);
    hasher.update(responder_nonce);
    let hash = hasher.finalize();
    let first: [u8; 8] = hash.as_bytes()[..8].try_into().expect("8 bytes");
    format!("{:06}", u64::from_be_bytes(first) % 1_000_000)
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::Protocol(e.to_string()))?;
    Ok(bytes)
}

pub(crate) async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: &Message) -> Result<()> {
    frame::write(writer, message, MAX_PAIR_FRAME).await
}

pub(crate) async fn receive<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Message> {
    match frame::read(reader, MAX_PAIR_FRAME).await? {
        Some(Message::Error { reason }) => {
            Err(Error::Protocol(format!("the other machine refused: {reason}")))
        }
        Some(Message::Cancel) => Err(Error::Protocol("the other machine cancelled".into())),
        Some(message) => Ok(message),
        None => Err(Error::Protocol("the stream ended early".into())),
    }
}

fn policy(name: &str) -> Result<Policy> {
    Policy::from_name(name).ok_or_else(|| Error::Protocol(format!("unknown policy {name}")))
}

/// Pairs as the machine that picked the other. `confirm` shows the code and the other machine's
/// name, and returns the spaces the owner chose to share once the owner says the codes match, or
/// `None` when the owner declines.
pub async fn initiate<S, C>(
    stream: S,
    me: &Party,
    peer_key: MachineKey,
    confirm: C,
) -> Result<Paired>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: AsyncFnOnce(&Party, &str) -> Option<Vec<Id>>,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let nonce: [u8; 32] = random()?;
    let commitment = blake3::hash(&nonce).as_bytes().to_vec();
    send(
        &mut writer,
        &Message::Request {
            machine: me.machine,
            name: me.name.clone(),
            policy: me.policy.name().into(),
            commitment,
        },
    )
    .await?;
    let Message::Nonce { machine, name, policy: their_policy, nonce: their_nonce } =
        receive(&mut reader).await?
    else {
        return Err(Error::Protocol("expected nonce".into()));
    };
    let peer = Party { machine, name, policy: policy(&their_policy)?, key: peer_key };
    send(&mut writer, &Message::Reveal { nonce: nonce.to_vec() }).await?;
    let code = code(&me.key, &peer_key, &nonce, &their_nonce);
    finish(&mut reader, &mut writer, peer, &code, confirm).await
}

/// Pairs as the machine that another picked, while its Add machine screen is open, after the
/// `request` arrived.
pub async fn respond<S, C>(
    stream: S,
    me: &Party,
    peer_key: MachineKey,
    confirm: C,
) -> Result<Paired>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: AsyncFnOnce(&Party, &str) -> Option<Vec<Id>>,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let Message::Request { machine, name, policy: their_policy, commitment } =
        receive(&mut reader).await?
    else {
        return Err(Error::Protocol("expected request".into()));
    };
    let peer = Party { machine, name, policy: policy(&their_policy)?, key: peer_key };
    let nonce: [u8; 32] = random()?;
    send(
        &mut writer,
        &Message::Nonce {
            machine: me.machine,
            name: me.name.clone(),
            policy: me.policy.name().into(),
            nonce: nonce.to_vec(),
        },
    )
    .await?;
    let Message::Reveal { nonce: their_nonce } = receive(&mut reader).await? else {
        return Err(Error::Protocol("expected reveal".into()));
    };
    if blake3::hash(&their_nonce).as_bytes().as_slice() != commitment.as_slice() {
        let _ =
            send(&mut writer, &Message::Error { reason: "the nonce doesn't match".into() }).await;
        return Err(Error::Protocol("the revealed nonce doesn't match the commitment".into()));
    }
    let code = code(&peer_key, &me.key, &their_nonce, &nonce);
    finish(&mut reader, &mut writer, peer, &code, confirm).await
}

async fn finish<R, W, C>(
    reader: &mut R,
    writer: &mut W,
    peer: Party,
    code: &str,
    confirm: C,
) -> Result<Paired>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    C: AsyncFnOnce(&Party, &str) -> Option<Vec<Id>>,
{
    let confirmed =
        tokio::time::timeout(CONFIRM_TIMEOUT, confirm(&peer, code)).await.ok().flatten();
    let Some(share) = confirmed else {
        let _ = send(writer, &Message::Cancel).await;
        return Err(Error::Protocol("the owner didn't confirm the code".into()));
    };
    send(writer, &Message::Confirm).await?;
    let Message::Confirm = receive(reader).await? else {
        return Err(Error::Protocol("expected confirm".into()));
    };
    send(writer, &Message::Share { spaces: share }).await?;
    let Message::Share { spaces } = receive(reader).await? else {
        return Err(Error::Protocol("expected share".into()));
    };
    Ok(Paired { peer, received: spaces })
}

/// An invite to join one space, as its link holds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invite {
    pub key: MachineKey,
    pub space: Id,
    pub secret: [u8; 16],
    pub hints: Vec<SocketAddr>,
}

impl Invite {
    /// A new invite from the member with `key`, for `space`.
    pub fn new(key: MachineKey, space: Id, hints: Vec<SocketAddr>) -> Result<Self> {
        Ok(Self { key, space, secret: random()?, hints })
    }

    /// The invite's link.
    pub fn uri(&self) -> String {
        let mut uri = format!(
            "zib:invite/{}/{}/{}",
            self.key.to_base32(),
            self.space,
            base32::encode(&self.secret, 26)
        );
        for (i, hint) in self.hints.iter().enumerate() {
            uri.push_str(if i == 0 { "?hints=" } else { "," });
            uri.push_str(&hint.to_string());
        }
        uri
    }

    /// Reads a link, in any case, as a QR code may hold it in uppercase.
    pub fn parse(uri: &str) -> Result<Self> {
        let invalid = || Error::invalid("invite", uri);
        let rest =
            uri.get(..11).filter(|p| p.eq_ignore_ascii_case("zib:invite/")).ok_or_else(invalid)?;
        let _ = rest;
        let rest = &uri[11..];
        let (path, query) = rest.split_once('?').map_or((rest, None), |(p, q)| (p, Some(q)));
        let parts: Vec<&str> = path.split('/').collect();
        let [key, space, secret] = parts.as_slice() else { return Err(invalid()) };
        let key = base32::decode::<32>(key, 52, false).map(MachineKey).ok_or_else(invalid)?;
        let space = Id::parse(space)?;
        let secret = base32::decode::<16>(secret, 26, false).ok_or_else(invalid)?;
        let hints = match query {
            None => Vec::new(),
            Some(query) => {
                let value = query
                    .get(..6)
                    .filter(|p| p.eq_ignore_ascii_case("hints="))
                    .ok_or_else(invalid)?;
                let _ = value;
                query[6..]
                    .split(',')
                    .map(|h| h.parse().map_err(|_| invalid()))
                    .collect::<Result<_>>()?
            }
        };
        Ok(Self { key, space, secret, hints })
    }
}

/// The invites a member made and hasn't seen used.
#[derive(Debug, Default)]
pub struct Invites(Vec<(Invite, Instant)>);

impl Invites {
    pub fn add(&mut self, invite: Invite) {
        self.0.push((invite, Instant::now()));
    }

    /// Uses up the invite with `secret` for `space`, if it exists and hasn't expired.
    pub fn take(&mut self, space: Id, secret: &[u8]) -> bool {
        self.0.retain(|(_, made)| made.elapsed() < INVITE_LIFETIME);
        let found =
            self.0.iter().position(|(invite, _)| invite.space == space && invite.secret == secret);
        found.map(|i| self.0.remove(i)).is_some()
    }
}

/// Joins a space by invite, as the machine that opened the link.
pub async fn join<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    me: &Party,
    invite: &Invite,
) -> Result<Id> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    send(
        &mut writer,
        &Message::Invite {
            secret: invite.secret.to_vec(),
            space: invite.space,
            machine: me.machine,
            name: me.name.clone(),
            policy: me.policy.name().into(),
        },
    )
    .await?;
    match receive(&mut reader).await? {
        Message::Joined { space } if space == invite.space => Ok(space),
        other => Err(Error::Protocol(format!("unexpected {other:?}"))),
    }
}

/// Answers an invite, as the member that made it, after the `invite` message arrived. It returns
/// the machine to add to the space, and replies `joined` once `add` succeeds.
pub async fn admit<S, A>(
    stream: S,
    peer_key: MachineKey,
    invites: &std::sync::Mutex<Invites>,
    add: A,
) -> Result<(Id, Party)>
where
    S: AsyncRead + AsyncWrite + Unpin,
    A: AsyncFnOnce(Id, &Party) -> Result<()>,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let Message::Invite { secret, space, machine, name, policy: their_policy } =
        receive(&mut reader).await?
    else {
        return Err(Error::Protocol("expected invite".into()));
    };
    let valid = invites.lock().expect("unpoisoned").take(space, &secret);
    if !valid {
        let _ = send(&mut writer, &Message::Error { reason: "no such invite".into() }).await;
        return Err(Error::Protocol("an invite that doesn't exist or expired".into()));
    }
    let party = Party { machine, name, policy: policy(&their_policy)?, key: peer_key };
    add(space, &party).await?;
    send(&mut writer, &Message::Joined { space }).await?;
    Ok((space, party))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party(byte: u8, policy: Policy) -> Party {
        Party {
            machine: Id::generate(),
            name: format!("M{byte}"),
            policy,
            key: MachineKey([byte; 32]),
        }
    }

    #[tokio::test]
    async fn pairs_with_matching_codes() {
        let (a, b) = (party(1, Policy::All), party(2, Policy::OnDemand));
        let (one, two) = tokio::io::duplex(1024);
        let space = Id::generate();
        let codes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (c1, c2) = (codes.clone(), codes.clone());
        let b2 = b.clone();
        let responder = tokio::spawn(async move {
            respond(two, &b2, MachineKey([1; 32]), async move |_: &Party, code: &str| {
                c2.lock().unwrap().push(code.to_owned());
                Some(Vec::new())
            })
            .await
        });
        let paired = initiate(one, &a, b.key, async move |peer: &Party, code: &str| {
            assert_eq!(peer.name, "M2");
            c1.lock().unwrap().push(code.to_owned());
            Some(vec![space])
        })
        .await
        .unwrap();
        let other = responder.await.unwrap().unwrap();
        let codes = codes.lock().unwrap();
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0], codes[1]);
        assert_eq!(codes[0].len(), 6);
        assert_eq!(paired.peer.policy, Policy::OnDemand);
        assert_eq!(other.received, vec![space]);
    }

    #[tokio::test]
    async fn a_declined_code_cancels() {
        let (a, b) = (party(1, Policy::All), party(2, Policy::All));
        let (one, two) = tokio::io::duplex(1024);
        let b2 = b.clone();
        let responder = tokio::spawn(async move {
            respond(two, &b2, MachineKey([1; 32]), async |_: &Party, _: &str| None).await
        });
        assert!(
            initiate(one, &a, b.key, async |_: &Party, _: &str| Some(Vec::new())).await.is_err()
        );
        assert!(responder.await.unwrap().is_err());
    }

    #[test]
    fn codes_depend_on_every_input() {
        let (k1, k2) = (MachineKey([1; 32]), MachineKey([2; 32]));
        let base = code(&k1, &k2, &[1; 32], &[2; 32]);
        assert_ne!(base, code(&k2, &k1, &[1; 32], &[2; 32]));
        assert_ne!(base, code(&k1, &k2, &[3; 32], &[2; 32]));
    }

    #[test]
    fn invite_links_round_trip_in_any_case() {
        let invite = Invite::new(
            MachineKey([5; 32]),
            Id::generate(),
            vec!["10.0.0.2:4000".parse().unwrap()],
        )
        .unwrap();
        let uri = invite.uri();
        assert!(uri.starts_with("zib:invite/"));
        assert_eq!(Invite::parse(&uri).unwrap(), invite);
        assert_eq!(Invite::parse(&uri.to_uppercase()).unwrap(), invite);
    }

    #[tokio::test]
    async fn invites_work_once() {
        let member = party(1, Policy::All);
        let newcomer = party(2, Policy::OnDemand);
        let invite = Invite::new(member.key, Id::generate(), Vec::new()).unwrap();
        let invites = std::sync::Arc::new(std::sync::Mutex::new(Invites::default()));
        invites.lock().unwrap().add(invite.clone());
        for attempt in 0..2 {
            let (one, two) = tokio::io::duplex(1024);
            let shared = invites.clone();
            let admitting = tokio::spawn(async move {
                admit(two, MachineKey([2; 32]), &shared, async |_: Id, _: &Party| Ok(())).await
            });
            let joined = join(one, &newcomer, &invite).await;
            let admission = admitting.await.unwrap();
            assert_eq!(joined.is_ok(), attempt == 0);
            assert_eq!(admission.is_ok(), attempt == 0);
        }
    }
}
