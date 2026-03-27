use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use crate::path_utils::canonicalize_if_possible;
use crate::sandbox::env_filter::ForwardedEnv;

const FORBIDDEN_ENV_MOUNT_DIRS: &[&str] = &["/bin", "/sbin", "/usr/bin", "/usr/sbin"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvMountCandidate {
    pub var_name: String,
    pub host_path: PathBuf,
    pub kind: EnvMountKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvMountKind {
    File,
    Directory,
    Socket,
}

impl EnvMountCandidate {
    pub fn is_socket(&self) -> bool {
        self.kind == EnvMountKind::Socket
    }
}

pub fn discover_env_mount_candidates(
    env: &ForwardedEnv,
    home_dir: &Path,
) -> Vec<EnvMountCandidate> {
    let mut path_candidates = BTreeMap::<PathBuf, EnvMountCandidate>::new();
    let mut socket_candidates = BTreeSet::<EnvMountCandidate>::new();

    for (key, value) in &env.vars {
        if value.contains("://") {
            continue;
        }

        for path in value_segments(value) {
            let var_name = key.clone();
            if var_name == "PATH" && is_reserved_path_mount(&path) {
                continue;
            }

            if !path.is_absolute() {
                continue;
            }

            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };

            let kind = metadata.file_type();
            let Some(kind) = classify_mount_kind(kind) else {
                continue;
            };
            if is_forbidden_env_mount(&path, home_dir) {
                continue;
            }

            let candidate = EnvMountCandidate {
                var_name: var_name.clone(),
                host_path: path.clone(),
                kind,
            };

            if kind == EnvMountKind::Socket {
                socket_candidates.insert(candidate);
            } else {
                path_candidates.entry(path).or_insert(candidate);
            }
        }
    }

    path_candidates
        .into_values()
        .chain(socket_candidates)
        .collect()
}

fn classify_mount_kind(file_type: fs::FileType) -> Option<EnvMountKind> {
    if file_type.is_file() {
        Some(EnvMountKind::File)
    } else if file_type.is_dir() {
        Some(EnvMountKind::Directory)
    } else if file_type.is_socket() {
        Some(EnvMountKind::Socket)
    } else {
        None
    }
}

fn is_reserved_path_mount(path: &Path) -> bool {
    matches!(path, p if p == Path::new("/bin") || p == Path::new("/sbin"))
        || path == Path::new("/usr")
        || path.starts_with("/usr")
}

fn is_forbidden_env_mount(path: &Path, home_dir: &Path) -> bool {
    let canonical_path = canonicalize_if_possible(path);

    canonical_path == canonicalize_if_possible(home_dir)
        || is_forbidden_system_mount_path(path)
        || is_forbidden_system_mount_path(&canonical_path)
}

fn is_forbidden_system_mount_path(path: &Path) -> bool {
    FORBIDDEN_ENV_MOUNT_DIRS.iter().any(|dir| {
        let dir = Path::new(dir);
        path == dir || path.starts_with(dir)
    })
}

fn value_segments(value: &str) -> Vec<PathBuf> {
    value
        .split(':')
        .filter(|segment| !segment.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::path::Path;

    use tempfile::{tempdir, TempDir};

    use crate::sandbox::env_filter::ForwardedEnv;

    use super::{
        discover_env_mount_candidates, is_forbidden_env_mount, EnvMountCandidate, EnvMountKind,
    };

    fn symlink_socket_fixture(path: &Path) -> UnixStream {
        let (socket, _peer) = UnixStream::pair().unwrap();
        let target = format!("/proc/{}/fd/{}", std::process::id(), socket.as_raw_fd());
        std::os::unix::fs::symlink(target, path).unwrap();
        socket
    }

    fn socket_tempdir() -> TempDir {
        tempdir().unwrap()
    }

    #[test]
    fn discover_candidates_from_absolute_paths_and_path_lists() {
        let dir = socket_tempdir();
        let home = dir.path().join("home");
        let file = dir.path().join("cert.pem");
        let folder = dir.path().join("cache");
        let socket = dir.path().join("agent.sock");
        fs::create_dir_all(&home).unwrap();
        fs::write(&file, "ok").unwrap();
        fs::create_dir_all(&folder).unwrap();
        let _socket = symlink_socket_fixture(&socket);

        let forwarded = ForwardedEnv {
            vars: BTreeMap::from([
                ("CERT_PATH".into(), file.to_string_lossy().to_string()),
                (
                    "MANY".into(),
                    format!("{}:{}", folder.display(), socket.display()),
                ),
                ("HOME_PATH".into(), home.to_string_lossy().to_string()),
                (
                    "PATH".into(),
                    format!("/usr/bin:/bin:/sbin:{}", folder.display()),
                ),
            ]),
            path_prefix: None,
        };

        let candidates = discover_env_mount_candidates(&forwarded, &home);

        assert_eq!(candidates.len(), 3);
        assert!(candidates
            .iter()
            .any(|item| item.host_path == file && item.kind == EnvMountKind::File));
        assert!(candidates
            .iter()
            .any(|item| item.host_path == folder && item.kind == EnvMountKind::Directory));
        assert!(candidates
            .iter()
            .any(|item| item.host_path == socket && item.kind == EnvMountKind::Socket));
        assert!(!candidates.iter().any(|item| item.host_path == home));
        assert!(!candidates
            .iter()
            .any(|item| item.host_path == Path::new("/usr/bin")));
        assert!(!candidates
            .iter()
            .any(|item| item.host_path == Path::new("/bin")));
        assert!(!candidates
            .iter()
            .any(|item| item.host_path == Path::new("/sbin")));
    }

    #[test]
    fn discover_candidates_ignores_url_like_values() {
        let dir = socket_tempdir();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let forwarded = ForwardedEnv {
            vars: BTreeMap::from([
                ("SERVICE_URL".into(), "http://tmp".into()),
                ("GRPC_TARGET".into(), "grpc://tmp".into()),
            ]),
            path_prefix: None,
        };

        let candidates = discover_env_mount_candidates(&forwarded, &home);

        assert!(candidates.is_empty());
    }

    #[test]
    fn discover_candidates_follow_symlinks_to_files() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let real = dir.path().join("real.sock");
        let link = dir.path().join("agent.sock");
        fs::create_dir_all(&home).unwrap();
        fs::write(&real, "ok").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let forwarded = ForwardedEnv {
            vars: BTreeMap::from([("SSH_AUTH_SOCK".into(), link.to_string_lossy().into_owned())]),
            path_prefix: None,
        };

        let candidates = discover_env_mount_candidates(&forwarded, &home);

        assert_eq!(
            candidates,
            vec![EnvMountCandidate {
                var_name: "SSH_AUTH_SOCK".into(),
                host_path: link,
                kind: EnvMountKind::File,
            }]
        );
    }

    #[test]
    fn discover_candidates_keeps_socket_candidates_per_env_var() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let socket = dir.path().join("agent.sock");
        fs::create_dir_all(&home).unwrap();
        let _socket = symlink_socket_fixture(&socket);

        let forwarded = ForwardedEnv {
            vars: BTreeMap::from([
                ("A_SOCK".into(), socket.to_string_lossy().into_owned()),
                ("B_SOCK".into(), socket.to_string_lossy().into_owned()),
            ]),
            path_prefix: None,
        };

        let candidates = discover_env_mount_candidates(&forwarded, &home);

        assert_eq!(
            candidates,
            vec![
                EnvMountCandidate {
                    var_name: "A_SOCK".into(),
                    host_path: socket.clone(),
                    kind: EnvMountKind::Socket,
                },
                EnvMountCandidate {
                    var_name: "B_SOCK".into(),
                    host_path: socket,
                    kind: EnvMountKind::Socket,
                },
            ]
        );
    }

    #[test]
    fn discover_candidates_skip_forbidden_system_paths_for_any_env_var() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let allowed = dir.path().join("agent.sock");
        fs::create_dir_all(&home).unwrap();
        fs::write(&allowed, "ok").unwrap();

        let forwarded = ForwardedEnv {
            vars: BTreeMap::from([
                ("BIN_FILE".into(), "/usr/bin/env".into()),
                ("SBIN_DIR".into(), "/usr/sbin".into()),
                ("ALLOWED".into(), allowed.to_string_lossy().into_owned()),
            ]),
            path_prefix: None,
        };

        let candidates = discover_env_mount_candidates(&forwarded, &home);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].host_path, allowed);
        assert_eq!(candidates[0].var_name, "ALLOWED");
        assert_eq!(candidates[0].kind, EnvMountKind::File);
    }

    #[test]
    fn only_home_root_is_forbidden() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let subdir = home.join("project");
        fs::create_dir_all(&subdir).unwrap();

        assert!(is_forbidden_env_mount(&home, &home));
        assert!(!is_forbidden_env_mount(&subdir, &home));
    }

    #[test]
    fn system_bin_paths_and_descendants_are_forbidden() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();

        for path in [
            Path::new("/bin"),
            Path::new("/bin/sh"),
            Path::new("/sbin"),
            Path::new("/sbin/init"),
            Path::new("/usr/bin"),
            Path::new("/usr/bin/env"),
            Path::new("/usr/sbin"),
            Path::new("/usr/sbin/service"),
        ] {
            assert!(
                is_forbidden_env_mount(path, &home),
                "expected {path:?} to be forbidden"
            );
        }
    }

    #[test]
    fn symlinks_to_forbidden_system_paths_are_forbidden() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let link = dir.path().join("env");
        fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink("/usr/bin/env", &link).unwrap();

        assert!(is_forbidden_env_mount(&link, &home));
    }
}
