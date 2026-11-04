# Sync

This specification defines how two members of a space connect, exchange the space's changes and
files, and confirm before one drops a file. It follows
[decision ZIB0006](../decisions/ZIB0006-sync-of-spaces-and-files.md) for sync and
[decision ZIB0004](../decisions/ZIB0004-storage-and-sync-of-files.md) for files. The
[space document specification](space-document.md) defines `machines` and `files`, and the
[pairing specification](pairing.md) defines how a machine becomes a member.

## Connections

- **Endpoints.** Each running instance has one [iroh] endpoint, whose secret key is the machine's
  Ed25519 secret key, kept in its machine store. Its public key is the `key` in the machine's entry
  in `machines`.
- **Protocols.** An endpoint accepts three protocols, each named by the string that the two sides
  agree on when a connection opens:
  - `zib/sync` for exchanging a space's changes and asking about files, as this specification
    defines;
  - the [iroh-blobs] protocol, under the string iroh-blobs defines, for transferring blobs;
  - `zib/pair` for pairing, as the pairing specification defines.
- **Finding machines.** A machine finds a member by its public key, through iroh's address lookup
  and, on a local network, through multicast Domain Name System (DNS). Outside pairing, a machine
  announces nothing but its public key.
- **Relays.** A machine uses the relays and address lookup of n0 unless its owner sets a relay of
  their own, a machine setting that the machine store holds.

## Messages

Each `zib/sync` exchange runs on its own bidirectional stream. Each message on a stream is a frame:
a 4-byte length, as an unsigned big-endian integer, followed by that many bytes of a map in
[Concise Binary Object Representation][cbor] (CBOR).

- **Keys.** Each map has a `type` key, whose value is a text string naming the message, and keys for
  its contents. Keys are text strings.
- **Unknown keys.** A receiver ignores keys it doesn't know, so a later addition can add keys to a
  message.
- **Features.** Each side lists the features it supports in its `hello`, and a side sends a message
  that a later addition defines only when the other side listed its feature.
- **Errors.** A side that receives something it can't accept sends an `error` message, with a text
  `reason`, and closes the stream.
- **Size.** A frame holds at most 1,073,741,824 bytes, about a billion. A side that receives a
  longer length closes the stream without reading it.

Values use these CBOR types:

| Value            | CBOR type                                                |
| ---------------- | -------------------------------------------------------- |
| Identifier       | Text string, in canonical form                           |
| Hash             | Byte string of 32 bytes                                  |
| Version vector   | Byte string, as Loro's `VersionVector::encode` writes it |
| Updates          | Byte string, as Loro's `export` writes updates           |
| Boolean, integer | Their own CBOR types                                     |

## Syncing a space

One machine, the dialer, opens a stream to another member, the listener, for one space. A connection
can carry a stream for each space the two share, and the streams can run at the same time. An
exchange runs in this order:

- **Hello.** The dialer sends `hello`, with `space`, its own `machine` identifier, `version`, its
  version vector of the space, and `features`, a list of text strings.
- **Membership.** The listener checks that it holds the space, that its own copy lists the `machine`
  that the dialer sent in `machines` without a `removed` mark, and that the entry's `key` is the key
  the connection authenticated. If any check fails, it sends `error` and closes the stream.
  Otherwise it replies with its own `hello`. The dialer makes the same checks on the listener's
  reply.
- **Updates.** Each side sends `updates` messages, each with `updates`, the changes the other side's
  version vector lacks, then sends `done`. A side may split its changes across more than one
  `updates` message, as long as each one imports on its own.
- **Checking.** Each side imports the updates it receives into a copy of the space first. It rejects
  the whole exchange, imports nothing, and sends `error` if the copy's `meta.id` differs from the
  space's identifier, or if a root container that the space document specification defines has the
  wrong type. Otherwise it imports the updates into the space.
- **End.** After both sides send `done` and finish checking, each closes its side of the stream.

A side records the other's version vector after each exchange, in its machine store, to show sync
status and to send only what the other lacks next time.

## Files

- **Serving.** A machine serves a blob over iroh-blobs only to a machine that's a member of a space
  whose `files` map lists the blob's hash, and only while it holds the blob whole.
- **Fetching.** A machine fetches a blob from any holder that's online, and verifies each chunk
  against the hash as it arrives. Once it holds the whole blob, it adds itself to the blob's
  `holders`.
- **What to fetch.** A machine with the `all` policy fetches every blob in use that it lacks. A
  machine with the `on-demand` policy fetches a blob when the owner opens it, and fetches every blob
  of a derived field, as decision ZIB0004 requires.

### Confirming before a drop

Before an `on-demand` machine drops a blob in use, it opens a `zib/sync` stream to a member that the
space lists with the `all` policy as a holder of the blob, and the two exchange these messages in
order:

- It sends `hold`, with `space`, its own `machine` identifier, and `hash`.
- The other machine checks membership as for syncing, then replies `holding`, with `hash`, and
  `holds`: `true` if it has the `all` policy and holds the blob whole, and `false` otherwise.
- The asking machine drops the blob only after it receives `holds: true`, and then removes itself
  from the blob's `holders`.

The machine keeps the blob and asks again later when no such holder is online, or when every one
replies `false`.

## When to sync

While an instance runs, it accepts streams from the members of its spaces. It starts an exchange of
a space with every member:

- every 30 seconds;
- when the instance starts, when the network changes, and after the machine wakes from sleep;
- at once, when the owner opens one of the space's notes to edit.

After a failed exchange, it waits 30 seconds before trying that member again, then doubles the wait
each time, up to 10 minutes. A successful exchange resets the wait.

## Live editing

While two machines have the same note open for editing, each sends the other every change to the
note's space as it happens.

- **Watching.** When the owner opens a note to edit, the machine opens a stream to each member of
  the space it can reach, and sends `watch`, with `space`, its own `machine` identifier, and `note`,
  the note's identifier. The other machine checks membership as for syncing.
- **Joining.** A machine that has the same note open for editing, or opens it later, replies on that
  stream with `watching`, with the same `note`. From then on, the stream is live in both directions.
- **Changes.** On a live stream, each side sends `updates` with each local change to the space as
  soon as it commits it, gathering changes for at most 50 milliseconds. The editor commits the
  owner's edits to the document as the owner types, at least that often.
- **Catching up.** Before its first live change, each side sends `updates` with what the other's
  latest recorded version vector lacks, so live changes always apply to a current note.
- **Ending.** When the owner closes the note, or opens another one, the machine sends `unwatch` and
  closes the stream. A stream that breaks ends live editing too, and the machine opens it again when
  the member comes back while the note is still open.
- **Checking.** A side checks each live `updates` message as it checks an exchange, before it
  imports it.
- **Cursors.** On a live stream, each side sends `cursor`, with `note` and `selections`, whenever
  the owner's cursor or selection in the note moves, gathering moves for at most 50 milliseconds,
  and again every 20 seconds while nothing moves. `selections` is a list of maps, each with `anchor`
  and `head`: byte strings, as Loro's `Cursor::encode` writes a position in the note's `body`, which
  stay attached to the same characters as the text around them changes. A cursor without a selection
  has `anchor` equal to `head`. The other side shows each selection in the note, labeled with the
  sending machine's `name` from `machines`, and removes it when live editing ends or when no
  `cursor` message arrives for 60 seconds. Cursors never enter the document or the machine store.

## Sync status

For each space, a machine compares what it made with what each member holds. A member's holdings are
the version vector it recorded at the end of its last exchange or live change with that member, and
the space's `holders` entries.

- **Not synced.** Some change made on this machine is in no other member's version vector, or some
  blob created on this machine has no other holder.
- **Partly synced.** Every such change is in at least one other member's version vector, and every
  such blob has another holder, but some member that isn't marked removed lacks a change, or some
  member with the `all` policy lacks a blob.
- **Synced.** Every member that isn't marked removed has every change made on this machine, and
  every member with the `all` policy holds every blob created on it.

The app's icon shows the least synced state across the machine's spaces, and opening it shows the
state of each space and when each member last synced.

## Platforms

- **Linux.** The instance syncs whenever it runs, and can run without a window as a background
  service.
- **Android.** The app syncs while it runs, and a periodic background job syncs as often as Android
  allows.
- **Browser.** A web app, when one exists, connects through relays only, and syncs while it's open.

[cbor]: https://www.rfc-editor.org/rfc/rfc8949
[iroh]: https://www.iroh.computer/
[iroh-blobs]: https://github.com/n0-computer/iroh-blobs
