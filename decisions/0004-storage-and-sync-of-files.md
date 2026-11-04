# 0004 Storage and sync of files

## Context

Zib holds binary files, PDFs and images among them, beside its notes.
[Decision 0001](0001-storage-format-for-spaces.md) stores a space's notes in one Loro document, and
leaves the bytes of files to this decision. A note describes a file, and other notes can show or
link to it, as an image in a note's body or a book's cover.

Files are far larger than notes, and a space can collect gigabytes of them over the years. A laptop
can hold them all, but an Android phone or a web app in a browser should keep only some of them and
fetch the rest when the owner opens one. Even so, every file needs at least one machine that keeps
it, and a machine that drops a file to save space must not drop the last copy.

## Criteria

- **Partial copies.** A machine can keep any subset of a space's files and fetch the others when the
  owner opens them.
- **No lost files.** A machine never drops a file unless another machine still holds it.
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
- **Notes for files.** Every file has a note of the built-in `file` type, which holds the file's
  name, and a field of a new kind, a file, whose value is the blob's hash. The note's body describes
  the file. Other types can hold files in fields of the same kind, and a note's body can show or
  link to a file by its hash, with the syntax a later decision on links defines.
- **The `files` map.** A root `files` map in the space's document holds an entry for each blob,
  keyed by its hash: its size, its media type, the hashes of its derived blobs, and the machines
  that hold it. Each machine adds its own identifier, from
  [decision 0003](0003-space-identity-and-machine-settings.md), once it holds the whole blob and has
  checked its hash, and removes it when it drops the blob. Each machine writes only its own key, so
  these writes never conflict.
- **What a machine keeps.** Each machine's store holds a policy for each space. The `all` policy has
  the machine fetch and keep every blob. The `on demand` policy fetches a blob when the owner opens
  it, keeps it in a cache up to a size the owner sets, and drops the blobs it opened least recently
  when the cache fills. The owner can also keep particular files on a machine, whatever its policy.
  Linux defaults to `all`, and Android and the web default to `on demand`.
- **Dropping a blob.** A machine drops a blob only while the `files` map lists another machine that
  holds it. A blob created on a machine stays there until another machine holds it, whatever the
  cache's size.
- **Derived blobs.** The first machine that can read a file's text writes it to a blob of its own,
  and the first that can draw a small preview image writes that too. Every machine fetches every
  derived blob, whatever its policy, so it can search and preview every file. Each machine indexes
  the text as [decision 0002](0002-note-types-and-schemas.md) indexes `text` fields.
- **Collecting garbage.** A machine deletes a blob, and removes its own identifier from the blob's
  entry, once no note in its copy of the space has referred to the blob for 30 days. The wait gives
  machines that haven't synced yet time to add a reference back. An entry with no references and no
  holders leaves the map.

Against the criteria:

- **Partial copies:** the `on demand` policy keeps a cache of the files the owner uses, and the
  `files` map lists the other machines a machine can fetch the rest from.
- **No lost files:** a machine drops a blob only while the map lists another holder, and a new blob
  stays put until another machine has it.
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
- **Derived text inside the document.** Every machine would get every file's text with the notes,
  but long texts would swell the document and its history. It does worse on small documents, and a
  derived blob gives the same result.

## Consequences

- Decision 0001's outline gains the root `files` map, and decision 0002's kinds gain the file kind.
- The decision on sync needs to transfer blobs as well as document updates, verify them as they
  arrive, and fetch from whichever machine holds a blob. [iroh-blobs], which uses BLAKE3, is a
  candidate.
- The `files` map can list a machine that no longer exists, so a file can look safer than it really
  is. Removing a machine from a space needs to remove it from every entry.
- On a machine with the `on demand` policy, a file that no other machine is online to send can't
  open. A machine that keeps every file and stays online, such as a desktop or a small server, makes
  every file reachable.
- Each implementation needs text extractors and preview renderers for the media types it supports,
  and a machine without one leaves the work to another.
- Spaces never share blobs, so a file in two spaces takes space twice on a machine that holds both.

[blake3]: https://github.com/BLAKE3-team/BLAKE3
[iroh-blobs]: https://github.com/n0-computer/iroh-blobs
