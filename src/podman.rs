use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::config::UserContext;
use crate::env_filter::ForwardedEnv;
use crate::errors::{CodexboxError, Result};
use crate::mounts::{MountMode, MountSpec};

pub const DEFAULT_IMAGE: &str = "localhost/codexbox:latest";
const PATH_PREFIX_ENV: &str = "CODEXBOX_PATH_PREFIX";
const IMAGE_EXPORT_DIR_ENV: &str = "CODEXBOX_IMAGE_EXPORT_DIR";
const IMAGE_FINGERPRINT_LABEL: &str = "io.github.codexbox.image-fingerprint";
const CONTAINERFILE: &str = include_str!("../Containerfile");
const CONTAINERS_CONF: &[u8] = include_bytes!("../containers.conf");
const PODMAN_CONTAINERS_CONF: &[u8] = include_bytes!("../podman-containers.conf");
const CONTAINER_ENTRYPOINT: &[u8] = include_bytes!("../container-entrypoint.sh");

#[derive(Debug, Clone)]
pub struct PodmanPlan {
    pub image: String,
    pub mounts: Vec<MountSpec>,
    pub publish: Vec<String>,
    pub env: ForwardedEnv,
    pub extra_env: Vec<(String, String)>,
    pub command: Vec<String>,
    pub home_dir: std::path::PathBuf,
    pub workdir: std::path::PathBuf,
}

struct EmbeddedBuildContext {
    tempdir: TempDir,
}

impl EmbeddedBuildContext {
    fn create() -> Result<Self> {
        let temp_root = std::env::temp_dir();
        let tempdir = tempfile::Builder::new()
            .prefix("codexbox-build-")
            .tempdir_in(&temp_root)
            .map_err(|source| CodexboxError::WritePath {
                path: temp_root,
                source,
            })?;

        write_embedded_asset(
            tempdir.path().join("Containerfile"),
            CONTAINERFILE.as_bytes(),
        )?;
        write_embedded_asset(tempdir.path().join("containers.conf"), CONTAINERS_CONF)?;
        write_embedded_asset(
            tempdir.path().join("podman-containers.conf"),
            PODMAN_CONTAINERS_CONF,
        )?;
        write_embedded_asset(
            tempdir.path().join("container-entrypoint.sh"),
            CONTAINER_ENTRYPOINT,
        )?;

        Ok(Self { tempdir })
    }

    fn path(&self) -> &Path {
        self.tempdir.path()
    }

    fn containerfile_path(&self) -> PathBuf {
        self.tempdir.path().join("Containerfile")
    }
}

pub struct ImageExportDir {
    tempdir: TempDir,
}

impl ImageExportDir {
    pub fn path(&self) -> &Path {
        self.tempdir.path()
    }
}

pub fn ensure_image(image: &str, rebuild: bool) -> Result<()> {
    let fingerprint = embedded_image_fingerprint();
    if !rebuild && image_is_fresh(image, &fingerprint)? {
        return Ok(());
    }

    build_image(image, &fingerprint)
}

fn build_image(image: &str, fingerprint: &str) -> Result<()> {
    let context = EmbeddedBuildContext::create()?;

    let status = Command::new("podman")
        .arg("build")
        .arg("--tag")
        .arg(image)
        .arg("--label")
        .arg(format!("{IMAGE_FINGERPRINT_LABEL}={fingerprint}"))
        .arg("--file")
        .arg(context.containerfile_path())
        .arg(context.path())
        .status()
        .map_err(CodexboxError::PodmanSpawn)?;

    if status.success() {
        Ok(())
    } else {
        Err(CodexboxError::PodmanBuildFailed(status_to_string(
            status.code(),
        )))
    }
}

fn image_is_fresh(image: &str, fingerprint: &str) -> Result<bool> {
    if !image_exists(image)? {
        return Ok(false);
    }

    let output = Command::new("podman")
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg(image_fingerprint_format())
        .arg(image)
        .output()
        .map_err(CodexboxError::PodmanSpawn)?;

    if !output.status.success() {
        return Ok(false);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim() == fingerprint)
}

pub fn run_plan(plan: &PodmanPlan, user: &UserContext) -> Result<i32> {
    let mut command = build_command(plan, user);
    let status = command.status().map_err(CodexboxError::PodmanSpawn)?;
    Ok(status.code().unwrap_or(1))
}

fn write_embedded_asset(path: PathBuf, contents: &[u8]) -> Result<()> {
    fs::write(&path, contents).map_err(|source| CodexboxError::WritePath { path, source })
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
        .arg("--security-opt")
        .arg("label=disable")
        .arg("--workdir")
        .arg(&plan.workdir)
        .arg("--hostname")
        .arg("codexbox")
        .arg("--annotation")
        .arg(format!("codexbox.uid={}", user.uid))
        .arg("--annotation")
        .arg(format!("codexbox.gid={}", user.gid));

    for publish in &plan.publish {
        command.arg("--publish").arg(publish);
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

fn image_exists(image: &str) -> Result<bool> {
    let status = Command::new("podman")
        .arg("image")
        .arg("exists")
        .arg(image)
        .status()
        .map_err(CodexboxError::PodmanSpawn)?;

    Ok(status.success())
}

fn image_fingerprint_format() -> String {
    format!("{{{{ index .Config.Labels \"{IMAGE_FINGERPRINT_LABEL}\" }}}}")
}

fn embedded_image_fingerprint() -> String {
    let mut hash = Fnv1a64::new();

    for chunk in [
        env!("CARGO_PKG_VERSION").as_bytes(),
        CONTAINERFILE.as_bytes(),
        CONTAINERS_CONF,
        PODMAN_CONTAINERS_CONF,
        CONTAINER_ENTRYPOINT,
    ] {
        hash.write(chunk);
        hash.write(&[0]);
    }

    format!("{:016x}", hash.finish())
}

struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

pub fn create_image_export_dir(user: &UserContext) -> Result<ImageExportDir> {
    let root = user
        .home_dir
        .join(".local")
        .join("share")
        .join("codexbox")
        .join("image-exports");
    std::fs::create_dir_all(&root).map_err(|source| CodexboxError::WritePath {
        path: root.clone(),
        source,
    })?;

    let prefix = format!("run-{}-", std::process::id());
    let tempdir = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(&root)
        .map_err(|source| CodexboxError::WritePath { path: root, source })?;

    Ok(ImageExportDir { tempdir })
}

pub fn dry_run_image_export_dir(user: &UserContext) -> PathBuf {
    user.home_dir
        .join(".local")
        .join("share")
        .join("codexbox")
        .join("image-exports")
        .join("dry-run")
}

pub fn image_export_env(guest_dir: &Path) -> (String, String) {
    (
        IMAGE_EXPORT_DIR_ENV.to_string(),
        guest_dir.display().to_string(),
    )
}

pub fn import_exported_images(export_dir: &Path) -> Result<()> {
    let mut archives = Vec::new();
    for entry in std::fs::read_dir(export_dir).map_err(|source| CodexboxError::ReadPath {
        path: export_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| CodexboxError::ReadPath {
            path: export_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("tar") {
            archives.push(path);
        }
    }

    archives.sort();

    for archive in archives {
        let status = Command::new("podman")
            .arg("load")
            .arg("--input")
            .arg(&archive)
            .status()
            .map_err(CodexboxError::PodmanSpawn)?;
        if !status.success() {
            return Err(CodexboxError::PodmanLoadFailed {
                path: archive,
                status: status_to_string(status.code()),
            });
        }
    }

    Ok(())
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

fn status_to_string(code: Option<i32>) -> String {
    code.map(|value| value.to_string())
        .unwrap_or_else(|| "signal".to_string())
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

    use crate::config::UserContext;
    use crate::env_filter::ForwardedEnv;
    use crate::mounts::{MountMode, MountSource, MountSpec};

    use super::{
        embedded_image_fingerprint, format_mount, image_fingerprint_format, render_plan, PodmanPlan,
    };

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
            publish: vec!["127.0.0.1:8080:80".into()],
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
        assert!(!rendered.contains("--env PATH="));
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
            publish: vec!["9090:90".into()],
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

    #[test]
    fn embedded_image_fingerprint_is_stable_shape() {
        let fingerprint = embedded_image_fingerprint();

        assert_eq!(fingerprint.len(), 16);
        assert!(fingerprint.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(fingerprint, embedded_image_fingerprint());
    }

    #[test]
    fn image_inspect_format_reads_the_fingerprint_label() {
        assert_eq!(
            image_fingerprint_format(),
            "{{ index .Config.Labels \"io.github.codexbox.image-fingerprint\" }}"
        );
    }
}
