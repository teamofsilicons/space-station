//! The primitives behind every stored or checked secret: AES-256-GCM under `SS_KEY` for values we
//! must show again (access tokens, webhook secrets, IAM tokens, the login landing), HMAC-SHA256 for
//! webhook signatures, and a constant-time comparison for checking one. A sealed value is
//! `base64(nonce ‖ ciphertext)` with a fresh 12-byte nonce.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, AeadCore, KeyInit as _, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit as _, Mac};
use sha2::Sha256;

pub fn seal(key: &[u8; 32], plain: &str) -> String {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let mut out = nonce.to_vec();
    out.extend(Aes256Gcm::new(key.into()).encrypt(&nonce, plain.as_bytes()).expect("AES-GCM encryption is total"));
    STANDARD.encode(out)
}

/// `None` when the value was not sealed under `key`, or was tampered with.
pub fn open(key: &[u8; 32], sealed: &str) -> Option<String> {
    let bytes = STANDARD.decode(sealed).ok()?;
    let (nonce, ciphertext) = bytes.split_at_checked(12)?;
    let plain = Aes256Gcm::new(key.into()).decrypt(Nonce::from_slice(nonce), ciphertext).ok()?;
    String::from_utf8(plain).ok()
}

pub fn hmac_sha256_hex(secret: &[u8], message: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(message);
    hex(&mac.finalize().into_bytes())
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// Equal, in time that depends on the lengths alone: how a signature or a testing key presented
/// by a caller is compared, so a mismatch never says at which byte it happened.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    std::hint::black_box(diff) == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8; 32] = &[7; 32];

    #[test]
    fn sealed_values_round_trip_under_the_same_key_only() {
        let sealed = seal(KEY, "spacewindow-0123");
        assert_ne!(seal(KEY, "spacewindow-0123"), sealed, "every seal uses a fresh nonce");
        assert_eq!(open(KEY, &sealed).as_deref(), Some("spacewindow-0123"));
        assert_eq!(open(&[8; 32], &sealed), None);
        assert_eq!(open(KEY, "not base64!"), None);
        assert_eq!(open(KEY, ""), None);
    }

    #[test]
    fn a_tampered_ciphertext_does_not_open() {
        let mut bytes = STANDARD.decode(seal(KEY, "secret")).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(open(KEY, &STANDARD.encode(bytes)), None);
    }

    #[test]
    fn hmac_matches_the_known_answer() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(mac, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_ne!(random::<16>(), random::<16>());
    }

    #[test]
    fn constant_time_equality_is_equality() {
        assert!(ct_eq(b"v1=abc", b"v1=abc"));
        assert!(ct_eq(b"", b""));
        assert!(!ct_eq(b"v1=abc", b"v1=abd"));
        assert!(!ct_eq(b"v1=abc", b"v1=ab"), "a prefix is not equal");
        assert!(!ct_eq(b"v1=ab", b"v1=abc"));
        assert!(!ct_eq(b"", b"\0"), "nor is a zero byte the same as nothing");
    }
}
