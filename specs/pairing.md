# Pairing

This specification defines how a machine joins a space: finding nearby machines, pairing them with a
six-digit code, pairing by a typed address, and joining by invite. It follows
[decision ZIB0006](../decisions/ZIB0006-sync-of-spaces-and-files.md). The
[sync specification](sync.md) defines endpoints and the framing of messages, and the
[space document specification](space-document.md#machines) defines `machines`.

## Messages

Pairing runs on the `zib/pair` protocol of a machine's iroh endpoint, one bidirectional stream per
attempt. Its messages are frames of Concise Binary Object Representation (CBOR) maps, as the sync
specification's messages are, except that a frame holds at most 65,536 bytes. Each side closes the
stream after an `error` or a `cancel`, and a new attempt opens a new stream.

## Machine addresses

A machine's address is its 32-byte Ed25519 public key, the `key` in its entry in `machines`, in a
form people can read aloud and type.

- **Encoding.** A writer treats the 32 bytes as one 256-bit unsigned number, adds four zero bits
  beyond its highest bit to make 260 bits, and writes the 52 groups of 5 bits from the highest to
  the lowest, as digits of the same Crockford's Base32 alphabet that the
  [identifiers specification](identifiers.md#text-form) uses. The first character is therefore `0`
  or `1`.
- **Showing.** Apps show an address in uppercase, in 13 groups of 4 characters separated by hyphens.
- **Hints.** An address may end with `@` and a comma-separated list of network addresses to try
  before looking the key up, each an IPv4 address or a bracketed IPv6 address, then `:` and a port,
  such as `@192.168.1.20:41641`. The app shows the machine's current addresses after its key, on a
  line of their own.
- **Reading.** A reader removes white space and hyphens, turns letters to lowercase, reads `i` and
  `l` as `1` and `o` as `0`, as for identifiers, and splits off any hints at the `@`. It rejects a
  key that isn't 52 digits of the alphabet with the first one `1` or lower.

```text
0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000
0001-081G-8186-0W40-J2GB-1G6G-W3RG-2491-650N-2RBH-G68T-3CE1-T7GZ
1ZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ
```

Those are the addresses of the keys whose bytes are all zero, the bytes 0 to 31 in order, and all
ones.

## Finding nearby machines

- **Always.** Every running instance announces its public key on each local network it joins, with
  multicast Domain Name System (DNS), through iroh's local address lookup, so members on one network
  connect directly. This announcement carries no name.
- **While adding a machine.** While the owner has the Add machine screen open, the machine also
  announces itself under the service name `zib-pair`, with its name, in UTF-8, as the user data. It
  lists the other machines it hears under that service name, by name, and stops announcing when the
  screen closes.

## Pairing with a code

Either machine on the Add machine screen can pick the other from its list, or the owner can enter
the other's address on it. The machine that picks, the initiator, connects to the other, the
responder, under `zib/pair`. The responder refuses any request unless its own Add machine screen is
open. The two exchange these messages in order:

- **Request.** The initiator sends `request`, with `machine`, its identifier, `name`, `policy`, and
  `commitment`: the 32-byte [BLAKE3] hash of a random 32-byte value, its nonce, which it keeps
  secret for now.
- **Nonce.** The responder replies `nonce`, with its own `machine`, `name`, and `policy`, and
  `nonce`, a random 32-byte value of its own.
- **Reveal.** The initiator sends `reveal`, with its `nonce`. The responder checks that the hash of
  that nonce equals the `commitment`, and sends `error` and closes the stream if it doesn't.
- **Code.** Each side computes the code: the BLAKE3 hash of the text `zib pairing code`, in UTF-8,
  then the initiator's public key, the responder's public key, the initiator's nonce, and the
  responder's nonce, all as bytes. It reads the hash's first 8 bytes as an unsigned big-endian
  integer, takes the remainder after dividing by 1,000,000, and shows it as six decimal digits, with
  leading zeros.
- **Confirm.** Each side shows the code and the other machine's name, and asks its owner whether the
  codes on the two screens match. When the owner confirms, it sends `confirm`. When the owner
  declines, or doesn't answer within 2 minutes, it sends `cancel`.
- **Share.** Once each side has sent and received `confirm`, each sends `share`, with `spaces`: a
  list of the identifiers of the spaces its owner chose to share with the other machine, which may
  be empty. A side doesn't share a space it doesn't hold.

For each space that a side shares, it adds the other machine to the space's `machines` in one
commit, with the other's `key`, `name`, and `policy`, and then syncs the space with it as the sync
specification defines. The receiving machine asks its owner for a pet name for each space it gains.

A machine in the middle has to send its `commitment` before it learns the responder's nonce, and
receives the initiator's nonce only after the responder sent its own, so it can't choose nonces that
make the two codes match. Its one guess succeeds one time in a million, and a failed guess shows the
owner two different codes.

## Invites

An invite lets a machine join one space without being nearby. The new machine opens a link or scans
a quick response (QR) code.

- **Making one.** A member makes an invite for one space. It picks a random 16-byte secret, and
  remembers the secret, the space, and the time until someone uses it or 15 minutes pass.

- **The link.** An invite is a URI with the member's address, without hyphens, the space's
  identifier, and the secret, written as 26 digits of Crockford's Base32, as an identifier is, then
  any hints:

  ```abnf
  invite-uri = "zib:invite/" key "/" identifier "/" secret [ "?hints=" hint *( "," hint ) ]
  key        = 52digit
  secret     = 26digit
  ```

  `identifier` and `digit` are as the identifiers specification defines them, and `hint` is a
  network address as in [Machine addresses](#machine-addresses). Readers read the key, the
  identifier, and the secret as they read identifiers, ignoring case.

- **Showing.** The member shows the link as text to copy, and as a QR code that holds the link in
  uppercase, which QR codes store more compactly.

- **Joining.** The machine that opens the link connects to the member under `zib/pair` and sends
  `invite`, with `secret`, `space`, its own `machine`, `name`, and `policy`. The member checks that
  it made an invite with that secret for that space in the last 15 minutes, and that no one used it,
  and sends `error` if not. Otherwise it marks the invite used, adds the machine to the space's
  `machines` in one commit, replies `joined`, with `space`, and syncs the space with it.

## Platforms

- **Linux.** Every pairing method works.
- **Android.** Every pairing method works. The app scans invites with the camera, and holds a
  multicast lock while its Add machine screen is open.
- **Browser.** A web app, when one exists, can't use multicast DNS, so it joins by invite or pairs
  by address only.

[blake3]: https://github.com/BLAKE3-team/BLAKE3
