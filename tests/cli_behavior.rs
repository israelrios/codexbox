use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

fn codexbox_bin() -> &'static str {
    env!("CARGO_BIN_EXE_codexbox")
}

fn embedded_image_fingerprint() -> String {
    let mut hash = Fnv1a64::new();

    for chunk in [
        env!("CARGO_PKG_VERSION").as_bytes(),
        include_str!("../Containerfile").as_bytes(),
        include_bytes!("../containers.conf").as_slice(),
        include_bytes!("../podman-containers.conf").as_slice(),
        include_bytes!("../container-entrypoint.sh").as_slice(),
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

fn run_codexbox(
    home_dir: &Path,
    current_dir: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Output {
    let mut command = Command::new(codexbox_bin());
    command
        .args(args)
        .current_dir(current_dir)
        .env_clear()
        .env("HOME", home_dir)
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8");

    for (key, value) in extra_env {
        command.env(key, value);
    }

    command.output().unwrap()
}

fn write_fake_podman(path: &Path) {
    fs::write(
        path,
        r#"#!/bin/sh
log_file="${CODEXBOX_TEST_PODMAN_LOG:?}"
{
    printf 'ARGS:'
    for arg in "$@"; do
        printf '[%s]' "$arg"
    done
    printf '\n'
} >> "$log_file"

if [ "$1" = "image" ] && [ "$2" = "exists" ]; then
    exit 0
fi

if [ "$1" = "image" ] && [ "$2" = "inspect" ]; then
    printf '%s' "${CODEXBOX_TEST_IMAGE_FINGERPRINT_RESPONSE:-}"
    exit 0
fi

exit 0
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn dry_run_does_not_create_runtime_state() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();

    let output = run_codexbox(&home_dir, &workspace, &["--dry-run"], &[]);

    assert!(output.status.success());
    assert!(!home_dir.join(".codex").exists());
    assert!(!home_dir.join(".local/share/codexbox/containers").exists());
}

#[test]
fn user_config_supplies_defaults_and_repo_config_is_ignored() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let shared_dir = home_dir.join("shared");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&shared_dir).unwrap();
    fs::create_dir_all(workspace.join(".codex")).unwrap();
    fs::write(
        workspace.join(".codex/codexbox.json"),
        r#"{"publish":["1111:11"],"add_dirs":["/should/not/appear"]}"#,
    )
    .unwrap();
    fs::write(
        home_dir.join(".codexbox-conf.json"),
        format!(
            r#"{{
  "approved_paths": ["/tmp/global.sock"],
  "publish": ["9090:90"],
  "add_dirs": ["{}"]
}}"#,
            "~/shared"
        ),
    )
    .unwrap();

    let output = run_codexbox(&home_dir, &workspace, &["--dry-run"], &[]);
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("--publish 9090:90"));
    assert!(!stdout.contains("1111:11"));
    assert!(stdout.contains(&format!("src={}", shared_dir.display())));
    assert!(!stdout.contains("/should/not/appear"));
}

#[test]
fn directory_overrides_apply_publish_and_add_dirs() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = home_dir.join("work/project/app");
    let project_extra = home_dir.join("project-extra");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&project_extra).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    fs::write(
        home_dir.join(".codexbox-conf.json"),
        format!(
            r#"{{
  "directories": {{
    "{}": {{
      "publish": ["3000:3000"],
      "add_dirs": ["{}"]
    }}
  }}
}}"#,
            "~/work/project", "~/project-extra"
        ),
    )
    .unwrap();

    let output = run_codexbox(&home_dir, &workspace, &["--dry-run"], &[]);
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("--publish 3000:3000"));
    assert!(stdout.contains(&format!("src={}", project_extra.display())));
}

#[test]
fn dry_run_filters_internal_env_and_url_mount_detection() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();

    let output = run_codexbox(
        &home_dir,
        &workspace,
        &["--dry-run"],
        &[
            ("CODEXBOX_PATH_PREFIX", "/host/prefix"),
            ("SERVICE_URL", "http://tmp"),
            ("GRPC_TARGET", "grpc://tmp"),
        ],
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("--env CODEXBOX_PATH_PREFIX=/bin"));
    assert!(!stdout.contains("/host/prefix"));
    assert!(!stdout.contains("src=//tmp"));
}

#[test]
fn fresh_image_skips_rebuild() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let fake_bin = dir.path().join("fake-bin");
    let podman_log = dir.path().join("podman.log");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    write_fake_podman(&fake_bin.join("podman"));

    let path = format!("{}:/usr/bin:/bin", fake_bin.display());
    let output = Command::new(codexbox_bin())
        .arg("--container-command")
        .arg("podman")
        .arg("info")
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .env(
            "CODEXBOX_TEST_IMAGE_FINGERPRINT_RESPONSE",
            embedded_image_fingerprint(),
        )
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert!(log.contains("ARGS:[image][exists][localhost/codexbox:latest]"));
    assert!(log.contains("ARGS:[image][inspect]"));
    assert!(!log.contains("ARGS:[build]"));
}

#[test]
fn stale_image_triggers_rebuild() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let fake_bin = dir.path().join("fake-bin");
    let podman_log = dir.path().join("podman.log");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    write_fake_podman(&fake_bin.join("podman"));

    let path = format!("{}:/usr/bin:/bin", fake_bin.display());
    let output = Command::new(codexbox_bin())
        .arg("--container-command")
        .arg("podman")
        .arg("info")
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .env("CODEXBOX_TEST_IMAGE_FINGERPRINT_RESPONSE", "stale")
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert!(log.contains("ARGS:[image][inspect]"));
    assert!(log.contains("ARGS:[build]"));
}

#[test]
fn run_uses_argv_container_command_without_shell_env_channel() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let fake_bin = dir.path().join("fake-bin");
    let podman_log = dir.path().join("podman.log");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    write_fake_podman(&fake_bin.join("podman"));

    let path = format!("{}:/usr/bin:/bin", fake_bin.display());
    let mut command = Command::new(codexbox_bin());
    let output = command
        .arg("--container-command")
        .arg("podman")
        .arg("info")
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .env("CODEXBOX_TEST_IMAGE_FINGERPRINT_RESPONSE", "stale")
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert!(log.contains("ARGS:[image][exists][localhost/codexbox:latest]"));
    assert!(log.contains("ARGS:[run]"));
    assert!(log.contains("[localhost/codexbox:latest][podman][info]"));
    assert!(!log.contains("CODEXBOX_CONTAINER_COMMAND"));
}
