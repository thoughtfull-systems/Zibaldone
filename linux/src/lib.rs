//! Zibaldone on Linux: the instance that holds the machine's spaces, its window and tray icon, and
//! the `zib` command that talks to it, as the instance specification defines them.

pub mod adding;
pub mod app;
pub mod derive;
pub mod drawings;
pub mod editor;
pub mod icon;
pub mod instance;
pub mod remote;
pub mod sharing;
pub mod state;
pub mod status;
pub mod tray;
pub mod window;

use zib_core::Channel;

/// The channel this build belongs to. Builds from the source tree are development builds, so they
/// never touch the production data; the Nix package for everyday use turns on `production`.
pub const CHANNEL: Channel =
    if cfg!(feature = "production") { Channel::Production } else { Channel::Development };
