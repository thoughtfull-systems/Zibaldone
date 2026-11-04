# ZIB0011 Concurrency in the core

## Context

[Decision ZIB0007](ZIB0007-programming-languages-and-technologies.md) puts most of Zib in a Rust
core: storage, sync, the search index, rendering, and the behavior behind every screen. A lot of
that work waits or takes time. Sync waits on the network, files stream in and out of disk, the index
updates after each change, and rendering a diagram or a PDF takes a noticeable moment.

The apps that call the core each have one thread that owns their user interface: GTK's main loop on
Linux and the main thread on Android. Nothing that waits or takes time may run there, or the app
freezes. Meanwhile the core has to push changes to the apps as they happen, such as a note that
arrived through sync or a sync that finished.

The sync library from [decision ZIB0006](ZIB0006-sync-of-spaces-and-files.md), iroh, runs on the
[Tokio] asynchronous runtime. Linux and Android come first, and a browser may follow later.

## Criteria

- **Responsive apps.** No call from an app blocks its interface thread on the network, the disk, or
  heavy work.
- **One model.** Every app calls the core and hears from it the same way, so concurrency rules live
  in the core, not in each app.
- **Fits the libraries.** The model works with iroh and iroh-blobs without adapters between
  runtimes.
- **Natural in each language.** Kotlin and the GTK app each use the core through their own idioms
  for asynchronous work.
- **Room for a browser.** The choice doesn't rule out a WebAssembly build later.

## Decision

Run the core on a Tokio runtime that the core owns, and expose its operations to the apps as
asynchronous functions, with changes pushed back through listeners.

- **The runtime.** The core starts a multi-threaded Tokio runtime when the instance starts, on
  threads of its own, and stops it when the instance exits. Sync, file transfers, disk access, and
  indexing run there. Work that keeps a processor busy, such as parsing a long note, extracting a
  PDF's text, or rendering diagrams and pages, runs on Tokio's pool for blocking work.
- **One owner per space.** A single task owns each open space's Loro document. Operations on a space
  reach it as messages, in order, so changes from the owner and from sync never race, and nothing
  else locks the document.
- **Calls from the apps.** Every core operation is an `async` function. UniFFI turns each into a
  Kotlin `suspend` function, which Android calls from a coroutine. The GTK app awaits them on its
  main loop, through futures spawned on GLib's main context.
- **Changes to the apps.** Screen models and other state that changes on its own notify listeners
  that each app registers, as decision ZIB0007 anticipates. The core calls listeners from its own
  threads, and each app passes the change to its interface thread: Android through a Kotlin flow,
  and GTK through a channel that the main loop reads.
- **Typing.** Each keystroke goes to its space's task, which applies it and returns quickly, so the
  editor never waits on sync, indexing, or rendering. Those follow once the owner pauses, not after
  every commit, so fast typing doesn't repeat the work.
- **A browser later.** The core reaches the runtime only through a small module of its own, for
  spawning tasks, timers, and blocking work. A WebAssembly build can replace that module with one
  built on the browser's event loop, without changing the rest of the core.

Against the criteria:

- **Responsive apps:** every operation is asynchronous and runs on the core's threads, so an app's
  interface thread only starts calls and receives results.
- **One model:** the core decides where every piece of work runs, and the apps only await calls and
  receive changes.
- **Fits the libraries:** iroh and iroh-blobs run on Tokio, the same runtime as everything else.
- **Natural in each language:** Kotlin gets `suspend` functions and flows, and Rust's GTK bindings
  await futures on the main loop.
- **Room for a browser:** the runtime sits behind one module that a browser build can replace.

## Rejected alternatives

- **A synchronous core, with each app running calls on threads of its own.** The core would be
  simpler to write. But each app would decide what runs where, in Kotlin and in Rust, and sync still
  needs a runtime somewhere. It fails one model and fits the libraries.
- **GLib's main loop as the core's runtime.** The GTK app would need no bridge. But Android has no
  GLib, and iroh doesn't run on it. It fails one model and fits the libraries.
- **A different runtime, such as `smol`.** It's smaller than Tokio. But iroh depends on Tokio, so
  the core would run two runtimes side by side. It fails fits the libraries.
- **Shared locks around each document instead of one task per space.** Any thread could edit a
  document directly. But edits from the owner and from sync would contend for the lock, and the
  order of changes would depend on timing. It does worse on responsive apps and one model.

## Consequences

- The core depends on Tokio, and every operation it exposes is `async`, which UniFFI supports
  through its Tokio integration.
- Each app needs a small bridge from the core's listeners to its interface thread.
- Work for one space runs in order on its task, so a slow operation on a space delays the next one
  on that space. Slow work moves to the blocking pool and reports back when it finishes.
- A browser build needs its own version of the runtime module, and sync in a browser waits on iroh
  and iroh-blobs supporting WebAssembly.

[tokio]: https://tokio.rs/
