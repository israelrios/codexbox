use std::collections::BTreeMap;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::errors::{CodexboxError, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFilterConfig {
    pub blocked_patterns: Vec<String>,
    pub allowed_patterns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ForwardedEnv {
    pub vars: BTreeMap<String, String>,
    pub path_prefix: Option<String>,
}

pub fn filter_environment(config: &EnvFilterConfig) -> Result<ForwardedEnv> {
    filter_environment_from_iter(std::env::vars(), config)
}

pub fn filter_environment_from_iter<I, K, V>(
    iter: I,
    config: &EnvFilterConfig,
) -> Result<ForwardedEnv>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let blocked = build_matcher(&config.blocked_patterns)?;
    let allowed = build_matcher(&config.allowed_patterns)?;
    let mut vars = BTreeMap::new();
    let mut path_prefix = None;

    for (key, value) in iter.into_iter() {
        let key = key.into();
        if key.starts_with("CODEXBOX_") {
            continue;
        }

        if key != "PATH" && blocked.is_match(&key) && !allowed.is_match(&key) {
            continue;
        }

        let value = value.into();
        let value = if key == "PATH" {
            let sanitized = sanitize_path_value(&value);
            path_prefix = (!sanitized.is_empty()).then_some(sanitized.clone());
            sanitized
        } else {
            value
        };

        vars.insert(key, value);
    }

    Ok(ForwardedEnv { vars, path_prefix })
}

fn sanitize_path_value(value: &str) -> String {
    value
        .split(':')
        .filter(|segment| !is_reserved_path_segment(segment))
        .collect::<Vec<_>>()
        .join(":")
}

fn is_reserved_path_segment(segment: &str) -> bool {
    matches!(segment, "/bin" | "/sbin") || segment == "/usr" || segment.starts_with("/usr/")
}

fn build_matcher(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();

    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|source| CodexboxError::InvalidIgnorePattern {
            pattern: pattern.clone(),
            source,
        })?;
        builder.add(glob);
    }

    builder
        .build()
        .map_err(|source| CodexboxError::InvalidIgnorePattern {
            pattern: "<compiled-set>".into(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::{filter_environment_from_iter, EnvFilterConfig};

    #[test]
    fn filter_environment_keeps_path_even_if_pattern_matches() {
        let env = vec![
            ("PATH", "/usr/bin:/opt/codex/bin:/usr/local/bin"),
            ("SSH_AUTH_SOCK", "/tmp/ssh.sock"),
            ("USER", "alice"),
            ("CODEXBOX_PATH_PREFIX", "/host/bin"),
        ];

        let forwarded = filter_environment_from_iter(
            env,
            &EnvFilterConfig {
                blocked_patterns: vec!["PATH".into(), "SSH*".into()],
                allowed_patterns: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(forwarded.vars.len(), 2);
        assert_eq!(
            forwarded.vars.get("PATH"),
            Some(&"/opt/codex/bin".to_string())
        );
        assert_eq!(forwarded.path_prefix, Some("/opt/codex/bin".into()));
        assert!(forwarded.vars.contains_key("USER"));
        assert!(!forwarded.vars.contains_key("CODEXBOX_PATH_PREFIX"));
    }

    #[test]
    fn filter_environment_allows_explicit_overrides() {
        let forwarded = filter_environment_from_iter(
            [
                ("SSH_AUTH_SOCK", "/tmp/ssh.sock"),
                ("SSH_AGENT_PID", "1234"),
            ],
            &EnvFilterConfig {
                blocked_patterns: vec!["SSH*".into()],
                allowed_patterns: vec!["SSH_AUTH_SOCK".into()],
            },
        )
        .unwrap();

        assert_eq!(
            forwarded.vars.get("SSH_AUTH_SOCK"),
            Some(&"/tmp/ssh.sock".to_string())
        );
        assert!(!forwarded.vars.contains_key("SSH_AGENT_PID"));
    }

    #[test]
    fn filter_environment_removes_bin_and_sbin_from_path() {
        let forwarded = filter_environment_from_iter(
            [("PATH", "/usr/bin:/bin:/opt/tools:/sbin:/usr/local/bin")],
            &EnvFilterConfig::default(),
        )
        .unwrap();

        assert_eq!(forwarded.vars.get("PATH"), Some(&"/opt/tools".to_string()));
        assert_eq!(forwarded.path_prefix, Some("/opt/tools".into()));
    }
}
