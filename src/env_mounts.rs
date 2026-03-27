use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use crate::env_filter::ForwardedEnv;
use crate::policy::is_forbidden_env_mount;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvMountCandidate {
    pub var_name: String,
    pub host_path: PathBuf,
}

pub fn discover_env_mount_candidates(
    env: &ForwardedEnv,
    home_dir: &Path,
) -> Vec<EnvMountCandidate> {
    let mut candidates = BTreeMap::<PathBuf, String>::new();

    for (key, value) in &env.vars {
        let var_name = key.to_string_lossy().into_owned();
        for path in value_segments(value) {
            if var_name == "PATH" && is_usr_path(&path) {
                continue;
            }

            if !path.is_absolute() {
                continue;
            }

            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };

            let kind = metadata.file_type();
            if !(kind.is_file() || kind.is_dir() || kind.is_socket()) {
                continue;
            }

            if is_forbidden_env_mount(&path, home_dir) {
                continue;
            }

            candidates.entry(path).or_insert_with(|| var_name.clone());
        }
    }

    candidates
        .into_iter()
        .map(|(host_path, var_name)| EnvMountCandidate {
            var_name,
            host_path,
        })
        .collect()
}

fn is_usr_path(path: &Path) -> bool {
    path == Path::new("/usr") || path.starts_with("/usr")
}

fn value_segments(value: &OsString) -> Vec<PathBuf> {
    let text = value.to_string_lossy();
    text.split(':')
        .filter(|segment| !segment.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::path::Path;

    use tempfile::tempdir;

    use crate::env_filter::ForwardedEnv;

    use super::discover_env_mount_candidates;

    #[test]
    fn discover_candidates_from_absolute_paths_and_path_lists() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let file = dir.path().join("cert.pem");
        let folder = dir.path().join("cache");
        let socket = dir.path().join("agent.sock");
        fs::create_dir_all(&home).unwrap();
        fs::write(&file, "ok").unwrap();
        fs::create_dir_all(&folder).unwrap();
        let _listener = UnixListener::bind(&socket).unwrap();

        let forwarded = ForwardedEnv {
            vars: BTreeMap::from([
                (
                    OsString::from("CERT_PATH"),
                    OsString::from(file.to_string_lossy().to_string()),
                ),
                (
                    OsString::from("MANY"),
                    OsString::from(format!("{}:{}", folder.display(), socket.display())),
                ),
                (
                    OsString::from("HOME_PATH"),
                    OsString::from(home.to_string_lossy().to_string()),
                ),
                (
                    OsString::from("PATH"),
                    OsString::from(format!("/usr/bin:{}", folder.display())),
                ),
            ]),
            path_prefix: None,
        };

        let candidates = discover_env_mount_candidates(&forwarded, &home);

        assert_eq!(candidates.len(), 3);
        assert!(candidates.iter().any(|item| item.host_path == file));
        assert!(candidates.iter().any(|item| item.host_path == folder));
        assert!(candidates.iter().any(|item| item.host_path == socket));
        assert!(!candidates.iter().any(|item| item.host_path == home));
        assert!(!candidates
            .iter()
            .any(|item| item.host_path == Path::new("/usr/bin")));
    }
}
