use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use codexbox::podman::embedded_image_fingerprint;
use codexbox::sandbox::mounts::GUEST_SSH_KNOWN_HOSTS_SEED;
use tempfile::tempdir;

fn codexbox_bin() -> &'static str {
    env!("CARGO_BIN_EXE_codexbox")
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
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

if [ "$1" = "--cgroup-manager" ]; then
    shift 2
fi

if [ "$1" = "image" ] && [ "$2" = "exists" ]; then
    exit 0
fi

if [ "$1" = "image" ] && [ "$2" = "inspect" ]; then
    printf '%s' "${CODEXBOX_TEST_IMAGE_INSPECT_RESPONSE:-}"
    exit 0
fi

if [ "$1" = "build" ]; then
    printf '%s' "${CODEXBOX_TEST_PODMAN_BUILD_STDOUT:-}"
    printf '%s' "${CODEXBOX_TEST_PODMAN_BUILD_STDERR:-}" >&2
    exit 0
fi

if [ "$1" = "load" ]; then
    printf '%s' "${CODEXBOX_TEST_PODMAN_LOAD_STDOUT:-}"
    printf '%s' "${CODEXBOX_TEST_PODMAN_LOAD_STDERR:-}" >&2
    exit 0
fi

if [ "$1" = "run" ]; then
    printf '%s' "${CODEXBOX_TEST_PODMAN_RUN_STDOUT:-}"
    printf '%s' "${CODEXBOX_TEST_PODMAN_RUN_STDERR:-}" >&2
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

fn symlink_socket_fixture(path: &Path) -> UnixStream {
    let (socket, _peer) = UnixStream::pair().unwrap();
    let target = format!("/proc/{}/fd/{}", std::process::id(), socket.as_raw_fd());
    std::os::unix::fs::symlink(target, path).unwrap();
    socket
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
  "publish": [
    {{
      "host_port": 9090,
      "container_port": 90
    }}
  ],
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
  "directory_rules": [
    {{
      "path": "{}",
      "publish": [
        {{
          "host_port": 3000,
          "container_port": 3000
        }}
      ],
      "add_dirs": ["{}"]
    }}
  ]
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
    assert!(!stdout.contains("CODEXBOX_PATH_PREFIX"));
    assert!(!stdout.contains("/host/prefix"));
    assert!(!stdout.contains("src=//tmp"));
    assert!(!stdout.contains("src=/bin,target=/bin"));
    assert!(!stdout.contains("src=/sbin,target=/sbin"));
}

#[test]
fn allow_var_patterns_can_reallow_explicitly_blocked_ssh_socket() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let ssh_socket = dir.path().join("agent.sock");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    let _socket = symlink_socket_fixture(&ssh_socket);

    fs::write(
        home_dir.join(".codexbox-conf.json"),
        r#"{
  "approved_socket_vars": ["SSH_AUTH_SOCK"],
  "block_var_patterns": ["SSH*"],
  "allow_var_patterns": ["SSH_AUTH_SOCK"]
}"#,
    )
    .unwrap();

    let output = run_codexbox(
        &home_dir,
        &workspace,
        &["--dry-run"],
        &[("SSH_AUTH_SOCK", &ssh_socket.to_string_lossy())],
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains(&format!("--env SSH_AUTH_SOCK={}", ssh_socket.display())));
    assert!(stdout.contains(&format!(
        "src={},target={},ro",
        ssh_socket.display(),
        ssh_socket.display()
    )));
}

#[test]
fn dry_run_seeds_host_ssh_known_hosts_via_readonly_mount() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let known_hosts = home_dir.join(".ssh/known_hosts");
    fs::create_dir_all(known_hosts.parent().unwrap()).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(&known_hosts, "github.com ssh-ed25519 AAAA").unwrap();

    let output = run_codexbox(&home_dir, &workspace, &["--dry-run"], &[]);
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains(&format!(
        "src={},target={},ro",
        known_hosts.display(),
        GUEST_SSH_KNOWN_HOSTS_SEED
    )));
    assert!(stdout.contains(&format!(
        "--env CODEXBOX_SSH_KNOWN_HOSTS_SEED={}",
        GUEST_SSH_KNOWN_HOSTS_SEED
    )));
}

#[test]
fn dry_run_overlays_known_hosts_when_home_is_writable() {
    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let known_hosts = home_dir.join(".ssh/known_hosts");
    fs::create_dir_all(known_hosts.parent().unwrap()).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(home_dir.join(".codex")).unwrap();
    fs::write(&known_hosts, "github.com ssh-ed25519 AAAA").unwrap();
    fs::write(
        home_dir.join(".codex/config.toml"),
        format!(
            "[sandbox_workspace_write]\nwritable_roots = [\"{}\"]\n",
            home_dir.display()
        ),
    )
    .unwrap();

    let output = run_codexbox(&home_dir, &workspace, &["--dry-run"], &[]);
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains(&format!("target={}", known_hosts.display())));
    assert!(!stdout.contains(&format!(
        "src={},target={},ro",
        known_hosts.display(),
        GUEST_SSH_KNOWN_HOSTS_SEED
    )));
    assert!(!stdout.contains(&format!(
        "--env CODEXBOX_SSH_KNOWN_HOSTS_SEED={}",
        GUEST_SSH_KNOWN_HOSTS_SEED
    )));
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
            "CODEXBOX_TEST_IMAGE_INSPECT_RESPONSE",
            format!(
                "{}|{}",
                embedded_image_fingerprint(),
                current_unix_timestamp()
            ),
        )
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert!(log.contains("ARGS:[image][inspect]"));
    assert!(!log.contains("ARGS:[image][exists][localhost/codexbox:latest]"));
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
        .env(
            "CODEXBOX_TEST_IMAGE_INSPECT_RESPONSE",
            format!("{}|0", embedded_image_fingerprint()),
        )
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert!(log.contains("ARGS:[image][inspect]"));
    assert!(log.contains("ARGS:[--cgroup-manager][cgroupfs][build]"));
    assert!(!log.contains("[--isolation][chroot]"));
}

#[test]
fn rebuild_keeps_podman_build_stdout_out_of_command_stdout() {
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
        .arg("printf")
        .arg("smoke")
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .env(
            "CODEXBOX_TEST_IMAGE_INSPECT_RESPONSE",
            format!("{}|0", embedded_image_fingerprint()),
        )
        .env("CODEXBOX_TEST_PODMAN_BUILD_STDOUT", "build noise\n")
        .env("CODEXBOX_TEST_PODMAN_RUN_STDOUT", "smoke")
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(output.status.success());
    assert_eq!(stdout, "smoke");
    assert!(stderr.contains("build noise"));
    assert!(log.contains("ARGS:[--cgroup-manager][cgroupfs][build]"));
    assert!(log.contains("ARGS:[run]"));
}

#[test]
fn rebuild_image_only_builds_and_exits_without_running_container() {
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
        .arg("--rebuild-image-only")
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(log.contains("ARGS:[--cgroup-manager][cgroupfs][build]"));
    assert!(!log.contains("ARGS:[image][inspect]"));
    assert!(!log.contains("ARGS:[run]"));
    assert!(!home_dir.join(".codex").exists());
    assert!(!home_dir.join(".local/share/codexbox/containers").exists());
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
        .env(
            "CODEXBOX_TEST_IMAGE_INSPECT_RESPONSE",
            format!(
                "{}|{}",
                embedded_image_fingerprint(),
                current_unix_timestamp()
            ),
        )
        .output()
        .unwrap();
    let log = fs::read_to_string(&podman_log).unwrap();

    assert!(output.status.success());
    assert!(log.contains("ARGS:[image][inspect]"));
    assert!(!log.contains("ARGS:[image][exists][localhost/codexbox:latest]"));
    assert!(log.contains("ARGS:[run]"));
    assert!(log.contains("[localhost/codexbox:latest][podman][info]"));
    assert!(!log.contains("CODEXBOX_CONTAINER_COMMAND"));
}
