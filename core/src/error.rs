use crate::SourceId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A backend could not produce its package list.
    #[error("{source_id}: {message}")]
    Backend { source_id: SourceId, message: String },
    #[error("index error: {0}")]
    Index(#[from] rusqlite::Error),
    /// A package handed to the index belongs to a different source than the one being replaced.
    #[error("package from source {found} given while replacing source {expected}")]
    SourceMismatch { expected: SourceId, found: SourceId },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
