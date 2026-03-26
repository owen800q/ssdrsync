use thiserror::Error;

#[derive(Error, Debug)]
pub enum SsdrError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Source path does not exist: {path}")]
    SourceNotFound { path: String },

    #[error("Nix error: {0}")]
    Nix(#[from] nix::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SsdrError>;
