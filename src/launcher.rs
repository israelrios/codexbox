use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::cli::Cli;
use crate::config::{existing_writable_roots, load_codex_toml, load_launcher_config, PublishSpec};
use crate::errors::{CodexboxError, Result};
use crate::podman::{
    create_image_export_dir, dry_run_image_export_dir, ensure_image, image_export_env,
    import_exported_images, render_plan, run_plan, PodmanPlan, DEFAULT_IMAGE,
};
use crate::sandbox::add_dirs::{add_dir_mounts, plan_default_codex_command};
use crate::sandbox::approval::{approve_candidates, approved_candidates, StdioApprovalPrompt};
use crate::sandbox::env_filter::{filter_environment, ForwardedEnv};
use crate::sandbox::env_mounts::{discover_env_mount_candidates, EnvMountCandidate};
use crate::sandbox::mounts::{
    approved_env_mounts, base_mounts, ca_mounts, combine_mounts, discover_ca_trust_paths,
    filter_covered_env_candidates, has_ssh_known_hosts_mount, mount_covers_path,
    prepare_runtime_dirs, MountMode, MountSource, MountSpec, GUEST_SSH_KNOWN_HOSTS_SEED,
};
use crate::user_context::UserContext;

const SSH_KNOWN_HOSTS_SEED_ENV: &str = "CODEXBOX_SSH_KNOWN_HOSTS_SEED";

#[derive(Clone)]
enum KnownHostsPlan {
    Seed,
    Overlay(MountSpec),
}

struct RuntimeKnownHostsMount {
    _tempdir: TempDir,
    mount: MountSpec,
}

impl RuntimeKnownHostsMount {
    fn plan(&self) -> KnownHostsPlan {
        KnownHostsPlan::Overlay(self.mount.clone())
    }
}

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
    let mut config = load_launcher_config(&user)?;
    let filtered_env = filter_environment(&config.env_filter)?;
    let codex_config = load_codex_toml(&user.home_dir)?;
    let writable_roots = existing_writable_roots(&codex_config, &user.home_dir);
    let add_dir_plan =
        plan_default_codex_command(codex_args, &config.effective_config.add_dirs, &user);
    let base_mounts = combine_mounts(&[
        base_mounts(&user, &writable_roots)?,
        add_dir_mounts(&add_dir_plan.paths),
    ]);
    let runtime_known_hosts_mount = prepare_runtime_known_hosts_mount(&user, &base_mounts)?;
    let known_hosts_plan = plan_known_hosts_mount(&base_mounts, runtime_known_hosts_mount.as_ref());
    let env_candidates = discover_env_mount_candidates(&filtered_env, &user.home_dir);
    let env_candidates = filter_covered_env_candidates(env_candidates, &base_mounts);

    let approved_candidates = if dry_run {
        approved_candidates(
            env_candidates,
            &config.effective_config.approved_paths,
            &config.effective_config.approved_socket_vars,
        )
    } else {
        let mut prompt = StdioApprovalPrompt;
        approve_candidates(
            env_candidates,
            &config.effective_config.approved_paths,
            &config.effective_config.approved_socket_vars,
            &mut config.user_config,
            &config.config_path,
            &mut prompt,
        )?
    };

    let image = image.unwrap_or_else(|| DEFAULT_IMAGE.to_string());
    let publish = merge_publish(&config.effective_config.publish, publish);
    let command = container_command.unwrap_or(add_dir_plan.command);
    let export_guest_dir = PathBuf::from("/var/lib/codexbox-image-exports");
    let ca_paths = discover_ca_trust_paths();

    if dry_run {
        let plan = build_plan(PlanRequest {
            user: &user,
            image,
            publish,
            filtered_env,
            base_mounts,
            approved_candidates: &approved_candidates,
            ca_paths: &ca_paths,
            export_host_dir: dry_run_image_export_dir(&user),
            export_guest_dir,
            command,
            known_hosts_plan: known_hosts_plan.clone(),
        });
        println!("{}", render_plan(&plan, &user));
        return Ok(0);
    }

    prepare_runtime_dirs(&user)?;
    ensure_image(&image, rebuild_image)?;

    let export_host_dir = create_image_export_dir(&user)?;
    let plan = build_plan(PlanRequest {
        user: &user,
        image,
        publish,
        filtered_env,
        base_mounts,
        approved_candidates: &approved_candidates,
        ca_paths: &ca_paths,
        export_host_dir: export_host_dir.path().to_path_buf(),
        export_guest_dir: PathBuf::from("/var/lib/codexbox-image-exports"),
        command,
        known_hosts_plan,
    });

    let exit_code = run_plan(&plan, &user)?;
    import_exported_images(export_host_dir.path())?;

    Ok(exit_code)
}

struct PlanRequest<'a> {
    user: &'a UserContext,
    image: String,
    publish: Vec<PublishSpec>,
    filtered_env: ForwardedEnv,
    base_mounts: Vec<MountSpec>,
    approved_candidates: &'a [EnvMountCandidate],
    ca_paths: &'a [PathBuf],
    export_host_dir: PathBuf,
    export_guest_dir: PathBuf,
    command: Vec<String>,
    known_hosts_plan: Option<KnownHostsPlan>,
}

fn build_plan(request: PlanRequest<'_>) -> PodmanPlan {
    let mut extra_env = vec![image_export_env(&request.export_guest_dir)];
    let mut mounts = combine_mounts(&[
        request.base_mounts,
        approved_env_mounts(request.approved_candidates),
        ca_mounts(request.ca_paths),
        vec![MountSpec {
            host: request.export_host_dir,
            guest: request.export_guest_dir.clone(),
            mode: MountMode::ReadWrite,
            source: MountSource::Podman,
        }],
    ]);

    match request.known_hosts_plan {
        Some(KnownHostsPlan::Seed) => extra_env.push((
            SSH_KNOWN_HOSTS_SEED_ENV.into(),
            GUEST_SSH_KNOWN_HOSTS_SEED.into(),
        )),
        Some(KnownHostsPlan::Overlay(runtime_known_hosts_mount)) => {
            mounts.retain(|mount| {
                mount.guest != Path::new(GUEST_SSH_KNOWN_HOSTS_SEED)
                    && mount.guest != runtime_known_hosts_mount.guest
            });
            mounts.push(runtime_known_hosts_mount);
        }
        None => {}
    }

    PodmanPlan {
        image: request.image,
        mounts,
        publish: request.publish,
        env: request.filtered_env,
        extra_env,
        command: request.command,
        home_dir: request.user.home_dir.clone(),
        workdir: request.user.cwd.clone(),
    }
}

fn prepare_runtime_known_hosts_mount(
    user: &UserContext,
    mounts: &[MountSpec],
) -> Result<Option<RuntimeKnownHostsMount>> {
    let known_hosts_path = user.home_dir.join(".ssh").join("known_hosts");
    let covered_by_writable_mount = mounts.iter().any(|mount| {
        mount.mode == MountMode::ReadWrite && mount_covers_path(mount, &known_hosts_path)
    });
    if !known_hosts_path.exists() || !covered_by_writable_mount {
        return Ok(None);
    }

    let temp_root = std::env::temp_dir();
    let tempdir = tempfile::Builder::new()
        .prefix("codexbox-known-hosts-")
        .tempdir_in(&temp_root)
        .map_err(|source| CodexboxError::WritePath {
            path: temp_root,
            source,
        })?;
    let runtime_known_hosts_path = tempdir.path().join("known_hosts");
    let seed = fs::read(&known_hosts_path).map_err(|source| CodexboxError::ReadPath {
        path: known_hosts_path.clone(),
        source,
    })?;
    fs::write(&runtime_known_hosts_path, seed).map_err(|source| CodexboxError::WritePath {
        path: runtime_known_hosts_path.clone(),
        source,
    })?;

    Ok(Some(RuntimeKnownHostsMount {
        _tempdir: tempdir,
        mount: MountSpec {
            host: runtime_known_hosts_path,
            guest: known_hosts_path,
            mode: MountMode::ReadWrite,
            source: MountSource::Fixed,
        },
    }))
}

fn plan_known_hosts_mount(
    base_mounts: &[MountSpec],
    runtime_known_hosts_mount: Option<&RuntimeKnownHostsMount>,
) -> Option<KnownHostsPlan> {
    runtime_known_hosts_mount
        .map(RuntimeKnownHostsMount::plan)
        .or_else(|| has_ssh_known_hosts_mount(base_mounts).then_some(KnownHostsPlan::Seed))
}

fn merge_publish(
    configured_publish: &[PublishSpec],
    cli_publish: Vec<PublishSpec>,
) -> Vec<PublishSpec> {
    let mut publish = Vec::new();

    for entry in configured_publish.iter().chain(cli_publish.iter()) {
        if !publish.contains(entry) {
            publish.push(entry.clone());
        }
    }

    publish
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    use crate::config::PublishSpec;
    use crate::sandbox::env_filter::ForwardedEnv;
    use crate::sandbox::mounts::{base_mounts, MountMode, GUEST_SSH_KNOWN_HOSTS_SEED};
    use crate::user_context::UserContext;
    use tempfile::tempdir;

    use super::{
        build_plan, merge_publish, plan_known_hosts_mount, prepare_runtime_known_hosts_mount,
        PlanRequest, SSH_KNOWN_HOSTS_SEED_ENV,
    };

    #[test]
    fn merge_publish_keeps_order_and_deduplicates() {
        let publish = merge_publish(
            &[
                PublishSpec::from_str("127.0.0.1:8080:80").unwrap(),
                PublishSpec::from_str("8443:443").unwrap(),
            ],
            vec![
                PublishSpec::from_str("8443:443").unwrap(),
                PublishSpec::from_str("3000:3000").unwrap(),
            ],
        );

        assert_eq!(
            publish,
            vec![
                PublishSpec::from_str("127.0.0.1:8080:80").unwrap(),
                PublishSpec::from_str("8443:443").unwrap(),
                PublishSpec::from_str("3000:3000").unwrap()
            ]
        );
    }

    #[test]
    fn build_plan_overlays_seeded_known_hosts_when_home_is_writable() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        let known_hosts = home.join(".ssh/known_hosts");
        fs::create_dir_all(known_hosts.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(&known_hosts, "github.com ssh-ed25519 AAAA\n").unwrap();

        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home.clone(),
            cwd,
        };
        let base_mounts = base_mounts(&user, std::slice::from_ref(&home)).unwrap();
        let runtime_known_hosts_mount = prepare_runtime_known_hosts_mount(&user, &base_mounts)
            .unwrap()
            .unwrap();

        assert_eq!(
            fs::read_to_string(&runtime_known_hosts_mount.mount.host).unwrap(),
            "github.com ssh-ed25519 AAAA\n"
        );
        let known_hosts_plan =
            plan_known_hosts_mount(&base_mounts, Some(&runtime_known_hosts_mount));

        let plan = build_plan(PlanRequest {
            user: &user,
            image: "localhost/codexbox:test".into(),
            publish: Vec::new(),
            filtered_env: ForwardedEnv::default(),
            base_mounts,
            approved_candidates: &[],
            ca_paths: &[],
            export_host_dir: dir.path().join("exports"),
            export_guest_dir: PathBuf::from("/var/lib/codexbox-image-exports"),
            command: vec!["codex".into()],
            known_hosts_plan,
        });

        assert!(plan.mounts.iter().any(|mount| {
            mount.host == runtime_known_hosts_mount.mount.host
                && mount.guest == known_hosts
                && mount.mode == MountMode::ReadWrite
        }));
        assert!(!plan
            .mounts
            .iter()
            .any(|mount| mount.guest == Path::new(GUEST_SSH_KNOWN_HOSTS_SEED)));
        assert!(!plan
            .extra_env
            .iter()
            .any(|(key, _)| key == SSH_KNOWN_HOSTS_SEED_ENV));
    }
}
