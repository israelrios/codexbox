use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::{CodexboxError, Result};
use crate::path_utils::{canonicalize_if_possible, expand_tilde};

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
        let expanded = expand_tilde(root, home_dir);

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
