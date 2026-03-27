use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use crate::approval::{approve_candidates, StdioApprovalPrompt};
use crate::certs::discover_ca_trust_paths;
use crate::cli::Cli;
use crate::codex_config::{existing_writable_roots, load_codex_toml};
use crate::config::{load_launcher_config, load_workspace_codexbox_config, UserContext};
use crate::env_filter::filter_environment;
use crate::env_mounts::discover_env_mount_candidates;
use crate::errors::Result;
use crate::mounts::{
    approved_env_mounts, base_mounts, ca_mounts, combine_mounts, filter_covered_env_candidates,
    MountMode, MountSource, MountSpec,
};
use crate::podman::{
    create_image_export_dir, dry_run_image_export_dir, ensure_image, image_export_env,
    import_exported_images, render_plan, run_plan, ContainerCommand, PodmanPlan, DEFAULT_IMAGE,
};

pub fn launch(cli: Cli) -> Result<i32> {
    let Cli {
        image,
        rebuild_image,
        dry_run,
        publish,
        container_command,
        codex_args,
    } = cli;

    let user = UserContext::detect()?;
    let config = load_launcher_config(&user);
    let workspace_config = load_workspace_codexbox_config(&user.cwd)?;
    let filtered_env = filter_environment(&config.ignore_var_patterns)?;
    let codex_config = load_codex_toml(&user.home_dir)?;
    let writable_roots = existing_writable_roots(&codex_config, &user.home_dir);
    let add_dirs = resolve_add_dir_paths(&codex_args, &workspace_config.add_dirs, &user);
    let base_mounts = combine_mounts(&[
        base_mounts(&user, &writable_roots)?,
        codex_add_dir_mounts(&add_dirs),
    ]);
    let env_candidates = discover_env_mount_candidates(&filtered_env, &user.home_dir);
    let env_candidates = filter_covered_env_candidates(env_candidates, &base_mounts);

    let approved_candidates = if dry_run {
        env_candidates
    } else {
        let mut prompt = StdioApprovalPrompt;
        approve_candidates(env_candidates, &config.approval_db_path, &mut prompt)?
    };
    let ca_paths = discover_ca_trust_paths();
    let image = image.unwrap_or_else(|| DEFAULT_IMAGE.to_string());
    let publish = merge_publish(&workspace_config.publish, publish);
    let container_command = match container_command {
        Some(command) => ContainerCommand::Shell(command),
        None => ContainerCommand::Codex(extend_codex_args_with_add_dirs(
            codex_args, &add_dirs, &user,
        )),
    };
    let export_guest_dir = std::path::PathBuf::from("/var/lib/codexbox-image-exports");

    if dry_run {
        let plan = PodmanPlan {
            image,
            mounts: combine_mounts(&[
                base_mounts,
                approved_env_mounts(&approved_candidates),
                ca_mounts(&ca_paths),
                vec![MountSpec {
                    host: dry_run_image_export_dir(&user),
                    guest: export_guest_dir.clone(),
                    mode: MountMode::ReadWrite,
                    source: MountSource::Podman,
                }],
            ]),
            publish,
            env: filtered_env,
            extra_env: vec![image_export_env(&export_guest_dir)],
            container_command,
            home_dir: user.home_dir.clone(),
            workdir: user.cwd.clone(),
        };

        println!("{}", render_plan(&plan, &user));
        return Ok(0);
    }

    ensure_image(&image, rebuild_image)?;

    let export_host_dir = create_image_export_dir(&user)?;
    let plan = PodmanPlan {
        image,
        mounts: combine_mounts(&[
            base_mounts,
            approved_env_mounts(&approved_candidates),
            ca_mounts(&ca_paths),
            vec![MountSpec {
                host: export_host_dir.path().to_path_buf(),
                guest: export_guest_dir.clone(),
                mode: MountMode::ReadWrite,
                source: MountSource::Podman,
            }],
        ]),
        publish,
        env: filtered_env,
        extra_env: vec![image_export_env(&export_guest_dir)],
        container_command,
        home_dir: user.home_dir.clone(),
        workdir: user.cwd.clone(),
    };

    let exit_code = run_plan(&plan, &user)?;
    import_exported_images(export_host_dir.path())?;

    Ok(exit_code)
}

fn codex_add_dir_mounts(add_dirs: &[PathBuf]) -> Vec<MountSpec> {
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

fn resolve_add_dir_paths(
    codex_args: &[OsString],
    configured_add_dirs: &[PathBuf],
    user: &UserContext,
) -> Vec<PathBuf> {
    let mut add_dirs = Vec::new();

    for path in extract_add_dir_paths(codex_args)
        .into_iter()
        .chain(configured_add_dirs.iter().cloned())
    {
        let Some(path) = normalize_add_dir(path, user) else {
            continue;
        };

        if !add_dirs.contains(&path) {
            add_dirs.push(path);
        }
    }

    add_dirs
}

fn extend_codex_args_with_add_dirs(
    mut codex_args: Vec<OsString>,
    add_dirs: &[PathBuf],
    user: &UserContext,
) -> Vec<OsString> {
    let existing_add_dirs = resolve_add_dir_paths(&codex_args, &[], user);

    for path in add_dirs {
        if existing_add_dirs.iter().any(|existing| existing == path) {
            continue;
        }

        codex_args.push("--add-dir".into());
        codex_args.push(path.clone().into_os_string());
    }

    codex_args
}

fn merge_publish(configured_publish: &[String], cli_publish: Vec<String>) -> Vec<String> {
    let mut publish = Vec::new();

    for entry in configured_publish.iter().chain(cli_publish.iter()) {
        if !publish.contains(entry) {
            publish.push(entry.clone());
        }
    }

    publish
}

fn extract_add_dir_paths(codex_args: &[OsString]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut index = 0;

    while index < codex_args.len() {
        let arg = &codex_args[index];
        if arg == OsStr::new("--add-dir") {
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

fn add_dir_inline_value(arg: &OsString) -> Option<PathBuf> {
    const PREFIX: &[u8] = b"--add-dir=";

    let bytes = arg.as_os_str().as_bytes();
    if !bytes.starts_with(PREFIX) || bytes.len() == PREFIX.len() {
        return None;
    }

    Some(PathBuf::from(OsString::from_vec(
        bytes[PREFIX.len()..].to_vec(),
    )))
}

fn normalize_add_dir(path: PathBuf, user: &UserContext) -> Option<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        user.cwd.join(path)
    };

    let path = fs::canonicalize(&path).unwrap_or(path);
    path.is_dir().then_some(path)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        codex_add_dir_mounts, extend_codex_args_with_add_dirs, extract_add_dir_paths,
        merge_publish, resolve_add_dir_paths,
    };
    use crate::config::UserContext;
    use crate::mounts::{MountMode, MountSource};

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
                format!("--add-dir={}", sibling.display()).into(),
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
    fn codex_add_dir_mounts_map_existing_directories_without_approval() {
        let add_dir = PathBuf::from("/tmp/shared");

        let mounts = codex_add_dir_mounts(std::slice::from_ref(&add_dir));

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
                shared.clone().into_os_string(),
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
                OsString::from("--model"),
                OsString::from("gpt-5.4"),
                OsString::from("--add-dir"),
                shared.into_os_string(),
                OsString::from("--add-dir"),
                configured.into_os_string(),
            ]
        );
    }

    #[test]
    fn merge_publish_keeps_order_and_deduplicates() {
        let publish = merge_publish(
            &["127.0.0.1:8080:80".into(), "8443:443".into()],
            vec!["8443:443".into(), "3000:3000".into()],
        );

        assert_eq!(publish, vec!["127.0.0.1:8080:80", "8443:443", "3000:3000"]);
    }
}
