# 0006 Sync of spaces and files

## Context

Earlier decisions leave the sync transport and protocol to this one:

- [Decision 0001](0001-storage-format-for-spaces.md) stores each space as a Loro document, whose
  updates any channel can carry.
- [Decision 0003](0003-space-identity-and-machine-settings.md) puts each space's identifier in its
  document, so sync can offer a space by identifier. It leaves the names of other machines to sync.
- [Decision 0004](0004-storage-and-sync-of-files.md) stores files as blobs named by BLAKE3 hashes,
  outside the document. Sync has to fetch a single blob on demand from whichever machine holds it,
  check it as it arrives, and keep the `files` map free of machines that left the space.

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
- **Simple pairing.** Adding a machine to a space takes no typing. Most of the time the machines
  share a local network, so the common case is to pick the other machine from a list and confirm.

## Decision

Sync over [iroh], a Rust library that dials encrypted connections by public key, and transfer files
with [iroh-blobs]. Record each space's machines in the space's document. A later specification
defines the protocol.

- **Machine keys.** Each machine makes an Ed25519 key pair the first time it syncs and keeps the
  secret key in its own store. The public key is the machine's address. iroh runs Transport Layer
  Security (TLS) 1.3 inside each connection, keyed by the two machines' key pairs, so each side
  proves it holds the secret key for its address.
- **Members.** A root `machines` map in the space's document lists the space's machines, keyed by
  the machine identifiers from decision 0003. Each entry holds the machine's public key and its
  name. A machine names itself when it joins, and the owner can rename any machine from any machine
  in the space.
- **Trust.** A machine syncs a space only with machines that its own copy of the space lists in
  `machines`, and refuses any other connection for that space. Joining one space never grants access
  to another.
- **Nearby pairing.** Most machines join a space this way. While the owner has the Add machine
  screen open on two machines on the same network, each announces its public key and name with
  multicast Domain Name System (DNS), and lists the others that do. Either machine can pick the
  other. The member picks which of its spaces to share, both machines show a six-digit code derived
  from both public keys, and the owner confirms on the other machine once the codes match. The
  member then adds the new machine to `machines` and sends it each space. A machine ignores pairing
  requests unless its Add machine screen is open.
- **Invites.** For a machine on another network, or a browser, which can't use multicast DNS, a
  member makes an invite: a link that holds its address, the space's identifier, and a random
  secret, which works once, for 15 minutes. It shows the link as a QR code for a phone to scan, and
  as text to open on another machine. The new machine connects and sends the secret, and the
  inviting machine adds the new one to `machines` and sends it the space.
- **After pairing.** The new machine's owner gives each space a pet name, and from then on, the new
  machine syncs with every member of those spaces.
- **Removing a machine.** Any member can remove another from `machines`. Once the removal syncs, the
  remaining members refuse the removed machine, and each one deletes the removed machine's holder
  entries from the `files` map. A machine that leaves a space on its own removes itself first.
- **Documents.** A sync of a space is one exchange: each side sends its version vector, then the
  updates the other side lacks. Each side imports the updates into a copy first and checks them, so
  updates from a newer format, or from a document that isn't this space, never reach the space.
- **Files.** iroh-blobs fetches a blob by its hash and checks each chunk against BLAKE3's tree as it
  arrives, so a machine can fetch part of a file, such as a PDF's first pages. A machine with the
  `all` policy fetches every blob in `files` that it lacks. A machine with the `on demand` policy
  fetches a blob when the owner opens it, from any holder that's online. Every machine fetches every
  derived blob.
- **Connections.** iroh connects through a relay first, tries a direct path with hole punching, and
  moves the connection there once it works. Machines find each other by public key, through iroh's
  address lookup and, on a local network, through multicast DNS, so members on one network connect
  directly even without internet access. Outside the Add machine screen, a machine announces only
  its public key, never its name. A browser connects through relays only.
- **Relays.** By default, sync uses the public relays and address lookup of n0, the company behind
  iroh, which cost nothing and need no account. A relay learns which addresses connect, their IP
  addresses, and how much traffic passes, but no content. The owner can set their own relay instead.
- **When to sync.** While Zib runs, a machine accepts connections from the members of its spaces. It
  syncs with every member shortly after each local change, when it starts, when the network changes,
  after waking from sleep, and every 15 minutes. After a failed sync, it retries after 30 seconds,
  then doubles the wait each time, up to 10 minutes. On Android, a periodic background job syncs as
  often as Android allows. A browser syncs while the app is open.
- **Always-on machines.** The `zib` command can run as a background service on a desktop or a small
  server. It joins spaces as an ordinary member, with the `all` policy as a rule, so changes and
  files pass between machines that rarely run at the same time.

Against the criteria:

- **Private:** each connection runs TLS 1.3 with the machines' keys, which encrypts and
  authenticates every connection, and relays forward packets they can't decrypt.
- **Per-space membership:** a machine syncs a space only with the keys its own copy of that space
  lists.
- **Self-contained:** `machines` lives in the document, so a restored backup knows its members, and
  a new member learns every one of them with the space.
- **Reachable:** relays reach every machine that can make an outgoing connection, browsers included,
  and hole punching makes most other connections direct.
- **No server to run:** n0's public relays work out of the box, and the owner can switch to their
  own. An always-on machine is optional.
- **Files on demand:** iroh-blobs fetches single blobs by hash from any holder and verifies them as
  they stream.
- **Simple pairing:** machines on one network pair with a tap and a glance at two codes. Elsewhere,
  a phone joins by scanning a code, and other machines by opening a link. Every member then syncs
  with the new one, with no pairing between each pair of machines.

## Rejected alternatives

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
- **Nearby pairing without a code.** Picking a machine from the list would be enough, but anyone on
  the same network, such as a café's, could announce a machine with a familiar name. Comparing codes
  proves the two screens show the same pair of keys. It fails the private criterion.
- **Pet names for machines.** Each machine could name the others as it likes, as it names spaces.
  But a machine's name serves to answer which machine changed a note, which reads best when every
  machine gives the same answer. Machines rarely get renamed, so a shared name costs little.

## Consequences

- Decision 0001's outline gains the root `machines` map.
- Each implementation depends on iroh and iroh-blobs, through the Rust crates or bindings, and on an
  asynchronous runtime. They add a few megabytes to each build.
- Two machines sync only while both run. Without an always-on machine, a phone with the `on demand`
  policy can open only the files that a running machine holds.
- A member that holds a space can add any machine to it. Every member has the same trust, so a lost
  phone needs removing from each of its spaces.
- A removed machine keeps its copy of the space from before its removal. Removal stops future
  changes reaching it, but can't take back what it already has.
- Each implementation needs screens to pair nearby machines, make and accept invites, name machines,
  remove them, sync by hand, and set a relay. Android needs a camera to scan invites, and holds a
  multicast lock while it listens for nearby machines.
- A machine announces its public key on every local network it joins, so someone watching that
  network can tell when the same machine returns. Its name goes out only while the owner pairs.
- Address lookup through n0 publishes each machine's current relay under its public key, but nothing
  more.
- Machine stores now hold a secret key, which implementations keep out of backups that leave the
  machine.

[iroh]: https://www.iroh.computer/
[iroh-blobs]: https://github.com/n0-computer/iroh-blobs
