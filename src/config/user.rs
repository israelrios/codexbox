use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::errors::{CodexboxError, Result};
use crate::path_utils::{canonicalize_if_possible, resolve_from_home};
use crate::sandbox::env_filter::{build_matcher, EnvFilterConfig};
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
#[serde(default, deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default)]
    pub approved_paths: BTreeSet<PathBuf>,
    #[serde(default)]
    pub approved_socket_vars: BTreeSet<String>,
    #[serde(default)]
    pub publish: Vec<PublishSpec>,
    #[serde(default)]
    pub add_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub block_var_patterns: Vec<String>,
    #[serde(default)]
    pub allow_var_patterns: Vec<String>,
    #[serde(default)]
    pub directory_rules: Vec<DirectoryRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DirectoryRule {
    pub path: PathBuf,
    pub publish: Vec<PublishSpec>,
    pub add_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveUserConfig {
    pub approved_paths: BTreeSet<PathBuf>,
    pub approved_socket_vars: BTreeSet<String>,
    pub publish: Vec<PublishSpec>,
    pub add_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PublishProtocol {
    #[default]
    Tcp,
    Udp,
}

impl PublishProtocol {
    fn is_default(value: &Self) -> bool {
        *value == Self::Tcp
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublishSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_ip: Option<IpAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    pub container_port: u16,
    #[serde(default, skip_serializing_if = "PublishProtocol::is_default")]
    pub protocol: PublishProtocol,
}

impl PublishSpec {
    fn validate(&self) -> std::result::Result<(), String> {
        if self.host_ip.is_some() && self.host_port.is_none() {
            return Err("host_ip requires host_port".into());
        }

        Ok(())
    }
}

impl fmt::Display for PublishSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let protocol_suffix = if self.protocol == PublishProtocol::Tcp {
            ""
        } else {
            "/udp"
        };

        match (self.host_ip, self.host_port) {
            (Some(host_ip), Some(host_port)) => {
                write!(
                    f,
                    "{host_ip}:{host_port}:{}{}",
                    self.container_port, protocol_suffix
                )
            }
            (None, Some(host_port)) => {
                write!(f, "{host_port}:{}{}", self.container_port, protocol_suffix)
            }
            (None, None) => write!(f, "{}{}", self.container_port, protocol_suffix),
            (Some(_), None) => unreachable!("validated publish specs always include host_port"),
        }
    }
}

impl FromStr for PublishSpec {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err("publish spec must not be empty".into());
        }

        let (ports, protocol) = match value.rsplit_once('/') {
            Some((ports, protocol)) => (
                ports,
                match protocol {
                    "tcp" => PublishProtocol::Tcp,
                    "udp" => PublishProtocol::Udp,
                    _ => return Err(format!("unsupported protocol '{protocol}'")),
                },
            ),
            None => (value, PublishProtocol::Tcp),
        };

        let segments = ports.split(':').collect::<Vec<_>>();
        let spec = match segments.as_slice() {
            [container_port] => Self {
                host_ip: None,
                host_port: None,
                container_port: parse_port(container_port, "container_port")?,
                protocol,
            },
            [host_port, container_port] => Self {
                host_ip: None,
                host_port: Some(parse_port(host_port, "host_port")?),
                container_port: parse_port(container_port, "container_port")?,
                protocol,
            },
            [host_ip, host_port, container_port] => Self {
                host_ip: Some(
                    host_ip
                        .parse()
                        .map_err(|_| format!("invalid host_ip '{host_ip}'"))?,
                ),
                host_port: Some(parse_port(host_port, "host_port")?),
                container_port: parse_port(container_port, "container_port")?,
                protocol,
            },
            _ => {
                return Err(
                    "publish spec must match CONTAINER_PORT, HOST_PORT:CONTAINER_PORT, or HOST_IP:HOST_PORT:CONTAINER_PORT"
                        .into(),
                )
            }
        };

        spec.validate()?;
        Ok(spec)
    }
}

impl UserConfig {
    pub fn effective_for(&self, cwd: &Path, home_dir: &Path) -> EffectiveUserConfig {
        let cwd = canonicalize_if_possible(cwd);
        let mut effective = EffectiveUserConfig {
            approved_paths: self.approved_paths.clone(),
            approved_socket_vars: self.approved_socket_vars.clone(),
            publish: self.publish.clone(),
            add_dirs: self.add_dirs.clone(),
        };
        let mut matching_overrides = self
            .directory_rules
            .iter()
            .enumerate()
            .filter_map(|(index, rule)| {
                let resolved = resolve_from_home(&rule.path, home_dir);
                let resolved = canonicalize_if_possible(&resolved);
                cwd.starts_with(&resolved)
                    .then_some((resolved.components().count(), index, rule))
            })
            .collect::<Vec<_>>();

        matching_overrides.sort_by_key(|(depth, index, _)| (*depth, *index));

        for (_, _, rule) in matching_overrides {
            merge_unique(&mut effective.publish, &rule.publish);
            merge_unique(&mut effective.add_dirs, &rule.add_dirs);
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
    let mut allowed_patterns = default_allowed_var_patterns(&user_config)?;
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

    let config = serde_json::from_str(&contents).map_err(|source| CodexboxError::ParseJson {
        path: path.to_path_buf(),
        source,
    })?;
    validate_user_config(path, &config)?;
    Ok(config)
}

pub fn save_user_config(path: &Path, config: &UserConfig) -> Result<()> {
    validate_user_config(path, config)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CodexboxError::WritePath {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut json = serde_json::to_vec_pretty(config)?;
    json.push(b'\n');

    let temp_root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp_file =
        NamedTempFile::new_in(temp_root).map_err(|source| CodexboxError::WritePath {
            path: temp_root.to_path_buf(),
            source,
        })?;
    temp_file
        .write_all(&json)
        .map_err(|source| CodexboxError::WritePath {
            path: path.to_path_buf(),
            source,
        })?;
    temp_file
        .as_file_mut()
        .sync_all()
        .map_err(|source| CodexboxError::WritePath {
            path: path.to_path_buf(),
            source,
        })?;
    temp_file
        .persist(path)
        .map_err(|source| CodexboxError::WritePath {
            path: path.to_path_buf(),
            source: source.error,
        })?;

    Ok(())
}

fn validate_user_config(path: &Path, config: &UserConfig) -> Result<()> {
    for publish in &config.publish {
        validate_publish_spec(path, "publish", publish)?;
    }

    for (index, rule) in config.directory_rules.iter().enumerate() {
        if rule.path.as_os_str().is_empty() {
            return Err(CodexboxError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("directory_rules[{index}].path must not be empty"),
            });
        }

        for publish in &rule.publish {
            validate_publish_spec(path, &format!("directory_rules[{index}].publish"), publish)?;
        }
    }

    Ok(())
}

fn validate_publish_spec(path: &Path, context: &str, publish: &PublishSpec) -> Result<()> {
    publish
        .validate()
        .map_err(|message| CodexboxError::InvalidConfig {
            path: path.to_path_buf(),
            message: format!("{context}: {message}"),
        })
}

fn parse_port(value: &str, field: &str) -> std::result::Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| format!("invalid {field} '{value}'"))
}

fn default_blocked_var_patterns() -> Vec<String> {
    DEFAULT_BLOCKED_VAR_PATTERNS
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect()
}

fn default_allowed_var_patterns(user_config: &UserConfig) -> Result<Vec<String>> {
    let mut patterns = Vec::new();
    if !patterns_match_var(&user_config.block_var_patterns, "SSH_AUTH_SOCK")? {
        patterns.push("SSH_AUTH_SOCK".to_string());
    }

    Ok(patterns)
}

fn patterns_match_var(patterns: &[String], var_name: &str) -> Result<bool> {
    Ok(build_matcher(patterns)?.is_match(var_name))
}

fn merge_unique<T: Eq + Clone>(target: &mut Vec<T>, entries: &[T]) {
    for entry in entries {
        if !target.contains(entry) {
            target.push(entry.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::str::FromStr;

    use tempfile::tempdir;

    use super::{
        default_blocked_var_patterns, load_launcher_config, load_user_config, save_user_config,
        DirectoryRule, EffectiveUserConfig, PublishProtocol, PublishSpec, UserConfig,
    };
    use crate::errors::CodexboxError;
    use crate::user_context::UserContext;

    fn publish(value: &str) -> PublishSpec {
        PublishSpec::from_str(value).unwrap()
    }

    #[test]
    fn publish_spec_parses_supported_cli_forms() {
        assert_eq!(
            publish("8080:80"),
            PublishSpec {
                host_ip: None,
                host_port: Some(8080),
                container_port: 80,
                protocol: PublishProtocol::Tcp,
            }
        );
        assert_eq!(
            publish("127.0.0.1:8080:80/udp").to_string(),
            "127.0.0.1:8080:80/udp"
        );
        assert_eq!(publish("443").to_string(), "443");
    }

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
    fn load_user_config_rejects_unknown_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codexbox-conf.json");
        fs::write(&path, "{\n  \"mystery\": true\n}\n").unwrap();

        let error = load_user_config(&path).unwrap_err();

        assert!(matches!(error, CodexboxError::ParseJson { .. }));
    }

    #[test]
    fn load_user_config_rejects_invalid_publish_shape() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("codexbox-conf.json");
        fs::write(
            &path,
            r#"{
  "publish": [
    {
      "host_ip": "127.0.0.1",
      "container_port": 80
    }
  ]
}
"#,
        )
        .unwrap();

        let error = load_user_config(&path).unwrap_err();

        assert!(matches!(error, CodexboxError::InvalidConfig { .. }));
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
            approved_socket_vars: BTreeSet::from(["SSH_AUTH_SOCK".to_string()]),
            publish: vec![publish("127.0.0.1:8080:80"), publish("8443:443")],
            add_dirs: vec![PathBuf::from("~/shared"), PathBuf::from("/tmp/cache")],
            block_var_patterns: vec!["CUSTOM_*".into()],
            allow_var_patterns: vec!["SSH_AUTH_SOCK".into()],
            directory_rules: vec![DirectoryRule {
                path: PathBuf::from("~/project"),
                publish: vec![publish("3000:3000")],
                add_dirs: vec![PathBuf::from("~/project-extra")],
            }],
        };

        save_user_config(&path, &config).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let loaded = load_user_config(&path).unwrap();

        assert!(saved.contains("\"approved_paths\""));
        assert!(saved.contains("\"approved_socket_vars\""));
        assert!(saved.contains("\"block_var_patterns\""));
        assert!(saved.contains("\"allow_var_patterns\""));
        assert!(saved.contains("\"directory_rules\""));
        assert!(saved.ends_with('\n'));
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
            approved_socket_vars: BTreeSet::from(["SSH_AUTH_SOCK".to_string()]),
            publish: vec![publish("127.0.0.1:8080:80")],
            add_dirs: vec![PathBuf::from("~/shared")],
            block_var_patterns: Vec::new(),
            allow_var_patterns: Vec::new(),
            directory_rules: vec![
                DirectoryRule {
                    path: PathBuf::from("~/work"),
                    publish: vec![publish("3000:3000")],
                    add_dirs: vec![PathBuf::from("~/work-shared")],
                },
                DirectoryRule {
                    path: PathBuf::from("~/work/project"),
                    publish: vec![publish("4000:4000")],
                    add_dirs: vec![PathBuf::from("~/project-shared")],
                },
            ],
        };

        let effective = config.effective_for(&workspace, &home);

        assert_eq!(
            effective,
            EffectiveUserConfig {
                approved_paths: BTreeSet::from([PathBuf::from("/tmp/global.sock")]),
                approved_socket_vars: BTreeSet::from(["SSH_AUTH_SOCK".to_string()]),
                publish: vec![
                    publish("127.0.0.1:8080:80"),
                    publish("3000:3000"),
                    publish("4000:4000"),
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

    #[test]
    fn launcher_config_allows_ssh_auth_sock_by_default() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let config = load_launcher_config(&UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home,
            cwd,
        })
        .unwrap();

        assert_eq!(
            config.env_filter.allowed_patterns,
            vec!["SSH_AUTH_SOCK".to_string()]
        );
    }

    #[test]
    fn launcher_config_respects_explicit_ssh_auth_sock_block() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            home.join(".codexbox-conf.json"),
            r#"{
  "block_var_patterns": ["SSH_AUTH_SOCK"]
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

        assert!(config.env_filter.allowed_patterns.is_empty());
    }
}
