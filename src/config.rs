use std::fs;
use std::path::{Path, PathBuf};

use nix::unistd::{getgid, getuid};

use crate::errors::{CodexboxError, Result};

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub ignore_var_patterns: Vec<String>,
    pub approval_db_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RuntimeAssets {
    pub project_root: PathBuf,
    pub vars_to_ignore_path: PathBuf,
    pub containerfile_path: PathBuf,
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

impl RuntimeAssets {
    pub fn detect() -> Result<Self> {
        let mut roots = Vec::new();

        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(parent) = current_exe.parent() {
                roots.push(parent.to_path_buf());
            }
        }

        roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

        if let Ok(current_dir) = std::env::current_dir() {
            roots.push(current_dir);
        }

        for root in roots {
            let vars_to_ignore_path = root.join("vars-to-ignore.txt");
            let containerfile_path = root.join("Containerfile");

            if vars_to_ignore_path.exists() && containerfile_path.exists() {
                return Ok(Self {
                    project_root: root,
                    vars_to_ignore_path,
                    containerfile_path,
                });
            }
        }

        Err(CodexboxError::MissingAsset(
            "Containerfile or vars-to-ignore.txt",
        ))
    }
}

pub fn load_launcher_config(assets: &RuntimeAssets, user: &UserContext) -> Result<LauncherConfig> {
    let ignore_var_patterns = read_ignore_patterns(&assets.vars_to_ignore_path)?;
    let approval_db_path = user.home_dir.join(".codexbox-conf.json");

    Ok(LauncherConfig {
        ignore_var_patterns,
        approval_db_path,
    })
}

fn read_ignore_patterns(path: &Path) -> Result<Vec<String>> {
    let contents = fs::read_to_string(path).map_err(|source| CodexboxError::ReadPath {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::read_ignore_patterns;

    #[test]
    fn read_ignore_patterns_skips_comments_and_blank_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vars-to-ignore.txt");
        fs::write(&path, "# comment\n\nFOO\nBAR*\n").unwrap();

        let patterns = read_ignore_patterns(&path).unwrap();

        assert_eq!(patterns, vec!["FOO", "BAR*"]);
    }
}
