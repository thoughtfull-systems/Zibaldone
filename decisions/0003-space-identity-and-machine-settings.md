# 0003 Space identity and machine settings

## Context

[Decision 0001](0001-storage-format-for-spaces.md) stores each space as one Loro document, with
settings that sync to every machine that shares the space. Each machine also has settings that
belong to it alone, such as window sizes, which spaces it shows, and how much disk it may use. These
never sync.

A machine can hold more than one space, and sync has to know which document on one machine pairs
with which document on another. Each space needs an identity for that. Its name can't serve: each
machine gives each space a name of its own, a pet name, and renaming a space on one machine never
changes its name on another. The owner might call a space Work on a laptop and Job on a phone.

Machines also need an identity, because decision 0001 records which machine created and last changed
each note.

## Criteria

- **Unique.** Any machine can create a space or set itself up without coordinating with another, and
  no two spaces or machines share an identity.
- **Self-contained.** A space's document carries everything needed to restore the space, its
  identity included, so a backup of the document is a backup of the space.
- **Spaces can merge.** The owner can merge one space into another, and the result has one clear
  identity.
- **Names stay local.** Each machine names each space as its owner likes, and renaming it never
  changes or conflicts with its name on another machine.
- **Machine settings stay put.** A preference about one machine never reaches another.
- **Nothing to lose.** Losing a machine's own data, other than changes it hasn't synced yet, loses
  nothing that a sync or a rebuild can't restore.

## Decision

Store each space's identity in its document, so it syncs and backs up with the space. Keep each
space's pet name, and every machine setting, in a store that belongs to the machine and never syncs.

- **Space identifiers.** Creating a space gives it a random 128-bit identifier, which the creating
  machine writes to the `id` key of a root `meta` map in the space's document. Nothing changes it
  afterward, except a merge. Sync offers a space by its identifier, and a machine that receives one
  learns it again from the document's first update. A later decision on sync defines the details.
- **One document per identifier.** A machine holds at most one document for each identifier. A
  document it opens or receives with an identifier it already holds, such as a copy restored from a
  backup, is the same space, so the machine merges it into the one it has.
- **Merging spaces.** Merging one space into another imports the source's changes into the target,
  which brings the source's `id` along with the target's. The machine then writes the target's
  identifier to `id` again. That write comes after both, so it wins on every machine, and the
  machine removes the source.
- **Pet names.** Each machine stores its own name for each space. A machine asks for a name when it
  creates a space or receives one.
- **Machine identifiers.** Each machine creates a random 128-bit identifier the first time it runs
  and keeps it for as long as its store exists. The `meta` map of each note records it. A machine's
  identifier stays the same across every space it holds.
- **Machine settings.** The store holds the machine's preferences for each implementation, such as
  window sizes, whether each space shows, and the last space a note went into. Each implementation
  chooses the store's form, such as a file in the user's configuration directory on Linux or a
  preferences store on Android. Implementations that share a machine and a store, such as the Linux
  app and the `zib` command, share its format too.
- **Where a setting belongs.** A preference about one machine, its screen, or its owner's habits on
  it goes in the machine's store. A preference about a space's content that should hold on every
  machine goes in the space's `settings`. A preference about one note goes in the note's `settings`.
- **Derived data.** Search indexes, thumbnails, and caches live on the machine too, in its cache
  directory, apart from its settings. A machine can delete and rebuild them from its spaces at any
  time, so they never sync.

Against the criteria:

- **Unique:** 128 random bits make a collision between any two identifiers vanishingly unlikely,
  with no coordination.
- **Self-contained:** the identifier lives in the document, so restoring a backup of the document
  restores the space, and it syncs with the machines that hold the space as before.
- **Spaces can merge:** the target writes its own identifier after importing the source, so the
  merged space has the target's identity on every machine.
- **Names stay local:** names live only in machine stores.
- **Machine settings stay put:** the store never syncs, and no space carries its preferences.
- **Nothing to lose:** a machine that loses its store loses only its names and preferences. Its
  documents still know their identifiers, so it pairs again, and the first sync of each space merges
  cleanly, because the documents share history. Derived data rebuilds from the spaces.

## Rejected alternatives

- **An identifier in each machine's store, outside the document.** A copy of a document would never
  sync as the original, and a merge would have no identifiers to reconcile. But a document alone
  wouldn't identify its space, so a backup would need the machine's store or a note of the
  identifier beside it. It fails self-contained. Copying a document to start a new space, the case
  it guards against, isn't a way the owner works.
- **A name inside the space's document.** It would sync, so a new machine would show the space's
  name without asking. But two machines that rename a space at once conflict, and every machine
  would have to use the same name. It fails names stay local.
- **An identifier derived from the document, such as a hash of its first change.** It needs no
  storage, but a new space has no changes until someone writes one, and after a merge, two first
  changes compete. It depends on Loro's internals for no gain over storing the identifier.
- **Loro's peer identifiers as machine identifiers.** Loro already tags each change with a peer
  identifier. But two processes on one machine, such as the app and the `zib` command, must not
  share a peer identifier, and Loro's peer identifiers can change, so they can't name a machine.
- **Machine settings in a space of their own that never syncs.** The settings would use the same
  format as everything else. But they'd need a document that sync must always skip, and Loro's
  merging adds nothing to settings that only one machine ever writes. It adds a special case without
  meeting any criterion better.
- **One format for every machine store.** A shared format would let any implementation read any
  store. But no store ever leaves its machine, so only implementations on the same machine need to
  agree, and they already do.

## Consequences

- Sync pairs machines per space by identifier, and the decision on sync defines how one machine
  offers a space to another and how the other accepts it, names it, and records its identifier.
- Decision 0001's outline gains a root `meta` map, which holds the space's identifier.
- A copy of a space's document is the same space, not a new one. Starting a new space means creating
  one, and a feature that copies a space as a template would have to give the copy a new identifier.
- Two documents that hold different identifiers after a merge, because a machine missed the final
  write, settle on the same identifier once they sync, since the final write wins.
- Each implementation needs a way to create, name, rename, and remove spaces, and to merge one space
  into another.
- Notes record machine identifiers, not names. Showing which machine changed a note needs names for
  other machines, which the decision on sync can provide.
- The Linux app and the `zib` command share one store and one format on each machine.
