# ZIB0013 Development and production channels

## Context

The owner relies on Zib every day and also changes it. A build with a bug in it must never reach the
spaces the owner depends on, and the owner must always know which build is in front of them. Both
builds have to install side by side, on Linux through NixOS and on Android, and run at the same
time.

[Decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md) keeps each machine's data in
directories of its own, and the [instance specification](../specs/instance.md) runs one instance per
machine, which other processes find through a lock and a socket. Two builds that shared those would
fight over them.

## Criteria

- **Separate data.** The development build never reads or writes the production build's spaces,
  machine store, cache, or runtime files.
- **Side by side.** Both builds install and run at the same time, each with its own commands,
  launcher entry, and tray icon.
- **Obvious.** The owner knows which build is in front of them from a glance at an icon, a launcher,
  or a window.
- **Safe by default.** A build from the source tree is the development build unless someone asks for
  production on purpose.

## Decision

Build Zib in two channels, production and development, which differ only in names, colors, and
directories.

| What                 | Production                      | Development                           |
| -------------------- | ------------------------------- | ------------------------------------- |
| Name people see      | Zibaldone                       | Zibaldone Dev                         |
| App identifier       | `systems.thoughtfull.Zibaldone` | `systems.thoughtfull.Zibaldone.Devel` |
| Commands on Linux    | `zib`, `zibaldone`              | `zib-dev`, `zibaldone-dev`            |
| Directories on Linux | `zibaldone`                     | `zibaldone-dev`                       |
| Color                | Blue, `#3a6ea5`                 | Orange, `#e06c00`                     |

- **Directories.** Each channel keeps its data, cache, and runtime files in directories named for it
  under the X Desktop Group (XDG) base directories, so the two never share a space, a machine
  identity, or a lock.
- **Looks.** The development build's icon is orange with a white bar under its letter, in the tray,
  the launcher, and its window, and its name says "Dev."
- **Links.** Only the production build handles `zib:` links, so a link never opens in the wrong
  build.
- **Choosing.** The core takes the channel as a value, and the Linux crate's `production` build
  feature selects it. Without the feature, a build is the development build, so `cargo run` and the
  tests can't touch production data. The Nix package for production turns the feature on.
- **Android.** The development build adds `.dev` to the app identifier, which gives it storage of
  its own, and uses the same name and colors. The Android app applies this when it exists.
- **Machines and spaces.** The two channels are separate machines, with separate identities and
  keys. The development build can pair with other development builds, and with production builds,
  only through a space the owner chooses to share with it.

Against the criteria:

- **Separate data:** every directory, the lock, and the socket carry the channel's name.
- **Side by side:** commands, identifiers, and desktop entries differ, so both install together.
- **Obvious:** color, a mark on the icon, and the name all differ.
- **Safe by default:** production takes a build feature that only its Nix package sets.

## Rejected alternatives

- **One build with a setting or environment variable for the data directory.** It needs one build
  only, but a forgotten variable runs a development change on production data. It fails safe by
  default.
- **Separate user accounts for development.** They keep data apart, but the owner would switch
  accounts to try a change, and the tray of one account doesn't show the other. It fails side by
  side.
- **Production as the default build, with development as the feature.** It matches how most projects
  build releases. But a bare `cargo run` would then open production data. It fails safe by default.

## Consequences

- Every path, identifier, and name that differs by channel comes from the channel value, never a
  string written in place.
- To test with a space in development, the owner shares it with the development machine like any
  other machine, and that copy can sync changes back into production.
- The Nix flake builds two packages, `zibaldone` and `zibaldone-dev`.
