use std::collections::BTreeSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env_mounts::EnvMountCandidate;
use crate::errors::{CodexboxError, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApprovalDb {
    #[serde(default)]
    pub approved_paths: BTreeSet<PathBuf>,
}

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

pub fn load_approval_db(path: &Path) -> Result<ApprovalDb> {
    if !path.exists() {
        return Ok(ApprovalDb::default());
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

pub fn save_approval_db(path: &Path, db: &ApprovalDb) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CodexboxError::WritePath {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut json = serde_json::to_string_pretty(db)?;
    json.push('\n');

    fs::write(path, json).map_err(|source| CodexboxError::WritePath {
        path: path.to_path_buf(),
        source,
    })
}

pub fn approve_candidates<P: ApprovalPrompt>(
    candidates: Vec<EnvMountCandidate>,
    db_path: &Path,
    prompt: &mut P,
) -> Result<Vec<EnvMountCandidate>> {
    let mut db = load_approval_db(db_path)?;
    let mut approved = Vec::new();
    let mut changed = false;

    for candidate in candidates {
        if db.approved_paths.contains(&candidate.host_path) {
            approved.push(candidate);
            continue;
        }

        if prompt.confirm(&candidate)? {
            db.approved_paths.insert(candidate.host_path.clone());
            approved.push(candidate);
            changed = true;
        }
    }

    if changed {
        save_approval_db(db_path, &db)?;
    }

    Ok(approved)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tempfile::tempdir;

    use super::{approve_candidates, load_approval_db, ApprovalPrompt};
    use crate::env_mounts::EnvMountCandidate;

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
        let db_path = dir.path().join("approvals.json");
        let candidate = EnvMountCandidate {
            var_name: "SSH_AUTH_SOCK".into(),
            host_path: dir.path().join("sock"),
        };

        let mut prompt = FixedPrompt {
            answers: RefCell::new(vec![true]),
        };

        let approved = approve_candidates(vec![candidate.clone()], &db_path, &mut prompt).unwrap();
        let db = load_approval_db(&db_path).unwrap();

        assert_eq!(approved, vec![candidate.clone()]);
        assert!(db.approved_paths.contains(&candidate.host_path));
    }
}
