use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

use crate::errors::{CodexboxError, Result};
use crate::user_context::UserContext;

use super::assets::{
    embedded_image_fingerprint, CONTAINERFILE, CONTAINERS_CONF, CONTAINER_ENTRYPOINT,
    PODMAN_CONTAINERS_CONF,
};
use super::status_to_string;

pub const DEFAULT_IMAGE: &str = "localhost/codexbox:latest";
const IMAGE_EXPORT_DIR_ENV: &str = "CODEXBOX_IMAGE_EXPORT_DIR";
const IMAGE_FINGERPRINT_LABEL: &str = "io.github.codexbox.image-fingerprint";
const IMAGE_BUILT_AT_LABEL: &str = "io.github.codexbox.image-built-at";
const CODEX_NPM_REFRESH_TOKEN_ARG: &str = "CODEX_NPM_REFRESH_TOKEN";
const MAX_IMAGE_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

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
    let built_at = current_unix_timestamp()?;
    let mut command = Command::new("podman");
    configure_host_podman_command(&mut command);
    command
        .arg("build")
        .arg("--tag")
        .arg(image)
        .arg("--build-arg")
        .arg(format!("{CODEX_NPM_REFRESH_TOKEN_ARG}={built_at}"))
        .arg("--label")
        .arg(format!("{IMAGE_FINGERPRINT_LABEL}={fingerprint}"))
        .arg("--label")
        .arg(format!("{IMAGE_BUILT_AT_LABEL}={built_at}"))
        .arg("--file")
        .arg(context.containerfile_path())
        .arg(context.path());

    let status =
        run_with_stdout_forwarded_to_stderr(&mut command).map_err(CodexboxError::PodmanSpawn)?;

    if status.success() {
        Ok(())
    } else {
        Err(CodexboxError::PodmanBuildFailed(status_to_string(
            status.code(),
        )))
    }
}

fn image_is_fresh(image: &str, fingerprint: &str) -> Result<bool> {
    let output = Command::new("podman")
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg(image_metadata_format())
        .arg(image)
        .output()
        .map_err(CodexboxError::PodmanSpawn)?;

    if !output.status.success() {
        return Ok(false);
    }

    Ok(parse_image_metadata(&output.stdout)
        .is_some_and(|metadata| metadata.fingerprint == fingerprint && !metadata.is_expired()))
}

fn image_metadata_format() -> String {
    format!(
        "{{{{ index .Config.Labels \"{IMAGE_FINGERPRINT_LABEL}\" }}}}|{{{{ index .Config.Labels \"{IMAGE_BUILT_AT_LABEL}\" }}}}"
    )
}

fn parse_image_metadata(stdout: &[u8]) -> Option<ImageMetadata> {
    let text = String::from_utf8_lossy(stdout);
    let mut parts = text.trim().splitn(2, '|');
    let fingerprint = parts.next()?.trim();
    let built_at = parts.next()?.trim().parse().ok()?;

    (!fingerprint.is_empty()).then(|| ImageMetadata {
        fingerprint: fingerprint.to_string(),
        built_at,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageMetadata {
    fingerprint: String,
    built_at: u64,
}

impl ImageMetadata {
    fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(
                self.built_at
                    .saturating_add(MAX_IMAGE_AGE.as_secs())
                    .saturating_add(1),
            );

        now.saturating_sub(self.built_at) > MAX_IMAGE_AGE.as_secs()
    }
}

fn current_unix_timestamp() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| {
            CodexboxError::SystemTime(format!("system clock is before the Unix epoch: {source}"))
        })
}

fn configure_host_podman_command(command: &mut Command) {
    command.arg("--cgroup-manager").arg("cgroupfs");
}

fn run_with_stdout_forwarded_to_stderr(command: &mut Command) -> io::Result<ExitStatus> {
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture podman stdout"))?;
    let forwarder = thread::spawn(move || -> io::Result<()> {
        let mut reader = stdout;
        let mut stderr = io::stderr().lock();
        io::copy(&mut reader, &mut stderr)?;
        stderr.flush()
    });

    let wait_result = child.wait();
    let forward_result = forwarder
        .join()
        .map_err(|_| io::Error::other("stdout forwarding thread panicked"))?;
    let status = wait_result?;
    forward_result?;

    Ok(status)
}

fn write_embedded_asset(path: PathBuf, contents: &[u8]) -> Result<()> {
    fs::write(&path, contents).map_err(|source| CodexboxError::WritePath { path, source })
}

pub fn create_image_export_dir(user: &UserContext) -> Result<ImageExportDir> {
    let root = user
        .home_dir
        .join(".local")
        .join("share")
        .join("codexbox")
        .join("image-exports");
    fs::create_dir_all(&root).map_err(|source| CodexboxError::WritePath {
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
    for entry in fs::read_dir(export_dir).map_err(|source| CodexboxError::ReadPath {
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
        let mut command = Command::new("podman");
        configure_host_podman_command(&mut command);
        command.arg("load").arg("--input").arg(&archive);

        let status = run_with_stdout_forwarded_to_stderr(&mut command)
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

#[cfg(test)]
mod tests {
    use super::{
        configure_host_podman_command, image_metadata_format, parse_image_metadata, ImageMetadata,
        IMAGE_BUILT_AT_LABEL, IMAGE_FINGERPRINT_LABEL,
    };
    use std::process::Command;

    #[test]
    fn image_metadata_format_reads_both_labels() {
        assert_eq!(
            image_metadata_format(),
            format!(
                "{{{{ index .Config.Labels \"{IMAGE_FINGERPRINT_LABEL}\" }}}}|{{{{ index .Config.Labels \"{IMAGE_BUILT_AT_LABEL}\" }}}}"
            )
        );
    }

    #[test]
    fn parse_image_metadata_requires_fingerprint_and_timestamp() {
        assert_eq!(
            parse_image_metadata(b"fingerprint|123"),
            Some(ImageMetadata {
                fingerprint: "fingerprint".into(),
                built_at: 123,
            })
        );
        assert!(parse_image_metadata(b"fingerprint|").is_none());
        assert!(parse_image_metadata(b"|123").is_none());
    }

    #[test]
    fn host_build_command_uses_cgroupfs_manager() {
        let mut command = Command::new("podman");
        configure_host_podman_command(&mut command);

        assert_eq!(
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["--cgroup-manager".to_string(), "cgroupfs".to_string()]
        );
    }
}
