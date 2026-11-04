//! The errors that the core reports.

use std::path::PathBuf;

/// A failure in the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Text that should hold an identifier, an address, or a link doesn't.
    #[error("invalid {what}: {text:?}")]
    Invalid { what: &'static str, text: String },
    /// A file on disk couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Another process holds a lock that this one needs.
    #[error("{0} is locked by another process")]
    Locked(PathBuf),
    /// Loro refused an operation or an import.
    #[error("document: {0}")]
    Loro(String),
    /// No note, space, or machine has the identifier asked for.
    #[error("not found: {0}")]
    NotFound(String),
    /// Another machine sent something the protocol doesn't allow.
    #[error("protocol: {0}")]
    Protocol(String),
    /// The search index failed.
    #[error("search index: {0}")]
    Search(#[from] rusqlite::Error),
    /// A space's document breaks a rule of the space document specification.
    #[error("space: {0}")]
    Space(String),
    /// Reading or writing a stream failed.
    #[error("stream: {0}")]
    Stream(#[from] std::io::Error),
}

/// A result with the core's error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn invalid(what: &'static str, text: impl Into<String>) -> Self {
        Self::Invalid { what, text: text.into() }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}

impl From<loro::LoroError> for Error {
    fn from(error: loro::LoroError) -> Self {
        Self::Loro(error.to_string())
    }
}

impl From<loro::LoroEncodeError> for Error {
    fn from(error: loro::LoroEncodeError) -> Self {
        Self::Loro(error.to_string())
    }
}
