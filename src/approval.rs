use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::env_mounts::EnvMountCandidate;
use crate::errors::{CodexboxError, Result};
use crate::user_config::{save_user_config, UserConfig};

pub trait ApprovalPrompt {
    fn confirm(&mut self, candidate: &EnvMountCandidate) -> Result<bool>;
}

#[derive(Debug, Default)]
pub struct StdioApprovalPrompt;

impl ApprovalPrompt for StdioApprovalPrompt {
    fn confirm(&mut self, candidate: &EnvMountCandidate) -> Result<bool> {
        if !io::stdin().is_terminal() {
            eprintln!(
                "codexbox: skipping unapproved env mount '{}' because stdin is not interactive",
                candidate.host_path.display()
            );
            return Ok(false);
        }

        let mut stderr = io::stderr().lock();
        writeln!(
            stderr,
            "Allow readonly mount derived from environment variable {}?",
            candidate.var_name
        )
        .map_err(CodexboxError::PromptIo)?;
        writeln!(stderr, "Host path: {}", candidate.host_path.display())
            .map_err(CodexboxError::PromptIo)?;
        write!(stderr, "Approve and remember? [y/N] ").map_err(CodexboxError::PromptIo)?;
        stderr.flush().map_err(CodexboxError::PromptIo)?;

        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(CodexboxError::PromptIo)?;

        Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
    }
}

pub fn approved_candidates(
    candidates: Vec<EnvMountCandidate>,
    approved_paths: &BTreeSet<PathBuf>,
) -> Vec<EnvMountCandidate> {
    candidates
        .into_iter()
        .filter(|candidate| approved_paths.contains(&candidate.host_path))
        .collect()
}

pub fn approve_candidates<P: ApprovalPrompt>(
    candidates: Vec<EnvMountCandidate>,
    approved_paths: &BTreeSet<PathBuf>,
    user_config: &mut UserConfig,
    config_path: &Path,
    prompt: &mut P,
) -> Result<Vec<EnvMountCandidate>> {
    let mut approved = Vec::new();
    let mut changed = false;

    for candidate in candidates {
        if approved_paths.contains(&candidate.host_path) {
            approved.push(candidate);
            continue;
        }

        if prompt.confirm(&candidate)? {
            user_config
                .approved_paths
                .insert(candidate.host_path.clone());
            approved.push(candidate);
            changed = true;
        }
    }

    if changed {
        save_user_config(config_path, user_config)?;
    }

    Ok(approved)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{approve_candidates, approved_candidates, ApprovalPrompt};
    use crate::env_mounts::EnvMountCandidate;
    use crate::user_config::{load_user_config, UserConfig};

    struct FixedPrompt {
        answers: RefCell<Vec<bool>>,
    }

    impl ApprovalPrompt for FixedPrompt {
        fn confirm(&mut self, _candidate: &EnvMountCandidate) -> crate::errors::Result<bool> {
            Ok(self.answers.borrow_mut().remove(0))
        }
    }

    #[test]
    fn approve_candidates_persists_positive_answers() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("codexbox-conf.json");
        let candidate = EnvMountCandidate {
            var_name: "SSH_AUTH_SOCK".into(),
            host_path: dir.path().join("sock"),
        };
        let mut user_config = UserConfig::default();

        let mut prompt = FixedPrompt {
            answers: RefCell::new(vec![true]),
        };

        let approved = approve_candidates(
            vec![candidate.clone()],
            &BTreeSet::new(),
            &mut user_config,
            &config_path,
            &mut prompt,
        )
        .unwrap();
        let saved = load_user_config(&config_path).unwrap();

        assert_eq!(approved, vec![candidate.clone()]);
        assert!(saved.approved_paths.contains(&candidate.host_path));
    }

    #[test]
    fn approved_candidates_filter_without_prompting() {
        let approved = approved_candidates(
            vec![
                EnvMountCandidate {
                    var_name: "A".into(),
                    host_path: PathBuf::from("/tmp/a"),
                },
                EnvMountCandidate {
                    var_name: "B".into(),
                    host_path: PathBuf::from("/tmp/b"),
                },
            ],
            &BTreeSet::from([PathBuf::from("/tmp/b")]),
        );

        assert_eq!(
            approved,
            vec![EnvMountCandidate {
                var_name: "B".into(),
                host_path: PathBuf::from("/tmp/b"),
            }]
        );
    }
}
