use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CodexboxError>;

#[derive(Debug, Error)]
pub enum CodexboxError {
    #[error("failed to read {path}: {source}")]
    ReadPath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write {path}: {source}")]
    WritePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse TOML from {path}: {source}")]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to parse JSON from {path}: {source}")]
    ParseJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to serialize approval database: {0}")]
    SerializeJson(#[from] serde_json::Error),

    #[error("invalid ignore pattern '{pattern}': {source}")]
    InvalidIgnorePattern {
        pattern: String,
        #[source]
        source: globset::Error,
    },

    #[error("could not determine the invoking user's home directory")]
    MissingHomeDir,

    #[error("podman command failed to start: {0}")]
    PodmanSpawn(#[source] std::io::Error),

    #[error("podman image build failed with status {0}")]
    PodmanBuildFailed(String),

    #[error("podman image import failed for {path} with status {status}")]
    PodmanLoadFailed { path: PathBuf, status: String },

    #[error("interactive approval prompt failed: {0}")]
    PromptIo(#[source] std::io::Error),

    #[error("{0}")]
    SystemTime(String),
}
