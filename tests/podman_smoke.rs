use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

fn codexbox_bin() -> &'static str {
    env!("CARGO_BIN_EXE_codexbox")
}

fn configure_host_podman_env(command: &mut Command, home_dir: &Path, runtime_dir: &Path) {
    command
        .env_clear()
        .env("HOME", home_dir)
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8");
}

fn prepare_runtime_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

fn podman_is_unavailable(stderr: &str) -> bool {
    [
        "route_localnet",
        "mount_setattr `/sys`",
        "read-only file system",
        "cannot setup namespace using newuidmap",
        "newuidmap: write to uid_map failed",
        "cannot clone: Operation not permitted",
    ]
    .iter()
    .any(|pattern| stderr.contains(pattern))
}

#[test]
fn launches_real_podman_sandbox_when_enabled() {
    if std::env::var("CODEXBOX_RUN_PODMAN_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("skipping real Podman smoke test; set CODEXBOX_RUN_PODMAN_SMOKE=1 to enable");
        return;
    }

    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(home_dir.join(".codexbox-conf.json"), "{}\n").unwrap();
    prepare_runtime_dir(&runtime_dir);

    let mut podman_info_command = Command::new("podman");
    configure_host_podman_env(&mut podman_info_command, &home_dir, &runtime_dir);
    let podman_info = podman_info_command
        .arg("info")
        .output()
        .expect("failed to execute podman info");
    if !podman_info.status.success() {
        let stderr = String::from_utf8_lossy(&podman_info.stderr);
        if podman_is_unavailable(&stderr) {
            eprintln!(
                "skipping real Podman smoke test; host Podman is unavailable in this environment\nstderr:\n{}",
                stderr
            );
            return;
        }

        panic!(
            "podman info failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&podman_info.stdout),
            stderr
        );
    }

    let mut command = Command::new(codexbox_bin());
    configure_host_podman_env(&mut command, &home_dir, &runtime_dir);
    command
        .arg("--container-command")
        .arg("/bin/sh")
        .arg("-lc")
        .arg("test -S \"$XDG_RUNTIME_DIR/podman/podman.sock\" && test -S /var/run/docker.sock && command -v openssl >/dev/null && command -v perl >/dev/null && command -v 7z >/dev/null && printf smoke")
        .current_dir(&workspace);

    let output = command.output().unwrap();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if podman_is_unavailable(&stderr) {
            eprintln!(
                "skipping real Podman smoke test; host Podman cannot launch containers in this environment\nstderr:\n{}",
                stderr
            );
            return;
        }
    }

    assert!(
        output.status.success(),
        "codexbox smoke test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "smoke");
}
