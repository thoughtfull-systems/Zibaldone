//! The one running instance on a Linux machine and the `zib` command's connection to it, as the
//! instance specification defines them.

use std::fs::{self, File, TryLockError};
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::{UnixListener, UnixStream};

use crate::channel::Dirs;
use crate::frame::{self, MAX_FRAME};
use crate::store::create_private_dir;
use crate::web_clip::{Asset, Clip, Imported, Pdf};
use crate::{Error, Id, Result};

const LOCK: &str = "instance.lock";
const SOCKET: &str = "instance.sock";
/// How long the `zib` command waits for an instance it started.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);

/// A request from a client.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    Hello {
        #[serde(default)]
        features: Vec<String>,
    },
    /// Brings the window to the front, or opens one, and opens a URI if there is one.
    Activate {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
    /// Quits the instance.
    Stop { id: u64 },
    /// Lists the machine's spaces.
    Spaces { id: u64 },
    /// Creates a space with a pet name.
    CreateSpace { id: u64, name: String },
    /// Searches every space.
    Search {
        id: u64,
        query: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Searches one space by identifier or pet name.
    SearchSpace {
        id: u64,
        query: String,
        #[serde(default)]
        limit: Option<u32>,
        space: String,
    },
    /// Reads note bodies by identifier.
    GetNotes { id: u64, notes: Vec<Id> },
    /// Replaces a note body if it still matches the caller's copy.
    WriteNote { id: u64, note: Id, body: String, expected: String },
    /// Saves an article and its images together.
    ImportWebClip { id: u64, clip: Clip, images: Vec<Asset> },
    /// Saves an original PDF from the browser as a file note.
    ImportWebPdf { id: u64, pdf: Pdf },
    /// Prints notes to a PDF file.
    Print {
        id: u64,
        notes: Vec<Id>,
        #[serde(default)]
        linked: bool,
        path: String,
    },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Self::Hello { .. } => 0,
            Self::Activate { id, .. }
            | Self::Stop { id }
            | Self::Spaces { id }
            | Self::CreateSpace { id, .. }
            | Self::Search { id, .. }
            | Self::SearchSpace { id, .. }
            | Self::GetNotes { id, .. }
            | Self::WriteNote { id, .. }
            | Self::ImportWebClip { id, .. }
            | Self::ImportWebPdf { id, .. }
            | Self::Print { id, .. } => *id,
        }
    }
}

/// A space, as `spaces` lists it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SpaceInfo {
    pub id: Id,
    pub name: String,
}

/// A search result, as `search` returns it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HitInfo {
    pub space: Id,
    pub note: Id,
    pub title: String,
    pub snippet: String,
}

/// One entry in the ordered result of a batch note read.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NoteRead {
    Found { body: String, note: Id, space: Id, title: String },
    Unavailable { error: String, note: Id },
}

/// A reply from the instance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Reply {
    Hello {
        #[serde(default)]
        features: Vec<String>,
    },
    Result {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clip: Option<Imported>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        spaces: Vec<SpaceInfo>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hits: Vec<HitInfo>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notes: Vec<NoteRead>,
    },
    Error {
        id: u64,
        reason: String,
        code: String,
    },
}

impl Reply {
    /// An empty result for the request with `id`.
    pub fn done(id: u64) -> Self {
        Self::Result { id, clip: None, spaces: Vec::new(), hits: Vec::new(), notes: Vec::new() }
    }
}

/// The lock that makes a process the instance, held until it's dropped or the process ends.
pub struct Lock {
    _file: File,
}

impl Lock {
    /// Takes the instance's lock without waiting.
    pub fn take(dirs: &Dirs) -> Result<Self> {
        create_private_dir(&dirs.runtime)?;
        let path = dirs.runtime.join(LOCK);
        let file = File::create(&path).map_err(|e| Error::io(&path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Error::Locked(path)),
            Err(TryLockError::Error(e)) => Err(Error::io(path, e)),
        }
    }
}

pub fn socket_path(dirs: &Dirs) -> PathBuf {
    dirs.runtime.join(SOCKET)
}

/// Listens on the instance's socket, after removing one a previous instance left behind. Only the
/// instance, which holds the lock, may call it.
pub fn listen(dirs: &Dirs, _lock: &Lock) -> Result<UnixListener> {
    let path = socket_path(dirs);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(&path, e)),
    }
    let listener = UnixListener::bind(&path).map_err(|e| Error::io(&path, e))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::io(&path, e))?;
    Ok(listener)
}

/// What the instance does with each request.
pub type Handler =
    Arc<dyn Fn(Request) -> Pin<Box<dyn Future<Output = Reply> + Send>> + Send + Sync>;

/// Answers clients on the socket until the listener fails. Each client must belong to this user.
pub async fn serve(listener: UnixListener, handler: Handler) -> Result<()> {
    let uid = own_uid();
    loop {
        let (stream, _) = listener.accept().await?;
        if stream.peer_cred().map(|cred| cred.uid()).ok() != Some(uid) {
            continue;
        }
        let handler = handler.clone();
        tokio::spawn(async move {
            let _ = answer(stream, handler).await;
        });
    }
}

async fn answer(stream: UnixStream, handler: Handler) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    while let Some(request) = frame::read::<_, Request>(&mut reader, MAX_FRAME).await? {
        let reply = match request {
            Request::Hello { .. } => Reply::Hello { features: Vec::new() },
            other => handler(other).await,
        };
        frame::write(&mut writer, &reply, MAX_FRAME).await?;
    }
    Ok(())
}

fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self").map_or(u32::MAX, |m| m.uid())
}

/// A connection from the `zib` command to the instance.
pub struct Client {
    stream: UnixStream,
    next_id: u64,
}

impl Client {
    /// Connects to the running instance, starting one with `start` if none listens, and waiting up
    /// to 10 seconds for it.
    pub async fn connect(dirs: &Dirs, start: impl FnOnce() -> std::io::Result<()>) -> Result<Self> {
        let path = socket_path(dirs);
        let stream = if let Ok(stream) = UnixStream::connect(&path).await {
            stream
        } else {
            start().map_err(|e| Error::io(&path, e))?;
            wait_for(&path).await?
        };
        let mut client = Self { stream, next_id: 1 };
        frame::write(&mut client.stream, &Request::Hello { features: Vec::new() }, MAX_FRAME)
            .await?;
        match frame::read::<_, Reply>(&mut client.stream, MAX_FRAME).await? {
            Some(Reply::Hello { .. }) => Ok(client),
            other => Err(Error::Protocol(format!("expected hello, got {other:?}"))),
        }
    }

    /// The identifier for the next request.
    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Sends a request and waits for its reply.
    pub async fn request(&mut self, request: Request) -> Result<Reply> {
        let id = request.id();
        frame::write(&mut self.stream, &request, MAX_FRAME).await?;
        loop {
            match frame::read::<_, Reply>(&mut self.stream, MAX_FRAME).await? {
                Some(reply @ (Reply::Result { id: got, .. } | Reply::Error { id: got, .. }))
                    if got == id =>
                {
                    return Ok(reply);
                }
                Some(_) => {}
                None => return Err(Error::Protocol("the instance closed the connection".into())),
            }
        }
    }
}

async fn wait_for(path: &Path) -> Result<UnixStream> {
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        match UnixStream::connect(path).await {
            Ok(stream) => return Ok(stream),
            Err(e) if tokio::time::Instant::now() >= deadline => return Err(Error::io(path, e)),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_process_holds_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(root.path());
        let lock = Lock::take(&dirs).unwrap();
        assert!(matches!(Lock::take(&dirs), Err(Error::Locked(_))));
        drop(lock);
        assert!(Lock::take(&dirs).is_ok());
    }

    #[tokio::test]
    async fn answers_requests_and_starts_on_demand() {
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(root.path());
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = started.clone();
        let server_dirs = dirs.clone();
        let start = move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            let lock = Lock::take(&server_dirs).map_err(std::io::Error::other)?;
            let listener = listen(&server_dirs, &lock).map_err(std::io::Error::other)?;
            let handler: Handler = Arc::new(|request: Request| {
                Box::pin(async move {
                    match request {
                        Request::Spaces { id } => Reply::Result {
                            id,
                            clip: None,
                            spaces: vec![SpaceInfo {
                                id: Id::from_bytes([0; 16]),
                                name: "Home".into(),
                            }],
                            hits: Vec::new(),
                            notes: Vec::new(),
                        },
                        other => Reply::Error {
                            id: other.id(),
                            reason: "no".into(),
                            code: "unsupported".into(),
                        },
                    }
                })
            });
            tokio::spawn(async move {
                let _lock = lock;
                serve(listener, handler).await
            });
            Ok(())
        };
        let mut client = Client::connect(&dirs, start).await.unwrap();
        assert!(started.load(std::sync::atomic::Ordering::SeqCst));
        let id = client.next_id();
        let Reply::Result { spaces, .. } = client.request(Request::Spaces { id }).await.unwrap()
        else {
            panic!("expected a result")
        };
        assert_eq!(spaces[0].name, "Home");
        let id = client.next_id();
        assert!(matches!(client.request(Request::Stop { id }).await.unwrap(), Reply::Error { .. }));
        // A second client finds the running instance without starting another.
        let again = Client::connect(&dirs, || panic!("started twice")).await;
        assert!(again.is_ok());
    }
}
