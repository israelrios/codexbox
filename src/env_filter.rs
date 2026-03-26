use std::collections::BTreeMap;
use std::ffi::OsString;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::errors::{CodexboxError, Result};

#[derive(Debug, Clone, Default)]
pub struct ForwardedEnv {
    pub vars: BTreeMap<OsString, OsString>,
    pub path_prefix: Option<OsString>,
}

pub fn filter_environment(patterns: &[String]) -> Result<ForwardedEnv> {
    filter_environment_from_iter(std::env::vars_os(), patterns)
}

pub fn filter_environment_from_iter<I, K, V>(iter: I, patterns: &[String]) -> Result<ForwardedEnv>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<OsString>,
    V: Into<OsString>,
{
    let matcher = build_matcher(patterns)?;
    let mut vars = BTreeMap::new();
    let mut path_prefix = None;

    for (key, value) in iter.into_iter() {
        let key = key.into();
        let key_text = key.to_string_lossy();

        if key_text != "PATH" && matcher.is_match(key_text.as_ref()) {
            continue;
        }

        let value = if key_text == "PATH" {
            let sanitized = sanitize_path_value(&value.into());
            path_prefix = Some(sanitized.clone());
            sanitized
        } else {
            value.into()
        };

        vars.insert(key, value);
    }

    Ok(ForwardedEnv { vars, path_prefix })
}

fn sanitize_path_value(value: &OsString) -> OsString {
    let text = value.to_string_lossy();
    let filtered = text
        .split(':')
        .filter(|segment| !is_usr_path_segment(segment))
        .collect::<Vec<_>>()
        .join(":");

    OsString::from(filtered)
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
    use std::ffi::OsString;

    use super::filter_environment_from_iter;

    #[test]
    fn filter_environment_keeps_path_even_if_pattern_matches() {
        let env = vec![
            (
                OsString::from("PATH"),
                OsString::from("/usr/bin:/opt/codex/bin:/usr/local/bin"),
            ),
            (
                OsString::from("SSH_AUTH_SOCK"),
                OsString::from("/tmp/ssh.sock"),
            ),
            (OsString::from("USER"), OsString::from("alice")),
        ];

        let forwarded = filter_environment_from_iter(env, &["PATH".into(), "SSH*".into()]).unwrap();

        assert_eq!(forwarded.vars.len(), 2);
        assert_eq!(
            forwarded.vars.get(&OsString::from("PATH")),
            Some(&OsString::from("/opt/codex/bin"))
        );
        assert_eq!(
            forwarded.path_prefix,
            Some(OsString::from("/opt/codex/bin"))
        );
        assert!(forwarded.vars.contains_key(&OsString::from("USER")));
    }
}
