use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env_filter::EnvFilterConfig;
use crate::errors::{CodexboxError, Result};
use crate::user_context::UserContext;

const DEFAULT_BLOCKED_VAR_PATTERNS: &[&str] = &[
    "BASH*",
    "COLUMNS",
    "COMP_WORDBREAKS",
    "CONTAINER_CONNECTION",
    "CONTAINER_HOST",
    "CODEXBOX_*",
    "DBUS_SESSION_BUS_ADDRESS",
    "DEFAULTS_PATH",
    "DOCKER_*",
    "EDITOR",
    "EUID",
    "GPG_AGENT_INFO",
    "GROUPS",
    "GTK2_RC_FILES",
    "GTK_MODULES",
    "GTK_RC_FILES",
    "HISTCONTROL",
    "HIST*",
    "HOME",
    "INVOCATION_ID",
    "JOURNAL_STREAM",
    "LESSCLOSE",
    "LESSOPEN",
    "LIBRARY_ROOTS",
    "LINES",
    "LOCAL_IP",
    "LOGNAME",
    "LS_COLORS",
    "MACHTYPE",
    "MAILCHECK",
    "MANAGERPID",
    "MANDATORY_PATH",
    "*PWD*",
    "OSTYPE",
    "PAM_KWALLET5_LOGIN",
    "PODMAN_*",
    "PIPESTATUS",
    "PPID",
    "PROFILEHOME",
    "PROMPT_COMMAND",
    "PS*",
    "SESSION_MANAGER",
    "SHELL*",
    "SHLVL",
    "SSH*",
    "SYSTEMD_EXEC_PID",
    "TERM_SESSION_ID",
    "USER",
    "WINDOWID",
    "XAUTHORITY",
    "XCURSOR_*",
    "XDG*",
    "_",
];

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub env_filter: EnvFilterConfig,
    pub config_path: PathBuf,
    pub user_config: UserConfig,
    pub effective_config: EffectiveUserConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserConfig {
    #[serde(default)]
    pub approved_paths: BTreeSet<PathBuf>,
    #[serde(default)]
    pub publish: Vec<String>,
    #[serde(default)]
    pub add_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub block_var_patterns: Vec<String>,
    #[serde(default)]
    pub allow_var_patterns: Vec<String>,
    #[serde(default)]
    pub directories: BTreeMap<String, DirectoryConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryConfig {
    #[serde(default)]
    pub publish: Vec<String>,
    #[serde(default)]
    pub add_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveUserConfig {
    pub approved_paths: BTreeSet<PathBuf>,
    pub publish: Vec<String>,
    pub add_dirs: Vec<PathBuf>,
}

impl UserConfig {
    pub fn effective_for(&self, cwd: &Path, home_dir: &Path) -> EffectiveUserConfig {
        let cwd = canonicalize_if_possible(cwd);
        let mut effective = EffectiveUserConfig {
            approved_paths: self.approved_paths.clone(),
            publish: self.publish.clone(),
            add_dirs: self.add_dirs.clone(),
        };
        let mut matching_overrides = self
            .directories
            .iter()
            .filter_map(|(directory, config)| {
                let resolved = resolve_user_config_path(PathBuf::from(directory), home_dir);
                let resolved = canonicalize_if_possible(&resolved);
                cwd.starts_with(&resolved)
                    .then_some((resolved.components().count(), config))
            })
            .collect::<Vec<_>>();

        matching_overrides.sort_by_key(|(depth, _)| *depth);

        for (_, config) in matching_overrides {
            merge_unique(&mut effective.publish, &config.publish);
            merge_unique(&mut effective.add_dirs, &config.add_dirs);
        }

        effective
    }
}

pub fn load_launcher_config(user: &UserContext) -> Result<LauncherConfig> {
    let config_path = user.home_dir.join(".codexbox-conf.json");
    let user_config = load_user_config(&config_path)?;
    let effective_config = user_config.effective_for(&user.cwd, &user.home_dir);
    let mut blocked_patterns = default_blocked_var_patterns();
    merge_unique(&mut blocked_patterns, &user_config.block_var_patterns);
    let mut allowed_patterns = Vec::new();
    merge_unique(&mut allowed_patterns, &user_config.allow_var_patterns);

    Ok(LauncherConfig {
        env_filter: EnvFilterConfig {
            blocked_patterns,
            allowed_patterns,
        },
        config_path,
        user_config,
        effective_config,
    })
}

pub fn load_user_config(path: &Path) -> Result<UserConfig> {
    if !path.exists() {
        return Ok(UserConfig::default());
    }

    let contents = fs::read_to_string(path).map_err(|source| CodexboxError::ReadPath {
        path: path.to_path_buf(),
        source,
    })?;

    serde_json::from_str(&contents).map_err(|source| CodexboxError::ParseJson {
        path: path.to_path_buf(),
        source,
    })
}

pub fn save_user_config(path: &Path, config: &UserConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CodexboxError::WritePath {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut json = serde_json::to_string_pretty(config)?;
    json.push('\n');

    fs::write(path, json).map_err(|source| CodexboxError::WritePath {
        path: path.to_path_buf(),
        source,
    })
}

fn default_blocked_var_patterns() -> Vec<String> {
    DEFAULT_BLOCKED_VAR_PATTERNS
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect()
}

fn merge_unique<T: PartialEq + Clone>(target: &mut Vec<T>, entries: &[T]) {
    for entry in entries {
        if !target.contains(entry) {
            target.push(entry.clone());
        }
    }
}

fn resolve_user_config_path(path: PathBuf, home_dir: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return home_dir.to_path_buf();
    }

    if let Some(stripped) = raw.strip_prefix("~/") {
        return home_dir.join(stripped);
    }

    if path.is_absolute() {
        path
    } else {
        home_dir.join(path)
    }
}

fn canonicalize_if_possible(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        default_blocked_var_patterns, load_launcher_config, load_user_config, save_user_config,
        DirectoryConfig, EffectiveUserConfig, UserConfig,
    };
    use crate::user_context::UserContext;

    #[test]
    fn default_blocked_patterns_include_internal_namespace() {
        let patterns = default_blocked_var_patterns();

        assert!(patterns.contains(&"CODEXBOX_*".to_string()));
        assert!(patterns.contains(&"HOME".to_string()));
        assert!(patterns.contains(&"SSH*".to_string()));
    }

    #[test]
    fn user_config_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codexbox-conf.json");

        let config = load_user_config(&path).unwrap();

        assert_eq!(config, UserConfig::default());
    }

    #[test]
    fn user_config_reads_and_writes_all_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codexbox-conf.json");
        let config = UserConfig {
            approved_paths: BTreeSet::from([
                PathBuf::from("/run/user/1000/podman.sock"),
                PathBuf::from("/tmp/cache"),
            ]),
            publish: vec!["127.0.0.1:8080:80".into(), "8443:443".into()],
            add_dirs: vec![PathBuf::from("~/shared"), PathBuf::from("/tmp/cache")],
            block_var_patterns: vec!["CUSTOM_*".into()],
            allow_var_patterns: vec!["SSH_AUTH_SOCK".into()],
            directories: BTreeMap::from([(
                "~/project".into(),
                DirectoryConfig {
                    publish: vec!["3000:3000".into()],
                    add_dirs: vec![PathBuf::from("~/project-extra")],
                },
            )]),
        };

        save_user_config(&path, &config).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let loaded = load_user_config(&path).unwrap();

        assert!(saved.contains("\"approved_paths\""));
        assert!(saved.contains("\"block_var_patterns\""));
        assert!(saved.contains("\"allow_var_patterns\""));
        assert!(saved.contains("\"directories\""));
        assert_eq!(loaded, config);
    }

    #[test]
    fn effective_config_merges_matching_directory_overrides() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let workspace = home.join("work/project/app");
        fs::create_dir_all(&workspace).unwrap();

        let config = UserConfig {
            approved_paths: BTreeSet::from([PathBuf::from("/tmp/global.sock")]),
            publish: vec!["127.0.0.1:8080:80".into()],
            add_dirs: vec![PathBuf::from("~/shared")],
            block_var_patterns: Vec::new(),
            allow_var_patterns: Vec::new(),
            directories: BTreeMap::from([
                (
                    "~/work".into(),
                    DirectoryConfig {
                        publish: vec!["3000:3000".into()],
                        add_dirs: vec![PathBuf::from("~/work-shared")],
                    },
                ),
                (
                    "~/work/project".into(),
                    DirectoryConfig {
                        publish: vec!["4000:4000".into()],
                        add_dirs: vec![PathBuf::from("~/project-shared")],
                    },
                ),
            ]),
        };

        let effective = config.effective_for(&workspace, &home);

        assert_eq!(
            effective,
            EffectiveUserConfig {
                approved_paths: BTreeSet::from([PathBuf::from("/tmp/global.sock")]),
                publish: vec![
                    "127.0.0.1:8080:80".into(),
                    "3000:3000".into(),
                    "4000:4000".into(),
                ],
                add_dirs: vec![
                    PathBuf::from("~/shared"),
                    PathBuf::from("~/work-shared"),
                    PathBuf::from("~/project-shared"),
                ],
            }
        );
    }

    #[test]
    fn launcher_config_merges_default_blocked_and_user_overrides() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            home.join(".codexbox-conf.json"),
            r#"{
  "block_var_patterns": ["CUSTOM_*", "HOME"],
  "allow_var_patterns": ["SSH_AUTH_SOCK", "SSH_AUTH_SOCK"]
}
"#,
        )
        .unwrap();

        let config = load_launcher_config(&UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home,
            cwd,
        })
        .unwrap();

        assert!(config
            .env_filter
            .blocked_patterns
            .contains(&"CUSTOM_*".to_string()));
        assert!(config
            .env_filter
            .blocked_patterns
            .contains(&"HOME".to_string()));
        assert_eq!(
            config
                .env_filter
                .blocked_patterns
                .iter()
                .filter(|pattern| pattern.as_str() == "HOME")
                .count(),
            1
        );
        assert_eq!(
            config.env_filter.allowed_patterns,
            vec!["SSH_AUTH_SOCK".to_string()]
        );
    }
}
