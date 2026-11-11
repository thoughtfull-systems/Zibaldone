# ZIB0014 Building, testing, and packaging

## Context

[Decision ZIB0007](ZIB0007-programming-languages-and-technologies.md) puts Zib's logic in a Rust
core, with a native app on each platform. The owner installs Zib on NixOS and runs its tests while
changing it, so both the build for tests and the build for installing should be as fast as they can
be. [Decision ZIB0013](ZIB0013-development-and-production-channels.md) asks for two packages, one
per channel.

## Criteria

- **Fast tests.** A change to the core rebuilds and tests in seconds, and the first build in a fresh
  shell doesn't take long either.
- **Fast installs.** Rebuilding the NixOS packages after a change rebuilds Zib's own crates, not
  every dependency.
- **Checked commits.** A commit fails unless its code follows the formatter and passes the linter.
- **Small toolbox.** The tools come from the development environment and Nix, with nothing to
  install by hand.

## Decision

Build a Cargo workspace with one crate for the core and one for each platform's app, test it with
nextest, and package it with a Nix flake that uses crane.

- **Workspace.** `core` holds the `zib-core` crate, and `linux` holds the `zib-linux` crate with the
  `zib` and `zibaldone` executables. Later platforms add crates of their own beside them. The core's
  networking sits behind its `net` feature, on by default, so a quick test run can leave it out.
- **Linking.** The development environment and the Nix packages link with mold, which links far
  faster than the default linker.
- **Profiles.** Development builds keep only line tables for Zib's crates and no debug information
  for dependencies, which they build with light optimization once. Release builds skip link-time
  optimization, which would re-optimize every dependency on each install, so a rebuild of the
  packages after a change to Zib takes about a minute.
- **SQLite.** Linux builds link the system's SQLite, which Nix provides with FTS5, instead of
  compiling it. The core's `bundled-sqlite` feature compiles it for platforms without one, such as
  Android.
- **Tests.** `cargo nextest run --workspace` runs the tests, in parallel, each in its own process.
- **Worktrees.** The development shell uses sccache for dependencies from Cargo's registry and Git
  caches. Tests and `zib-dev` share the compiler cache across worktrees. Each worktree keeps its own
  `target/` directory and compiles Zib's crates incrementally.
- **Nix.** `flake.nix` defines the `zibaldone` and `zibaldone-dev` packages and a `tests` check.
  Crane builds the dependencies once into a derivation of their own, which every package and the
  tests reuse, so a change to Zib rebuilds only Zib's crates. The flake's overlay adds both packages
  to a NixOS configuration.
- **Hooks.** The git hooks run `rustfmt`, `clippy` with every warning denied and Clippy's pedantic
  lints, and `taplo` for Tom's Obvious, Minimal Language (TOML) files. Their settings live in
  `.config/rustfmt.toml` and `.config/taplo.toml`, and the lints in the workspace's `Cargo.toml`.

Against the criteria:

- **Fast tests:** mold, lean debug information, and nextest keep the edit and test loop short.
- **Fast installs:** crane's separate dependency derivation stays in the Nix store between builds.
- **Checked commits:** the hooks fail any commit that isn't formatted or doesn't pass Clippy.
- **Small toolbox:** devenv provides Rust, mold, nextest, SQLite, and the hooks, and the flake
  provides the rest.

## Rejected alternatives

- **`rustPlatform.buildRustPackage` without crane.** It's built into nixpkgs and needs no extra
  input. But it rebuilds every dependency whenever any source file changes. It fails fast installs.
- **SQLite compiled into every build.** One build would serve every platform. But it compiles
  SQLite's C source in every fresh build and every Nix build. It does worse on fast tests and fast
  installs, so only Android takes it.
- **A shared Cargo build directory.** Cargo can reuse a local crate from another worktree with
  different source files. Separate build directories let each worktree test its own changes.
- **`cargo test` instead of nextest.** It needs no extra tool. But it runs each test binary's tests
  in one process and reports failures in less detail. It does worse on fast tests.

## Consequences

- The Linux app depends on GTK 4, libadwaita, and GtkSourceView, which the development environment
  and the packages provide. It reads PDFs with the `pdftotext` and `pdftoppm` tools, which the
  packages put on the app's path.
- `flake.lock` pins nixpkgs and crane for the packages, apart from `devenv.lock`, which pins the
  development environment.
- The first commit with Rust and TOML files adds their hooks, as AGENTS.md requires.
- A later Android build turns on `bundled-sqlite` and builds the core for Android's targets.
