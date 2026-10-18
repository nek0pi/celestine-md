use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("path is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("note not found: {0}")]
    NoteNotFound(String),
    #[error("invalid vault path: {0}")]
    InvalidVaultPath(String),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}
