//! The two channels of Zib, which install side by side and never share data, as decision ZIB0013
//! describes.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The variable that gives a development instance a root directory of its own.
pub const ROOT_VAR: &str = "ZIB_DEV_ROOT";

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

    /// The directory that holds every file of an isolated development instance, from `ROOT_VAR`, as
    /// decision ZIB0015 describes. Production never reads the variable.
    pub fn root(self) -> Option<PathBuf> {
        match self {
            Self::Production => None,
            Self::Development => root_from(env::var_os(ROOT_VAR)),
        }
    }

    /// The name for the window and tray, with an isolated instance's root after it.
    pub fn title(self) -> String {
        self.title_with(self.root().as_deref())
    }

    fn title_with(self, root: Option<&Path>) -> String {
        match root {
            Some(root) => {
                let name = root.file_name().map_or_else(
                    || root.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                format!("{} ({name})", self.display_name())
            }
            None => self.display_name().into(),
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
    /// defines them, or under the root of an isolated development instance.
    pub fn for_channel(channel: Channel) -> Result<Self> {
        if let Some(root) = channel.root() {
            return Ok(Self::under(root));
        }
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

/// A root from the variable's value, made absolute so the `zib` command and the instance it starts
/// agree on it. An empty value counts as none.
fn root_from(value: Option<OsString>) -> Option<PathBuf> {
    let value = value.filter(|v| !v.is_empty())?;
    Some(std::path::absolute(&value).unwrap_or_else(|_| value.into()))
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

    #[test]
    fn isolated_instances_have_roots_of_their_own() {
        assert_eq!(Channel::Production.root(), None);
        assert_eq!(root_from(None), None);
        assert_eq!(root_from(Some("".into())), None);
        let root = root_from(Some("/tmp/zib/a".into())).unwrap();
        assert_eq!(root, Path::new("/tmp/zib/a"));
        assert_eq!(root_from(Some("a".into())).unwrap(), env::current_dir().unwrap().join("a"));
        let dirs = Dirs::under(&root);
        assert_eq!(dirs.runtime, Path::new("/tmp/zib/a/run"));
        assert_eq!(Channel::Development.title_with(Some(&root)), "Zibaldone Dev (a)");
        assert_eq!(Channel::Development.title_with(None), "Zibaldone Dev");
    }
}
