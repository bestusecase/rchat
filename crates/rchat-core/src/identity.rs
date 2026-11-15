//! Local identity.
//!
//! v1 uses a random 32-byte secret and a stable public id derived from it. That
//! is enough to name a profile on this device. Reticulum owns real addressing
//! and encryption. When app-layer group keys arrive (voice, multi-device), this
//! secret becomes the seed for an Ed25519/X25519 keypair, which is why it is
//! stored as raw bytes rather than a hash.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct SecretIdentity {
    pub id: String,
    pub secret: [u8; 32],
}

pub fn generate() -> SecretIdentity {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).expect("os rng unavailable");
    from_secret(secret)
}

pub fn from_secret(secret: [u8; 32]) -> SecretIdentity {
    SecretIdentity {
        id: derive_id(&secret),
        secret,
    }
}

pub fn derive_id(secret: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rchat-identity-v1");
    hasher.update(secret);
    let digest = hasher.finalize();
    hex(&digest[..16])
}

pub fn decode_secret(bytes: &[u8]) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .context("stored identity secret has the wrong length")
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_stable_and_short() {
        let a = from_secret([7u8; 32]);
        let b = from_secret([7u8; 32]);
        assert_eq!(a.id, b.id);
        assert_eq!(a.id.len(), 32);
        assert_ne!(a.id, from_secret([8u8; 32]).id);
    }
}
