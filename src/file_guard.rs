use anyhow::{Context, Result};
use std::path::Path;

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub fn file_hash(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(stable_hash_bytes(&bytes))
}

pub fn stable_hash_bytes(bytes: &[u8]) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv1a64:{hash:016x}")
}

pub fn hash_matches(expected: &str, actual: &str) -> bool {
    normalize_hash(expected) == normalize_hash(actual)
}

fn normalize_hash(value: &str) -> String {
    value
        .trim()
        .strip_prefix("fnv1a64:")
        .unwrap_or_else(|| value.trim())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_hash_is_prefixed_and_repeatable() {
        assert_eq!(stable_hash_bytes(b"abc"), stable_hash_bytes(b"abc"));
        assert!(stable_hash_bytes(b"abc").starts_with("fnv1a64:"));
        assert_ne!(stable_hash_bytes(b"abc"), stable_hash_bytes(b"abcd"));
    }

    #[test]
    fn hash_match_accepts_bare_hex() {
        let hash = stable_hash_bytes(b"abc");
        let bare = hash.trim_start_matches("fnv1a64:");
        assert!(hash_matches(bare, &hash));
    }
}
