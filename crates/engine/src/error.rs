//! Engine error type.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("unsupported or corrupt file: {0}")]
    BadFile(String),

    #[error("DST-compressed DSD is not supported in v1")]
    DstUnsupported,

    #[error("sink does not accept native DSD (rate {rate} Hz); no DoP fallback in v1")]
    DsdNotAccepted { rate: u32 },

    #[error("PipeWire error: {0}")]
    PipeWire(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

impl EngineError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
