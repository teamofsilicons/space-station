//! Webhook settings. Org webhooks are notification recipients (`webhook:{id}`), and deleting one
//! takes it out of every recipients list in the same transaction; a silicon's own delivery webhook
//! is where `@silicon` recipients land, set with PUT and dropped with DELETE. Secrets are sealed
//! and shown once. `validate_url` and `sign` are what the notification engine uses on every send.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use chrono::{DateTime, Utc};
use reqwest::Client;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use space_station_shared::secrets::webhook_secret;
use url::{Host, Url};
use uuid::Uuid;

use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json, Path};
use crate::iam::{Identity, Kind};
use crate::{crypto, notifications};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/webhooks", get(list).post(create))
        .route("/orgs/{org}/webhooks/{id}", delete(remove))
        .route("/orgs/{org}/silicon-webhook", put(silicon).delete(silicon_remove))
}

#[derive(sqlx::FromRow, Serialize)]
struct Webhook {
    id: Uuid,
    url: String,
    created_by: String,
    created_at: DateTime<Utc>,
}

async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<Webhook>>, ApiError> {
    auth.actor()?;
    let sql =
        "SELECT id, url, created_by, created_at FROM webhooks WHERE org = $1 AND actor IS NULL ORDER BY created_at";
    Ok(Json(sqlx::query_as(sql).bind(auth.org()).fetch_all(&state.store.pg).await?))
}

#[derive(Deserialize)]
struct Body {
    url: String,
}

async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<Body>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let me = auth.actor()?;
    let url = validate_url(&body.url, state.cfg.allow_private_webhooks).await?.url;
    let (id, secret) = (Uuid::new_v4(), webhook_secret());
    sqlx::query("INSERT INTO webhooks (id, org, url, secret_enc, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(&me.org)
        .bind(&url)
        .bind(crypto::seal(&state.cfg.key, &secret))
        .bind(&me.id)
        .execute(&state.store.pg)
        .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "url": url, "secret": secret}))))
}

/// Deleting a webhook also strikes `webhook:{id}` from every notification's recipients in the org,
/// in the same transaction, so no list goes on naming a target that no longer exists.
async fn remove(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    auth.actor()?;
    let mut tx = state.store.pg.begin().await?;
    let sql = "DELETE FROM webhooks WHERE org = $1 AND id = $2 AND actor IS NULL";
    let deleted = sqlx::query(sql).bind(auth.org()).bind(id).execute(&mut *tx).await?.rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found("webhook"));
    }
    let pruned = notifications::forget_recipient(&mut tx, auth.org(), &format!("webhook:{id}")).await?;
    tx.commit().await?;
    if pruned > 0 {
        notifications::engine::changed();
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Only a silicon has a delivery webhook.
fn silicon_only(me: &Identity) -> Result<(), ApiError> {
    match me.kind {
        Kind::Silicon => Ok(()),
        Kind::Carbon => {
            Err(ApiError::new(StatusCode::FORBIDDEN, "silicons_only", "only a silicon has a delivery webhook"))
        }
    }
}

/// The calling silicon's delivery webhook; every PUT replaces the URL and mints a new secret.
async fn silicon(State(state): State<AppState>, auth: Auth, Json(body): Json<Body>) -> Result<Json<Value>, ApiError> {
    let me = auth.actor()?;
    silicon_only(me)?;
    let url = validate_url(&body.url, state.cfg.allow_private_webhooks).await?.url;
    let secret = webhook_secret();
    sqlx::query(
        "INSERT INTO webhooks (id, org, url, secret_enc, actor, created_by) VALUES ($1, $2, $3, $4, $5, $5) \
         ON CONFLICT (org, actor) WHERE actor IS NOT NULL DO UPDATE SET url = EXCLUDED.url, secret_enc = EXCLUDED.secret_enc, \
         created_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(&me.org)
    .bind(&url)
    .bind(crypto::seal(&state.cfg.key, &secret))
    .bind(&me.id)
    .execute(&state.store.pg)
    .await?;
    Ok(Json(json!({"url": url, "secret": secret})))
}

/// The calling silicon drops its delivery webhook; its subscriptions stay, and deliver again the
/// moment it sets a new one. Idempotent: nothing to drop is still a 204.
async fn silicon_remove(State(state): State<AppState>, auth: Auth) -> Result<StatusCode, ApiError> {
    let me = auth.actor()?;
    silicon_only(me)?;
    sqlx::query("DELETE FROM webhooks WHERE org = $1 AND actor = $2")
        .bind(&me.org)
        .bind(&me.id)
        .execute(&state.store.pg)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A URL that passed `validate_url`, with the addresses its host resolved to while it did.
pub struct Vetted {
    pub url: String,
    host: String,
    addrs: Vec<SocketAddr>,
}

impl Vetted {
    /// The client for one attempt: no redirects, 10 s, and pinned to the addresses that were
    /// vetted, so a second DNS answer cannot send the request somewhere the first one hid.
    /// Nothing is pinned when the check was skipped or the host is already an address.
    pub fn client(&self) -> Client {
        let builder = Client::builder().redirect(Policy::none()).timeout(Duration::from_secs(10));
        let builder = if self.addrs.is_empty() { builder } else { builder.resolve_to_addrs(&self.host, &self.addrs) };
        builder.build().expect("a plain reqwest client builds")
    }
}

/// A URL we may call: https, no userinfo, and a host that resolves to nothing loopback, private,
/// link-local, ULA or otherwise local. `SS_ALLOW_PRIVATE_WEBHOOKS` relaxes exactly the address
/// rule, for local development: a private address is then allowed, and plain http is allowed to
/// a private address only — an `http://` URL to a public host is refused as https-only whatever
/// the flag says. Checked on write and again on every send, and `Vetted::client` is what makes
/// the send land on what was checked.
pub async fn validate_url(url: &str, allow_private: bool) -> Result<Vetted, ApiError> {
    let bad = |m: String| ApiError::bad_request("invalid_url", m);
    let parsed = Url::parse(url).map_err(|e| bad(e.to_string()))?;
    let https_only = || bad(format!("{}: webhooks are https only", parsed.scheme()));
    match parsed.scheme() {
        "https" => {}
        "http" if allow_private => {}
        _ => return Err(https_only()),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad("credentials in the URL are not allowed".into()));
    }
    let host = parsed.host().ok_or_else(|| bad("no host".into()))?;
    let mut addrs = Vec::new();
    let ips: Vec<IpAddr> = match host {
        Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
        Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        Host::Domain(name) => {
            let port = parsed.port_or_known_default().unwrap_or(443);
            addrs = tokio::net::lookup_host((name, port))
                .await
                .map_err(|_| bad(format!("{name} does not resolve")))?
                .collect();
            addrs.iter().map(SocketAddr::ip).collect()
        }
    };
    let private = ips.iter().find(|ip| is_private(**ip));
    match (parsed.scheme(), private, allow_private) {
        (_, Some(ip), false) => {
            return Err(bad(format!("{} resolves to {ip}, a private address", parsed.host_str().unwrap_or_default())));
        }
        ("http", None, _) => return Err(https_only()),
        _ => {}
    }
    Ok(Vetted { url: parsed.to_string(), host: parsed.host_str().unwrap_or_default().to_owned(), addrs })
}

/// Loopback, RFC 1918, CGNAT, link-local (with the cloud metadata address), ULA, unspecified,
/// multicast, broadcast, and IPv4-mapped forms of those.
fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
                || v6.to_ipv4_mapped().is_some_and(|v4| is_private(IpAddr::V4(v4)))
        }
    }
}

/// `v1=<hex hmac_sha256(secret, "{ts}.{body}")>`, the signature on every outgoing webhook.
pub fn sign(secret: &str, ts: i64, body: &[u8]) -> String {
    let mut message = ts.to_string().into_bytes();
    message.push(b'.');
    message.extend_from_slice(body);
    format!("v1={}", crypto::hmac_sha256_hex(secret.as_bytes(), &message))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn code(url: &str, allow_private: bool) -> Result<String, String> {
        validate_url(url, allow_private).await.map(|v| v.url).map_err(|e| e.message)
    }

    #[tokio::test]
    async fn private_hosts_userinfo_and_plain_http_are_refused_unless_allowed() {
        for url in [
            "https://127.0.0.1/hook",
            "https://10.1.2.3/hook",
            "https://172.16.5.5/hook",
            "https://192.168.1.1/hook",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/hook",
            "https://[::1]/hook",
            "https://[fc00::1]/hook",
            "https://[fe80::1]/hook",
            "https://[::ffff:10.0.0.1]/hook",
            "https://localhost/hook",
        ] {
            assert!(code(url, false).await.is_err(), "{url} must be refused");
            assert!(code(url, true).await.is_ok(), "{url} is fine for local development");
        }
        assert_eq!(
            code("https://user:pw@example.com/hook", false).await.unwrap_err(),
            "credentials in the URL are not allowed"
        );
        assert_eq!(code("http://93.184.216.34/hook", false).await.unwrap_err(), "http: webhooks are https only");
        assert!(code("http://127.0.0.1:9/hook", true).await.is_ok());
        assert!(code("http://localhost:4747/hook", true).await.is_ok(), "plain http reaches a private address");
        assert!(code("ftp://example.com/x", true).await.is_err());
        assert_eq!(code("https://93.184.216.34/hook?x=1", false).await.unwrap(), "https://93.184.216.34/hook?x=1");
    }

    /// The flag relaxes the address rule and nothing else: plain http to a public host is refused
    /// as https-only whether or not private webhooks are allowed.
    #[tokio::test]
    async fn allowing_private_webhooks_never_allows_plain_http_to_a_public_host() {
        for url in ["http://93.184.216.34/hook", "http://[2606:2800:220:1:248:1893:25c8:1946]/hook"] {
            assert_eq!(code(url, true).await.unwrap_err(), "http: webhooks are https only", "{url}");
            assert_eq!(code(url, false).await.unwrap_err(), "http: webhooks are https only", "{url}");
        }
        assert!(code("https://93.184.216.34/hook", true).await.is_ok(), "https to a public host is always fine");
    }

    /// The connect must land on the address the check vetted, not on whatever DNS answers a
    /// moment later. `.invalid` never resolves, so only the pin can reach the listener.
    #[tokio::test]
    async fn a_vetted_url_is_dialled_at_the_address_that_was_checked() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dialled = tokio::spawn(async move { listener.accept().await.is_ok() });
        let vetted = |addrs: Vec<SocketAddr>| Vetted {
            url: "http://pinned.invalid/hook".into(),
            host: "pinned.invalid".into(),
            addrs,
        };
        let pinned = vetted(vec![addr]);
        let _ = pinned.client().post(&pinned.url).body("{}").send().await;
        let dialled = tokio::time::timeout(Duration::from_secs(5), dialled).await;
        assert!(dialled.is_ok_and(|r| r.unwrap()), "the address that was checked is the one dialled");
        let loose = vetted(Vec::new());
        assert!(loose.client().post(&loose.url).send().await.is_err(), "with nothing pinned the name is resolved");
        assert!(
            validate_url("https://93.184.216.34/hook", false).await.unwrap().addrs.is_empty(),
            "an ip needs no pin"
        );
        assert!(
            !validate_url("http://localhost:1/x", true).await.unwrap().addrs.is_empty(),
            "a name is resolved, and pinned, even when private addresses are allowed"
        );
        if let Ok(vetted) = validate_url("https://example.com/hook", false).await {
            assert!(!vetted.addrs.is_empty(), "a domain keeps the addresses its check resolved");
        }
    }

    #[test]
    fn signatures_are_v1_hmac_over_timestamp_dot_body() {
        let body = br#"{"dedup_key":"k","text":"t","metadata":{}}"#;
        let mut message = b"1700000000.".to_vec();
        message.extend_from_slice(body);
        assert_eq!(
            sign("whsec-abc", 1_700_000_000, body),
            format!("v1={}", crypto::hmac_sha256_hex(b"whsec-abc", &message))
        );
        assert!(sign("whsec-abc", 1, body).starts_with("v1=") && sign("whsec-abc", 1, body).len() == 67);
    }
}
