//! Everything the server takes from the environment, read once at boot into `Config`, with a
//! `.env` in the working directory underneath it. One clear error names the first variable that
//! is missing or malformed; nothing else reads `env`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use url::Url;

const DEFAULT_TELEMETRY_URL: &str = "http://127.0.0.1:8080";

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
    pub accounts_url: String,
    /// Browser-facing Silicon Accounts login endpoint; the API client still uses `accounts_url`.
    pub accounts_auth_url: String,
    /// The globally unique bare Application id, `spacestation`.
    pub accounts_app_id: String,
    pub accounts_app_secret: String,
    /// What Silicon Accounts signs webhook deliveries with: caller-chosen, 32 to 512 characters.
    pub accounts_webhook_secret: String,
    /// The secret before the last rotation, accepted while Silicon Accounts drains deliveries signed with it.
    pub accounts_webhook_secret_previous: Option<String>,
    pub allow_private_webhooks: bool,
    /// Ordinary `tos.spacestation` table key for backend self-telemetry. Missing disables it.
    pub telemetry_key: Option<String>,
    /// Optional browser-facing tables. These are written through the same-origin frontend
    /// collector; the keys never leave the backend.
    pub frontend_analytics_key: Option<String>,
    pub frontend_events_key: Option<String>,
    pub telemetry_home: PathBuf,
    pub telemetry_url: String,
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
        let accounts_app_id = need("SILICON_ACCOUNTS_APP_ID")?;
        if !canonical_app_id(&accounts_app_id) {
            return Err("SILICON_ACCOUNTS_APP_ID must be the bare Application handle (for example spacestation)".into());
        }
        // Validate webhook secrets once at boot,
        // so a misconfigured secret fails here at boot rather than on every webhook delivery.
        let webhook_secret = |name: &str, value: &str| match (32..=512).contains(&value.len())
            && value.bytes().all(|b| b.is_ascii_graphic())
        {
            true => Ok(value.to_owned()),
            false => Err(format!("{name} must be 32 to 512 non-whitespace ASCII characters")),
        };
        let accounts_webhook_secret =
            webhook_secret("SILICON_ACCOUNTS_WEBHOOK_SECRET", &need("SILICON_ACCOUNTS_WEBHOOK_SECRET")?)?;
        let accounts_webhook_secret_previous = optional("SILICON_ACCOUNTS_WEBHOOK_SECRET_PREVIOUS")
            .map(|v| webhook_secret("SILICON_ACCOUNTS_WEBHOOK_SECRET_PREVIOUS", &v))
            .transpose()?;
        let accounts_url =
            optional("SILICON_ACCOUNTS_URL").unwrap_or_else(|| "https://accounts.teamofsilicons.com".into());
        let accounts_auth_url = optional("SILICON_ACCOUNTS_AUTH_URL")
            .unwrap_or_else(|| format!("{}/authorize", accounts_url.trim_end_matches('/')));
        let telemetry_key = if var("SPACE_STATION_TELEMETRY")
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        {
            None
        } else {
            optional("SPACE_STATION_TELEMETRY_KEY")
        };
        let frontend_analytics_key = if var("SPACE_STATION_TELEMETRY")
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        {
            None
        } else {
            optional("SPACE_STATION_FRONTEND_ANALYTICS_KEY")
        };
        let frontend_events_key = if var("SPACE_STATION_TELEMETRY")
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        {
            None
        } else {
            optional("SPACE_STATION_FRONTEND_EVENTS_KEY")
        };
        let telemetry_home = optional("SILICON_HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".space-station"))
            .or_else(|| optional("SPACE_STATION_HOME").map(PathBuf::from))
            .or_else(|| optional("HOME").map(|home| PathBuf::from(home).join(".space-station")))
            .unwrap_or_else(|| PathBuf::from(".space-station"));
        let telemetry_url = optional("SPACE_STATION_TELEMETRY_URL")
            .or_else(|| optional("SPACE_STATION_URL"))
            .unwrap_or_else(|| DEFAULT_TELEMETRY_URL.to_owned());
        Ok(Config {
            bind: SocketAddr::from(([0, 0, 0, 0], port)),
            origin: origin.origin().ascii_serialization(),
            cookie_domain,
            key: bytes,
            clickhouse_url,
            clickhouse_query_password: need("CLICKHOUSE_QUERY_PASSWORD")?,
            database_url: need("DATABASE_URL")?,
            redis_url: need("REDIS_URL")?,
            accounts_url: accounts_url.trim_end_matches('/').to_owned(),
            accounts_auth_url,
            accounts_app_id,
            accounts_app_secret: need("SILICON_ACCOUNTS_APP_SECRET")?,
            accounts_webhook_secret,
            accounts_webhook_secret_previous,
            allow_private_webhooks: var("SS_ALLOW_PRIVATE_WEBHOOKS")
                .is_some_and(|v| !matches!(v.as_str(), "" | "0" | "false")),
            telemetry_key,
            frontend_analytics_key,
            frontend_events_key,
            telemetry_home,
            telemetry_url,
        })
    }
}

/// Silicon Apps application handle.
fn canonical_app_id(id: &str) -> bool {
    let word = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-';
    (1..=80).contains(&id.len()) && id.as_bytes()[0].is_ascii_lowercase() && id.bytes().all(word)
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
