use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

fn entrypoint_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("container-entrypoint.sh")
}

fn write_fake_podman(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
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
fn entrypoint_fails_fast_when_podman_service_socket_never_appears() {
    let dir = tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&home_dir).unwrap();

    write_fake_podman(
        &fake_bin.join("podman"),
        r#"#!/bin/sh
if [ "$1" = "system" ] && [ "$2" = "service" ]; then
    exit 0
fi
exit 0
"#,
    );

    let output = Command::new("/bin/sh")
        .arg(entrypoint_path())
        .arg("/bin/sh")
        .arg("-c")
        .arg("true")
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBOX_PODMAN_SERVICE_WAIT_ATTEMPTS", "1")
        .env("CODEXBOX_PODMAN_SERVICE_WAIT_DELAY_SECS", "0")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("podman system service did not create")
    );
}

#[test]
fn entrypoint_exports_new_images_after_command_finishes() {
    let dir = tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    let export_dir = dir.path().join("exports");
    let podman_log = dir.path().join("podman.log");
    let images_count = dir.path().join("images.count");
    let socket_dir = runtime_dir.join("podman");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&socket_dir).unwrap();
    let _socket = symlink_socket_fixture(&socket_dir.join("podman.sock"));

    write_fake_podman(
        &fake_bin.join("podman"),
        r#"#!/bin/sh
log_file="${CODEXBOX_TEST_PODMAN_LOG:?}"
count_file="${CODEXBOX_TEST_PODMAN_IMAGES_COUNT:?}"

if [ "$1" = "system" ] && [ "$2" = "service" ]; then
    exit 0
fi

if [ "$1" = "images" ]; then
    count=0
    if [ -f "$count_file" ]; then
        count=$(cat "$count_file")
    fi
    count=$((count + 1))
    printf '%s' "$count" > "$count_file"
    if [ "$count" -eq 1 ]; then
        printf 'base|latest|sha256:base\n'
    else
        printf 'base|latest|sha256:base\napp|latest|sha256:new\n'
    fi
    exit 0
fi

if [ "$1" = "save" ]; then
    output_path=
    image_ref=
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --output)
                shift
                output_path="$1"
                ;;
            *)
                image_ref="$1"
                ;;
        esac
        shift
    done
    : > "$output_path"
    printf 'SAVE:%s\n' "$image_ref" >> "$log_file"
    exit 0
fi

exit 0
"#,
    );

    let output = Command::new("/bin/sh")
        .arg(entrypoint_path())
        .arg("/bin/sh")
        .arg("-c")
        .arg("true")
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBOX_IMAGE_EXPORT_DIR", &export_dir)
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .env("CODEXBOX_TEST_PODMAN_IMAGES_COUNT", &images_count)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(export_dir.join("image-0.tar").is_file());
    assert!(fs::read_to_string(&podman_log)
        .unwrap()
        .contains("SAVE:app:latest"));
}

#[test]
fn entrypoint_seeds_home_known_hosts_and_keeps_session_edits_ephemeral() {
    let dir = tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    let socket_dir = runtime_dir.join("podman");
    let seed_dir = dir.path().join("seed");
    let seed_path = seed_dir.join("known_hosts");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&socket_dir).unwrap();
    fs::create_dir_all(&seed_dir).unwrap();
    fs::write(&seed_path, "github.com ssh-ed25519 AAAA\n").unwrap();
    let _socket = symlink_socket_fixture(&socket_dir.join("podman.sock"));

    write_fake_podman(
        &fake_bin.join("podman"),
        r#"#!/bin/sh
if [ "$1" = "system" ] && [ "$2" = "service" ]; then
    exit 0
fi
exit 0
"#,
    );

    let output = Command::new("/bin/sh")
        .arg(entrypoint_path())
        .arg("/bin/sh")
        .arg("-c")
        .arg("printf 'gitlab.com ssh-ed25519 BBBB\n' >> \"$HOME/.ssh/known_hosts\"")
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBOX_SSH_KNOWN_HOSTS_SEED", &seed_path)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&seed_path).unwrap(),
        "github.com ssh-ed25519 AAAA\n"
    );
    assert_eq!(
        fs::read_to_string(home_dir.join(".ssh/known_hosts")).unwrap(),
        "github.com ssh-ed25519 AAAA\ngitlab.com ssh-ed25519 BBBB\n"
    );
}

#[test]
fn entrypoint_ignores_images_from_readonly_host_store_during_export() {
    let dir = tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    let export_dir = dir.path().join("exports");
    let podman_log = dir.path().join("podman.log");
    let images_count = dir.path().join("images.count");
    let socket_dir = runtime_dir.join("podman");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&socket_dir).unwrap();
    let _socket = symlink_socket_fixture(&socket_dir.join("podman.sock"));

    write_fake_podman(
        &fake_bin.join("podman"),
        r#"#!/bin/sh
log_file="${CODEXBOX_TEST_PODMAN_LOG:?}"
count_file="${CODEXBOX_TEST_PODMAN_IMAGES_COUNT:?}"

if [ "$1" = "system" ] && [ "$2" = "service" ]; then
    exit 0
fi

if [ "$1" = "images" ]; then
    has_readonly_filter=false
    prev=
    for arg in "$@"; do
        if [ "$prev" = "--filter" ] && [ "$arg" = "readonly=false" ]; then
            has_readonly_filter=true
        fi
        prev="$arg"
    done

    count=0
    if [ -f "$count_file" ]; then
        count=$(cat "$count_file")
    fi
    count=$((count + 1))
    printf '%s' "$count" > "$count_file"

    if [ "$count" -eq 1 ]; then
        printf 'base|latest|sha256:base\n'
    elif [ "$has_readonly_filter" = true ]; then
        printf 'base|latest|sha256:base\napp|latest|sha256:new\n'
    else
        printf 'base|latest|sha256:base\napp|latest|sha256:new\nhost|latest|sha256:host\n'
    fi
    exit 0
fi

if [ "$1" = "save" ]; then
    output_path=
    image_ref=
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --output)
                shift
                output_path="$1"
                ;;
            *)
                image_ref="$1"
                ;;
        esac
        shift
    done
    : > "$output_path"
    printf 'SAVE:%s\n' "$image_ref" >> "$log_file"
    exit 0
fi

exit 0
"#,
    );

    let output = Command::new("/bin/sh")
        .arg(entrypoint_path())
        .arg("/bin/sh")
        .arg("-c")
        .arg("true")
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBOX_IMAGE_EXPORT_DIR", &export_dir)
        .env("CODEXBOX_TEST_PODMAN_LOG", &podman_log)
        .env("CODEXBOX_TEST_PODMAN_IMAGES_COUNT", &images_count)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(export_dir.join("image-0.tar").is_file());
    assert!(!export_dir.join("image-1.tar").exists());

    let log = fs::read_to_string(&podman_log).unwrap();
    assert!(log.contains("SAVE:app:latest"));
    assert!(!log.contains("SAVE:host:latest"));
}

#[test]
fn entrypoint_links_root_bash_init_files_when_missing() {
    let dir = tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    let guest_containers_dir = dir.path().join("guest-containers");
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    let socket_dir = runtime_dir.join("podman");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&guest_containers_dir).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&socket_dir).unwrap();
    let _socket = symlink_socket_fixture(&socket_dir.join("podman.sock"));

    fs::write(
        guest_containers_dir.join("containers.conf"),
        "[containers]\n",
    )
    .unwrap();
    fs::write(home_dir.join(".bash_profile"), "existing profile").unwrap();

    write_fake_podman(
        &fake_bin.join("podman"),
        r#"#!/bin/sh
if [ "$1" = "system" ] && [ "$2" = "service" ]; then
    exit 0
fi
exit 0
"#,
    );

    let output = Command::new("/bin/sh")
        .arg(entrypoint_path())
        .arg("/bin/sh")
        .arg("-c")
        .arg("true")
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBOX_GUEST_CONTAINERS_DIR", &guest_containers_dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        fs::read_link(home_dir.join(".config/containers")).unwrap(),
        guest_containers_dir
    );
    assert_eq!(
        fs::read_link(home_dir.join(".bashrc")).unwrap(),
        PathBuf::from("/root/.bashrc")
    );
    assert_eq!(
        fs::read_to_string(home_dir.join(".bash_profile")).unwrap(),
        "existing profile"
    );
}

#[test]
fn entrypoint_merges_guest_containers_config_into_existing_home_dir() {
    let dir = tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    let guest_containers_dir = dir.path().join("guest-containers");
    let home_dir = dir.path().join("home");
    let runtime_dir = dir.path().join("runtime");
    let socket_dir = runtime_dir.join("podman");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(guest_containers_dir.join("certs.d/registry.example")).unwrap();
    fs::create_dir_all(home_dir.join(".config/containers")).unwrap();
    fs::create_dir_all(&socket_dir).unwrap();
    let _socket = symlink_socket_fixture(&socket_dir.join("podman.sock"));

    fs::write(
        guest_containers_dir.join("containers.conf"),
        "[containers]\nlog_driver=\"k8s-file\"\n",
    )
    .unwrap();
    fs::write(
        guest_containers_dir.join("certs.d/registry.example/ca.crt"),
        "guest-ca",
    )
    .unwrap();
    fs::write(
        home_dir.join(".config/containers/auth.json"),
        "{\"auths\":{\"registry.example\":{}}}\n",
    )
    .unwrap();

    write_fake_podman(
        &fake_bin.join("podman"),
        r#"#!/bin/sh
if [ "$1" = "system" ] && [ "$2" = "service" ]; then
    exit 0
fi
exit 0
"#,
    );

    let output = Command::new("/bin/sh")
        .arg(entrypoint_path())
        .arg("/bin/sh")
        .arg("-c")
        .arg("true")
        .env_clear()
        .env("HOME", &home_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBOX_GUEST_CONTAINERS_DIR", &guest_containers_dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(home_dir.join(".config/containers/auth.json")).unwrap(),
        "{\"auths\":{\"registry.example\":{}}}\n"
    );
    assert_eq!(
        fs::read_to_string(home_dir.join(".config/containers/containers.conf")).unwrap(),
        "[containers]\nlog_driver=\"k8s-file\"\n"
    );
    assert_eq!(
        fs::read_to_string(home_dir.join(".config/containers/certs.d/registry.example/ca.crt"))
            .unwrap(),
        "guest-ca"
    );
}
