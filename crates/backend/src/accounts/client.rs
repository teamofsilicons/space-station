//! Silicon Accounts OAuth endpoints; app credentials stay on the server.
use super::Kind;
use crate::{config::Config, http::ApiError};
use axum::http::StatusCode;
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

pub struct Client {
    http: reqwest::Client,
    url: String,
    app: String,
    secret: String,
    pub world: String,
}
#[derive(Deserialize)]
pub struct Account {
    pub uuid: Uuid,
    pub id: String,
    pub kind: Kind,
}
pub struct Tokens {
    pub oat: String,
    pub ort: String,
    pub account: Account,
    pub access_expires_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
#[derive(Deserialize)]
pub struct Introspection {
    pub active: bool,
    pub sub: Option<Uuid>,
    pub id: Option<String>,
    pub kind: Option<Kind>,
    pub exp: Option<i64>,
    pub aud: Option<String>,
    pub client_id: Option<String>,
}
#[derive(Debug)]
pub enum AccountsError {
    Refused { code: String },
    Unavailable(String),
}
impl AccountsError {
    pub fn is_invalid_grant(&self) -> bool {
        matches!(self, Self::Refused {code} if code == "invalid_grant")
    }
}
impl From<AccountsError> for ApiError {
    fn from(e: AccountsError) -> Self {
        match e {
            AccountsError::Refused { code } => ApiError::new(
                StatusCode::BAD_GATEWAY,
                "accounts_refused",
                format!("Silicon Accounts refused the request ({code})"),
            ),
            AccountsError::Unavailable(message) => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "accounts_unavailable", message)
            }
        }
    }
}
impl Client {
    pub async fn connect(cfg: &Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("space-station-backend/", env!("CARGO_PKG_VERSION")))
                .build()?,
            url: cfg.accounts_url.clone(),
            app: cfg.accounts_app_id.clone(),
            secret: cfg.accounts_app_secret.clone(),
            world: space_station_shared::secrets::sha256_hex(&format!("{}:{}", cfg.accounts_url, cfg.accounts_app_id)),
        })
    }
    async fn request(&self, path: &str, form: &[(&str, &str)]) -> Result<Value, AccountsError> {
        let response = self
            .http
            .post(format!("{}{path}", self.url))
            .basic_auth(&self.app, Some(&self.secret))
            .form(form)
            .send()
            .await
            .map_err(|_| AccountsError::Unavailable("Silicon Accounts could not be reached; retry shortly".into()))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|_| AccountsError::Unavailable("Silicon Accounts returned an unreadable response".into()))?;
        if !status.is_success() {
            if status.is_server_error() || status.as_u16() == 429 {
                return Err(AccountsError::Unavailable("Silicon Accounts is temporarily unavailable".into()));
            }
            return Err(AccountsError::Refused {
                code: body["error"].as_str().or(body["error"]["code"].as_str()).unwrap_or("unknown_error").into(),
            });
        }
        Ok(body)
    }
    async fn token(&self, form: &[(&str, &str)]) -> Result<Tokens, AccountsError> {
        #[derive(Deserialize)]
        struct Reply {
            access_token: String,
            refresh_token: String,
            token_type: String,
            expires_in: i64,
            refresh_token_expires_at: DateTime<Utc>,
            account: Account,
        }
        let r: Reply = serde_json::from_value(self.request("/v1/oauth/token", form).await?)
            .map_err(|_| AccountsError::Unavailable("Silicon Accounts returned an invalid token response".into()))?;
        if r.token_type != "Bearer"
            || r.access_token.is_empty()
            || r.refresh_token.is_empty()
            || r.expires_in <= 0
            || r.refresh_token_expires_at <= Utc::now()
            || r.account.uuid.is_nil()
            || Kind::of(&r.account.id) != Some(r.account.kind)
        {
            return Err(AccountsError::Unavailable("Silicon Accounts returned an inconsistent account".into()));
        }
        Ok(Tokens {
            oat: r.access_token,
            ort: r.refresh_token,
            account: r.account,
            access_expires_at: Utc::now() + TimeDelta::seconds(r.expires_in.min(1800)),
            expires_at: r.refresh_token_expires_at,
        })
    }
    pub async fn exchange(&self, slt: &str) -> Result<Tokens, AccountsError> {
        self.token(&[("grant_type", "urn:silicon:params:oauth:grant-type:slt"), ("slt", slt)]).await
    }
    pub async fn code(&self, code: &str, redirect: &str, verifier: &str) -> Result<Tokens, AccountsError> {
        self.token(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
        ])
        .await
    }
    pub async fn refresh(&self, token: &str) -> Result<Tokens, AccountsError> {
        self.token(&[("grant_type", "refresh_token"), ("refresh_token", token)]).await
    }
    pub async fn introspect(&self, token: &str) -> Result<Introspection, AccountsError> {
        let proof: Introspection =
            serde_json::from_value(self.request("/v1/oauth/introspect", &[("token", token)]).await?)
                .map_err(|_| AccountsError::Unavailable("Silicon Accounts returned an invalid introspection".into()))?;
        if proof.active
            && (proof.aud.as_deref() != Some(&self.app)
                || proof.client_id.as_deref() != Some(&self.app)
                || proof.sub.is_none()
                || proof.exp.is_none_or(|e| e <= Utc::now().timestamp()))
        {
            return Err(AccountsError::Unavailable("Silicon Accounts returned a mismatched token audience".into()));
        }
        Ok(proof)
    }
    pub async fn revoke(&self, token: &str) -> Result<(), AccountsError> {
        self.request("/v1/oauth/revoke", &[("token", token)]).await.map(|_| ())
    }
}
