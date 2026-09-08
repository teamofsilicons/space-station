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

/// The first secret-shaped token in `text`, if any: ours — including the `sscli-` session a
/// terminal holds — and every IAM credential: a silicon's `stk-` token, an Application's `ask_`
/// secret, and the bearers and refresh tokens. Equivalent to
/// `\b(spacewindow|apikey|whsec|stk|table-[a-z0-9]{1,50})-[0-9a-f]{32}\b|\b((sat|cat|rft|oat|ort|ask)_|sscli-)[A-Za-z0-9_-]{43}\b`
/// and catches tokens in comments and strings alike. Code that matches must never be stored, and a
/// short-lived token (`oac_`, the one thing meant to be handed over) is deliberately not here.
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
        if let Some(end) = match_hex32(rest).or_else(|| match_base64(rest)) {
            let after = bytes.get(i + end).copied();
            if after.is_none_or(|b| !is_word(b)) {
                return Some(&rest[..end]);
            }
        }
        i += 1;
    }
    None
}

/// `spacewindow-`, `apikey-`, `whsec-`, `stk-` or `table-{id}-` followed by 32 hex; returns the length.
fn match_hex32(s: &str) -> Option<usize> {
    for prefix in ["spacewindow-", "apikey-", "whsec-", "stk-"] {
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

/// `sat_|cat_|rft_|oat_|ort_|ask_` or `sscli-`, followed by exactly 43 URL-safe base64 characters.
fn match_base64(s: &str) -> Option<usize> {
    let prefix = ["sat_", "cat_", "rft_", "oat_", "ort_", "ask_", "sscli-"].into_iter().find(|p| s.starts_with(p))?;
    let body = s.get(prefix.len()..prefix.len() + 43)?;
    body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')).then_some(prefix.len() + 43)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_keys_round_trip() {
        let key = table_key("orders");
        assert_eq!(parse_table_key(&key), Some("orders"));
        assert_eq!(parse_table_key("table-Orders-00000000000000000000000000000000"), None);
        assert_eq!(parse_table_key("spacewindow-00000000000000000000000000000000"), None);
    }

    #[test]
    fn scanner_finds_every_shape_and_respects_boundaries() {
        let hex = "0123456789abcdef0123456789abcdef";
        for s in [
            format!("// token: spacewindow-{hex}"),
            format!("const k = 'apikey-{hex}'"),
            format!("whsec-{hex}"),
            format!("table-orders-{hex}"),
            format!("Bearer sat_{}", "a".repeat(43)),
            format!("rft_{}\n", "B-_9".repeat(11).chars().take(43).collect::<String>()),
            format!("IAM_TEST_SILICON_STK=stk-{hex}"),
            format!("SILICON_IAM_APP_SECRET='ask_{}'", "k".repeat(43)),
            format!("Authorization: Bearer sscli-{}", "s".repeat(43)),
        ] {
            assert!(find_secret(&s).is_some(), "{s}");
        }
        let session = format!("sscli-{}", "A-_9".repeat(11).chars().take(43).collect::<String>());
        assert_eq!(find_secret(&session), Some(session.as_str()), "a terminal's own session is a secret");
        assert_eq!(find_secret(&format!("sscli-{}", "s".repeat(42))), None);
        assert_eq!(find_secret(&format!("sscli_{}", "s".repeat(43))), None, "the separator is part of the shape");
        assert_eq!(find_secret(&format!("stk-{hex}")), Some(&*format!("stk-{hex}")), "a silicon's token is a secret");
        assert_eq!(find_secret(&format!("ask_{}", "k".repeat(43))), Some(&*format!("ask_{}", "k".repeat(43))));
        assert_eq!(
            find_secret(&format!("oac_{}", "k".repeat(43))),
            None,
            "a short-lived token is meant to be handed over"
        );
        assert_eq!(find_secret(&format!("stk_{hex}")), None);
        assert_eq!(find_secret(&format!("stk-{}", &hex[..31])), None);
        assert_eq!(find_secret(&format!("xspacewindow-{hex}")), None);
        assert_eq!(find_secret(&format!("spacewindow-{hex}f")), None);
        assert_eq!(find_secret(&format!("sat_{}", "a".repeat(42))), None);
        assert_eq!(find_secret(&format!("sat_{}", "a".repeat(44))), None);
        assert_eq!(find_secret("spacewindow-notahexvalue"), None);
        assert_eq!(find_secret("mission_control.query('select 1')"), None);
    }

    #[test]
    fn scanner_walks_past_multi_byte_characters() {
        // Window code is prose as much as code: an em dash in a comment must not stop a publish,
        // and must not hide a token that follows it.
        let hex = "0123456789abcdef0123456789abcdef";
        assert_eq!(find_secret("// a note — nothing secret here"), None);
        assert_eq!(find_secret("// café ☕ — 日本語"), None);
        assert_eq!(find_secret(&format!("// café — spacewindow-{hex}")), Some(&*format!("spacewindow-{hex}")));
        assert_eq!(find_secret(&format!("—spacewindow-{hex}")), Some(&*format!("spacewindow-{hex}")));
        assert_eq!(find_secret(&format!("spacewindow-{hex} — done")), Some(&*format!("spacewindow-{hex}")));
        assert_eq!(find_secret(&"é".repeat(1000)), None);
    }

    #[test]
    fn sha256_is_hex() {
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
