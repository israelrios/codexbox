use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::env_mounts::EnvMountCandidate;
use crate::errors::{CodexboxError, Result};
use crate::user_context::UserContext;

pub const GUEST_PODMAN_ROOT: &str = "/var/lib/containers";
pub const GUEST_ADDITIONAL_IMAGE_STORE: &str = "/var/lib/shared";
const CA_TRUST_PATHS: &[&str] = &[
    "/etc/ssl/certs",
    "/etc/pki/tls/certs",
    "/etc/ca-certificates",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/ca-bundle.crt",
    "/etc/pki/ca-trust",
    "/etc/pki/tls/cert.pem",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountSource {
    Fixed,
    CodexWritableRoot,
    CodexAddDir,
    EnvDerived { var_name: String },
    CaTrust,
    Podman,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub mode: MountMode,
    pub source: MountSource,
}

pub fn base_mounts(user: &UserContext, writable_roots: &[PathBuf]) -> Result<Vec<MountSpec>> {
    let mut mounts = Vec::new();

    mounts.push(MountSpec {
        host: user.cwd.clone(),
        guest: user.cwd.clone(),
        mode: MountMode::ReadWrite,
        source: MountSource::Fixed,
    });

    let codex_dir = user.home_dir.join(".codex");
    mounts.push(MountSpec {
        host: codex_dir.clone(),
        guest: codex_dir,
        mode: MountMode::ReadWrite,
        source: MountSource::Fixed,
    });

    for path in [
        user.home_dir.join(".gitconfig"),
        user.home_dir.join(".config").join("gh"),
        user.home_dir.join(".config").join("glab-cli"),
    ] {
        if path.exists() {
            mounts.push(MountSpec {
                host: path.clone(),
                guest: path,
                mode: MountMode::ReadOnly,
                source: MountSource::Fixed,
            });
        }
    }

    for root in writable_roots {
        mounts.push(MountSpec {
            host: root.clone(),
            guest: root.clone(),
            mode: MountMode::ReadWrite,
            source: MountSource::CodexWritableRoot,
        });
    }

    mounts.extend(podman_persistence_mounts(user)?);
    Ok(dedupe_mounts(mounts))
}

pub fn prepare_runtime_dirs(user: &UserContext) -> Result<()> {
    ensure_dir(&user.home_dir.join(".codex"))?;
    ensure_dir(
        &user
            .home_dir
            .join(".local")
            .join("share")
            .join("codexbox")
            .join("containers"),
    )?;
    Ok(())
}

pub fn approved_env_mounts(candidates: &[EnvMountCandidate]) -> Vec<MountSpec> {
    candidates
        .iter()
        .map(|candidate| MountSpec {
            host: candidate.host_path.clone(),
            guest: candidate.host_path.clone(),
            mode: MountMode::ReadOnly,
            source: MountSource::EnvDerived {
                var_name: candidate.var_name.clone(),
            },
        })
        .collect()
}

pub fn ca_mounts(paths: &[PathBuf]) -> Vec<MountSpec> {
    paths
        .iter()
        .map(|path| MountSpec {
            host: path.clone(),
            guest: path.clone(),
            mode: MountMode::ReadOnly,
            source: MountSource::CaTrust,
        })
        .collect()
}

pub fn discover_ca_trust_paths() -> Vec<PathBuf> {
    let mut paths = CA_TRUST_PATHS
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

pub fn combine_mounts(groups: &[Vec<MountSpec>]) -> Vec<MountSpec> {
    let all = groups
        .iter()
        .flat_map(|group| group.iter().cloned())
        .collect();
    dedupe_mounts(all)
}

pub fn filter_covered_env_candidates(
    candidates: Vec<EnvMountCandidate>,
    existing_mounts: &[MountSpec],
) -> Vec<EnvMountCandidate> {
    candidates
        .into_iter()
        .filter(|candidate| {
            !existing_mounts
                .iter()
                .any(|mount| mount_covers_path(mount, &candidate.host_path))
        })
        .collect()
}

pub fn mount_covers_path(mount: &MountSpec, path: &Path) -> bool {
    if mount.guest != mount.host {
        return false;
    }

    let mount_is_dir = fs::metadata(&mount.host)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false);

    if mount_is_dir {
        path == mount.host || path.starts_with(&mount.host)
    } else {
        path == mount.host
    }
}

fn podman_persistence_mounts(user: &UserContext) -> Result<Vec<MountSpec>> {
    let storage_root = user
        .home_dir
        .join(".local")
        .join("share")
        .join("codexbox")
        .join("containers");

    let mut mounts = vec![MountSpec {
        host: storage_root,
        guest: PathBuf::from(GUEST_PODMAN_ROOT),
        mode: MountMode::ReadWrite,
        source: MountSource::Podman,
    }];

    let host_image_store = user
        .home_dir
        .join(".local")
        .join("share")
        .join("containers")
        .join("storage");
    if host_image_store.exists() {
        mounts.push(MountSpec {
            host: host_image_store,
            guest: PathBuf::from(GUEST_ADDITIONAL_IMAGE_STORE),
            mode: MountMode::ReadOnly,
            source: MountSource::Podman,
        });
    }

    Ok(mounts)
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| CodexboxError::WritePath {
        path: path.to_path_buf(),
        source,
    })
}

fn dedupe_mounts(mounts: Vec<MountSpec>) -> Vec<MountSpec> {
    let mut by_guest = BTreeMap::<PathBuf, MountSpec>::new();

    for mount in mounts {
        by_guest
            .entry(mount.guest.clone())
            .and_modify(|existing| {
                if existing.host == mount.host && existing.mode != MountMode::ReadWrite {
                    existing.mode = mount.mode;
                }
            })
            .or_insert(mount);
    }

    by_guest.into_values().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use crate::env_mounts::EnvMountCandidate;
    use crate::user_context::UserContext;

    use super::{
        base_mounts, discover_ca_trust_paths, filter_covered_env_candidates, prepare_runtime_dirs,
        MountMode, GUEST_ADDITIONAL_IMAGE_STORE, GUEST_PODMAN_ROOT,
    };

    #[test]
    fn filter_covered_env_candidates_skips_paths_inside_existing_mounts() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        let nested = cwd.join(".direnv");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(home.join(".local/share/containers")).unwrap();

        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home,
            cwd: cwd.clone(),
        };

        let mounts = base_mounts(&user, &[]).unwrap();
        let candidates = vec![EnvMountCandidate {
            var_name: "DIRENV_DIR".into(),
            host_path: nested,
        }];

        let filtered = filter_covered_env_candidates(candidates, &mounts);
        assert!(filtered.is_empty());
        assert!(mounts
            .iter()
            .any(|mount| mount.mode == MountMode::ReadWrite));
    }

    #[test]
    fn podman_mounts_keep_private_store_without_host_config_mount() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        fs::create_dir_all(home.join(".local/share/containers/storage")).unwrap();
        fs::create_dir_all(home.join(".config/containers")).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home.clone(),
            cwd,
        };

        let mounts = base_mounts(&user, &[]).unwrap();

        assert!(mounts.iter().any(|mount| {
            mount.host == home.join(".local/share/codexbox/containers")
                && mount.guest == Path::new(GUEST_PODMAN_ROOT)
                && mount.mode == MountMode::ReadWrite
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == home.join(".local/share/containers/storage")
                && mount.guest == Path::new(GUEST_ADDITIONAL_IMAGE_STORE)
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(!mounts
            .iter()
            .any(|mount| mount.host == home.join(".config/containers")));
    }

    #[test]
    fn prepare_runtime_dirs_creates_only_runtime_state() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        prepare_runtime_dirs(&UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home.clone(),
            cwd,
        })
        .unwrap();

        assert!(home.join(".codex").is_dir());
        assert!(home.join(".local/share/codexbox/containers").is_dir());
    }

    #[test]
    fn discover_ca_trust_paths_are_existing_and_unique() {
        let paths = discover_ca_trust_paths();

        for path in &paths {
            assert!(path.exists());
        }

        let mut unique = paths.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(paths, unique);
    }
}
