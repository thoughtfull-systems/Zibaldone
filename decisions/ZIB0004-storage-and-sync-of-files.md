# ZIB0004 Storage and sync of files

## Context

Zib holds binary files, PDFs and images among them, beside its notes.
[Decision ZIB0001](ZIB0001-storage-format-for-spaces.md) stores a space's notes in one Loro
document, and leaves the bytes of files to this decision. A note describes a file, and other notes
can show or link to it, as an image in a note's body or a book's cover.

Files are far larger than notes, and a space can collect gigabytes of them over the years. A laptop
can hold them all, but an Android phone or a web app in a browser should keep only some of them and
fetch the rest when the owner opens one. Even so, every file needs at least one machine that keeps
it, and a machine that drops a file to save space must not drop the last copy.

## Criteria

- **Partial copies.** A machine can keep any subset of a space's files and fetch the others when the
  owner opens them.
- **No lost files.** A machine never drops a file unless a machine that keeps every file confirms it
  holds it, even while machines leave the space, lose their disks, or drop files at the same time.
- **Verifiable.** A machine can check that bytes it receives are the file it asked for, whichever
  machine sent them, and can check part of a file before the rest arrives.
- **Small documents.** A file's bytes stay out of the space's document and its history, so the
  document stays small enough for every machine to hold whole.
- **Searchable without the bytes.** A machine can search a file's text and show a preview of it
  without holding the file.
- **Every platform.** Linux, Android, and a web app can store and fetch files, offline for those
  they hold.
- **No server.** Files sync between the owner's machines without a server of their own.

## Decision

Store each file as an immutable blob, named by the [BLAKE3] hash of its bytes, outside the space's
document. A later specification defines the exact encoding, and the decision on sync defines how
machines transfer blobs.

- **Blobs.** A blob never changes. Changing a file makes a new blob with a new hash, and the note
  points to the new one. Annotations and other changes the app makes to a file live in notes, not in
  the file. Two notes that hold the same bytes share one blob.
- **Where blobs live.** Each machine keeps the blobs of each space apart from its other spaces, in a
  place the implementation chooses: a directory on Linux, the app's private storage on Android, and
  the Origin Private File System (OPFS) in a browser.
- **Notes for files.** Every file has a note of the built-in `file` type, with a field of a new
  kind, a file, whose value is the blob's hash. The note's body starts with the file's name, which
  serves as its title, and goes on to describe the file. Other types can hold files in fields of the
  same kind, and a note's body can show or link to a file through its note, as
  [decision ZIB0005](ZIB0005-note-identifiers-and-links.md) defines.
- **The `files` map.** A root `files` map in the space's document holds an entry for each blob,
  keyed by its hash: its size, its media type, the hashes of its derived blobs, and the machines
  that hold it. Each machine adds its own identifier, from
  [decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md), once it holds the whole blob
  and has checked its hash, and removes it when it drops the blob. Each machine writes only its own
  key, so these writes never conflict.
- **What a machine keeps.** Each machine has a fixed policy, which its implementation sets and which
  never changes. The space's document records it in the machine's entry in the `machines` map, as
  [decision ZIB0006](ZIB0006-sync-of-spaces-and-files.md) defines it, so every machine knows which
  others keep every file. The `all` policy has the machine fetch and keep every blob. The
  `on demand` policy fetches a blob when the owner opens it, keeps it in a cache up to a size the
  owner sets, and drops the blobs it opened least recently when the cache fills. The owner can also
  keep particular files on a machine, whatever its policy. The Linux app and the `zib` command use
  `all`, and Android and the web use `on demand`.
- **Dropping a blob.** Only a machine with the `on demand` policy drops a blob that a note refers
  to. A machine with the `all` policy deletes blobs only by collecting garbage. Before it drops a
  blob, a machine connects to a holder that the space lists with the `all` policy, and asks whether
  it holds the blob. It drops the blob only when the holder confirms it. The `files` map alone isn't
  enough, because its holders may since have left the space or lost their disks. If no such holder
  is online, the machine keeps the blob and asks again later, even when its cache overflows.
- **New blobs.** A blob created on a machine with the `on demand` policy stays there until a machine
  with the `all` policy confirms it holds the blob, whatever the cache's size.
- **Derived blobs.** The first machine that can read a file's text writes it to a blob of its own,
  and the first that can draw a small preview image writes that too. Every machine fetches every
  derived blob, whatever its policy, so it can search and preview every file. Each machine indexes
  the text as [decision ZIB0002](ZIB0002-note-types-and-schemas.md) indexes `text` fields.
- **Collecting garbage.** A machine deletes a blob, and removes its own identifier from the blob's
  entry, once no note in its copy of the space has referred to the blob for 30 days. The wait gives
  machines that haven't synced yet time to add a reference back. An entry with no references and no
  holders leaves the map.

Against the criteria:

- **Partial copies:** the `on demand` policy keeps a cache of the files the owner uses, and the
  `files` map lists the other machines a machine can fetch the rest from.
- **No lost files:** a machine drops a blob only after an `all` machine confirms, at that moment,
  that it holds the blob, and `all` machines never drop blobs that notes refer to. A stale `files`
  map can't lead a machine to drop the last copy.
- **Verifiable:** a BLAKE3 hash names each blob, and BLAKE3's tree structure lets a machine check
  each chunk of a file as it arrives, so it can show a PDF's first pages before the rest arrives.
- **Small documents:** the document holds a few hundred bytes per file, and no file's bytes.
- **Searchable without the bytes:** derived blobs hold the text and a preview of each file, and
  every machine keeps them.
- **Every platform:** each platform stores blobs with its own storage, and BLAKE3 has a Rust crate
  that builds for every target, the browser through WebAssembly included.
- **No server:** blobs travel over the same sync between machines as the documents.

## Rejected alternatives

- **Files inside the space's document, as binary values.** One sync would carry everything, but the
  document's history would keep every version of every file forever, and every machine would hold
  every byte. It fails the small documents and partial copies criteria.
- **Secure Hash Algorithm (SHA) 256.** Every platform has SHA-256 built in, the browser's Web Crypto
  interface included. But a SHA-256 hash covers the whole file, so a machine can't check a part of a
  large file until the whole file arrives. It does worse on the verifiable criterion, and BLAKE3 is
  also faster.
- **Random identifiers for files instead of hashes.** A file could keep its identifier through
  edits, but a machine couldn't check the bytes it receives, two copies of a file would take twice
  the space, and an edit on two machines would conflict with no merge. It fails the verifiable
  criterion.
- **A file sync tool, such as Syncthing, for a folder of files.** It works well between laptops, but
  it needs another app on every machine, it can't run in a browser, and it can't fetch one file on
  demand. It fails the every platform and partial copies criteria.
- **Cloud object storage, such as Amazon S3.** Any machine could fetch any file at any time, without
  another machine running. But someone has to run or pay for the storage. It fails the no server
  criterion as the only path, though a later decision could add it as an optional holder.
- **Every machine keeps every file.** It's the simplest rule and never loses a file, but a phone
  can't hold years of PDFs. It fails the partial copies criterion.
- **Dropping blobs without tracking holders.** A cache that drops whatever it likes needs no `files`
  map, but if every machine drops the same file, it's gone. It fails the no lost files criterion.
- **Dropping while the `files` map lists another holder.** It needs no connection to drop a blob,
  but the map can be out of date. Two `on demand` machines can each see the other as a holder and
  drop the same blob at once, and a listed holder may have lost the blob since. It fails the no lost
  files criterion.
- **Derived text inside the document.** Every machine would get every file's text with the notes,
  but long texts would swell the document and its history. It does worse on small documents, and a
  derived blob gives the same result.

## Consequences

- Decision ZIB0001's outline gains the root `files` map, and decision ZIB0002's kinds gain the file
  kind.
- The decision on sync needs to transfer blobs as well as document updates, verify them as they
  arrive, and fetch from whichever machine holds a blob. [iroh-blobs], which uses BLAKE3, is a
  candidate.
- The `files` map can list a machine that no longer exists, so a file can look safer than it really
  is. Removing a machine from a space needs to remove it from every entry.
- On a machine with the `on demand` policy, a file that no other machine is online to send can't
  open. A machine that keeps every file and stays online, such as a desktop or a small server, makes
  every file reachable.
- A space with no `all` machine never drops a file from an `on demand` machine, so its caches can
  grow past their size. Each implementation warns the owner when a space has no `all` machine.
- A machine can't change its policy. Keeping every file on a phone, or only some on a laptop, takes
  a change to its implementation, which this decision would record.
- An `on demand` machine drops a blob only while an `all` machine is online to confirm, so its cache
  can overflow while that machine is off.
- One `all` machine holds the only guaranteed copy of a file. If it loses its disk before another
  `all` machine fetches the file, the space loses the file unless an `on demand` machine still has
  it.
- An `on demand` machine that drops a blob, then stays offline for more than 30 days before adding a
  reference to it, finds that the `all` machines collected the blob as garbage.
- Each implementation needs text extractors and preview renderers for the media types it supports,
  and a machine without one leaves the work to another.
- Spaces never share blobs, so a file in two spaces takes space twice on a machine that holds both.

[blake3]: https://github.com/BLAKE3-team/BLAKE3
[iroh-blobs]: https://github.com/n0-computer/iroh-blobs
