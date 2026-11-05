# ZIB0016 Sharing spaces with paired machines

## Context

The [pairing specification](../specs/pairing.md) shares spaces only while two machines pair: each
owner picks spaces then, and each side adds the other to them. After that, the owner has no way to
share another space with a machine it paired with, short of pairing again.

A machine also can't learn about a space it doesn't hold. The [sync specification](../specs/sync.md)
has a listener refuse an exchange for a space it doesn't hold, so writing a machine into a space's
`machines` doesn't reach it.

## Criteria

- **Any space, any time.** The owner shares any space it holds with any machine it paired with,
  without pairing again.
- **Trust from pairing.** Only a machine whose owner compared codes with this one can hand it a
  space.
- **Owner names spaces.** A machine that gains a space asks its owner for a pet name, as pairing
  does.
- **Additive.** The change only adds to the specifications, as
  [decision ZIB0012](ZIB0012-specifications-and-how-they-change.md) requires.

## Decision

Each machine remembers the machines it paired with, and a new `offer` message hands one of them a
space. Sharing only adds machines: the app offers no way to remove one from a space.

- **Paired machines.** When pairing finishes, each side records the other machine's identifier, key,
  name, and policy in its machine store, even when neither side shared a space. Each implementation
  keeps the list in its store as it chooses, as
  [decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md) leaves the store's format to
  it.
- **Sharing.** Each space has a list of machines in the app: its members, and the paired machines
  that aren't members, each with a button to share the space. Sharing adds the machine to `machines`
  in one commit, as pairing does, and offers it the space.
- **Offering.** The member opens a `zib/sync` stream to the machine and sends `offer`, with `space`
  and its own `machine`. The machine accepts only when the connection's key belongs to a machine in
  its list of paired machines. It replies `offered`, receives the space as pairing receives a shared
  space, and asks its owner for a pet name. A machine that already holds the space replies `offered`
  and does nothing more.
- **Retrying.** When an exchange with a member fails because the member doesn't hold the space, and
  the member is a paired machine, the next exchange with it starts with `offer` instead. A machine
  that's off when the owner shares a space then receives it once it's back.
- **No removal.** The app never sets `removed` on a machine's entry. A space's members only grow.

Against the criteria:

- **Any space, any time:** a button shares each space with each paired machine.
- **Trust from pairing:** a machine accepts `offer` only from a key it paired with, so a machine
  that joined one space by invite can't push others onto it.
- **Owner names spaces:** an accepted offer asks for a pet name, as pairing does.
- **Additive:** the sync specification gains `offer` and `offered`, and nothing in the existing
  messages or keys changes meaning.

## Rejected alternatives

- **Pairing again for each space.** It needs no new message, but the owner compares codes every
  time. It fails any space, any time.
- **Invites between paired machines.** An invite already hands over one space, but the owner copies
  a link from one screen to the other each time. It fails any space, any time.
- **Accepting an offer from any member of a shared space.** It needs no list of paired machines, but
  a machine that joined one space by invite could then hand this machine any space. It fails trust
  from pairing.
- **Removing machines from a space.** The space document specification's `removed` mark would let
  the owner stop sharing a space with a machine, for good. A new space does the same job with
  nothing new to build, so the app doesn't offer removal.

## Consequences

- The sync specification gains a section on offering, with `offer` and `offered`.
- The pairing specification adds that each side records the other as a paired machine.
- Machines paired before this change aren't in each other's lists. The owner pairs them again once.
- To stop sharing with a machine, the owner makes a new space, moves the notes into it, and shares
  it only with the machines to keep. The old space stays on the dropped machine as it last synced.
- A lost or stolen machine keeps syncing every space it shares until the owner moves the notes out
  of those spaces.
