use std::env;
use std::fs;
use std::path::PathBuf;

const EMBEDDED_ASSET_PATHS: &[&str] = &[
    "Containerfile",
    "containers.conf",
    "podman-containers.conf",
    "container-entrypoint.sh",
];

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"));

    let mut hash = Fnv1a64::new();
    hash.write(env!("CARGO_PKG_VERSION").as_bytes());
    hash.write(&[0]);

    for relative_path in EMBEDDED_ASSET_PATHS {
        println!("cargo:rerun-if-changed={relative_path}");

        let bytes = fs::read(manifest_dir.join(relative_path)).unwrap_or_else(|error| {
            panic!("failed to read embedded asset {relative_path}: {error}")
        });
        hash.write(&bytes);
        hash.write(&[0]);
    }

    let output = format!(
        "const EMBEDDED_IMAGE_FINGERPRINT: &str = \"{:016x}\";\n",
        hash.finish()
    );
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set"));
    fs::write(out_dir.join("embedded_assets.rs"), output)
        .expect("failed to write embedded asset fingerprint");
}

struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}
