use std::fs;
use std::path::{Path, PathBuf};

pub fn expand_tilde(path: &Path, home_dir: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return home_dir.to_path_buf();
    }

    if let Some(stripped) = raw.strip_prefix("~/") {
        return home_dir.join(stripped);
    }

    path.to_path_buf()
}

pub fn resolve_from_home(path: &Path, home_dir: &Path) -> PathBuf {
    let expanded = expand_tilde(path, home_dir);
    if expanded.is_absolute() {
        expanded
    } else {
        home_dir.join(expanded)
    }
}

pub fn resolve_from_base(path: &Path, base_dir: &Path, home_dir: &Path) -> PathBuf {
    let expanded = expand_tilde(path, home_dir);
    if expanded.is_absolute() {
        expanded
    } else {
        base_dir.join(expanded)
    }
}

pub fn canonicalize_if_possible(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
