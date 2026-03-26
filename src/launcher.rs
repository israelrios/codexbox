use crate::approval::{approve_candidates, StdioApprovalPrompt};
use crate::certs::discover_ca_trust_paths;
use crate::cli::Cli;
use crate::codex_config::{existing_writable_roots, load_codex_toml};
use crate::config::{load_launcher_config, RuntimeAssets, UserContext};
use crate::env_filter::filter_environment;
use crate::env_mounts::discover_env_mount_candidates;
use crate::errors::Result;
use crate::mounts::{
    approved_env_mounts, base_mounts, ca_mounts, combine_mounts, filter_covered_env_candidates,
    MountMode, MountSource, MountSpec,
};
use crate::podman::{
    create_image_export_dir, dry_run_image_export_dir, ensure_image, image_export_env,
    import_exported_images, remove_image_export_dir, render_plan, run_plan, ContainerCommand,
    PodmanPlan, DEFAULT_IMAGE,
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
    let assets = RuntimeAssets::detect()?;
    let config = load_launcher_config(&assets, &user)?;
    let filtered_env = filter_environment(&config.ignore_var_patterns)?;
    let codex_config = load_codex_toml(&user.home_dir)?;
    let writable_roots = existing_writable_roots(&codex_config, &user.home_dir);
    let base_mounts = base_mounts(&user, &writable_roots);
    let env_candidates = discover_env_mount_candidates(&filtered_env, &user.home_dir);
    let env_candidates = filter_covered_env_candidates(env_candidates, &base_mounts);

    let approved_candidates = if dry_run {
        env_candidates
    } else {
        let mut prompt = StdioApprovalPrompt;
        approve_candidates(env_candidates, &config.approval_db_path, &mut prompt)?
    };
    let ca_paths = discover_ca_trust_paths();
    let export_host_dir = if dry_run {
        dry_run_image_export_dir(&user)
    } else {
        create_image_export_dir(&user)?
    };
    let export_guest_dir = std::path::PathBuf::from("/var/lib/codexbox-image-exports");
    let mounts = combine_mounts(&[
        base_mounts,
        approved_env_mounts(&approved_candidates),
        ca_mounts(&ca_paths),
        vec![MountSpec {
            host: export_host_dir.clone(),
            guest: export_guest_dir.clone(),
            mode: MountMode::ReadWrite,
            source: MountSource::Podman,
        }],
    ]);

    let image = image.unwrap_or_else(|| DEFAULT_IMAGE.to_string());
    let container_command = match container_command {
        Some(command) => ContainerCommand::Shell(command),
        None => ContainerCommand::Codex(codex_args),
    };

    let plan = PodmanPlan {
        image,
        mounts,
        publish,
        env: filtered_env,
        extra_env: vec![image_export_env(&export_guest_dir)],
        container_command,
        home_dir: user.home_dir.clone(),
        workdir: user.cwd.clone(),
    };

    if dry_run {
        println!("{}", render_plan(&plan, &user));
        return Ok(0);
    }

    ensure_image(&assets, &plan.image, rebuild_image)?;

    let exit_code = run_plan(&plan, &user)?;
    import_exported_images(&export_host_dir)?;
    remove_image_export_dir(&export_host_dir)?;

    Ok(exit_code)
}
