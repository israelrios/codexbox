use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::errors::{CodexboxError, Result};
use crate::sandbox::env_mounts::EnvMountCandidate;
use crate::user_context::UserContext;

pub const GUEST_PODMAN_ROOT: &str = "/var/lib/containers";
pub const GUEST_ADDITIONAL_IMAGE_STORE: &str = "/var/lib/shared";
pub const GUEST_SSH_KNOWN_HOSTS_SEED: &str = "/tmp/codexbox-host-ssh-known_hosts";
const HOST_DOCKER_CONFIG_DIR: &str = "/etc/docker";
const HOST_DOCKER_CERTS_DIR: &str = "/etc/docker/certs.d";
const HOST_CONTAINERS_CERTS_DIR: &str = "/etc/containers/certs.d";
const GUEST_CONTAINERS_CERTS_DIR: &str = "/etc/containers/certs.d";
const GUEST_ROOT_CONTAINERS_AUTH_FILE: &str = "/root/.config/containers/auth.json";
const GUEST_ROOT_CONTAINERS_CERTS_DIR: &str = "/root/.config/containers/certs.d";
const CA_TRUST_DIR_PATHS: &[&str] = &[
    "/etc/ssl/certs",
    "/etc/pki/tls/certs",
    "/etc/ca-certificates",
    "/etc/pki/ca-trust",
];
const CA_BUNDLE_GUEST_TARGETS: &[(&str, &[&str])] = &[
    (
        "/etc/pki/tls/cert.pem",
        &[
            "/etc/pki/tls/cert.pem",
            "/etc/ssl/cert.pem",
            "/usr/lib/ssl/cert.pem",
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/ssl/ca-bundle.crt",
            "/etc/ssl/ca-bundle.pem",
        ],
    ),
    (
        "/etc/ssl/cert.pem",
        &[
            "/etc/ssl/cert.pem",
            "/usr/lib/ssl/cert.pem",
            "/etc/pki/tls/cert.pem",
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/ssl/ca-bundle.crt",
            "/etc/ssl/ca-bundle.pem",
        ],
    ),
    (
        "/etc/ssl/ca-bundle.crt",
        &[
            "/etc/ssl/ca-bundle.crt",
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/cert.pem",
            "/etc/ssl/cert.pem",
            "/usr/lib/ssl/cert.pem",
            "/etc/ssl/ca-bundle.pem",
        ],
    ),
    (
        "/etc/ssl/ca-bundle.pem",
        &[
            "/etc/ssl/ca-bundle.pem",
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/cert.pem",
            "/etc/ssl/cert.pem",
            "/usr/lib/ssl/cert.pem",
            "/etc/ssl/ca-bundle.crt",
        ],
    ),
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

    if should_mount_cwd(user) {
        mounts.push(MountSpec {
            host: user.cwd.clone(),
            guest: user.cwd.clone(),
            mode: MountMode::ReadWrite,
            source: MountSource::Fixed,
        });
    }

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
                mode: MountMode::ReadWrite,
                source: MountSource::Fixed,
            });
        }
    }

    mounts.extend(registry_mounts(user));

    if let Some(mount) = ssh_known_hosts_mount(user) {
        mounts.push(mount);
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

pub fn should_mount_cwd(user: &UserContext) -> bool {
    user.cwd != user.home_dir
}

pub fn has_ssh_known_hosts_mount(mounts: &[MountSpec]) -> bool {
    mounts
        .iter()
        .any(|mount| mount.guest == Path::new(GUEST_SSH_KNOWN_HOSTS_SEED))
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

pub fn discover_ca_trust_mounts() -> Vec<MountSpec> {
    let existing_paths = CA_TRUST_DIR_PATHS
        .iter()
        .map(PathBuf::from)
        .chain(
            CA_BUNDLE_GUEST_TARGETS
                .iter()
                .flat_map(|(_, candidates)| candidates.iter().map(PathBuf::from)),
        )
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    discover_ca_trust_mounts_from_existing(existing_paths)
}

fn discover_ca_trust_mounts_from_existing<I>(existing_paths: I) -> Vec<MountSpec>
where
    I: IntoIterator<Item = PathBuf>,
{
    let existing_paths = existing_paths.into_iter().collect::<Vec<_>>();
    let mut mounts = Vec::new();

    for path in CA_TRUST_DIR_PATHS.iter().map(PathBuf::from) {
        if existing_paths.contains(&path) {
            mounts.push(MountSpec {
                host: path.clone(),
                guest: path,
                mode: MountMode::ReadOnly,
                source: MountSource::CaTrust,
            });
        }
    }

    for (guest, candidates) in CA_BUNDLE_GUEST_TARGETS {
        if let Some(host) = candidates
            .iter()
            .map(PathBuf::from)
            .find(|path| existing_paths.contains(path))
        {
            mounts.push(MountSpec {
                host,
                guest: PathBuf::from(guest),
                mode: MountMode::ReadOnly,
                source: MountSource::CaTrust,
            });
        }
    }

    dedupe_mounts(mounts)
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

fn existing_readonly_self_mounts(paths: &[PathBuf]) -> Vec<MountSpec> {
    paths
        .iter()
        .filter(|path| path.exists())
        .map(|path| MountSpec {
            host: path.clone(),
            guest: path.clone(),
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        })
        .collect()
}

fn registry_mounts(user: &UserContext) -> Vec<MountSpec> {
    let docker_auth = user.home_dir.join(".docker").join("config.json");
    let mut mounts = existing_readonly_self_mounts(&[
        PathBuf::from(HOST_DOCKER_CONFIG_DIR),
        docker_auth.clone(),
    ]);
    let existing_paths = [
        PathBuf::from(HOST_DOCKER_CERTS_DIR),
        PathBuf::from(HOST_CONTAINERS_CERTS_DIR),
        docker_auth,
        user.home_dir
            .join(".config")
            .join("containers")
            .join("auth.json"),
        user.home_dir
            .join(".config")
            .join("containers")
            .join("certs.d"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect::<Vec<_>>();

    mounts.extend(registry_mounts_from_existing(
        &user.home_dir,
        existing_paths,
    ));
    dedupe_mounts(mounts)
}

fn registry_mounts_from_existing(home_dir: &Path, existing_paths: Vec<PathBuf>) -> Vec<MountSpec> {
    let mut mounts = Vec::new();
    let docker_config_dir = PathBuf::from(HOST_DOCKER_CONFIG_DIR);
    if existing_paths.contains(&docker_config_dir) {
        mounts.push(MountSpec {
            host: docker_config_dir.clone(),
            guest: docker_config_dir,
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        });
    }

    let containers_certs_dir = PathBuf::from(HOST_CONTAINERS_CERTS_DIR);
    if existing_paths.contains(&containers_certs_dir) {
        mounts.push(MountSpec {
            host: containers_certs_dir,
            guest: PathBuf::from(GUEST_CONTAINERS_CERTS_DIR),
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        });
    } else {
        let docker_certs_dir = PathBuf::from(HOST_DOCKER_CERTS_DIR);
        if existing_paths.contains(&docker_certs_dir) {
            mounts.push(MountSpec {
                host: docker_certs_dir,
                guest: PathBuf::from(GUEST_CONTAINERS_CERTS_DIR),
                mode: MountMode::ReadOnly,
                source: MountSource::Fixed,
            });
        }
    }

    let docker_auth = home_dir.join(".docker").join("config.json");
    if existing_paths.contains(&docker_auth) {
        mounts.push(MountSpec {
            host: docker_auth.clone(),
            guest: docker_auth.clone(),
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        });
    }

    let containers_auth = home_dir
        .join(".config")
        .join("containers")
        .join("auth.json");
    let auth_source = if existing_paths.contains(&containers_auth) {
        Some(containers_auth)
    } else if existing_paths.contains(&docker_auth) {
        Some(docker_auth)
    } else {
        None
    };
    if let Some(auth_source) = auth_source {
        mounts.push(MountSpec {
            host: auth_source,
            guest: PathBuf::from(GUEST_ROOT_CONTAINERS_AUTH_FILE),
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        });
    }

    let user_certs_dir = home_dir.join(".config").join("containers").join("certs.d");
    if existing_paths.contains(&user_certs_dir) {
        mounts.push(MountSpec {
            host: user_certs_dir,
            guest: PathBuf::from(GUEST_ROOT_CONTAINERS_CERTS_DIR),
            mode: MountMode::ReadOnly,
            source: MountSource::Fixed,
        });
    }

    dedupe_mounts(mounts)
}

fn ssh_known_hosts_mount(user: &UserContext) -> Option<MountSpec> {
    let host_path = user.home_dir.join(".ssh").join("known_hosts");
    if !host_path.exists() {
        return None;
    }

    Some(MountSpec {
        host: host_path,
        guest: PathBuf::from(GUEST_SSH_KNOWN_HOSTS_SEED),
        mode: MountMode::ReadOnly,
        source: MountSource::Fixed,
    })
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

    use crate::sandbox::env_mounts::{EnvMountCandidate, EnvMountKind};
    use crate::user_context::UserContext;

    use super::{
        base_mounts, discover_ca_trust_mounts, discover_ca_trust_mounts_from_existing,
        existing_readonly_self_mounts, filter_covered_env_candidates, has_ssh_known_hosts_mount,
        prepare_runtime_dirs, registry_mounts_from_existing, should_mount_cwd, MountMode,
        GUEST_ADDITIONAL_IMAGE_STORE, GUEST_CONTAINERS_CERTS_DIR, GUEST_PODMAN_ROOT,
        GUEST_ROOT_CONTAINERS_AUTH_FILE, GUEST_ROOT_CONTAINERS_CERTS_DIR,
        GUEST_SSH_KNOWN_HOSTS_SEED, HOST_CONTAINERS_CERTS_DIR, HOST_DOCKER_CERTS_DIR,
        HOST_DOCKER_CONFIG_DIR,
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
            kind: EnvMountKind::Directory,
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
    fn cwd_mount_is_skipped_when_cwd_is_home() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(home.join(".local/share/containers/storage")).unwrap();

        let user = UserContext {
            uid: 1000,
            gid: 1000,
            home_dir: home.clone(),
            cwd: home.clone(),
        };

        assert!(!should_mount_cwd(&user));

        let mounts = base_mounts(&user, &[]).unwrap();
        assert!(!mounts
            .iter()
            .any(|mount| mount.host == home && mount.guest == Path::new(&user.cwd)));
    }

    #[test]
    fn base_mounts_include_host_docker_config_readonly_when_present() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let mounts = base_mounts(
            &UserContext {
                uid: 1000,
                gid: 1000,
                home_dir: home,
                cwd,
            },
            &[],
        )
        .unwrap();

        let docker_config = Path::new(HOST_DOCKER_CONFIG_DIR);
        assert_eq!(
            mounts.iter().any(|mount| {
                mount.host == docker_config
                    && mount.guest == docker_config
                    && mount.mode == MountMode::ReadOnly
            }),
            docker_config.exists()
        );
    }

    #[test]
    fn registry_mounts_map_docker_certs_and_auth_into_guest_container_paths() {
        let home = Path::new("/home/tester");
        let mounts = registry_mounts_from_existing(
            home,
            vec![
                Path::new(HOST_DOCKER_CONFIG_DIR).to_path_buf(),
                Path::new(HOST_DOCKER_CERTS_DIR).to_path_buf(),
                home.join(".docker").join("config.json"),
            ],
        );

        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new(HOST_DOCKER_CONFIG_DIR)
                && mount.guest == Path::new(HOST_DOCKER_CONFIG_DIR)
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new(HOST_DOCKER_CERTS_DIR)
                && mount.guest == Path::new(GUEST_CONTAINERS_CERTS_DIR)
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == home.join(".docker").join("config.json")
                && mount.guest == home.join(".docker").join("config.json")
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == home.join(".docker").join("config.json")
                && mount.guest == Path::new(GUEST_ROOT_CONTAINERS_AUTH_FILE)
                && mount.mode == MountMode::ReadOnly
        }));
    }

    #[test]
    fn registry_mounts_prefer_host_containers_certs_and_auth_when_present() {
        let home = Path::new("/home/tester");
        let mounts = registry_mounts_from_existing(
            home,
            vec![
                Path::new(HOST_DOCKER_CERTS_DIR).to_path_buf(),
                Path::new(HOST_CONTAINERS_CERTS_DIR).to_path_buf(),
                home.join(".docker").join("config.json"),
                home.join(".config").join("containers").join("auth.json"),
                home.join(".config").join("containers").join("certs.d"),
            ],
        );

        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new(HOST_CONTAINERS_CERTS_DIR)
                && mount.guest == Path::new(GUEST_CONTAINERS_CERTS_DIR)
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(!mounts.iter().any(|mount| {
            mount.host == Path::new(HOST_DOCKER_CERTS_DIR)
                && mount.guest == Path::new(GUEST_CONTAINERS_CERTS_DIR)
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == home.join(".config").join("containers").join("auth.json")
                && mount.guest == Path::new(GUEST_ROOT_CONTAINERS_AUTH_FILE)
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == home.join(".config").join("containers").join("certs.d")
                && mount.guest == Path::new(GUEST_ROOT_CONTAINERS_CERTS_DIR)
                && mount.mode == MountMode::ReadOnly
        }));
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
    fn base_mounts_seed_ssh_known_hosts_readonly_when_present() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("workspace");
        let known_hosts = home.join(".ssh/known_hosts");
        fs::create_dir_all(known_hosts.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(&known_hosts, "github.com ssh-ed25519 AAAA").unwrap();

        let mounts = base_mounts(
            &UserContext {
                uid: 1000,
                gid: 1000,
                home_dir: home,
                cwd,
            },
            &[],
        )
        .unwrap();

        assert!(has_ssh_known_hosts_mount(&mounts));
        assert!(mounts.iter().any(|mount| {
            mount.host == known_hosts
                && mount.guest == Path::new(GUEST_SSH_KNOWN_HOSTS_SEED)
                && mount.mode == MountMode::ReadOnly
        }));
    }

    #[test]
    fn discover_ca_trust_paths_are_existing_and_unique() {
        let mounts = discover_ca_trust_mounts();

        for mount in &mounts {
            assert!(mount.host.exists());
            assert_eq!(mount.mode, MountMode::ReadOnly);
        }

        let mut guests = mounts
            .iter()
            .map(|mount| mount.guest.clone())
            .collect::<Vec<_>>();
        guests.sort();
        guests.dedup();
        assert_eq!(mounts.len(), guests.len());
    }

    #[test]
    fn discover_ca_trust_mounts_map_debian_bundle_into_fedora_paths() {
        let mounts = discover_ca_trust_mounts_from_existing([
            Path::new("/etc/ssl/certs").to_path_buf(),
            Path::new("/etc/ca-certificates").to_path_buf(),
            Path::new("/etc/ssl/certs/ca-certificates.crt").to_path_buf(),
        ]);

        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new("/etc/ssl/certs")
                && mount.guest == Path::new("/etc/ssl/certs")
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new("/etc/ca-certificates")
                && mount.guest == Path::new("/etc/ca-certificates")
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new("/etc/ssl/certs/ca-certificates.crt")
                && mount.guest == Path::new("/etc/pki/tls/cert.pem")
                && mount.mode == MountMode::ReadOnly
        }));
        assert!(mounts.iter().any(|mount| {
            mount.host == Path::new("/etc/ssl/certs/ca-certificates.crt")
                && mount.guest == Path::new("/etc/ssl/cert.pem")
                && mount.mode == MountMode::ReadOnly
        }));
    }

    #[test]
    fn existing_readonly_self_mounts_skip_missing_paths() {
        let dir = tempdir().unwrap();
        let present = dir.path().join("docker");
        let missing = dir.path().join("missing");
        fs::create_dir_all(&present).unwrap();

        let mounts = existing_readonly_self_mounts(&[present.clone(), missing]);

        assert_eq!(mounts.len(), 1);
        assert!(mounts.iter().any(|mount| {
            mount.host == present && mount.guest == present && mount.mode == MountMode::ReadOnly
        }));
    }
}
