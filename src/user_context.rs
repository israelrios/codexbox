use std::path::PathBuf;

use nix::unistd::{getgid, getuid};

use crate::errors::{CodexboxError, Result};

#[derive(Debug, Clone)]
pub struct UserContext {
    pub uid: u32,
    pub gid: u32,
    pub home_dir: PathBuf,
    pub cwd: PathBuf,
}

impl UserContext {
    pub fn detect() -> Result<Self> {
        let home_dir = home::home_dir().ok_or(CodexboxError::MissingHomeDir)?;
        let cwd = std::env::current_dir().map_err(|source| CodexboxError::ReadPath {
            path: PathBuf::from("."),
            source,
        })?;

        Ok(Self {
            uid: getuid().as_raw(),
            gid: getgid().as_raw(),
            home_dir,
            cwd,
        })
    }
}
