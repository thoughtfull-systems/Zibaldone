//! The machine's own store, which never syncs, as decision ZIB0003 describes: its identity, its
//! settings, the pet names of its spaces, and each space's saved document.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::address::MachineKey;
use crate::channel::Dirs;
use crate::pairing::Party;
use crate::space::Policy;
use crate::time::Timestamp;
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
    /// When each member last finished an exchange with this machine, by space and member, for the
    /// sync details.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last_synced: BTreeMap<Id, BTreeMap<Id, Timestamp>>,
    /// The Loro peers this machine has written each space with, which sync status compares
    /// against what other members hold.
    #[serde(default)]
    pub peers: BTreeMap<Id, Vec<u64>>,
    /// The space that the owner last put a note into, where new notes go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_space: Option<Id>,
    /// The machines this one paired with, which may offer it spaces, as decision ZIB0016
    /// describes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub paired: BTreeMap<Id, PairedMachine>,
    /// The page that notes print on, which depends on the printer at hand. Saved only when it
    /// differs from the default, so a machine that never chose a page follows the default.
    #[serde(default, skip_serializing_if = "Page::is_default")]
    pub page: Page,
}

/// A machine this one paired with.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PairedMachine {
    /// Its public key, as 64 lowercase hexadecimal digits.
    pub key: String,
    pub name: String,
    /// Its file policy, by the name that decision ZIB0004 gives it.
    pub policy: String,
}

/// The page that notes print on, a machine setting from decision ZIB0003.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(from = "StoredPage", rename_all = "kebab-case")]
pub struct Page {
    /// The margin on each side, in millimetres.
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub margin_top: f64,
    /// A paper size by Typst's name for it, such as `a4` or `us-letter`.
    pub paper: String,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            margin_bottom: 25.4,
            margin_left: 12.7,
            margin_right: 12.7,
            margin_top: 25.4,
            paper: "us-letter".into(),
        }
    }
}

impl Page {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A page as saved. One without a margin for each side comes from before the sides had their
/// own, when every machine saved the default of the time and nothing could change it, so it reads
/// as today's default.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct StoredPage {
    margin_bottom: Option<f64>,
    margin_left: Option<f64>,
    margin_right: Option<f64>,
    margin_top: Option<f64>,
    paper: String,
}

impl From<StoredPage> for Page {
    fn from(stored: StoredPage) -> Self {
        match (stored.margin_bottom, stored.margin_left, stored.margin_right, stored.margin_top) {
            (Some(margin_bottom), Some(margin_left), Some(margin_right), Some(margin_top)) => {
                Self { margin_bottom, margin_left, margin_right, margin_top, paper: stored.paper }
            }
            _ => Self::default(),
        }
    }
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
    /// Whether this open created the machine's identity, so the machine is new.
    pub created: bool,
}

impl Store {
    /// Opens the store in `dirs.data`, creating the machine's identity on first use.
    pub fn open(dirs: &Dirs, policy: Policy) -> Result<Self> {
        let dir = dirs.data.clone();
        create_private_dir(&dir)?;
        let path = dir.join(MACHINE);
        let mut created = false;
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
                    last_synced: BTreeMap::new(),
                    peers: BTreeMap::new(),
                    last_space: None,
                    page: Page::default(),
                    paired: BTreeMap::new(),
                };
                write_atomic(&path, &serde_json::to_vec_pretty(&config).expect("serializable"))?;
                created = true;
                config
            }
            Err(e) => return Err(Error::io(path, e)),
        };
        Ok(Self { dir, config, policy, created })
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

    /// Records a machine that this one paired with, or updates its name.
    pub fn add_paired(&mut self, party: &Party) -> Result<()> {
        let entry = PairedMachine {
            key: party.key.to_hex(),
            name: party.name.clone(),
            policy: party.policy.name().to_owned(),
        };
        if self.config.paired.get(&party.machine) == Some(&entry) {
            return Ok(());
        }
        self.config.paired.insert(party.machine, entry);
        self.save()
    }

    /// The machines this one paired with.
    pub fn paired(&self) -> Vec<Party> {
        self.config
            .paired
            .iter()
            .filter_map(|(id, entry)| {
                Some(Party {
                    machine: *id,
                    name: entry.name.clone(),
                    policy: Policy::from_name(&entry.policy)?,
                    key: MachineKey::from_hex(&entry.key).ok()?,
                })
            })
            .collect()
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
    fn page_follows_the_default_unless_chosen() {
        let config = |page: &str| -> MachineConfig {
            let json = format!(r#"{{"id": "{}", "name": "m"{page}}}"#, Id::generate());
            serde_json::from_str(&json).unwrap()
        };
        let old = config(r#", "page": {"paper": "a4", "margin": 20.0}"#);
        assert_eq!(old.page, Page::default());
        assert!(!serde_json::to_string(&old).unwrap().contains("page"));
        let chosen = Page {
            margin_bottom: 1.0,
            margin_left: 2.0,
            margin_right: 3.0,
            margin_top: 4.0,
            paper: "a5".into(),
        };
        let saved = serde_json::to_string(&chosen).unwrap();
        assert_eq!(config(&format!(r#", "page": {saved}"#)).page, chosen);
        let half = config(r#", "page": {"paper": "a5", "margin-x": 10.0, "margin-y": 15.0}"#);
        assert_eq!(half.page, Page::default());
    }

    #[test]
    fn keeps_identity_spaces_and_keys() {
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(root.path());
        let mut store = Store::open(&dirs, Policy::All).unwrap();
        assert!(store.created);
        let machine = store.config.id;
        let mut space = store.create_space("Personal").unwrap();
        let note = space.create_note(schema::NOTE, "Hello").unwrap();
        store.save_space(&space).unwrap();
        let key = store.secret_key().unwrap();

        let reopened = Store::open(&dirs, Policy::All).unwrap();
        assert!(!reopened.created);
        assert_eq!(reopened.config.id, machine);
        assert_eq!(reopened.secret_key().unwrap(), key);
        let id = space.id().unwrap();
        assert_eq!(reopened.config.spaces[&id].name, "Personal");
        assert_eq!(reopened.load_space(id).unwrap().note(note).unwrap().body, "Hello");
        let mode = fs::metadata(dirs.data.join(SECRET_KEY)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let party = Party {
            machine: Id::generate(),
            name: "other".into(),
            policy: Policy::OnDemand,
            key: MachineKey([7; 32]),
        };
        let mut store = reopened;
        store.add_paired(&party).unwrap();
        assert_eq!(Store::open(&dirs, Policy::All).unwrap().paired(), vec![party]);
    }
}
