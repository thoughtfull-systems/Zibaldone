# ZIB0006 Sync of spaces and files

## Context

Earlier decisions leave the sync transport and protocol to this one:

- [Decision ZIB0001](ZIB0001-storage-format-for-spaces.md) stores each space as a Loro document,
  whose updates any channel can carry.
- [Decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md) puts each space's identifier in
  its document, so sync can offer a space by identifier. It leaves the names of other machines to
  sync.
- [Decision ZIB0004](ZIB0004-storage-and-sync-of-files.md) stores files as blobs named by BLAKE3
  hashes, outside the document. Sync has to fetch a single blob on demand from whichever machine
  holds it, check it as it arrives, and keep the `files` map free of machines that left the space.

The owner runs Zib on Linux machines, an Android phone, and in a browser. Any two of them should
sync at home on one network or over the internet. A laptop sits behind a home router, and a phone on
mobile data sits behind carrier-grade Network Address Translation (NAT), so often neither machine
can accept a connection from the other. A browser can't accept connections at all. Some path then
has to forward traffic between them, and anything on that path must learn nothing from it.

A work machine shares the work space and must never receive the personal one.

## Criteria

- **Private.** Encryption and authentication cover every byte between machines, end to end. Nothing
  on the path, a relay included, can read or change a space, and only the space's machines can sync
  it.
- **Per-space membership.** Each space syncs with the machines its owner chose for it, and with no
  others.
- **Self-contained.** A space's document records which machines share it, so a backup restores the
  space's membership along with its identity.
- **Reachable.** Two machines sync whether they share a network or sit behind NAT on different ones,
  phones on mobile data and browsers included.
- **No server to run.** Sync works without a server the owner has to set up, and the owner can still
  use their own.
- **Files on demand.** A machine can fetch one file from any machine that holds it, and check each
  part as it arrives.
- **Simple pairing.** Adding a machine to a space takes no typing in the usual cases. Most of the
  time the machines share a local network, so the common case is to pick the other machine from a
  list and confirm.

## Decision

Sync over [iroh], a Rust library that dials encrypted connections by public key, and transfer files
over the same protocol as the space's changes. Record each space's machines in the space's document.
A later specification defines the protocol.

- **Machine keys.** Each machine makes an Ed25519 key pair the first time it syncs and keeps the
  secret key in its own store. The public key is the machine's address. iroh runs Transport Layer
  Security (TLS) 1.3 inside each connection, keyed by the two machines' key pairs, so each side
  proves it holds the secret key for its address.
- **Members.** A root `machines` map in the space's document lists the space's machines, keyed by
  the machine identifiers from decision ZIB0003. Each entry holds the machine's public key, its
  name, and its file policy from decision ZIB0004. A machine names itself when it joins, and the
  owner can rename any machine from any machine in the space.
- **Trust.** A machine syncs a space only with machines that its own copy of the space lists in
  `machines`, and refuses any other connection for that space. Joining one space never grants access
  to another.
- **Nearby pairing.** Most machines join a space this way. While the owner has the Add machine
  screen open on two machines on the same network, each announces its public key and name with
  multicast Domain Name System (DNS), and lists the others that do. Either machine can pick the
  other. The member picks which of its spaces to share, and the two machines connect and agree on a
  six-digit code, which both show. The owner confirms on each machine once the codes match. The
  member then adds the new machine to `machines` and sends it each space. A machine ignores pairing
  requests unless its Add machine screen is open.
- **Pairing codes.** The two machines derive the code from both public keys and a random value from
  each machine, exchanged so that neither side can choose its value after seeing the other's. The
  machine that picked the other sends a hash of its random value first, the other machine replies
  with its own random value, and the first then reveals its value, which the second checks against
  the hash. A machine in the middle has to commit to its values before it learns the real ones, so
  it gets one guess in a million at making the codes match, and a failed guess shows the owner two
  different codes. Every attempt uses new random values.
- **Invites.** For a machine on another network, or a browser, which can't use multicast DNS, a
  member makes an invite: a link that holds its address, the space's identifier, and a random
  secret, which works once, for 15 minutes. It shows the link as a QR code for a phone to scan, and
  as text to open on another machine. The new machine connects and sends the secret, and the
  inviting machine adds the new one to `machines` and sends it the space.
- **Pairing by address.** When multicast DNS can't reach the other machine, such as on a network
  that blocks it, the owner can type or paste the other machine's address on a member instead: its
  public key, shown and typed as 52 characters of Crockford's Base32 in uppercase, in groups of four
  separated by hyphens, optionally followed by network addresses to try first. Readers forgive case
  and the letters people confuse, as they do for identifiers. The member connects to it, and the two
  pair as nearby machines do, with the other machine's Add machine screen open and the same
  six-digit code.
- **After pairing.** The new machine's owner gives each space a pet name, and from then on, the new
  machine syncs with every member of those spaces.
- **Removing a machine.** Any member can remove another. Removal marks the machine's entry in
  `machines` as removed instead of deleting it, and nothing ever clears the mark. Once the removal
  syncs, members refuse the removed machine's key, even if that machine later writes to its entry
  again, and each member deletes the removed machine's holder entries from the `files` map. A
  machine that leaves a space on its own removes itself first. To rejoin, a removed machine pairs
  again as a new machine, which makes a fresh identifier and key pair.
- **Documents.** A sync of a space is one exchange: each side sends its version vector, then the
  updates the other side lacks. Each side imports the updates into a copy first and checks them, so
  updates from a document that isn't this space never reach the space.
- **Files.** A machine fetches a blob by its hash from a holder, which serves it only to members of
  a space that lists it, and checks the whole blob against its BLAKE3 hash before it keeps it. A
  machine with the `all` policy fetches every blob in `files` that it lacks. A machine with the
  `on demand` policy fetches a blob when the owner opens it, from any holder that's online. Every
  machine fetches the blob of every derived field. Before a machine drops a blob, it asks an `all`
  holder over the same connection to confirm that it holds the blob, as decision ZIB0004 requires.
- **Connections.** iroh connects through a relay first, tries a direct path with hole punching, and
  moves the connection there once it works. Machines find each other by public key, through iroh's
  address lookup and, on a local network, through multicast DNS, so members on one network connect
  directly even without internet access. Outside the Add machine screen, a machine announces only
  its public key, never its name. A browser connects through relays only.
- **Relays.** By default, sync uses the public relays and address lookup of n0, the company behind
  iroh, which cost nothing and need no account. A relay learns which addresses connect, their IP
  addresses, and how much traffic passes, but no content. The owner can set their own relay instead.
- **When to sync.** While Zib runs, a machine accepts connections from the members of its spaces. In
  the background, it takes its time: it syncs each space with every member every 30 seconds, and
  when it starts, when the network changes, and after waking from sleep. When the owner opens a note
  to edit, the machine syncs the note's space with every member at once, so the note is as current
  as it can be. After a failed sync, it retries after 30 seconds, then doubles the wait each time,
  up to 10 minutes. On Android, a periodic background job syncs as often as Android allows. A
  browser syncs while the app is open.
- **Live editing.** While two machines both have the same note open for editing, they send each
  other every change to its space as it happens, within a fraction of a second, so the owner watches
  one machine's typing appear on the other, much as in a shared online document. Each machine also
  shows the other's cursor and selection in the note, labeled with the other machine's name. Cursors
  never enter the document. Live editing ends when either closes the note.
- **Sync status.** Every app shows an icon with three states for the changes and files this machine
  made: not synced while any of them has reached no other member, partly synced while each has
  reached at least one other member but not every member, and synced once every member has every
  one. For files, every member with the `all` policy counts. The icon shows the least synced state
  across the machine's spaces. A machine learns what each member holds from the version vectors and
  holder entries it receives, so the icon changes as soon as a sync completes. Opening the icon
  shows the state of each space and when it last synced.
- **Always-on machines.** Zib can run without a window, as a background service on a desktop or a
  small server, which the owner starts and controls through the `zib` command. It joins spaces as an
  ordinary member, with the `all` policy, so changes and files pass between machines that rarely run
  at the same time.

Against the criteria:

- **Private:** each connection runs TLS 1.3 with the machines' keys, which encrypts and
  authenticates every connection, and relays forward packets they can't decrypt. A machine in the
  middle of nearby pairing has one chance in a million of matching codes, and only one try before
  the two screens show different codes.
- **Per-space membership:** a machine syncs a space only with the keys its own copy of that space
  lists. A removed machine stays removed, because no write clears the mark.
- **Self-contained:** `machines` lives in the document, so a restored backup knows its members, and
  a new member learns every one of them with the space.
- **Reachable:** relays reach every machine that can make an outgoing connection, browsers included,
  and hole punching makes most other connections direct.
- **No server to run:** n0's public relays work out of the box, and the owner can switch to their
  own. An always-on machine is optional.
- **Files on demand:** a machine fetches single blobs by hash from any holder and verifies them
  before it keeps them.
- **Simple pairing:** machines on one network pair with a tap and a glance at two codes. Elsewhere,
  a phone joins by scanning a code, and other machines by opening a link, or, where multicast DNS
  fails, by pasting an address. Every member then syncs with the new one, with no pairing between
  each pair of machines.

## Rejected alternatives

- **iroh-blobs for files.** It checks each chunk of a file against BLAKE3's tree as it arrives, so a
  machine could show a PDF's first pages before the rest. But its server answers for one store of
  blobs per connection, before it knows which space a request is for, so it can't keep each space's
  blobs apart or serve a blob only to members of the space that lists it. It fails per-space
  membership. Fetching on `zib/sync` checks membership for each blob, and checks the whole blob
  against its hash before keeping it.
- **Trust between each pair of machines, with share identifiers in machine stores.** It keeps
  membership out of the document, but a backup then loses who shares the space, and each new machine
  has to pair with every other one. It fails the self-contained and simple pairing criteria.
- **Plain TCP connections, encrypted with the Noise protocol.** It's small and private, but a
  machine behind NAT can't accept a connection, and a browser can't open a raw TCP connection at
  all. It fails the reachable criterion unless Zib builds its own relays.
- **WebRTC.** Browsers support it natively, and it punches holes through NAT. But it needs a
  signaling server to set up each connection, and its native libraries for Linux and Android are
  large and awkward to embed. It fails the no server to run criterion, and iroh's relays give
  browsers the same reach.
- **libp2p.** It also offers hole punching and relays, but it's a larger framework with more parts
  to configure, and its relays need more setup for the same result.
- **A file sync tool, such as Syncthing, carrying update files.** Loro updates merge in any order,
  so a shared folder of updates would work between laptops. But it can't run in a browser, can't
  fetch one file on demand, and shares every space in the folder. It fails the reachable, files on
  demand, and per-space membership criteria.
- **A cloud drive as a mailbox for encrypted updates.** Machines would sync without running at the
  same time, through storage the owner already has. But each provider needs its own interface and
  sign-in, and the provider learns when each space changes. It fails the no server to run criterion
  in spirit, since it depends on an account with a third party. A later decision could add it as an
  optional transport.
- **A virtual private network, such as Tailscale.** It solves reaching machines, but the owner has
  to run it on every machine, Android allows one virtual private network at a time, and a browser
  can't join one. It fails the reachable and no server to run criteria.
- **Deleting a removed machine's entry.** It keeps `machines` small, but the last write to each key
  wins, so a removed machine that syncs with a member that hasn't heard of its removal writes its
  entry again, and that write can spread to every member. It fails the per-space membership
  criterion.
- **Nearby pairing without a code.** Picking a machine from the list would be enough, but anyone on
  the same network, such as a café's, could announce a machine with a familiar name. Comparing codes
  proves the two screens show the same pair of keys. It fails the private criterion.
- **A code derived from the public keys alone.** It needs no exchange beyond the keys. But a machine
  in the middle can make new key pairs until the code for its key and one machine's key matches the
  code for another of its keys and the other machine's key. A six-digit code takes about a million
  tries, which a laptop makes in seconds. It fails the private criterion.
- **Pet names for machines.** Each machine could name the others as it likes, as it names spaces.
  But a machine's name serves to answer which machine changed a note, which reads best when every
  machine gives the same answer. Machines rarely get renamed, so a shared name costs little.

## Consequences

- Decision ZIB0001's outline gains the root `machines` map.
- Every implementation reaches iroh through the core, on the Tokio runtime that
  [decision ZIB0011](ZIB0011-concurrency-in-the-core.md) chooses. They add a few megabytes to each
  build.
- Two machines sync only while both run. Without an always-on machine, a phone with the `on demand`
  policy can open only the files that a running machine holds.
- A member that holds a space can add any machine to it. Every member has the same trust, so a lost
  phone needs removing from each of its spaces.
- A removed machine keeps its copy of the space from before its removal. Removal stops future
  changes reaching it, but can't take back what it already has.
- Each implementation needs screens to pair nearby machines, make and accept invites, pair by
  address, name machines, remove them, sync by hand, show sync status, and set a relay. Android
  needs a camera to scan invites, and holds a multicast lock while it listens for nearby machines.
- Live editing sends a message for each change while two machines share an open note, which costs
  little on a local network and a little more through a relay. Background sync every 30 seconds
  keeps connections to members open, or reopens them, while Zib runs.
- A machine announces its public key on every local network it joins, so someone watching that
  network can tell when the same machine returns. Its name goes out only while the owner pairs.
- Address lookup through n0 publishes each machine's current relay under its public key, but nothing
  more.
- Machine stores now hold a secret key, which implementations keep out of backups that leave the
  machine.

[iroh]: https://www.iroh.computer/
