include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

pub(crate) const CONTAINERFILE: &str = include_str!("../../Containerfile");
pub(crate) const CONTAINERS_CONF: &[u8] = include_bytes!("../../containers.conf");
pub(crate) const PODMAN_CONTAINERS_CONF: &[u8] = include_bytes!("../../podman-containers.conf");
pub(crate) const CONTAINER_ENTRYPOINT: &[u8] = include_bytes!("../../container-entrypoint.sh");

pub fn embedded_image_fingerprint() -> String {
    EMBEDDED_IMAGE_FINGERPRINT.to_string()
}

#[cfg(test)]
struct Fnv1a64(u64);

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::{
        embedded_image_fingerprint, Fnv1a64, CONTAINERFILE, CONTAINERS_CONF, CONTAINER_ENTRYPOINT,
        EMBEDDED_IMAGE_FINGERPRINT, PODMAN_CONTAINERS_CONF,
    };

    fn compute_embedded_fingerprint() -> String {
        let mut hash = Fnv1a64::new();

        for chunk in [
            env!("CARGO_PKG_VERSION").as_bytes(),
            CONTAINERFILE.as_bytes(),
            CONTAINERS_CONF,
            PODMAN_CONTAINERS_CONF,
            CONTAINER_ENTRYPOINT,
        ] {
            hash.write(chunk);
            hash.write(&[0]);
        }

        format!("{:016x}", hash.finish())
    }

    #[test]
    fn embedded_image_fingerprint_is_stable_shape() {
        let fingerprint = embedded_image_fingerprint();

        assert_eq!(fingerprint.len(), 16);
        assert!(fingerprint.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(fingerprint, embedded_image_fingerprint());
    }

    #[test]
    fn embedded_image_fingerprint_matches_embedded_assets() {
        assert_eq!(EMBEDDED_IMAGE_FINGERPRINT, compute_embedded_fingerprint());
    }
}
