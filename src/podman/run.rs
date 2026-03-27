use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;

use crate::config::PublishSpec;
use crate::errors::{CodexboxError, Result};
use crate::sandbox::env_filter::ForwardedEnv;
use crate::sandbox::mounts::{MountMode, MountSpec};
use crate::user_context::UserContext;

const PATH_PREFIX_ENV: &str = "CODEXBOX_PATH_PREFIX";

#[derive(Debug, Clone)]
pub struct PodmanPlan {
    pub image: String,
    pub mounts: Vec<MountSpec>,
    pub publish: Vec<PublishSpec>,
    pub env: ForwardedEnv,
    pub extra_env: Vec<(String, String)>,
    pub command: Vec<String>,
    pub home_dir: std::path::PathBuf,
    pub workdir: std::path::PathBuf,
}

pub fn run_plan(plan: &PodmanPlan, user: &UserContext) -> Result<i32> {
    let mut command = build_command(plan, user);
    let status = command.status().map_err(CodexboxError::PodmanSpawn)?;
    Ok(status.code().unwrap_or(1))
}

pub fn render_plan(plan: &PodmanPlan, user: &UserContext) -> String {
    let command = build_command(plan, user);
    let mut parts = vec![shell_quote(command.get_program())];
    parts.extend(command.get_args().map(shell_quote));
    parts.join(" ")
}

fn build_command(plan: &PodmanPlan, user: &UserContext) -> Command {
    let mut command = Command::new("podman");
    command.arg("run").arg("--rm").arg("-i");

    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        command.arg("-t");
    }

    command
        .arg("--sysctl")
        .arg("net.ipv4.ip_unprivileged_port_start=0")
        .arg("--device")
        .arg("/dev/net/tun")
        .arg("--device")
        .arg("/dev/fuse")
        .arg("--workdir")
        .arg(&plan.workdir)
        .arg("--annotation")
        .arg(format!("codexbox.uid={}", user.uid))
        .arg("--annotation")
        .arg(format!("codexbox.gid={}", user.gid));

    for publish in &plan.publish {
        command.arg("--publish").arg(publish.to_string());
    }

    for mount in &plan.mounts {
        command.arg("--mount").arg(format_mount(mount));
    }

    command
        .arg("--env")
        .arg(format!("HOME={}", plan.home_dir.display()));

    if let Some(path_prefix) = &plan.env.path_prefix {
        let mut pair = String::from(PATH_PREFIX_ENV);
        pair.push('=');
        pair.push_str(path_prefix);
        command.arg("--env").arg(pair);
    }

    for (key, value) in &plan.extra_env {
        command.arg("--env").arg(format!("{key}={value}"));
    }

    for (key, value) in &plan.env.vars {
        if key == "PATH" {
            continue;
        }

        let mut pair = key.clone();
        pair.push('=');
        pair.push_str(value);
        command.arg("--env").arg(pair);
    }

    command.arg(&plan.image);
    for arg in &plan.command {
        command.arg(arg);
    }

    command
}

fn format_mount(mount: &MountSpec) -> String {
    let mut parts = vec![
        "type=bind".to_string(),
        format!("src={}", escape_mount_value(&mount.host)),
        format!("target={}", escape_mount_value(&mount.guest)),
    ];

    if mount.mode == MountMode::ReadOnly {
        parts.push("ro".to_string());
    }

    parts.join(",")
}

fn escape_mount_value(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace(',', "\\,")
}

fn shell_quote(value: &std::ffi::OsStr) -> String {
    let text = value.to_string_lossy();
    if !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return text.into_owned();
    }

    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::str::FromStr;

    use crate::config::PublishSpec;
    use crate::sandbox::env_filter::ForwardedEnv;
    use crate::sandbox::mounts::{MountMode, MountSource, MountSpec};
    use crate::user_context::UserContext;

    use super::{format_mount, render_plan, PodmanPlan};

    #[test]
    fn format_mount_uses_bind_syntax() {
        let mount = MountSpec {
            host: PathBuf::from("/host"),
            guest: PathBuf::from("/guest"),
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        };

        assert_eq!(format_mount(&mount), "type=bind,src=/host,target=/guest,ro");
    }

    #[test]
    fn format_mount_preserves_colons_in_paths() {
        let mount = MountSpec {
            host: PathBuf::from("/host:with:colon"),
            guest: PathBuf::from("/guest:with:colon"),
            mode: MountMode::ReadWrite,
            source: MountSource::Fixed,
        };

        assert_eq!(
            format_mount(&mount),
            "type=bind,src=/host:with:colon,target=/guest:with:colon"
        );
    }

    #[test]
    fn render_plan_shows_full_podman_command() {
        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: PathBuf::from("/home/test user"),
            cwd: PathBuf::from("/work tree"),
        };
        let plan = PodmanPlan {
            image: "localhost/codexbox:latest".into(),
            mounts: vec![MountSpec {
                host: PathBuf::from("/host path"),
                guest: PathBuf::from("/guest path"),
                mode: MountMode::ReadWrite,
                source: MountSource::Fixed,
            }],
            publish: vec![PublishSpec::from_str("127.0.0.1:8080:80").unwrap()],
            env: ForwardedEnv {
                vars: Default::default(),
                path_prefix: Some("/opt/bin:/custom/bin".into()),
            },
            extra_env: vec![(
                "CODEXBOX_IMAGE_EXPORT_DIR".into(),
                "/var/lib/codexbox-image-exports".into(),
            )],
            command: vec![
                "codex".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "--model".into(),
                "gpt-5.4".into(),
            ],
            home_dir: user.home_dir.clone(),
            workdir: user.cwd.clone(),
        };

        let rendered = render_plan(&plan, &user);

        assert!(rendered.contains("podman run --rm -i"));
        assert!(rendered.contains("--workdir '/work tree'"));
        assert!(rendered.contains("--publish 127.0.0.1:8080:80"));
        assert!(rendered.contains("--mount 'type=bind,src=/host path,target=/guest path'"));
        assert!(rendered.contains("--env CODEXBOX_PATH_PREFIX=/opt/bin:/custom/bin"));
        assert!(
            rendered.contains("--env CODEXBOX_IMAGE_EXPORT_DIR=/var/lib/codexbox-image-exports")
        );
        assert!(!rendered.contains("--hostname"));
        assert!(!rendered.contains("--env PATH="));
        assert!(!rendered.contains("--security-opt"));
        assert!(rendered.contains(
            "localhost/codexbox:latest codex --dangerously-bypass-approvals-and-sandbox --model gpt-5.4"
        ));
    }

    #[test]
    fn render_plan_supports_argv_command_override() {
        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: PathBuf::from("/home/test"),
            cwd: PathBuf::from("/work"),
        };
        let plan = PodmanPlan {
            image: "localhost/codexbox:latest".into(),
            mounts: Vec::new(),
            publish: vec![PublishSpec::from_str("9090:90").unwrap()],
            env: ForwardedEnv {
                vars: Default::default(),
                path_prefix: None,
            },
            extra_env: Vec::new(),
            command: vec!["podman".into(), "info".into()],
            home_dir: user.home_dir.clone(),
            workdir: user.cwd.clone(),
        };

        let rendered = render_plan(&plan, &user);

        assert!(rendered.contains("--publish 9090:90"));
        assert!(rendered.ends_with("localhost/codexbox:latest podman info"));
    }
}
