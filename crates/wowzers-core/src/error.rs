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
    #[error("CurseForge request failed: {0}")]
    Http(String),
    #[error("unexpected CurseForge response: {0}")]
    Decode(String),
    #[error("{0}")]
    Archive(String),
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
