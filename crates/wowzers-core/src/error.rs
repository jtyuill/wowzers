use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no CurseForge API key set (get one at console.curseforge.com)")]
    MissingApiKey,
    #[error("CurseForge rejected the API key (HTTP {0})")]
    BadApiKey(u16),
    #[error("CurseForge request failed: {0}")]
    Http(String),
    #[error("unexpected CurseForge response: {0}")]
    Decode(String),
    #[error("{0}")]
    Archive(String),
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    #[error(
        "{0} cannot be downloaded through the API (the author disabled third-party distribution)"
    )]
    DistributionDisabled(String),
    #[error("not a World of Warcraft folder: {}", .0.display())]
    NotWow(PathBuf),
}

impl Error {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
