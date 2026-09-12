//! Everything the server takes from the environment, read once at boot into `Config`, with a
//! `.env` in the working directory underneath it. One clear error names the first variable that
//! is missing or malformed; nothing else reads `env`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;

use url::Url;

#[derive(Clone)]
pub struct Config {
    /// `0.0.0.0:PORT`; tests bind `127.0.0.1:0`.
    pub bind: SocketAddr,
    /// `SS_ORIGIN` as `scheme://host[:port]`: the frontend's origin, which cookie-authenticated
    /// mutations and WebSocket upgrades must come from, and where a login lands.
    pub origin: String,
    /// Optional parent domain shared by the frontend and its direct WebSocket API subdomain.
    pub cookie_domain: Option<String>,
    pub key: [u8; 32],
    pub clickhouse_url: Url,
    pub clickhouse_query_password: String,
    pub database_url: String,
    pub redis_url: String,
    pub iam_url: String,
    /// Browser-facing IAM login endpoint; the API client still uses `iam_url`.
    pub iam_auth_url: String,
    /// The canonical `{org}>{handle}` Application id, `tos>spacestation`.
    pub iam_app_id: String,
    pub iam_app_secret: String,
    /// What IAM signs webhook deliveries with: caller-chosen, 32 to 512 characters.
    pub iam_webhook_secret: String,
    /// The secret before the last rotation, accepted while IAM drains deliveries signed with it.
    pub iam_webhook_secret_previous: Option<String>,
    /// The IAM testing environment this deployment lives in, if any: sent on every IAM request
    /// and expected inside every webhook's `test` envelope. Root authority — never logged.
    pub iam_test_key: Option<String>,
    pub allow_private_webhooks: bool,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        Self::from_vars(dotenv(Path::new(".env"), |name| std::env::var(name).ok()))
    }

    /// `var` answers like `std::env::var`; tests pass a map.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        let optional = |name: &str| var(name).filter(|v| !v.is_empty());
        let need = |name: &str| optional(name).ok_or_else(|| format!("{name} is not set"));
        let port = var("PORT").map_or(Ok(8080), |p| p.parse::<u16>().map_err(|_| "PORT must be a port number"))?;
        let origin = Url::parse(&need("SS_ORIGIN")?)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
            .ok_or("SS_ORIGIN must be an http(s) URL")?;
        let key = need("SS_KEY")?;
        let cookie_domain = optional("SS_COOKIE_DOMAIN");
        if let Some(domain) = &cookie_domain {
            let host = origin.host_str().unwrap_or_default();
            if origin.scheme() != "https"
                || domain != host
                || !domain.contains('.')
                || !domain.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
            {
                return Err("SS_COOKIE_DOMAIN must exactly match the HTTPS frontend hostname".into());
            }
        }
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("SS_KEY must be 64 hex characters".into());
        }
        let mut bytes = [0; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&key[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
        }
        let clickhouse_url = Url::parse(&need("CLICKHOUSE_URL")?)
            .ok()
            .filter(|u| u.host().is_some() && u.path().len() > 1)
            .ok_or("CLICKHOUSE_URL must be http(s)://user:password@host:port/database")?;
        let iam_app_id = need("SILICON_IAM_APP_ID")?;
        if !canonical_app_id(&iam_app_id) {
            return Err("SILICON_IAM_APP_ID must be the canonical {org}>{handle} Application id".into());
        }
        // 32 to 512 non-whitespace ASCII, exactly what silicon-iam-client's WebhookSecret accepts,
        // so a misconfigured secret fails here at boot rather than on every webhook delivery.
        let webhook_secret = |name: &str, value: &str| match (32..=512).contains(&value.len())
            && value.bytes().all(|b| b.is_ascii_graphic())
        {
            true => Ok(value.to_owned()),
            false => Err(format!("{name} must be 32 to 512 non-whitespace ASCII characters")),
        };
        let iam_webhook_secret = webhook_secret("SILICON_IAM_WEBHOOK_SECRET", &need("SILICON_IAM_WEBHOOK_SECRET")?)?;
        let iam_webhook_secret_previous = optional("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS")
            .map(|v| webhook_secret("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS", &v))
            .transpose()?;
        let iam_test_key = optional("SILICON_IAM_TEST_KEY");
        if iam_test_key.as_deref().is_some_and(|k| k.len() != 32 || !k.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return Err("SILICON_IAM_TEST_KEY must be the 32-character alphanumeric environment key".into());
        }
        let iam_url = need("SILICON_IAM_URL")?;
        let iam_auth_url = optional("SILICON_IAM_AUTH_URL")
            .map(|url| format!("{}/login", url.trim_end_matches('/')))
            .unwrap_or_else(|| format!("{}/api/v1/login", iam_url.trim_end_matches('/')));
        Ok(Config {
            bind: SocketAddr::from(([0, 0, 0, 0], port)),
            origin: origin.origin().ascii_serialization(),
            cookie_domain,
            key: bytes,
            clickhouse_url,
            clickhouse_query_password: need("CLICKHOUSE_QUERY_PASSWORD")?,
            database_url: need("DATABASE_URL")?,
            redis_url: need("REDIS_URL")?,
            iam_url: iam_url.trim_end_matches('/').to_owned(),
            iam_auth_url,
            iam_app_id,
            iam_app_secret: need("SILICON_IAM_APP_SECRET")?,
            iam_webhook_secret,
            iam_webhook_secret_previous,
            iam_test_key,
            allow_private_webhooks: var("SS_ALLOW_PRIVATE_WEBHOOKS")
                .is_some_and(|v| !matches!(v.as_str(), "" | "0" | "false")),
        })
    }
}

/// IAM's `AppId`: `^[a-z0-9_-]{3,50}>[a-z][a-z0-9_-]{2,79}$`. A bare handle is what the retired
/// SDK took, and what IAM now refuses, so it is refused here with a message that says why.
fn canonical_app_id(id: &str) -> bool {
    let word = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-';
    let Some((org, handle)) = id.split_once('>') else { return false };
    (3..=50).contains(&org.len())
        && org.bytes().all(word)
        && (3..=80).contains(&handle.len())
        && handle.as_bytes()[0].is_ascii_lowercase()
        && handle.bytes().all(word)
}

/// `env` with the `KEY=value` lines of `path` underneath it: a real environment variable always
/// wins, so the file is only a convenience for running the server by hand. A missing file is an
/// empty one; `#` comments and blank lines are skipped and one pair of surrounding quotes is
/// dropped, the way `node:util`'s `parseEnv` reads the dev server's `.env`. No escapes, no
/// expansion, no search of parent directories.
pub fn dotenv<E: Fn(&str) -> Option<String>>(path: &Path, env: E) -> impl Fn(&str) -> Option<String> + use<E> {
    let unquote =
        |v: &str| ['"', '\''].iter().find_map(|q| v.strip_prefix(*q)?.strip_suffix(*q)).unwrap_or(v).to_owned();
    let file: HashMap<String, String> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.trim().to_owned(), unquote(value.trim())))
        .collect();
    move |name| env(name).or_else(|| file.get(name).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("SS_ORIGIN", "http://localhost:3000/"),
            ("SS_KEY", "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
            ("CLICKHOUSE_URL", "http://dev:dev@localhost:8123/space_station"),
            ("CLICKHOUSE_QUERY_PASSWORD", "pw"),
            ("DATABASE_URL", "postgres://dev:dev@localhost:5433/space_station"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("SILICON_IAM_URL", "http://127.0.0.1:8099/"),
            ("SILICON_IAM_APP_ID", "tos>spacestation"),
            ("SILICON_IAM_APP_SECRET", "ask_x"),
            ("SILICON_IAM_WEBHOOK_SECRET", "whs_stubstubstubstubstubstubstubstubstubstubabc"),
        ])
    }

    fn from(map: &HashMap<&str, &str>) -> Result<Config, String> {
        Config::from_vars(|name| map.get(name).map(|v| v.to_string()))
    }

    fn error(map: &HashMap<&str, &str>) -> String {
        from(map).err().expect("an invalid environment is refused")
    }

    #[test]
    fn a_full_environment_parses_and_normalises() {
        let cfg = from(&full()).unwrap();
        assert_eq!(cfg.origin, "http://localhost:3000");
        assert_eq!(cfg.bind.port(), 8080);
        assert_eq!(cfg.key[..3], [0x00, 0x11, 0x22]);
        assert_eq!(cfg.iam_url, "http://127.0.0.1:8099");
        assert_eq!(cfg.iam_auth_url, "http://127.0.0.1:8099/api/v1/login");
        assert_eq!(cfg.iam_app_id, "tos>spacestation");
        assert!(cfg.iam_webhook_secret_previous.is_none() && cfg.iam_test_key.is_none());
        assert!(!cfg.allow_private_webhooks);
    }

    #[test]
    fn cookie_domain_is_limited_to_the_https_frontend_hostname() {
        let mut map = full();
        assert!(from(&map).unwrap().cookie_domain.is_none());
        map.insert("SS_ORIGIN", "https://spacestation.teamofsilicons.com");
        map.insert("SS_COOKIE_DOMAIN", "spacestation.teamofsilicons.com");
        assert_eq!(from(&map).unwrap().cookie_domain.as_deref(), Some("spacestation.teamofsilicons.com"));
        for bad in [
            "teamofsilicons.com",
            "unrelated.example",
            ".spacestation.teamofsilicons.com",
            "spacestation.teamofsilicons.com; Secure",
        ] {
            map.insert("SS_COOKIE_DOMAIN", bad);
            assert!(from(&map).is_err(), "{bad} would widen or corrupt the cookie");
        }
        map.insert("SS_COOKIE_DOMAIN", "spacestation.teamofsilicons.com");
        map.insert("SS_ORIGIN", "http://spacestation.teamofsilicons.com");
        assert!(from(&map).is_err());
    }

    #[test]
    fn each_missing_or_malformed_variable_is_named() {
        let mut map = full();
        map.remove("DATABASE_URL");
        assert_eq!(error(&map), "DATABASE_URL is not set");
        let mut map = full();
        map.insert("SS_KEY", "abc");
        assert_eq!(error(&map), "SS_KEY must be 64 hex characters");
        let mut map = full();
        map.insert("SS_ORIGIN", "localhost:3000");
        assert_eq!(error(&map), "SS_ORIGIN must be an http(s) URL");
        let mut map = full();
        map.insert("PORT", "http");
        assert_eq!(error(&map), "PORT must be a port number");
        let mut map = full();
        map.insert("SS_ALLOW_PRIVATE_WEBHOOKS", "1");
        assert!(from(&map).unwrap().allow_private_webhooks);
    }

    #[test]
    fn the_application_id_is_the_canonical_one_iam_issues() {
        let mut map = full();
        map.insert("SILICON_IAM_APP_ID", "space-station");
        assert_eq!(error(&map), "SILICON_IAM_APP_ID must be the canonical {org}>{handle} Application id");
        for bad in ["tos>", ">spacestation", "tos>1station", "TOS>spacestation", "to>spacestation"] {
            map.insert("SILICON_IAM_APP_ID", bad);
            assert!(from(&map).is_err(), "{bad:?} is not a canonical id");
        }
        map.insert("SILICON_IAM_APP_ID", "acme_co>obs-2");
        assert!(from(&map).is_ok());
    }

    #[test]
    fn webhook_secrets_are_32_to_512_characters_with_no_prefix_rule() {
        let mut map = full();
        map.insert("SILICON_IAM_WEBHOOK_SECRET", "short");
        assert_eq!(error(&map), "SILICON_IAM_WEBHOOK_SECRET must be 32 to 512 non-whitespace ASCII characters");
        let long = "x".repeat(513);
        map.insert("SILICON_IAM_WEBHOOK_SECRET", &long);
        assert!(from(&map).is_err());
        let plain = "p".repeat(32);
        map.insert("SILICON_IAM_WEBHOOK_SECRET", &plain);
        map.insert("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS", "whs_stubstubstubstubstubstubstubstubstubstubabc");
        let cfg = from(&map).unwrap();
        assert_eq!(cfg.iam_webhook_secret, plain, "no whs_ prefix is required");
        assert!(cfg.iam_webhook_secret_previous.is_some());
        map.insert("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS", "tiny");
        assert_eq!(
            error(&map),
            "SILICON_IAM_WEBHOOK_SECRET_PREVIOUS must be 32 to 512 non-whitespace ASCII characters"
        );
        map.insert("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS", "");
        assert!(from(&map).unwrap().iam_webhook_secret_previous.is_none(), "empty is unset");
    }

    #[test]
    fn the_testing_key_is_optional_and_exactly_the_environment_key_shape() {
        let mut map = full();
        map.insert("SILICON_IAM_TEST_KEY", "");
        assert!(from(&map).unwrap().iam_test_key.is_none());
        map.insert("SILICON_IAM_TEST_KEY", "not-a-key");
        assert_eq!(error(&map), "SILICON_IAM_TEST_KEY must be the 32-character alphanumeric environment key");
        let key = "Ab9".repeat(11).chars().take(32).collect::<String>();
        map.insert("SILICON_IAM_TEST_KEY", &key);
        assert_eq!(from(&map).unwrap().iam_test_key.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn a_dotenv_file_fills_in_only_what_the_environment_leaves_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let text =
            "# backend\n\nSS_ORIGIN=http://from-the-file:3000\n PORT = \"9999\" \nSS_ALLOW_PRIVATE_WEBHOOKS='1'\n";
        std::fs::write(&path, text).unwrap();
        let map = full();
        let env = |name: &str| map.get(name).map(|v| v.to_string());
        let cfg = Config::from_vars(dotenv(&path, env)).unwrap();
        assert_eq!(cfg.origin, "http://localhost:3000", "a real environment variable wins over the file");
        assert_eq!(cfg.bind.port(), 9999, "the file fills in what the environment leaves unset, trimmed and unquoted");
        assert!(cfg.allow_private_webhooks, "in either quoting");
        let missing = dotenv(&dir.path().join("absent"), |_| None);
        assert!(missing("SS_ORIGIN").is_none(), "no file is an empty one, not a failure");
    }
}
