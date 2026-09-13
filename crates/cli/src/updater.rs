use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

const DEFAULT_MANIFEST: &str = "https://github.com/teamofsilicons/space-station/releases/latest/download/SHA256SUMS";
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
const PUBLIC_KEY: &[u8] = include_bytes!("../update-public.pem");

/// Start the detached hourly checker. SPACE_STATION_UPDATE=0 opts out.
pub fn spawn(home: PathBuf) {
    if std::env::var("SPACE_STATION_UPDATE").is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false")) {
        return;
    }
    thread::spawn(move || {
        loop {
            if let Err(e) = check(&home) {
                eprintln!("spacestation update: {e}");
            }
            thread::sleep(CHECK_EVERY);
        }
    });
}

fn check(home: &Path) -> Result<(), String> {
    let manifest_url = std::env::var("SPACE_STATION_UPDATE_URL").unwrap_or_else(|_| DEFAULT_MANIFEST.into());
    let manifest = fetch(&manifest_url)?;
    let signature = fetch(&(manifest_url.clone() + ".sig"))?;
    verify_signature(&manifest, &signature)?;
    let digest = hex(&Sha256::digest(&manifest));
    let marker = home.join("update-manifest.sha256");
    if fs::read_to_string(&marker).ok().as_deref() == Some(&digest) {
        return Ok(());
    }
    let asset =
        asset_name().ok_or_else(|| "unsupported platform (updates support macOS/Linux x86_64/arm64)".to_string())?;
    let expected = checksum(&manifest, asset).ok_or_else(|| format!("signed manifest has no checksum for {asset}"))?;
    let base = manifest_url.rsplit_once('/').map_or("", |(base, _)| base);
    let archive = fetch(&format!("{base}/{asset}"))?;
    if hex(&Sha256::digest(&archive)) != expected {
        return Err(format!("checksum mismatch for {asset}; refusing update"));
    }
    let binary = extract(&archive)?;
    install(&binary)?;
    fs::create_dir_all(home).map_err(|e| format!("create update state: {e}"))?;
    fs::write(&marker, digest).map_err(|e| format!("write update state: {e}"))?;
    eprintln!("spacestation update: installed {asset}");
    Ok(())
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let mut response = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    response.body_mut().with_config().limit(64 << 20).read_to_vec().map_err(|e| format!("read {url}: {e}"))
}

fn verify_signature(manifest: &[u8], encoded: &[u8]) -> Result<(), String> {
    let key = parse_public_key(PUBLIC_KEY)?;
    verify_with_key(manifest, encoded, &key)
}

fn parse_public_key(pem: &[u8]) -> Result<VerifyingKey, String> {
    let key_text = std::str::from_utf8(pem).map_err(|e| format!("update key is not UTF-8: {e}"))?;
    let body = key_text.lines().filter(|line| !line.starts_with("---")).collect::<String>();
    let key =
        base64::engine::general_purpose::STANDARD.decode(body).map_err(|e| format!("embedded update key: {e}"))?;
    let key: [u8; 32] = match key.as_slice() {
        raw if raw.len() == 32 => raw.try_into().expect("length checked"),
        // Ed25519 SubjectPublicKeyInfo (the format emitted by `openssl pkey -pubout`).
        der if der.len() == 44
            && der[..12] == [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00] =>
        {
            der[12..].try_into().expect("length checked")
        }
        _ => return Err("update key is not an Ed25519 raw key or SubjectPublicKeyInfo".into()),
    };
    VerifyingKey::from_bytes(&key).map_err(|e| format!("update key is not Ed25519: {e}"))
}

fn verify_with_key(manifest: &[u8], encoded: &[u8], key: &VerifyingKey) -> Result<(), String> {
    let text = std::str::from_utf8(encoded).ok().map(str::trim).unwrap_or("");
    let sig = if encoded.len() == 64 {
        Signature::from_bytes(encoded.try_into().expect("length checked"))
    } else if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text) {
        Signature::from_slice(&bytes).map_err(|e| format!("invalid update signature: {e}"))?
    } else if let Ok(bytes) = hex_decode(text) {
        Signature::from_slice(&bytes).map_err(|e| format!("invalid update signature: {e}"))?
    } else {
        return Err("update signature is neither raw, base64, nor hexadecimal Ed25519".into());
    };
    key.verify(manifest, &sig).map_err(|_| "update manifest signature verification failed".into())
}

fn checksum(manifest: &[u8], asset: &str) -> Option<String> {
    std::str::from_utf8(manifest).ok()?.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let sum = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        (name == asset && sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| sum.to_ascii_lowercase())
    })
}

fn extract(archive: &[u8]) -> Result<Vec<u8>, String> {
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| format!("read update archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("read update archive: {e}"))?;
        let path = entry.path().map_err(|e| format!("read update archive path: {e}"))?;
        if path == Path::new("spacestation") && entry.header().entry_type().is_file() {
            let mut binary = Vec::new();
            entry.read_to_end(&mut binary).map_err(|e| format!("read update binary: {e}"))?;
            return Ok(binary);
        }
    }
    Err("update archive has no spacestation binary".into())
}

fn install(binary: &[u8]) -> Result<(), String> {
    let current = env::current_exe().map_err(|e| format!("find current executable: {e}"))?;
    let temp = current.with_extension("update.new");
    let mut file = fs::File::create(&temp).map_err(|e| format!("write update: {e}"))?;
    file.write_all(binary).and_then(|_| file.sync_all()).map_err(|e| format!("write update: {e}"))?;
    drop(file);
    let mut permissions = fs::metadata(&current).map_err(|e| format!("stat current executable: {e}"))?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(&temp, permissions).map_err(|e| format!("set update permissions: {e}"))?;
    fs::rename(&temp, &current).map_err(|e| format!("replace current executable: {e}"))
}

fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("spacestation-darwin-arm64.tar.gz"),
        ("macos", "x86_64") => Some("spacestation-darwin-x64.tar.gz"),
        ("linux", "aarch64") => Some("spacestation-linux-arm64.tar.gz"),
        ("linux", "x86_64") => Some("spacestation-linux-x64.tar.gz"),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn finds_checksums_and_platform_asset() {
        let sums = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  spacestation-linux-x64.tar.gz\n";
        assert_eq!(checksum(sums, "spacestation-linux-x64.tar.gz"), Some("a".repeat(64)));
        assert!(asset_name().is_some());
    }

    #[test]
    fn verifies_a_detached_signature_round_trip() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().to_bytes())
        );
        let key = parse_public_key(pem.as_bytes()).unwrap();
        let manifest = b"signed checksums\n";
        let signature = signing.sign(manifest);
        verify_with_key(manifest, &signature.to_bytes(), &key).unwrap();
        assert!(verify_with_key(b"tampered", &signature.to_bytes(), &key).is_err());
    }

    #[test]
    fn parses_the_embedded_openssl_public_key() {
        let key = parse_public_key(PUBLIC_KEY).unwrap();
        let signature = hex_decode("a8a337b3a02e2d32332193e683fb1afcb278be68b18bcf02ab3988e4671f35c0bfa520f0a1e34e949e38ab718a2f6b568b17d253e15ccd802150fc73eaf97003").unwrap();
        verify_with_key(b"release checksum bytes\n", &signature, &key).unwrap();
    }
}
