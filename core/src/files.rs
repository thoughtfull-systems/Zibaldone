//! The blobs of one space on this machine, and the work around them that decision ZIB0004 and the
//! schemas specification describe: adding files as notes, derived fields, and collecting garbage.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::schema::{self, Value};
use crate::space::FileHash;
use crate::store::{create_private_dir, write_atomic};
use crate::time::{self, Timestamp};
use crate::{Error, Id, Result, Space};

/// How long a blob stays after it falls out of use.
pub const GARBAGE_WAIT: Timestamp = 30 * 24 * 60 * 60 * 1000;

/// The blobs of one space, each in a file named by its hash.
#[derive(Clone, Debug)]
pub struct Blobs {
    dir: PathBuf,
}

impl Blobs {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self, hash: FileHash) -> PathBuf {
        self.dir.join(hash.to_string())
    }

    pub fn has(&self, hash: FileHash) -> bool {
        self.path(hash).is_file()
    }

    /// Stores bytes and returns their hash.
    pub fn add_bytes(&self, bytes: &[u8]) -> Result<FileHash> {
        let hash = FileHash::of(bytes);
        if !self.has(hash) {
            create_private_dir(&self.dir)?;
            write_atomic(&self.path(hash), bytes)?;
        }
        Ok(hash)
    }

    /// Copies a file in, hashing it as it reads, and returns its hash and size.
    pub fn add_file(&self, source: &Path) -> Result<(FileHash, u64)> {
        create_private_dir(&self.dir)?;
        let temporary = self.dir.join(format!(".incoming-{}", Id::generate()));
        let mut input = File::open(source).map_err(|e| Error::io(source, e))?;
        let mut output = File::create(&temporary).map_err(|e| Error::io(&temporary, e))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1 << 16];
        let mut size = 0u64;
        loop {
            let read = input.read(&mut buffer).map_err(|e| Error::io(source, e))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read]).map_err(|e| Error::io(&temporary, e))?;
            size += read as u64;
        }
        output.sync_all().map_err(|e| Error::io(&temporary, e))?;
        let hash = FileHash(*hasher.finalize().as_bytes());
        fs::rename(&temporary, self.path(hash)).map_err(|e| Error::io(self.path(hash), e))?;
        Ok((hash, size))
    }

    /// A file for a blob arriving from another machine, which `finish_incoming` checks.
    pub fn incoming(&self) -> Result<(PathBuf, File)> {
        create_private_dir(&self.dir)?;
        let path = self.dir.join(format!(".incoming-{}", Id::generate()));
        let file = File::create(&path).map_err(|e| Error::io(&path, e))?;
        Ok((path, file))
    }

    /// Keeps an arrived blob if its bytes match `hash`, and removes it otherwise.
    pub fn finish_incoming(&self, path: &Path, hash: FileHash) -> Result<bool> {
        let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| Error::io(path, e))?;
        if hasher.finalize().as_bytes() == &hash.0 {
            fs::rename(path, self.path(hash)).map_err(|e| Error::io(self.path(hash), e))?;
            Ok(true)
        } else {
            let _ = fs::remove_file(path);
            Ok(false)
        }
    }

    pub fn read(&self, hash: FileHash) -> Result<Vec<u8>> {
        fs::read(self.path(hash)).map_err(|e| Error::io(self.path(hash), e))
    }

    pub fn remove(&self, hash: FileHash) -> Result<()> {
        match fs::remove_file(self.path(hash)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(self.path(hash), e)),
        }
    }
}

/// Adds a file to a space as one note, as the built-in types specification describes: an `image`
/// note for an image, and a `file` note otherwise, with the file's name as its title. The blob
/// must already be in `blobs`.
pub fn add_file_note(
    space: &mut Space,
    hash: FileHash,
    size: u64,
    name: &str,
    media_type: &str,
) -> Result<Id> {
    let (type_name, field) = if media_type.starts_with("image/") {
        (schema::IMAGE, "image")
    } else {
        (schema::FILE, "file")
    };
    let id = space.create_note(type_name, &format!("{name}\n\n"))?;
    space.add_file(hash, size, media_type)?;
    space.set_field(id, field, &Value::File(hash))?;
    Ok(id)
}

/// A derived field that a machine can fill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub note: Id,
    pub field: String,
    pub derivation: String,
    pub source: FileHash,
    pub media_type: String,
}

/// The derived fields that are empty while their source holds a blob this machine holds.
pub fn pending_derivations(space: &Space, blobs: &Blobs) -> Vec<Pending> {
    let mut pending = Vec::new();
    for id in space.note_ids() {
        let Ok(note) = space.note(id) else { continue };
        let Some(ty) = schema::read_type(space.doc(), &note.type_name) else { continue };
        for (name, def) in &ty.fields {
            let (Some(derivation), Some(from)) = (&def.derivation, &def.derived_from) else {
                continue;
            };
            if note.fields.contains_key(name) {
                continue;
            }
            let Some(Value::File(source)) = note.field(space, from) else { continue };
            if !blobs.has(source) {
                continue;
            }
            let media_type = space.file(source).map(|f| f.media_type).unwrap_or_default();
            pending.push(Pending {
                note: id,
                field: name.clone(),
                derivation: derivation.clone(),
                source,
                media_type,
            });
        }
    }
    pending
}

/// Sets a derived field to a blob a derivation made, in one commit, unless the source changed
/// meanwhile.
pub fn complete_derivation(
    space: &mut Space,
    blobs: &Blobs,
    pending: &Pending,
    bytes: &[u8],
    media_type: &str,
) -> Result<bool> {
    let note = space.note(pending.note)?;
    let Some(from) = schema::read_type(space.doc(), &note.type_name)
        .and_then(|ty| ty.fields.get(&pending.field).and_then(|d| d.derived_from.clone()))
    else {
        return Ok(false);
    };
    if note.field(space, &from) != Some(Value::File(pending.source)) {
        return Ok(false);
    }
    let hash = blobs.add_bytes(bytes)?;
    space.add_file(hash, bytes.len() as u64, media_type)?;
    let fields = crate::schema::child_map(&space_note_map(space, pending.note)?, "fields")?;
    fields.insert(&pending.field, Value::File(hash).to_loro())?;
    space.commit();
    Ok(true)
}

fn space_note_map(space: &Space, id: Id) -> Result<loro::LoroMap> {
    match space.doc().get_map("notes").get(&id.to_string()) {
        Some(loro::ValueOrContainer::Container(c)) => {
            c.into_map().map_err(|_| Error::Space(format!("note {id}")))
        }
        _ => Err(Error::NotFound(format!("note {id}"))),
    }
}

/// When this machine first found each blob out of use, kept in its cache directory.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GarbageRecords {
    #[serde(default)]
    pub since: BTreeMap<String, Timestamp>,
    /// Derivations that failed, by derivation and source, which wait for the source to change.
    #[serde(default)]
    pub failed: BTreeMap<String, Timestamp>,
}

impl GarbageRecords {
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        write_atomic(path, &serde_json::to_vec(self).expect("serializable"))
    }

    fn failure_key(pending: &Pending) -> String {
        format!("{}:{}", pending.derivation, pending.source)
    }

    pub fn has_failed(&self, pending: &Pending) -> bool {
        self.failed.contains_key(&Self::failure_key(pending))
    }

    pub fn record_failure(&mut self, pending: &Pending) {
        self.failed.insert(Self::failure_key(pending), time::now());
    }
}

/// Deletes the blobs this machine holds that haven't been in use for 30 days, and returns their
/// hashes. Entries with no holders and no use leave the `files` map.
pub fn collect_garbage(
    space: &mut Space,
    blobs: &Blobs,
    records: &mut GarbageRecords,
    now: Timestamp,
) -> Result<Vec<FileHash>> {
    let in_use: std::collections::BTreeSet<FileHash> = space.files_in_use().into_iter().collect();
    let files = space.doc().get_map("files");
    let mut deleted = Vec::new();
    for key in files.keys() {
        let Ok(hash) = key.parse::<FileHash>() else { continue };
        if in_use.contains(&hash) {
            records.since.remove(&key.to_string());
            continue;
        }
        let since = *records.since.entry(key.to_string()).or_insert(now);
        if now - since < GARBAGE_WAIT {
            continue;
        }
        if blobs.has(hash) {
            blobs.remove(hash)?;
            deleted.push(hash);
        }
        if space.file(hash).is_some_and(|f| f.holders.contains_key(&space.machine())) {
            space.set_holding(hash, false)?;
        }
        if space.file(hash).is_some_and(|f| f.holders.is_empty()) {
            files.delete(&key)?;
            space.commit();
        }
        records.since.remove(&key.to_string());
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_files_as_notes_and_derives_fields() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::new(dir.path().join("blobs"));
        let mut space = Space::create(Id::generate()).unwrap();
        let source = dir.path().join("report.pdf");
        fs::write(&source, b"%PDF fake").unwrap();
        let (hash, size) = blobs.add_file(&source).unwrap();
        assert_eq!(hash, FileHash::of(b"%PDF fake"));
        let note = add_file_note(&mut space, hash, size, "report.pdf", "application/pdf").unwrap();
        assert_eq!(space.title(note).unwrap().plain, "report.pdf");
        assert_eq!(space.note(note).unwrap().type_name, "file");

        let pending = pending_derivations(&space, &blobs);
        assert_eq!(pending.len(), 2);
        let text = pending.iter().find(|p| p.derivation == "text").unwrap();
        assert!(complete_derivation(&mut space, &blobs, text, b"the text", "text/plain").unwrap());
        assert_eq!(pending_derivations(&space, &blobs).len(), 1);

        let image =
            add_file_note(&mut space, blobs.add_bytes(b"png").unwrap(), 3, "a.png", "image/png")
                .unwrap();
        assert_eq!(space.note(image).unwrap().type_name, "image");
    }

    #[test]
    fn collects_garbage_after_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::new(dir.path());
        let mut space = Space::create(Id::generate()).unwrap();
        let hash = blobs.add_bytes(b"old").unwrap();
        let note = add_file_note(&mut space, hash, 3, "old.txt", "text/plain").unwrap();
        space.delete_note(note).unwrap();
        let mut records = GarbageRecords::default();
        assert_eq!(collect_garbage(&mut space, &blobs, &mut records, 0).unwrap(), Vec::new());
        assert!(blobs.has(hash));
        assert_eq!(
            collect_garbage(&mut space, &blobs, &mut records, GARBAGE_WAIT).unwrap(),
            vec![hash]
        );
        assert!(!blobs.has(hash));
        assert!(space.file(hash).is_none());
    }

    #[test]
    fn checks_incoming_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::new(dir.path());
        let (path, mut file) = blobs.incoming().unwrap();
        file.write_all(b"right").unwrap();
        assert!(!blobs.finish_incoming(&path, FileHash::of(b"wrong")).unwrap());
        let (path, mut file) = blobs.incoming().unwrap();
        file.write_all(b"right").unwrap();
        assert!(blobs.finish_incoming(&path, FileHash::of(b"right")).unwrap());
    }
}
