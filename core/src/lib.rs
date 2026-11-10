//! The core of Zibaldone, which every app builds on, as decision ZIB0007 describes.
//!
//! The core holds the only implementation of each format in `specs/`, and the behavior behind every
//! feature, so the apps only draw screens and connect platform services.

pub mod address;
pub mod agent;
pub mod base32;
pub mod body;
pub mod channel;
pub mod editing;
pub mod editor;
pub mod error;
pub mod files;
pub mod frame;
pub mod id;
pub mod instance;
pub mod lists;
pub mod markup;
#[cfg(feature = "net")]
pub mod net;
pub mod pairing;
pub mod pipe_tables;
#[cfg(feature = "print")]
pub mod print;
#[cfg(feature = "render")]
pub mod render;
pub mod schema;
pub mod search;
pub mod space;
pub mod spelling;
pub mod store;
pub mod sync;
pub mod time;
pub mod web_clip;

pub use channel::Channel;
pub use error::{Error, Result};
pub use id::Id;
pub use space::Space;
