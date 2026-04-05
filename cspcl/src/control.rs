use hardy_bpa::Bytes;

pub fn bundle_digest(bundle: &Bytes) -> [u8; 32] {
    *blake3::hash(bundle).as_bytes()
}
