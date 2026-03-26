use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::{CodexboxError, Result};

#[derive(Debug, Default, Deserialize)]
pub struct CodexToml {
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SandboxWorkspaceWrite {
    pub writable_roots: Option<Vec<PathBuf>>,
}

pub fn load_codex_toml(home_dir: &Path) -> Result<CodexToml> {
    let path = home_dir.join(".codex").join("config.toml");
    if !path.exists() {
        return Ok(CodexToml::default());
    }

    let contents = fs::read_to_string(&path).map_err(|source| CodexboxError::ReadPath {
        path: path.clone(),
        source,
    })?;

    toml::from_str(&contents).map_err(|source| CodexboxError::ParseToml { path, source })
}

pub fn existing_writable_roots(config: &CodexToml, home_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    for root in config
        .sandbox_workspace_write
        .as_ref()
        .and_then(|cfg| cfg.writable_roots.as_ref())
        .into_iter()
        .flatten()
    {
        let Some(expanded) = expand_tilde(root, home_dir) else {
            continue;
        };

        if !expanded.is_absolute() {
            eprintln!(
                "codexbox: skipping relative writable_root '{}' from ~/.codex/config.toml",
                expanded.display()
            );
            continue;
        }

        if !expanded.exists() {
            continue;
        }

        roots.push(canonicalize_if_possible(&expanded));
    }

    roots.sort();
    roots.dedup();
    roots
}

fn expand_tilde(path: &Path, home_dir: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return Some(home_dir.to_path_buf());
    }

    if let Some(stripped) = raw.strip_prefix("~/") {
        return Some(home_dir.join(stripped));
    }

    Some(path.to_path_buf())
}

fn canonicalize_if_possible(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{existing_writable_roots, CodexToml, SandboxWorkspaceWrite};

    #[test]
    fn writable_roots_expand_tilde_and_skip_missing() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let project = home.join("project");
        fs::create_dir_all(&project).unwrap();

        let config = CodexToml {
            sandbox_workspace_write: Some(SandboxWorkspaceWrite {
                writable_roots: Some(vec![
                    PathBuf::from("~/project"),
                    PathBuf::from("missing"),
                    home.join("ghost"),
                ]),
            }),
        };

        let roots = existing_writable_roots(&config, &home);

        assert_eq!(roots, vec![project.canonicalize().unwrap()]);
    }
}
