use std::collections::BTreeMap;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::errors::{CodexboxError, Result};

#[derive(Debug, Clone, Default)]
pub struct ForwardedEnv {
    pub vars: BTreeMap<String, String>,
    pub path_prefix: Option<String>,
}

pub fn filter_environment(patterns: &[String]) -> Result<ForwardedEnv> {
    filter_environment_from_iter(std::env::vars(), patterns)
}

pub fn filter_environment_from_iter<I, K, V>(iter: I, patterns: &[String]) -> Result<ForwardedEnv>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let matcher = build_matcher(patterns)?;
    let mut vars = BTreeMap::new();
    let mut path_prefix = None;

    for (key, value) in iter.into_iter() {
        let key = key.into();
        if key.starts_with("CODEXBOX_") {
            continue;
        }

        if key != "PATH" && matcher.is_match(&key) {
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
        .filter(|segment| !is_usr_path_segment(segment))
        .collect::<Vec<_>>()
        .join(":")
}

fn is_usr_path_segment(segment: &str) -> bool {
    segment == "/usr" || segment.starts_with("/usr/")
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
    use super::filter_environment_from_iter;

    #[test]
    fn filter_environment_keeps_path_even_if_pattern_matches() {
        let env = vec![
            ("PATH", "/usr/bin:/opt/codex/bin:/usr/local/bin"),
            ("SSH_AUTH_SOCK", "/tmp/ssh.sock"),
            ("USER", "alice"),
            ("CODEXBOX_PATH_PREFIX", "/host/bin"),
        ];

        let forwarded = filter_environment_from_iter(env, &["PATH".into(), "SSH*".into()]).unwrap();

        assert_eq!(forwarded.vars.len(), 2);
        assert_eq!(
            forwarded.vars.get("PATH"),
            Some(&"/opt/codex/bin".to_string())
        );
        assert_eq!(forwarded.path_prefix, Some("/opt/codex/bin".into()));
        assert!(forwarded.vars.contains_key("USER"));
        assert!(!forwarded.vars.contains_key("CODEXBOX_PATH_PREFIX"));
    }
}
