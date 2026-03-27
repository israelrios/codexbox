use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::config::user::{save_user_config, UserConfig};
use crate::errors::{CodexboxError, Result};
use crate::sandbox::env_mounts::EnvMountCandidate;

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
    approved_socket_vars: &BTreeSet<String>,
) -> Vec<EnvMountCandidate> {
    let fully_approved_socket_paths =
        fully_approved_socket_paths(&candidates, approved_paths, approved_socket_vars);

    candidates
        .into_iter()
        .filter(|candidate| {
            candidate_is_fully_approved(
                candidate,
                approved_paths,
                approved_socket_vars,
                &fully_approved_socket_paths,
            )
        })
        .collect()
}

pub fn approve_candidates<P: ApprovalPrompt>(
    candidates: Vec<EnvMountCandidate>,
    approved_paths: &BTreeSet<PathBuf>,
    approved_socket_vars: &BTreeSet<String>,
    user_config: &mut UserConfig,
    config_path: &Path,
    prompt: &mut P,
) -> Result<Vec<EnvMountCandidate>> {
    let mut approved_paths = approved_paths.clone();
    let mut approved_socket_vars = approved_socket_vars.clone();
    let mut all_candidates = Vec::new();
    let mut individually_approved = Vec::new();
    let mut changed = false;

    for candidate in candidates {
        all_candidates.push(candidate.clone());

        if candidate.is_socket()
            && !approved_socket_vars.contains(&candidate.var_name)
            && approved_paths.contains(&candidate.host_path)
        {
            approved_socket_vars.insert(candidate.var_name.clone());
            changed |= user_config
                .approved_socket_vars
                .insert(candidate.var_name.clone());
            individually_approved.push(candidate);
            continue;
        }
        if candidate_has_individual_approval(&candidate, &approved_paths, &approved_socket_vars) {
            individually_approved.push(candidate);
            continue;
        }

        if prompt.confirm(&candidate)? {
            changed |= persist_candidate_approval(&candidate, user_config);
            if candidate.is_socket() {
                approved_socket_vars.insert(candidate.var_name.clone());
            } else {
                approved_paths.insert(candidate.host_path.clone());
            }
            individually_approved.push(candidate);
        }
    }

    if changed {
        save_user_config(config_path, user_config)?;
    }

    let fully_approved_socket_paths =
        fully_approved_socket_paths(&all_candidates, &approved_paths, &approved_socket_vars);

    Ok(individually_approved
        .into_iter()
        .filter(|candidate| {
            candidate_is_fully_approved(
                candidate,
                &approved_paths,
                &approved_socket_vars,
                &fully_approved_socket_paths,
            )
        })
        .collect())
}

fn candidate_has_individual_approval(
    candidate: &EnvMountCandidate,
    approved_paths: &BTreeSet<PathBuf>,
    approved_socket_vars: &BTreeSet<String>,
) -> bool {
    if candidate.is_socket() {
        approved_socket_vars.contains(&candidate.var_name)
            || approved_paths.contains(&candidate.host_path)
    } else {
        approved_paths.contains(&candidate.host_path)
    }
}

fn candidate_is_fully_approved(
    candidate: &EnvMountCandidate,
    approved_paths: &BTreeSet<PathBuf>,
    approved_socket_vars: &BTreeSet<String>,
    fully_approved_socket_paths: &BTreeSet<PathBuf>,
) -> bool {
    if candidate.is_socket() {
        candidate_has_individual_approval(candidate, approved_paths, approved_socket_vars)
            && fully_approved_socket_paths.contains(&candidate.host_path)
    } else {
        approved_paths.contains(&candidate.host_path)
    }
}

fn fully_approved_socket_paths(
    candidates: &[EnvMountCandidate],
    approved_paths: &BTreeSet<PathBuf>,
    approved_socket_vars: &BTreeSet<String>,
) -> BTreeSet<PathBuf> {
    let mut socket_paths = BTreeSet::new();

    for candidate in candidates {
        if !candidate.is_socket() {
            continue;
        }

        if candidates
            .iter()
            .filter(|other| other.is_socket())
            .all(|other| {
                other.host_path != candidate.host_path
                    || candidate_has_individual_approval(
                        other,
                        approved_paths,
                        approved_socket_vars,
                    )
            })
        {
            socket_paths.insert(candidate.host_path.clone());
        }
    }

    socket_paths
}

fn persist_candidate_approval(candidate: &EnvMountCandidate, user_config: &mut UserConfig) -> bool {
    if candidate.is_socket() {
        user_config
            .approved_socket_vars
            .insert(candidate.var_name.clone())
    } else {
        user_config
            .approved_paths
            .insert(candidate.host_path.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{approve_candidates, approved_candidates, ApprovalPrompt};
    use crate::config::user::{load_user_config, UserConfig};
    use crate::sandbox::env_mounts::{EnvMountCandidate, EnvMountKind};

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
            kind: EnvMountKind::Socket,
        };
        let mut user_config = UserConfig::default();

        let mut prompt = FixedPrompt {
            answers: RefCell::new(vec![true]),
        };

        let approved = approve_candidates(
            vec![candidate.clone()],
            &BTreeSet::new(),
            &BTreeSet::new(),
            &mut user_config,
            &config_path,
            &mut prompt,
        )
        .unwrap();
        let saved = load_user_config(&config_path).unwrap();

        assert_eq!(approved, vec![candidate.clone()]);
        assert!(saved.approved_socket_vars.contains(&candidate.var_name));
    }

    #[test]
    fn approved_candidates_filter_without_prompting() {
        let approved = approved_candidates(
            vec![
                EnvMountCandidate {
                    var_name: "A".into(),
                    host_path: PathBuf::from("/tmp/a"),
                    kind: EnvMountKind::File,
                },
                EnvMountCandidate {
                    var_name: "B".into(),
                    host_path: PathBuf::from("/tmp/b"),
                    kind: EnvMountKind::File,
                },
            ],
            &BTreeSet::from([PathBuf::from("/tmp/b")]),
            &BTreeSet::new(),
        );

        assert_eq!(
            approved,
            vec![EnvMountCandidate {
                var_name: "B".into(),
                host_path: PathBuf::from("/tmp/b"),
                kind: EnvMountKind::File,
            }]
        );
    }

    #[test]
    fn approved_candidates_accept_socket_by_var_name() {
        let approved = approved_candidates(
            vec![EnvMountCandidate {
                var_name: "SSH_AUTH_SOCK".into(),
                host_path: PathBuf::from("/tmp/rotated.sock"),
                kind: EnvMountKind::Socket,
            }],
            &BTreeSet::new(),
            &BTreeSet::from(["SSH_AUTH_SOCK".to_string()]),
        );

        assert_eq!(approved.len(), 1);
    }

    #[test]
    fn approve_candidates_migrates_legacy_socket_path_approval() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("codexbox-conf.json");
        let host_path = dir.path().join("sock");
        let candidate = EnvMountCandidate {
            var_name: "SSH_AUTH_SOCK".into(),
            host_path: host_path.clone(),
            kind: EnvMountKind::Socket,
        };
        let mut user_config = UserConfig {
            approved_paths: BTreeSet::from([host_path]),
            ..UserConfig::default()
        };
        let approved_paths = user_config.approved_paths.clone();
        let mut prompt = FixedPrompt {
            answers: RefCell::new(vec![]),
        };

        let approved = approve_candidates(
            vec![candidate.clone()],
            &approved_paths,
            &BTreeSet::new(),
            &mut user_config,
            &config_path,
            &mut prompt,
        )
        .unwrap();
        let saved = load_user_config(&config_path).unwrap();

        assert_eq!(approved, vec![candidate]);
        assert!(saved.approved_socket_vars.contains("SSH_AUTH_SOCK"));
    }

    #[test]
    fn approved_candidates_require_all_socket_aliases_to_be_approved() {
        let host_path = PathBuf::from("/tmp/agent.sock");

        let approved = approved_candidates(
            vec![
                EnvMountCandidate {
                    var_name: "A_SOCK".into(),
                    host_path: host_path.clone(),
                    kind: EnvMountKind::Socket,
                },
                EnvMountCandidate {
                    var_name: "B_SOCK".into(),
                    host_path,
                    kind: EnvMountKind::Socket,
                },
            ],
            &BTreeSet::new(),
            &BTreeSet::from(["A_SOCK".to_string()]),
        );

        assert!(approved.is_empty());
    }

    #[test]
    fn approve_candidates_require_all_socket_aliases_before_mounting() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("codexbox-conf.json");
        let host_path = dir.path().join("sock");
        let candidates = vec![
            EnvMountCandidate {
                var_name: "A_SOCK".into(),
                host_path: host_path.clone(),
                kind: EnvMountKind::Socket,
            },
            EnvMountCandidate {
                var_name: "B_SOCK".into(),
                host_path,
                kind: EnvMountKind::Socket,
            },
        ];
        let mut user_config = UserConfig::default();
        let mut prompt = FixedPrompt {
            answers: RefCell::new(vec![true, false]),
        };

        let approved = approve_candidates(
            candidates,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &mut user_config,
            &config_path,
            &mut prompt,
        )
        .unwrap();
        let saved = load_user_config(&config_path).unwrap();

        assert!(approved.is_empty());
        assert!(saved.approved_socket_vars.contains("A_SOCK"));
        assert!(!saved.approved_socket_vars.contains("B_SOCK"));
    }
}
