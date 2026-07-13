use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid registry: {0}")]
    InvalidRegistry(String),
    #[error("invalid plugin package: {0}")]
    InvalidPackage(String),
    #[error("package exceeds the {limit} byte size limit")]
    PackageTooLarge { limit: u64 },
    #[error("package checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("package {field} mismatch: expected {expected}, got {actual}")]
    IdentityMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("plugin is not installed: {0}")]
    NotInstalled(String),
}

pub(crate) fn io_error(path: PathBuf, source: io::Error) -> CoreError {
    CoreError::Io { path, source }
}
