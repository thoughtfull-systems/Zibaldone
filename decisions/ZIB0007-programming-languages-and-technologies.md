# ZIB0007 Programming languages and technologies

## Context

Zib runs on Linux, on an Android phone, and in a browser. Apple and Windows platforms may come
later. On Linux, the `zib` command controls the running app, which can also run without a window as
an always-on member of a space, as [decision ZIB0006](ZIB0006-sync-of-spaces-and-files.md)
describes.

Earlier decisions fixed the libraries that every implementation depends on, and each is a Rust
library:

- [Loro](ZIB0001-storage-format-for-spaces.md), for the spaces' documents.
- [BLAKE3](ZIB0004-storage-and-sync-of-files.md), the hash that names each file.
- [iroh](ZIB0006-sync-of-spaces-and-files.md), for sync.

Beyond those libraries, a lot of logic doesn't depend on the platform: the schema rules of
[decision ZIB0002](ZIB0002-note-types-and-schemas.md), the identifiers and wikilink parser of
[decision ZIB0005](ZIB0005-note-identifiers-and-links.md), the checks on incoming updates, the rules
for keeping and dropping blobs, and the search index.

The owner prefers a native app on each platform to one implementation that feels slightly off
everywhere.

## Criteria

- **Feature parity.** Every implementation offers the same features and the same screens, and
  behaves the same way, except where a platform's conventions or limits call for a difference.
- **Native feel.** Each app uses its platform's own toolkit and follows its conventions for
  navigation, text input, accessibility, sharing, and notifications.
- **One source of truth for logic.** Logic that doesn't depend on the platform gets written, tested,
  and fixed once.
- **Interoperability.** Every implementation reads the data every other one writes and syncs with
  it, with no room for two implementations to read a format differently.
- **Room for more platforms.** The choices don't rule out macOS, Windows, or iOS later.
- **Small toolbox.** A single maintainer, working with coding agents, can keep every language and
  toolchain in working order.

## Decision

Put as much of Zib as possible in one Rust library, the core, and build a native app on each
platform around it as a thin layer:

- **Core:** Rust. It holds the only implementation of each format, and owns spaces, schemas, links,
  the search index, files, and sync. It also owns the behavior behind every screen, so the apps
  don't each reimplement it:
  - **Features.** Every action the owner can take, such as creating, editing, moving, linking,
    pairing, or importing, is a core operation, with its validation, its undo, and its errors.
  - **Screen state.** For each screen, the core provides a model of what it shows and what the owner
    can do there: the notes in a list and their order, a note's title, fields, and backlinks, the
    results of a search, link suggestions while the owner types `[[`, and the progress of a sync.
    The app renders the model and sends the owner's actions back.
  - **Markdown.** The core parses note bodies into a tree that every app renders the same way, and
    applies edits to the Loro text.
  - **Rules for settings.** The core reads and writes space, note, and machine settings, including
    each setting's default and where it belongs.
- **Apps:** each app draws its screens with its platform's toolkit, follows its platform's
  conventions, and connects the core to the platform's services, such as notifications, sharing,
  file pickers, URI handlers, the camera, background jobs, and storage. The core calls those
  services through interfaces that each app implements, and never contains user interface code.
- **Linux:** a Rust app built with GTK 4 and libadwaita, which links the core directly.
- **`zib` command:** Rust. It sends commands to the running Linux app, and starts it first if none
  runs, as [decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md) requires.
- **Android:** a Kotlin app built with Jetpack Compose, which calls the core through bindings that
  [UniFFI] generates.
- **Web:** a TypeScript app, which loads the core as WebAssembly through `wasm-bindgen`. It chooses
  its own framework, if any.

Every implementation reaches Loro, BLAKE3, and iroh only through the core, never through a separate
package of its own. Adding a language or a platform toolkit to this list takes a revision of this
decision.

A feature lands in the core first, then in every app. When an app has to differ, because its
platform works another way or lacks a capability, such as the local discovery that a browser can't
do, the specification for that feature records the difference.

Against the criteria:

- **Feature parity:** every app exposes the same core operations and screen models, so features and
  behavior match by construction, and the specifications record the differences.
- **Native feel:** every user interface uses its platform's own toolkit, and the core's models
  describe what a screen shows, not how it looks.
- **One source of truth for logic:** the core holds the logic once, with one set of tests, down to
  the behavior of each screen.
- **Interoperability:** every implementation runs the same code to read and write the formats, so
  two can't disagree about them. The specifications stay the contract, so a later implementation
  could still write its own.
- **Room for more platforms:** UniFFI also generates Swift bindings, and Rust builds for macOS,
  Windows, and iOS, so a later Apple or Windows app reuses the same core.
- **Small toolbox:** three languages, one per platform family, and Rust does double duty as the core
  language and the Linux language.

## Rejected alternatives

- **Tauri,** a Rust backend with a web user interface. It covers Linux, Android, and the web with
  one interface in TypeScript and keeps the core in Rust, so it does well on feature parity, one
  source of truth, and small toolbox. But its interface lives in a web view on every platform, so it
  fails native feel, the outcome this decision exists to avoid.
- **Flutter, Compose Multiplatform, or React Native.** Each covers every platform with one
  interface, but draws its own widgets or bridges to them, and none feels native on both Linux and
  Android. They fail native feel, and each would need its own bridge to the Rust libraries.
- **A core limited to formats, storage, and sync.** Each app builds its own features and screens on
  a core that only reads, writes, and syncs. The core's interface stays small, but every list's
  order, every action's rules, and every screen's behavior get written once per app and drift apart.
  It fails feature parity and one source of truth for logic.
- **No shared core, with the Rust libraries called from each platform.** Each implementation would
  use Loro's and iroh's own packages or bindings. But every schema rule, link parser, and blob rule
  would exist once per platform, and the copies would drift. It fails one source of truth for logic
  and interoperability.
- **Kotlin Multiplatform for the shared logic.** It shares Kotlin naturally with Android, but the
  libraries Zib depends on are Rust, so it would wrap them anyway, and the Linux app would need a
  second language. It fails small toolbox.
- **A shared core in C or C++.** Every platform can call C, but the core would need manual memory
  management and handwritten bindings, and it would still have to call the Rust libraries. It fails
  small toolbox.

## Consequences

- The development environment provides the Rust toolchain, GTK 4, and libadwaita, and gains Android
  and web toolchains when those implementations begin.
- The first Rust files bring `rustfmt` and `clippy` hooks, with a 100-character line limit. Kotlin
  and TypeScript files bring their own formatters and linters in the same way.
- The core exposes an interface that UniFFI and `wasm-bindgen` can both express, which rules out
  some Rust idioms, such as generic types and borrowed references, at that boundary.
- The core's interface is large, since it covers every screen. Its screen models need a way to tell
  each app when they change, through callbacks that both bindings support.
- Most new features and many changes to a screen touch the core as well as each app. Tests of
  behavior live in the core, and the apps' tests cover rendering and platform services.
- Each feature's specification lists the ways each platform differs.
- The core builds for WebAssembly, so its dependencies must too. The search index's library has to
  build for a browser too, which [decision ZIB0008](ZIB0008-search-index.md) allows for.
- Android and the web carry the core's code and its libraries in every build, which adds a few
  megabytes.

[uniffi]: https://mozilla.github.io/uniffi-rs/
