//! The shapes of every secret this system issues, how to mint and hash them, and the one
//! scanner that refuses code carrying any of them.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 32 lowercase hex characters, 122 bits of entropy.
pub fn hex32() -> String {
    Uuid::new_v4().simple().to_string()
}

/// `sha256` of the bytes as lowercase hex. How every hashed secret is stored and looked up.
pub fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `table-{table_id}-{32hex}`. Table ids are `^[a-z0-9]{1,50}$`, so the dashes are unambiguous.
pub fn table_key(table_id: &str) -> String {
    format!("table-{table_id}-{}", hex32())
}

/// The `table_id` inside a table key, or `None` if the key is not one.
pub fn parse_table_key(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("table-")?;
    let (table_id, hex) = rest.rsplit_once('-')?;
    (valid_table_id(table_id) && is_hex32(hex)).then_some(table_id)
}

pub fn access_token() -> String {
    format!("spacewindow-{}", hex32())
}

pub fn api_key() -> String {
    format!("apikey-{}", hex32())
}

pub fn webhook_secret() -> String {
    format!("whsec-{}", hex32())
}

pub fn valid_table_id(id: &str) -> bool {
    (1..=50).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The first Space Station or Silicon Accounts secret in `text`, including credentials in
/// comments and strings. Short-lived `slt_` sign-in tokens are deliberately excluded.
pub fn find_secret(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        // A match may only start at a word boundary, and only where a character does.
        if !text.is_char_boundary(i) || (i > 0 && is_word(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if let Some(end) = match_hex32(rest).or_else(|| match_base64(rest)).or_else(|| match_accounts(rest)) {
            let after = bytes.get(i + end).copied();
            if after.is_none_or(|b| !is_word(b)) {
                return Some(&rest[..end]);
            }
        }
        i += 1;
    }
    None
}

/// Space Station secrets followed by 32 hex; returns the length.
fn match_hex32(s: &str) -> Option<usize> {
    for prefix in ["spacewindow-", "apikey-", "whsec-"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return is_hex32(rest.get(..32)?).then_some(prefix.len() + 32);
        }
    }
    let rest = s.strip_prefix("table-")?;
    let id_len = rest.bytes().take(50).take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit()).count();
    if id_len == 0 || rest.as_bytes().get(id_len) != Some(&b'-') {
        return None;
    }
    let hex = rest.get(id_len + 1..id_len + 33)?;
    is_hex32(hex).then_some(6 + id_len + 33)
}

/// Current 64-hex CLI sessions, or opaque credentials with 43 URL-safe base64 characters.
fn match_base64(s: &str) -> Option<usize> {
    if let Some(body) = s.strip_prefix("sscli-") {
        if body.get(..64).is_some_and(|v| v.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Some(6 + 64);
        }
        let legacy = body.get(..43)?;
        return legacy.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')).then_some(6 + 43);
    }
    let prefix = [
        "sar_", "sas_", "sa_app_", "whsec_", "sap_", "sapr_", "sad_", "saf_", "sau_", "sarq_", "sac_", "sat_", "cat_",
        "rft_", "oat_", "ort_", "ask_",
    ]
    .into_iter()
    .find(|p| s.starts_with(p))?;
    let body = s.get(prefix.len()..prefix.len() + 43)?;
    body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')).then_some(prefix.len() + 43)
}

/// Current STKs can be 8–32 hex characters; Accounts access and identity tokens are JWTs.
fn match_accounts(s: &str) -> Option<usize> {
    if let Some(body) = s.strip_prefix("stk-") {
        let len = body.bytes().take_while(u8::is_ascii_hexdigit).count();
        return (8..=32).contains(&len).then_some(4 + len);
    }
    if !s.starts_with("eyJ") {
        return None;
    }
    let mut offset = 0;
    for part in 0..3 {
        let len = s[offset..].bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')).count();
        if len == 0 {
            return None;
        }
        offset += len;
        if part < 2 {
            if s.as_bytes().get(offset) != Some(&b'.') {
                return None;
            }
            offset += 1;
        }
    }
    Some(offset)
}
