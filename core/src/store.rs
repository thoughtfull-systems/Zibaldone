//! The machine's own store, which never syncs, as decision ZIB0003 describes: its identity, its
//! settings, the pet names of its spaces, and each space's saved document.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::channel::Dirs;
use crate::space::Policy;
use crate::{Error, Id, Result, Space};

const MACHINE: &str = "machine.json";
const SECRET_KEY: &str = "machine.key";
const SPACES: &str = "spaces";
const DOCUMENT: &str = "document.loro";

/// What the machine keeps about itself and its spaces.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct MachineConfig {
    pub id: Id,
    /// The machine's own name, which it gives itself when it joins a space.
    pub name: String,
    /// A relay of the owner's own, instead of n0's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    /// Each space the machine holds, with its pet name.
    #[serde(default)]
    pub spaces: BTreeMap<Id, SpaceEntry>,
    /// The version vector each member recorded at the end of its last exchange, by space and
    /// member, for sync status and smaller exchanges.
    #[serde(default)]
    pub versions: BTreeMap<Id, BTreeMap<Id, Vec<u8>>>,
    /// The Loro peers this machine has written each space with, which sync status compares
    /// against what other members hold.
    #[serde(default)]
    pub peers: BTreeMap<Id, Vec<u64>>,
    /// The space that the owner last put a note into, where new notes go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_space: Option<Id>,
}

/// A space that the machine holds.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SpaceEntry {
    /// The pet name that this machine's owner gave the space.
    pub name: String,
    /// Whether the owner hid the space's notes from the window.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// The machine's store in one channel's data directory.
pub struct Store {
    dir: PathBuf,
    pub config: MachineConfig,
    /// The policy that this platform's implementation sets, as decision ZIB0004 requires.
    pub policy: Policy,
}

impl Store {
    /// Opens the store in `dirs.data`, creating the machine's identity on first use.
    pub fn open(dirs: &Dirs, policy: Policy) -> Result<Self> {
        let dir = dirs.data.clone();
        create_private_dir(&dir)?;
        let path = dir.join(MACHINE);
        let config = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| Error::Space(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = MachineConfig {
                    id: Id::generate(),
                    name: default_name(),
                    relay: None,
                    spaces: BTreeMap::new(),
                    versions: BTreeMap::new(),
                    peers: BTreeMap::new(),
                    last_space: None,
                };
                write_atomic(&path, &serde_json::to_vec_pretty(&config).expect("serializable"))?;
                config
            }
            Err(e) => return Err(Error::io(path, e)),
        };
        Ok(Self { dir, config, policy })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Saves the machine's configuration.
    pub fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.config).expect("serializable");
        write_atomic(&self.dir.join(MACHINE), &bytes)
    }

    /// The machine's Ed25519 secret key, made from 32 random bytes the first time it's needed,
    /// and readable only by the user.
    pub fn secret_key(&self) -> Result<[u8; 32]> {
        let path = self.dir.join(SECRET_KEY);
        match fs::read(&path) {
            Ok(bytes) => bytes
                .try_into()
                .map_err(|_| Error::Space(format!("{} isn't 32 bytes", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut key = [0u8; 32];
                getrandom::fill(&mut key).map_err(|e| Error::Space(e.to_string()))?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .map_err(|e| Error::io(&path, e))?;
                file.write_all(&key).map_err(|e| Error::io(&path, e))?;
                Ok(key)
            }
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// The directory of one space's document and blobs.
    pub fn space_dir(&self, space: Id) -> PathBuf {
        self.dir.join(SPACES).join(space.to_string())
    }

    /// Creates a space, saves it, and records its pet name.
    pub fn create_space(&mut self, name: &str) -> Result<Space> {
        let space = Space::create(self.config.id)?;
        let id = space.id()?;
        self.config.spaces.insert(id, SpaceEntry { name: name.to_owned(), hidden: false });
        self.save_space(&space)?;
        self.save()?;
        Ok(space)
    }

    /// Loads a space that the machine holds.
    pub fn load_space(&self, id: Id) -> Result<Space> {
        let path = self.space_dir(id).join(DOCUMENT);
        let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        Space::open(&bytes, self.config.id)
    }

    /// Saves a space's whole document, replacing the old file only once the new one is written.
    pub fn save_space(&self, space: &Space) -> Result<()> {
        space.commit();
        let dir = self.space_dir(space.id()?);
        create_private_dir(&dir)?;
        write_atomic(&dir.join(DOCUMENT), &space.snapshot()?)
    }

    /// Records the Loro peer that this process writes `space` with, so sync status can tell this
    /// machine's changes apart.
    pub fn record_peer(&mut self, space: &Space) -> Result<()> {
        let peers = self.config.peers.entry(space.id()?).or_default();
        let peer = space.doc().peer_id();
        if !peers.contains(&peer) {
            peers.push(peer);
            self.save()?;
        }
        Ok(())
    }

    /// The blobs of a space on this machine.
    pub fn blobs(&self, space: Id) -> crate::files::Blobs {
        crate::files::Blobs::new(self.space_dir(space).join("blobs"))
    }

    /// The directory that holds each space's directory.
    pub fn spaces_dir(&self) -> PathBuf {
        self.dir.join(SPACES)
    }

    /// Renames a space on this machine.
    pub fn rename_space(&mut self, space: Id, name: &str) -> Result<()> {
        if let Some(entry) = self.config.spaces.get_mut(&space) {
            name.clone_into(&mut entry.name);
        }
        self.save()
    }

    /// Shows or hides a space's notes in the window.
    pub fn set_hidden(&mut self, space: Id, hidden: bool) -> Result<()> {
        if let Some(entry) = self.config.spaces.get_mut(&space) {
            entry.hidden = hidden;
        }
        self.save()
    }

    /// Records the space the owner last put a note into.
    pub fn set_last_space(&mut self, space: Id) -> Result<()> {
        if self.config.last_space == Some(space) {
            return Ok(());
        }
        self.config.last_space = Some(space);
        self.save()
    }

    /// Records a space that arrived through pairing, with the pet name its owner gave it.
    pub fn add_space(&mut self, space: &Space, name: &str) -> Result<()> {
        self.config.spaces.insert(space.id()?, SpaceEntry { name: name.to_owned(), hidden: false });
        self.save_space(space)?;
        self.save()
    }
}

/// The machine's name until its owner chooses one: the host's name.
fn default_name() -> String {
    fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Linux".to_owned())
}

/// Creates a directory, and its parents, that only the user may read, write, or enter.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| Error::io(dir, e))
}

/// Writes a file through a temporary file and a rename, so a crash never leaves half of it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary).map_err(|e| Error::io(&temporary, e))?;
    file.write_all(bytes).map_err(|e| Error::io(&temporary, e))?;
    file.sync_all().map_err(|e| Error::io(&temporary, e))?;
    fs::rename(&temporary, path).map_err(|e| Error::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    #[test]
    fn keeps_identity_spaces_and_keys() {
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(root.path());
        let mut store = Store::open(&dirs, Policy::All).unwrap();
        let machine = store.config.id;
        let mut space = store.create_space("Personal").unwrap();
        let note = space.create_note(schema::NOTE, "Hello").unwrap();
        store.save_space(&space).unwrap();
        let key = store.secret_key().unwrap();

        let reopened = Store::open(&dirs, Policy::All).unwrap();
        assert_eq!(reopened.config.id, machine);
        assert_eq!(reopened.secret_key().unwrap(), key);
        let id = space.id().unwrap();
        assert_eq!(reopened.config.spaces[&id].name, "Personal");
        assert_eq!(reopened.load_space(id).unwrap().note(note).unwrap().body, "Hello");
        let mode = fs::metadata(dirs.data.join(SECRET_KEY)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
