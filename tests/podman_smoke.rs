use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn codexbox_bin() -> &'static str {
    env!("CARGO_BIN_EXE_codexbox")
}

#[test]
fn launches_real_podman_sandbox_when_enabled() {
    if std::env::var("CODEXBOX_RUN_PODMAN_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("skipping real Podman smoke test; set CODEXBOX_RUN_PODMAN_SMOKE=1 to enable");
        return;
    }

    let podman_info = Command::new("podman")
        .arg("info")
        .output()
        .expect("failed to execute podman info");
    assert!(
        podman_info.status.success(),
        "podman info failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&podman_info.stdout),
        String::from_utf8_lossy(&podman_info.stderr)
    );

    let dir = tempdir().unwrap();
    let home_dir = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(home_dir.join(".codexbox-conf.json"), "{}\n").unwrap();

    let mut command = Command::new(codexbox_bin());
    command
        .arg("--container-command")
        .arg("/bin/sh")
        .arg("-lc")
        .arg("test -S \"$XDG_RUNTIME_DIR/podman/podman.sock\" && test -S /var/run/docker.sock && printf smoke")
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8");

    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        command.env("XDG_RUNTIME_DIR", runtime_dir);
    }

    let output = command.output().unwrap();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("route_localnet") || stderr.contains("mount_setattr `/sys`") {
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
