use std::path::PathBuf;

use crate::add_dirs::{add_dir_mounts, plan_default_codex_command};
use crate::approval::{approve_candidates, approved_candidates, StdioApprovalPrompt};
use crate::cli::Cli;
use crate::codex_config::{existing_writable_roots, load_codex_toml};
use crate::env_filter::{filter_environment, ForwardedEnv};
use crate::env_mounts::{discover_env_mount_candidates, EnvMountCandidate};
use crate::errors::Result;
use crate::mounts::{
    approved_env_mounts, base_mounts, ca_mounts, combine_mounts, discover_ca_trust_paths,
    filter_covered_env_candidates, prepare_runtime_dirs, MountMode, MountSource, MountSpec,
};
use crate::podman::{
    create_image_export_dir, dry_run_image_export_dir, ensure_image, image_export_env,
    import_exported_images, render_plan, run_plan, PodmanPlan, DEFAULT_IMAGE,
};
use crate::user_config::load_launcher_config;
use crate::user_context::UserContext;

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
    let env_candidates = discover_env_mount_candidates(&filtered_env, &user.home_dir);
    let env_candidates = filter_covered_env_candidates(env_candidates, &base_mounts);

    let approved_candidates = if dry_run {
        approved_candidates(env_candidates, &config.effective_config.approved_paths)
    } else {
        let mut prompt = StdioApprovalPrompt;
        approve_candidates(
            env_candidates,
            &config.effective_config.approved_paths,
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
    });

    let exit_code = run_plan(&plan, &user)?;
    import_exported_images(export_host_dir.path())?;

    Ok(exit_code)
}

struct PlanRequest<'a> {
    user: &'a UserContext,
    image: String,
    publish: Vec<String>,
    filtered_env: ForwardedEnv,
    base_mounts: Vec<MountSpec>,
    approved_candidates: &'a [EnvMountCandidate],
    ca_paths: &'a [PathBuf],
    export_host_dir: PathBuf,
    export_guest_dir: PathBuf,
    command: Vec<String>,
}

fn build_plan(request: PlanRequest<'_>) -> PodmanPlan {
    PodmanPlan {
        image: request.image,
        mounts: combine_mounts(&[
            request.base_mounts,
            approved_env_mounts(request.approved_candidates),
            ca_mounts(request.ca_paths),
            vec![MountSpec {
                host: request.export_host_dir,
                guest: request.export_guest_dir.clone(),
                mode: MountMode::ReadWrite,
                source: MountSource::Podman,
            }],
        ]),
        publish: request.publish,
        env: request.filtered_env,
        extra_env: vec![image_export_env(&request.export_guest_dir)],
        command: request.command,
        home_dir: request.user.home_dir.clone(),
        workdir: request.user.cwd.clone(),
    }
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

#[cfg(test)]
mod tests {
    use super::merge_publish;

    #[test]
    fn merge_publish_keeps_order_and_deduplicates() {
        let publish = merge_publish(
            &["127.0.0.1:8080:80".into(), "8443:443".into()],
            vec!["8443:443".into(), "3000:3000".into()],
        );

        assert_eq!(publish, vec!["127.0.0.1:8080:80", "8443:443", "3000:3000"]);
    }
}
