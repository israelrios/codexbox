use std::path::PathBuf;

pub fn discover_ca_trust_paths() -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/etc/ssl/certs"),
        PathBuf::from("/etc/pki/tls/certs"),
        PathBuf::from("/etc/ca-certificates"),
        PathBuf::from("/etc/ssl/cert.pem"),
        PathBuf::from("/etc/ssl/ca-bundle.pem"),
        PathBuf::from("/etc/ssl/ca-bundle.crt"),
        PathBuf::from("/etc/pki/ca-trust"),
        PathBuf::from("/etc/pki/tls/cert.pem"),
    ];

    paths.retain(|path| path.exists());
    paths.sort();
    paths.dedup();
    paths
}
