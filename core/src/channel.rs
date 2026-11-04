//! The two channels of Zib, which install side by side and never share data, as decision ZIB0013
//! describes.

use std::env;
use std::path::PathBuf;

use crate::{Error, Result};

/// Which build of Zib is running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    /// The build the owner relies on every day.
    Production,
    /// The build for trying changes, with data of its own.
    Development,
}

impl Channel {
    /// The name of the channel's directories and runtime files.
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Production => "zibaldone",
            Self::Development => "zibaldone-dev",
        }
    }

    /// The application identifier, for desktop entries, D-Bus, and Android.
    pub const fn app_id(self) -> &'static str {
        match self {
            Self::Production => "systems.thoughtfull.Zibaldone",
            Self::Development => "systems.thoughtfull.Zibaldone.Devel",
        }
    }

    /// The name people see, which marks the development build.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Production => "Zibaldone",
            Self::Development => "Zibaldone Dev",
        }
    }

    /// The `zib` command's name in this channel.
    pub const fn command(self) -> &'static str {
        match self {
            Self::Production => "zib",
            Self::Development => "zib-dev",
        }
    }

    /// The name of the instance's executable in this channel.
    pub const fn instance(self) -> &'static str {
        match self {
            Self::Production => "zibaldone",
            Self::Development => "zibaldone-dev",
        }
    }

    /// The color that marks the channel in icons, as `0xRRGGBB`.
    pub const fn accent(self) -> u32 {
        match self {
            Self::Production => 0x3a_6e_a5,
            Self::Development => 0xe0_6c_00,
        }
    }
}

/// The directories where one channel keeps its files on Linux.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dirs {
    /// Spaces, their files, and the machine store.
    pub data: PathBuf,
    /// Derived data that the instance can rebuild: search indexes and rendered images.
    pub cache: PathBuf,
    /// The lock and socket of the running instance.
    pub runtime: PathBuf,
}

impl Dirs {
    /// The channel's directories under the XDG base directories, as the instance specification
    /// defines them.
    pub fn for_channel(channel: Channel) -> Result<Self> {
        let home = || {
            env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| Error::NotFound("the home directory".into()))
        };
        let base = |var: &str, fallback: &str| -> Result<PathBuf> {
            match env::var_os(var).filter(|v| !v.is_empty()) {
                Some(dir) => Ok(PathBuf::from(dir)),
                None => Ok(home()?.join(fallback)),
            }
        };
        let name = channel.dir_name();
        let runtime = match env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir).join(name),
            None => PathBuf::from(format!("/tmp/{name}-{}", user_id())),
        };
        Ok(Self {
            data: base("XDG_DATA_HOME", ".local/share")?.join(name),
            cache: base("XDG_CACHE_HOME", ".cache")?.join(name),
            runtime,
        })
    }

    /// Directories inside `root`, for tests and for running a throwaway copy.
    pub fn under(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self { data: root.join("data"), cache: root.join("cache"), runtime: root.join("run") }
    }
}

/// The user's numeric identifier, from the owner of the home directory's runtime files.
fn user_id() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map_or(0, |m| m.uid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_never_share_directories() {
        let production = Channel::Production;
        let development = Channel::Development;
        assert_ne!(production.dir_name(), development.dir_name());
        assert_ne!(production.app_id(), development.app_id());
        assert_ne!(production.command(), development.command());
        assert_ne!(production.accent(), development.accent());
    }
}
