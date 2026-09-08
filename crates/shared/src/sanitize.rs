//! Keeps a record inside the limits before it leaves the process, and re-checks it on the
//! server. Strings over `VALUE_MAX` are cut in the middle; strings that look like files become
//! `"[FILETYPE:SIZE]"`; a record still over `RECORD_MAX` is refused.

use serde_json::Value;

use crate::limits::{RECORD_MAX, VALUE_MAX};

/// The record is over `RECORD_MAX` bytes even after sanitising.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeExceeded {
    pub bytes: usize,
}

impl std::fmt::Display for SizeExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "record is {} bytes, the limit is {RECORD_MAX}", self.bytes)
    }
}

impl std::error::Error for SizeExceeded {}

/// Sanitise every string in `value` in place, then check the whole record's size.
pub fn record(value: &mut Value) -> Result<(), SizeExceeded> {
    walk(value);
    let bytes = size(value);
    if bytes > RECORD_MAX { Err(SizeExceeded { bytes }) } else { Ok(()) }
}

/// UTF-8 byte length of the JSON text this value serialises to.
pub fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map(|v| v.len()).unwrap_or(usize::MAX)
}

fn walk(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Some(kind) = file_kind(s) {
                *s = format!("[{kind}:{}]", s.len());
            } else if s.len() > VALUE_MAX {
                *s = truncate_middle(s, VALUE_MAX);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(walk),
        Value::Object(map) => map.values_mut().for_each(walk),
        _ => {}
    }
}

/// `"abc...[SIZE]...xyz"`: the first and last halves of `max` minus the marker, snapped to
/// char boundaries, with `SIZE` the original byte length.
pub fn truncate_middle(s: &str, max: usize) -> String {
    let marker = format!("...[{}]...", s.len());
    let keep = max.saturating_sub(marker.len()) / 2;
    let head = &s[..floor_char_boundary(s, keep)];
    let tail = &s[ceil_char_boundary(s, s.len() - keep)..];
    format!("{head}{marker}{tail}")
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// What kind of file this string looks like, if any.
///
/// A JSON string is always UTF-8, so raw binary can only arrive as a data URI, as a base64
/// blob, or as ASCII magic followed by a control-character-heavy body. Those are what we detect.
pub fn file_kind(s: &str) -> Option<&'static str> {
    if s.len() < 64 {
        return None;
    }
    if s.starts_with("data:") && s[..s.len().min(160)].contains(";base64,") {
        return Some("DATA_URI");
    }
    const MAGIC: &[(&str, &str)] = &[
        ("%PDF-", "PDF"),
        ("PK\u{3}\u{4}", "ZIP"),
        ("GIF87a", "GIF"),
        ("GIF89a", "GIF"),
        ("RIFF", "RIFF"),
        ("OggS", "OGG"),
        ("ID3", "MP3"),
        ("%!PS", "POSTSCRIPT"),
        ("\u{7f}ELF", "ELF"),
        ("\u{89}PNG", "PNG"),
    ];
    if let Some((_, kind)) = MAGIC.iter().find(|(magic, _)| s.starts_with(magic)) {
        return Some(kind);
    }
    if s.len() >= 256
        && s.len().is_multiple_of(4)
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
        && s.bytes().any(|b| b.is_ascii_digit())
        && s.bytes().any(|b| b.is_ascii_uppercase())
        && s.bytes().any(|b| b.is_ascii_lowercase())
    {
        return Some("BASE64");
    }
    let sample = &s.as_bytes()[..s.len().min(4096)];
    let control = sample.iter().filter(|b| b.is_ascii_control() && !matches!(b, b'\n' | b'\r' | b'\t')).count();
    (control * 10 > sample.len()).then_some("BINARY")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn long_strings_are_cut_in_the_middle_with_the_size() {
        let s = "a".repeat(40_000) + &"z".repeat(40_000);
        let cut = truncate_middle(&s, VALUE_MAX);
        assert!(cut.len() <= VALUE_MAX);
        assert!(cut.starts_with("aaa") && cut.ends_with("zzz"));
        assert!(cut.contains("...[80000]..."));
    }

    #[test]
    fn multibyte_cuts_land_on_char_boundaries() {
        let s = "é".repeat(30_000);
        let cut = truncate_middle(&s, VALUE_MAX);
        assert!(cut.len() <= VALUE_MAX);
        assert!(cut.contains("...[60000]..."));
    }

    #[test]
    fn files_become_markers() {
        let png = format!("data:image/png;base64,{}", "A".repeat(200));
        assert_eq!(file_kind(&png), Some("DATA_URI"));
        assert_eq!(file_kind(&format!("%PDF-1.7 {}", "x".repeat(100))), Some("PDF"));
        assert_eq!(file_kind(&"aZ9+/".repeat(64).chars().take(512).collect::<String>()), Some("BASE64"));
        assert_eq!(file_kind(&"\u{1}\u{2}\u{3}abc".repeat(40)), Some("BINARY"));
        assert_eq!(file_kind("hello world, a perfectly ordinary sentence that is long enough to check"), None);
        assert_eq!(file_kind(&"lorem ipsum ".repeat(50)), None);
    }

    #[test]
    fn record_is_sanitised_recursively_and_sized() {
        let mut v = json!({"a": {"b": ["x".repeat(50_000), {"c": format!("%PDF-{}", "y".repeat(100))}]}, "n": 1});
        record(&mut v).unwrap();
        let s = v["a"]["b"][0].as_str().unwrap();
        assert!(s.len() <= VALUE_MAX && s.contains("...[50000]..."));
        assert_eq!(v["a"]["b"][1]["c"], "[PDF:105]");
        assert_eq!(v["n"], 1);

        let mut big = json!((0..20).map(|_| "q".repeat(20_000)).collect::<Vec<_>>());
        let err = record(&mut big).unwrap_err();
        assert!(err.bytes > RECORD_MAX);
    }
}
