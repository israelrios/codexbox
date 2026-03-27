use std::fs;
use std::path::{Path, PathBuf};

use nix::unistd::{getgid, getuid};
use serde::Deserialize;

use crate::errors::{CodexboxError, Result};

const VARS_TO_IGNORE: &str = include_str!("../vars-to-ignore.txt");

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub ignore_var_patterns: Vec<String>,
    pub approval_db_path: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct WorkspaceCodexboxConfig {
    #[serde(default)]
    pub publish: Vec<String>,
    #[serde(default)]
    pub add_dirs: Vec<PathBuf>,
}

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

pub fn load_launcher_config(user: &UserContext) -> LauncherConfig {
    LauncherConfig {
        ignore_var_patterns: parse_ignore_patterns(VARS_TO_IGNORE),
        approval_db_path: user.home_dir.join(".codexbox-conf.json"),
    }
}

pub fn load_workspace_codexbox_config(cwd: &Path) -> Result<WorkspaceCodexboxConfig> {
    let path = cwd.join(".codex").join("codexbox.json");
    if !path.exists() {
        return Ok(WorkspaceCodexboxConfig::default());
    }

    let contents = fs::read_to_string(&path).map_err(|source| CodexboxError::ReadPath {
        path: path.clone(),
        source,
    })?;

    serde_json::from_str(&contents).map_err(|source| CodexboxError::ParseJson { path, source })
}

fn parse_ignore_patterns(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use std::fs;

    use super::{load_workspace_codexbox_config, parse_ignore_patterns, WorkspaceCodexboxConfig};

    #[test]
    fn read_ignore_patterns_skips_comments_and_blank_lines() {
        let patterns = parse_ignore_patterns("# comment\n\nFOO\nBAR*\n");

        assert_eq!(patterns, vec!["FOO", "BAR*"]);
    }

    #[test]
    fn workspace_codexbox_config_defaults_when_missing() {
        let dir = tempdir().unwrap();

        let config = load_workspace_codexbox_config(dir.path()).unwrap();

        assert_eq!(config, WorkspaceCodexboxConfig::default());
    }

    #[test]
    fn workspace_codexbox_config_reads_publish_and_add_dirs() {
        let dir = tempdir().unwrap();
        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("codexbox.json"),
            r#"{
  "publish": ["127.0.0.1:8080:80", "8443:443"],
  "add_dirs": ["../shared", "/tmp/cache"]
}
"#,
        )
        .unwrap();

        let config = load_workspace_codexbox_config(dir.path()).unwrap();

        assert_eq!(config.publish, vec!["127.0.0.1:8080:80", "8443:443"]);
        assert_eq!(
            config.add_dirs,
            vec![PathBuf::from("../shared"), PathBuf::from("/tmp/cache")]
        );
    }
}
