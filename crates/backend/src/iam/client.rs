//! The one IAM client, the official `silicon-iam-client`. One `Client` is built at boot with the
//! Application's Basic credential and, in a testing environment, its `X-Testing-Environment-Key`;
//! `auto_update(false)` ALWAYS, because the crate otherwise runs `cargo update` against this
//! project's manifest at runtime. `system().negotiate()` is the boot handshake and is fail-closed.
//! Everything Space Station asks IAM is here: exchanging a short-lived or refresh token at
//! `app-auth/tokens`, introspecting a token — which since IAM 1.2.0 carries the `authorization`
//! snapshot this server reads its tags and membership from — and revoking one. The crate owns the
//! HTTP: the form-encoded Basic exchange, the `Idempotency-Key`, redirects refused, the mandatory
//! User-Agent. Nothing here reads the directory; an Application may not, so tags arrive on the
//! snapshot or by webhook.

use std::time::Duration;

use axum::http::StatusCode;
use silicon_iam_client::{Credential, EnvironmentKey, Error, IdempotencyKey, Mutation, models};

use crate::config::Config;
use crate::http::ApiError;

pub struct Client {
    inner: silicon_iam_client::Client,
    app_id: String,
}

/// What `app-auth/tokens` answers to an exchange or a refresh.
pub struct Tokens {
    pub oat: String,
    pub ort: String,
    pub expires_in: i64,
    /// The actor's public id: `alice`, `bot:tos`.
    pub actor: String,
    /// The organization the login was bound to; `None` for an unscoped login.
    pub org: Option<String>,
}

/// What introspection says about a token. `authorization` is the synchronous bootstrap snapshot an
/// active org-bound access token carries since IAM 1.2.0; it is `None` for a refresh or unscoped
/// token, or an IAM too old to send it.
#[derive(Debug)]
pub struct Introspection {
    pub active: bool,
    pub org: Option<String>,
    pub membership_id: Option<String>,
    pub authorization: Option<Authorization>,
    pub authorizations: Vec<Authorization>,
}

/// The membership the snapshot proves: who, in which org, at what version, with which tags.
/// `tags: None` is undisclosed (not "no tags"); `Some([])` is no tags.
#[derive(Debug, Clone)]
pub struct Authorization {
    pub public_id: String,
    pub principal_id: String,
    pub org: String,
    /// IAM's uuid for the org — what a webhook envelope names it by.
    pub org_uuid: String,
    pub membership_id: String,
    pub membership_version: i64,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug)]
pub enum IamError {
    /// IAM answered a 4xx: the request or credential is wrong, not IAM.
    Refused { status: u16, code: String, message: String },
    /// Transport, a 5xx, a rate limit, or an answer that is not the contract's: nothing the caller did.
    Unavailable(String),
}

impl IamError {
    /// `400 invalid_grant`: the credential presented is spent for good — a used or expired slt,
    /// or a refresh token whose family IAM has revoked.
    pub fn is_invalid_grant(&self) -> bool {
        matches!(self, IamError::Refused { status: 400, code, .. } if code == "invalid_grant")
    }
}

impl From<IamError> for ApiError {
    /// A refusal is IAM saying no to this Application — a wrong secret, a malformed request — and
    /// so a 502 for the caller, who did nothing; IAM being unreachable is a 503 worth a retry.
    fn from(e: IamError) -> ApiError {
        match e {
            IamError::Refused { status, code, message } => ApiError::new(
                StatusCode::BAD_GATEWAY,
                "iam_refused",
                format!("IAM answered {status} {code}: {message}"),
            ),
            IamError::Unavailable(m) => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "iam_unavailable", m),
        }
    }
}

/// The crate's error, mapped to ours, logging the request id that gets a failure investigated.
fn map_error(what: &str, e: Error) -> IamError {
    let request_id = e.request_id().unwrap_or("none").to_owned();
    match e {
        Error::Api(api) if (400..500).contains(&api.status) => {
            tracing::warn!("IAM refused {what}: {} {} (request {request_id})", api.status, api.code);
            IamError::Refused { status: api.status, code: api.code, message: api.message }
        }
        other => {
            tracing::warn!("IAM unavailable for {what}: {other} (request {request_id})");
            IamError::Unavailable(other.to_string())
        }
    }
}

impl Client {
    /// Builds the client and runs the version handshake; a service that does not select `v1` is
    /// not one this server can talk to, so the boot fails.
    pub async fn connect(cfg: &Config) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
        let mut builder = silicon_iam_client::Client::builder(&cfg.iam_url)?
            .credential(Credential::application(cfg.iam_app_id.clone(), cfg.iam_app_secret.clone()))
            // Never on: the crate otherwise edits this project's Cargo.lock at runtime.
            .auto_update(false)
            .user_agent(concat!("space-station-backend/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10));
        if let Some(key) = &cfg.iam_test_key {
            builder = builder.environment(EnvironmentKey::new(key.clone())?);
        }
        let inner = builder.build()?;
        // The handshake: fail closed if IAM does not agree on v1.
        let negotiated = inner.system().negotiate().await?;
        tracing::info!("IAM negotiated {} (build {})", negotiated.selected_api_version, negotiated.build);
        Ok(Client { inner, app_id: cfg.iam_app_id.clone() })
    }

    /// Spends a short-lived token: single-use, two minutes old at most.
    pub async fn exchange(&self, slt: &str) -> Result<Tokens, IamError> {
        let tokens = self
            .inner
            .oauth()
            .login(&self.app_id, slt, &Mutation::new())
            .await
            .map_err(|e| map_error("the short-lived-token exchange", e))?;
        Ok(tokens_of(tokens))
    }

    /// Rotates a refresh token. The caller owns `idempotency_key` and must present the same one
    /// until this succeeds: a second key for the same `ort_` is a reuse, and reuse revokes the
    /// whole family.
    pub async fn refresh(&self, ort: &str, idempotency_key: &str) -> Result<Tokens, IamError> {
        let key = IdempotencyKey::parse(idempotency_key)
            .map_err(|e| IamError::Unavailable(format!("the persisted refresh key is unusable: {e}")))?;
        let tokens = self
            .inner
            .oauth()
            .refresh(&self.app_id, ort, &Mutation::with_key(key))
            .await
            .map_err(|e| map_error("the token refresh", e))?;
        Ok(tokens_of(tokens))
    }

    /// `{active, org_id, membership_id, authorization?}`, or `{active: false}`. No `X-Org-ID`.
    pub async fn introspect(&self, token: &str) -> Result<Introspection, IamError> {
        let request = models::TokenIntrospectionRequest {
            token: token.to_owned(),
            token_type_hint: Some(models::TokenIntrospectionRequestTokenTypeHint::AccessToken),
        };
        let seen =
            self.inner.oauth().introspect(&request, None).await.map_err(|e| map_error("token introspection", e))?;
        Ok(Introspection {
            active: seen.active,
            org: seen.org_id,
            membership_id: seen.membership_id.map(|id| id.to_string()),
            authorization: seen.authorization.map(authorization_of),
            authorizations: seen.authorizations.unwrap_or_default().into_iter().map(authorization_of).collect(),
        })
    }

    /// Revokes an access token, or a refresh token and its whole family; unknown tokens are 200.
    pub async fn revoke(&self, token: &str) -> Result<(), IamError> {
        let request = models::OAuthRevocationRequest { token: token.to_owned(), token_type_hint: None };
        self.inner.oauth().revoke(&request, &Mutation::new()).await.map_err(|e| map_error("token revocation", e))
    }
}

fn tokens_of(r: models::OAuthTokenResponse) -> Tokens {
    Tokens {
        oat: r.access_token,
        ort: r.refresh_token,
        expires_in: r.expires_in,
        actor: r.actor.public_id,
        org: r.org_id,
    }
}

fn authorization_of(a: models::ApplicationAuthorization) -> Authorization {
    Authorization {
        public_id: a.public_id,
        principal_id: a.principal_id.to_string(),
        org: a.org_id,
        org_uuid: a.organization_id.to_string(),
        membership_id: a.membership_id.to_string(),
        membership_version: a.membership_version,
        tags: a.tags.map(|tags| tags.into_iter().map(|t| t.name).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(status: u16, code: &str) -> IamError {
        IamError::Refused { status, code: code.into(), message: "prose".into() }
    }

    #[test]
    fn only_a_400_invalid_grant_is_a_spent_credential() {
        assert!(refused(400, "invalid_grant").is_invalid_grant());
        assert!(!refused(401, "invalid_grant").is_invalid_grant(), "a 401 is this Application, not the credential");
        assert!(!refused(400, "invalid_request").is_invalid_grant());
        assert!(!IamError::Unavailable("timeout".into()).is_invalid_grant());
    }

    #[test]
    fn a_refusal_is_a_502_and_an_outage_a_503() {
        let e: ApiError = refused(401, "invalid_client").into();
        assert_eq!((e.status, e.code.as_str()), (StatusCode::BAD_GATEWAY, "iam_refused"));
        assert!(e.message.contains("invalid_client") && e.message.contains("prose"));
        let e: ApiError = IamError::Unavailable("IAM answered 503".into()).into();
        assert_eq!((e.status, e.code.as_str()), (StatusCode::SERVICE_UNAVAILABLE, "iam_unavailable"));
    }

    #[test]
    fn the_crates_4xx_is_a_refusal_and_everything_else_is_unavailable() {
        let refused = map_error(
            "x",
            Error::Api(Box::new(silicon_iam_client::ApiError {
                status: 400,
                code: "invalid_grant".into(),
                message: "spent".into(),
                details: None,
                request_id: Some("01a0-req".into()),
            })),
        );
        assert!(refused.is_invalid_grant());
        let server = map_error(
            "x",
            Error::Api(Box::new(silicon_iam_client::ApiError {
                status: 503,
                code: "unavailable".into(),
                message: "down".into(),
                details: None,
                request_id: None,
            })),
        );
        assert!(matches!(server, IamError::Unavailable(_)), "a 5xx is not the caller's fault");
        let invalid = map_error("x", Error::Invalid("bad url".into()));
        assert!(matches!(invalid, IamError::Unavailable(_)));
    }
}
