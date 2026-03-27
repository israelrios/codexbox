use std::fs;
use std::path::{Path, PathBuf};

use crate::mounts::{MountMode, MountSource, MountSpec};
use crate::user_context::UserContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddDirPlan {
    pub paths: Vec<PathBuf>,
    pub command: Vec<String>,
}

pub fn plan_default_codex_command(
    codex_args: Vec<String>,
    configured_add_dirs: &[PathBuf],
    user: &UserContext,
) -> AddDirPlan {
    let paths = resolve_add_dir_paths(&codex_args, configured_add_dirs, user);
    let command = codex_command(extend_codex_args_with_add_dirs(codex_args, &paths, user));

    AddDirPlan { paths, command }
}

pub fn add_dir_mounts(add_dirs: &[PathBuf]) -> Vec<MountSpec> {
    add_dirs
        .iter()
        .cloned()
        .map(|path| MountSpec {
            host: path.clone(),
            guest: path,
            mode: MountMode::ReadWrite,
            source: MountSource::CodexAddDir,
        })
        .collect()
}

fn codex_command(codex_args: Vec<String>) -> Vec<String> {
    let mut command = vec![
        "codex".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
    ];
    command.extend(codex_args);
    command
}

fn resolve_add_dir_paths(
    codex_args: &[String],
    configured_add_dirs: &[PathBuf],
    user: &UserContext,
) -> Vec<PathBuf> {
    let mut add_dirs = Vec::new();

    for path in extract_add_dir_paths(codex_args) {
        push_add_dir(&mut add_dirs, path, &user.cwd, &user.home_dir);
    }

    for path in configured_add_dirs.iter().cloned() {
        push_add_dir(&mut add_dirs, path, &user.home_dir, &user.home_dir);
    }

    add_dirs
}

fn extend_codex_args_with_add_dirs(
    mut codex_args: Vec<String>,
    add_dirs: &[PathBuf],
    user: &UserContext,
) -> Vec<String> {
    let existing_add_dirs = resolve_add_dir_paths(&codex_args, &[], user);

    for path in add_dirs {
        if existing_add_dirs.iter().any(|existing| existing == path) {
            continue;
        }

        codex_args.push("--add-dir".into());
        codex_args.push(path.to_string_lossy().into_owned());
    }

    codex_args
}

fn extract_add_dir_paths(codex_args: &[String]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut index = 0;

    while index < codex_args.len() {
        let arg = &codex_args[index];
        if arg == "--add-dir" {
            if let Some(path) = codex_args.get(index + 1) {
                paths.push(PathBuf::from(path));
                index += 1;
            }
        } else if let Some(path) = add_dir_inline_value(arg) {
            paths.push(path);
        }

        index += 1;
    }

    paths
}

fn add_dir_inline_value(arg: &str) -> Option<PathBuf> {
    arg.strip_prefix("--add-dir=")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn push_add_dir(add_dirs: &mut Vec<PathBuf>, path: PathBuf, base_dir: &Path, home_dir: &Path) {
    let Some(path) = normalize_add_dir(path, base_dir, home_dir) else {
        return;
    };

    if !add_dirs.contains(&path) {
        add_dirs.push(path);
    }
}

fn normalize_add_dir(path: PathBuf, base_dir: &Path, home_dir: &Path) -> Option<PathBuf> {
    let path = expand_tilde(path, home_dir);
    let path = if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    };

    let path = fs::canonicalize(&path).unwrap_or(path);
    path.is_dir().then_some(path)
}

fn expand_tilde(path: PathBuf, home_dir: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return home_dir.to_path_buf();
    }

    if let Some(stripped) = raw.strip_prefix("~/") {
        return home_dir.join(stripped);
    }

    path
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        add_dir_mounts, codex_command, extend_codex_args_with_add_dirs, extract_add_dir_paths,
        plan_default_codex_command, resolve_add_dir_paths,
    };
    use crate::mounts::{MountMode, MountSource};
    use crate::user_context::UserContext;

    #[test]
    fn extract_add_dir_paths_supports_split_and_inline_forms() {
        let paths = extract_add_dir_paths(&[
            "--model".into(),
            "gpt-5.4".into(),
            "--add-dir".into(),
            "../shared".into(),
            "--add-dir=/tmp/cache".into(),
        ]);

        assert_eq!(
            paths,
            vec![PathBuf::from("../shared"), PathBuf::from("/tmp/cache")]
        );
    }

    #[test]
    fn resolve_add_dir_paths_merge_cli_and_workspace_entries() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        let sibling = dir.path().join("shared");
        let configured = dir.path().join("configured");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::create_dir_all(&configured).unwrap();

        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home,
            cwd,
        };

        let add_dirs = resolve_add_dir_paths(
            &[
                "--add-dir".into(),
                "../shared".into(),
                format!("--add-dir={}", sibling.display()),
            ],
            &[PathBuf::from("../configured"), PathBuf::from("/missing")],
            &user,
        );

        assert_eq!(
            add_dirs,
            vec![
                sibling.canonicalize().unwrap(),
                configured.canonicalize().unwrap()
            ]
        );
    }

    #[test]
    fn add_dir_mounts_map_existing_directories_without_approval() {
        let add_dir = PathBuf::from("/tmp/shared");

        let mounts = add_dir_mounts(std::slice::from_ref(&add_dir));

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].host, add_dir);
        assert_eq!(mounts[0].guest, PathBuf::from("/tmp/shared"));
        assert_eq!(mounts[0].mode, MountMode::ReadWrite);
        assert_eq!(mounts[0].source, MountSource::CodexAddDir);
    }

    #[test]
    fn extend_codex_args_with_add_dirs_appends_missing_configured_dirs() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        let shared = dir.path().join("shared");
        let configured = dir.path().join("configured");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&configured).unwrap();

        let args = extend_codex_args_with_add_dirs(
            vec![
                "--model".into(),
                "gpt-5.4".into(),
                "--add-dir".into(),
                shared.to_string_lossy().into_owned(),
            ],
            &[shared.clone(), configured.clone()],
            &UserContext {
                uid: 1000,
                gid: 1000,
                home_dir: home,
                cwd,
            },
        );

        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "gpt-5.4".to_string(),
                "--add-dir".to_string(),
                shared.to_string_lossy().into_owned(),
                "--add-dir".to_string(),
                configured.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn plan_default_codex_command_wraps_codex_and_appends_configured_dirs() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        let configured = dir.path().join("configured");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&configured).unwrap();

        let plan = plan_default_codex_command(
            vec!["--model".into(), "gpt-5.4".into()],
            std::slice::from_ref(&configured),
            &UserContext {
                uid: 1000,
                gid: 1000,
                home_dir: home,
                cwd,
            },
        );

        assert_eq!(plan.paths, vec![configured.clone()]);
        assert_eq!(
            plan.command,
            vec![
                "codex".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "--model".to_string(),
                "gpt-5.4".to_string(),
                "--add-dir".to_string(),
                configured.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn codex_command_wraps_codex_args_in_argv_form() {
        assert_eq!(
            codex_command(vec!["--model".into(), "gpt-5.4".into()]),
            vec![
                "codex",
                "--dangerously-bypass-approvals-and-sandbox",
                "--model",
                "gpt-5.4"
            ]
        );
    }
}
