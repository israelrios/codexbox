use std::fs;
use std::path::{Path, PathBuf};

pub fn is_forbidden_env_mount(path: &Path, home_dir: &Path) -> bool {
    canonicalize_if_possible(path) == canonicalize_if_possible(home_dir)
}

fn canonicalize_if_possible(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::is_forbidden_env_mount;

    #[test]
    fn only_home_root_is_forbidden() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        let subdir = home.join("project");
        fs::create_dir_all(&subdir).unwrap();

        assert!(is_forbidden_env_mount(&home, &home));
        assert!(!is_forbidden_env_mount(&subdir, &home));
    }
}
