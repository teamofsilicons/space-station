//! `POST /iam/webhook` (also `/webhooks/api[/]`, the URL registered with IAM): the directory
//! mirror's only writer. A delivery is authenticated by the official client's `WebhookVerifier`
//! against the current and previous signing secret (keyed by the version each delivery declares),
//! and a `test` envelope only by `verify_testing_environment` against this deployment's key — the
//! root key is never read out of the crate. Then, in one transaction: the event id is recorded (a
//! replay is a 204 and nothing else); every full `current.members[]` row lands in `iam_members`
//! unless the mirror already holds a newer version; and every member no longer active — whether a
//! full row with `status: removed` or a bare removal *tombstone* that names only a membership and
//! its principal — has its Space Station credentials in that org revoked. A tombstone is resolved
//! by membership id through the mirror, the sessions and the access tokens, else by principal id
//! inside the org the envelope names. A removal that resolves to no credential we hold, or a row
//! that parses as neither a member nor a tombstone, is logged distinctly: silence about a member is
//! how somebody keeps access they lost.

use std::collections::BTreeSet;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde_json::Value;
use silicon_iam_client::EnvironmentKey;
use silicon_iam_client::webhook::{
    VerifiedWebhook, WebhookError, WebhookSecret, WebhookSecretKeyring, WebhookVerifier,
};
use sqlx::PgConnection;
use uuid::Uuid;

use super::Kind;
use crate::http::{ApiError, AppState};
use crate::{access, notifications};

/// The `X-Silicon-IAM-Key-Version` a delivery carries; the keyring is built for exactly it, so
/// current-then-previous fallback works whatever number IAM uses. Malformed here is caught by the
/// verifier, which re-reads and rejects the header.
const KEY_VERSION_HEADER: &str = "x-silicon-iam-key-version";
const TIMESTAMP_HEADER: &str = "x-silicon-iam-timestamp";

pub fn routes() -> Router<AppState> {
    routes_at("/iam/webhook")
}

/// The same receiver at another path.
pub fn routes_at(path: &str) -> Router<AppState> {
    Router::new().route(path, post(receive))
}

/// A verified delivery, read the same way out of either envelope.
#[derive(Debug)]
pub struct Event {
    pub id: Uuid,
    pub event_type: String,
    /// The org the envelope names, by IAM's uuid: what scopes a tombstone resolved by principal.
    pub org_uuid: Option<String>,
    pub members: Vec<Member>,
    pub tombstones: Vec<Tombstone>,
    /// How many `current.members[]` rows the event carried, parseable or not.
    pub rows: usize,
}

/// One membership as the mirror keeps it: a full `current.members[]` row, or the login snapshot.
/// `tags: None` is undisclosed and leaves the stored tags alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub org: String,
    pub org_uuid: Option<String>,
    pub org_name: Option<String>,
    pub actor: String,
    pub kind: Kind,
    pub membership_id: String,
    pub principal_id: Option<String>,
    pub status: String,
    pub tags: Option<Vec<String>>,
    pub version: i64,
}

/// A removal row IAM projects to before-only recipients: the stable resource and version alone, no
/// principal/organization/membership sub-objects, so the actor is resolved from what we already
/// store about the membership rather than from the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub membership_id: String,
    pub principal_id: Option<String>,
    pub status: String,
    pub version: i64,
}

/// The secrets a delivery is checked against, and the environment key that authenticates a `test`
/// envelope. Borrowed from config per request.
pub struct Secrets<'a> {
    pub current: &'a str,
    pub previous: Option<&'a str>,
    pub test_key: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    StaleTimestamp,
    BadSignature,
    /// A `test` envelope this deployment has no key for, or whose key is not ours.
    BadTestingKey,
    /// Not JSON, or an authenticated body that is not an IAM webhook event.
    BadEnvelope,
}

impl From<Refusal> for ApiError {
    fn from(r: Refusal) -> ApiError {
        match r {
            Refusal::StaleTimestamp => {
                ApiError::unauthorized("stale_timestamp", "X-Silicon-IAM-Timestamp is missing or too far from now")
            }
            Refusal::BadSignature => {
                ApiError::unauthorized("invalid_signature", "X-Silicon-IAM-Signature does not verify")
            }
            Refusal::BadTestingKey => {
                ApiError::unauthorized("invalid_testing_key", "the test envelope is not for this deployment")
            }
            Refusal::BadEnvelope => ApiError::bad_request("invalid_event", "the body is not an IAM webhook event"),
        }
    }
}

/// Authenticates `body` against `headers` and reads the event out of it. The official verifier does
/// the exact-byte HMAC (five-minute clock tolerance, four required headers, event-id cross-check),
/// tried under the current secret then the previous; a `test` envelope is additionally confirmed to
/// name this deployment's environment.
pub fn verify(headers: &HeaderMap, body: &[u8], secrets: &Secrets) -> Result<Event, Refusal> {
    let verified = authenticate(headers, body, secrets)?;
    if verified.is_testing() {
        let key = secrets.test_key.ok_or(Refusal::BadTestingKey)?;
        let expected = EnvironmentKey::new(key).map_err(|_| Refusal::BadTestingKey)?;
        verified.verify_testing_environment(&expected).map_err(|_| Refusal::BadTestingKey)?;
    }
    Ok(read(&verified))
}

/// The signature check, current secret then previous. The key version is whatever this delivery
/// declares: our config knows secrets, not version numbers, so the keyring is built for the
/// declared version and both secrets are tried under it.
fn authenticate(headers: &HeaderMap, body: &[u8], secrets: &Secrets) -> Result<VerifiedWebhook, Refusal> {
    let version = headers
        .get(KEY_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(1);
    match verifier(secrets.current, version).and_then(|v| v.verify(headers, body)) {
        Ok(verified) => Ok(verified),
        Err(WebhookError::InvalidSignature) if secrets.previous.is_some() => {
            let previous = secrets.previous.expect("checked");
            verifier(previous, version).and_then(|v| v.verify(headers, body)).map_err(refusal)
        }
        Err(e) => Err(refusal(e)),
    }
}

fn verifier(secret: &str, version: i64) -> Result<WebhookVerifier, WebhookError> {
    let keyring = WebhookSecretKeyring::new(version, WebhookSecret::new(secret)?)?;
    Ok(WebhookVerifier::new(keyring))
}

/// The crate's verification error, mapped to what the receiver answers.
fn refusal(e: WebhookError) -> Refusal {
    match e {
        WebhookError::TimestampOutsideTolerance => Refusal::StaleTimestamp,
        WebhookError::MissingHeader(h) | WebhookError::DuplicateHeader(h) | WebhookError::InvalidHeader(h)
            if h == TIMESTAMP_HEADER =>
        {
            Refusal::StaleTimestamp
        }
        WebhookError::InvalidPayload | WebhookError::EventIdMismatch => Refusal::BadEnvelope,
        WebhookError::ProductionEvent | WebhookError::TestingEnvironmentMismatch => Refusal::BadTestingKey,
        _ => Refusal::BadSignature,
    }
}

/// The members and tombstones of an authenticated event.
fn read(verified: &VerifiedWebhook) -> Event {
    let event = verified.event();
    let rows = event.data["current"]["members"].as_array().map(Vec::as_slice).unwrap_or_default();
    let (mut members, mut tombstones) = (Vec::new(), Vec::new());
    for row in rows {
        match member(row) {
            Some(m) => members.push(m),
            None => {
                if let Some(t) = tombstone(row) {
                    tombstones.push(t);
                }
            }
        }
    }
    Event {
        id: event.event_id,
        event_type: event.event_type.clone(),
        org_uuid: event.organization_id.map(|id| id.to_string()),
        members,
        tombstones,
        rows: rows.len(),
    }
}

/// A full `current.members[]` row: the actor from `principal.public_id`, the org from
/// `organization.org_id`, and the membership's id, status, version and tag names. A row missing any
/// of those keys is not a member (it may be a tombstone).
fn member(row: &Value) -> Option<Member> {
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let (membership, principal, organization) = (&row["membership"], &row["principal"], &row["organization"]);
    let actor = text(&principal["public_id"])?;
    Some(Member {
        org: text(&organization["org_id"])?,
        org_uuid: text(&organization["id"]),
        org_name: text(&organization["name"]),
        kind: Kind::of(&actor),
        actor,
        membership_id: text(&membership["id"])?,
        principal_id: text(&principal["principal_id"]),
        status: text(&membership["status"])?,
        tags: membership["tags"].as_array().map(|tags| tags.iter().filter_map(|t| text(&t["name"])).collect()),
        version: membership["version"].as_i64()?,
    })
}

/// A removal tombstone: `resource` naming an `organization_membership`, no member sub-objects.
fn tombstone(row: &Value) -> Option<Tombstone> {
    let resource = &row["resource"];
    if resource["type"].as_str() != Some("organization_membership") {
        return None;
    }
    Some(Tombstone {
        membership_id: resource["id"].as_str()?.to_owned(),
        principal_id: resource["principal_id"].as_str().map(str::to_owned),
        status: resource["status"].as_str()?.to_owned(),
        version: resource["version"].as_i64()?,
    })
}

async fn receive(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Result<StatusCode, ApiError> {
    let cfg = &state.cfg;
    let secrets = Secrets {
        current: &cfg.iam_webhook_secret,
        previous: cfg.iam_webhook_secret_previous.as_deref(),
        test_key: cfg.iam_test_key.as_deref(),
    };
    let event = verify(&headers, &body, &secrets)?;
    let mut tx = state.store.pg.begin().await?;
    let recorded = sqlx::query("INSERT INTO iam_events (event_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(event.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if recorded == 0 {
        return Ok(StatusCode::NO_CONTENT);
    }
    let removal = event.event_type.ends_with(".removed.v1");
    let mut touched: BTreeSet<String> = BTreeSet::new();
    let mut removed = 0;
    for m in &event.members {
        upsert(&mut tx, m).await?;
        if removal || m.status != "active" {
            remove_actor(&mut tx, &m.org, &m.actor).await?;
            removed += 1;
        }
        touched.insert(m.org.clone());
    }
    let mut orphans = 0;
    for t in &event.tombstones {
        if removal || t.status != "active" {
            let orgs = remove_tombstone(&mut tx, t, event.org_uuid.as_deref()).await?;
            if orgs.is_empty() {
                orphans += 1;
            }
            removed += orgs.len();
            touched.extend(orgs);
        }
    }
    tx.commit().await?;
    for org in &touched {
        access::forget(&state, org);
    }
    if removed > 0 {
        // Recipients lists changed under the running engine: rebuild its fleet.
        notifications::engine::changed();
    }
    warn_about(&event, orphans);
    Ok(StatusCode::NO_CONTENT)
}

/// Every silence that could mean somebody kept access they lost: a row that parsed as neither a
/// member nor a tombstone, a member-bearing event that named nobody, and — distinctly — a removal
/// that resolved to no credential we hold. Events that never carry members (session, application)
/// are quiet.
fn warn_about(event: &Event, orphans: usize) {
    let readable = event.members.len() + event.tombstones.len();
    let memberless = event.event_type.starts_with("session.") || event.event_type.starts_with("application.");
    if event.rows > readable {
        tracing::warn!(
            "IAM {} (event {}) carried {} rows, {} readable as members or tombstones",
            event.event_type,
            event.id,
            event.rows,
            readable
        );
    } else if event.rows == 0 && !memberless {
        tracing::warn!("IAM {} (event {}) verified but named no member", event.event_type, event.id);
    }
    if orphans > 0 {
        tracing::warn!(
            "IAM {} (event {}) removed {} membership(s) we hold no credential for: nothing to de-provision",
            event.event_type,
            event.id,
            orphans
        );
    }
}

/// The mirror keeps the highest version it has seen of each membership; a row for another
/// membership of the same (org, actor) — a member removed and invited again — always lands. `tags`
/// `None` leaves the stored tags alone (an undisclosed snapshot), `Some` replaces them; `org_name`,
/// `org_uuid` and `principal_id` likewise. Shared by the webhook receiver and the login snapshot.
pub async fn upsert(tx: &mut PgConnection, m: &Member) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO iam_members (org, actor, membership_id, kind, status, tags, org_name, version, principal_id, org_uuid) \
         VALUES ($1, $2, $3, $4, $5, COALESCE($6, '[]'::jsonb), $7, $8, $9, $10) \
         ON CONFLICT (org, actor) DO UPDATE SET membership_id = EXCLUDED.membership_id, kind = EXCLUDED.kind, \
         status = EXCLUDED.status, tags = COALESCE($6, iam_members.tags), version = EXCLUDED.version, updated_at = now(), \
         org_name = COALESCE($7, iam_members.org_name), principal_id = COALESCE($9, iam_members.principal_id), \
         org_uuid = COALESCE($10, iam_members.org_uuid) \
         WHERE iam_members.membership_id <> EXCLUDED.membership_id OR iam_members.version < EXCLUDED.version",
    )
    .bind(&m.org)
    .bind(&m.actor)
    .bind(&m.membership_id)
    .bind(m.kind.as_str())
    .bind(&m.status)
    .bind(m.tags.as_ref().map(|t| Value::from(t.clone())))
    .bind(&m.org_name)
    .bind(m.version)
    .bind(&m.principal_id)
    .bind(&m.org_uuid)
    .execute(&mut *tx)
    .await
    .map(drop)
}

/// Everything that stops working for `actor` in `org` once IAM says they left: access tokens,
/// the sessions bound to that org, the delivery webhook, and their place in every recipients list.
pub async fn remove_actor(tx: &mut PgConnection, org: &str, actor: &str) -> sqlx::Result<()> {
    for sql in [
        "DELETE FROM access_tokens WHERE org = $1 AND actor = $2",
        "DELETE FROM sessions WHERE org = $1 AND actor = $2",
        "DELETE FROM webhooks WHERE org = $1 AND actor = $2",
    ] {
        sqlx::query(sql).bind(org).bind(actor).execute(&mut *tx).await?;
    }
    notifications::forget_recipient(tx, org, &format!("@{actor}")).await.map(drop)
}

/// A removal named only by its ids: mark the mirror row removed (version-ordered) and revoke the
/// credentials of whoever we know holds that membership — from the mirror, a live session, or a
/// `spacewindow-` token that outlived both. When the membership id matches nothing, the principal
/// id inside the org the envelope names is tried, since IAM may know the membership under an id we
/// never saw. Returns the orgs it de-provisioned; empty means the removal changed nothing here.
async fn remove_tombstone(tx: &mut PgConnection, t: &Tombstone, org_uuid: Option<&str>) -> sqlx::Result<Vec<String>> {
    let mut resolved: BTreeSet<(String, String)> = BTreeSet::new();
    let mirrored: Vec<(String, String)> = sqlx::query_as(
        "UPDATE iam_members SET status = 'removed', version = GREATEST(version, $2), updated_at = now() \
         WHERE membership_id = $1 RETURNING org, actor",
    )
    .bind(&t.membership_id)
    .bind(t.version)
    .fetch_all(&mut *tx)
    .await?;
    resolved.extend(mirrored);
    for sql in [
        "SELECT org, actor FROM sessions WHERE membership_id = $1",
        "SELECT org, actor FROM access_tokens WHERE membership_id = $1",
    ] {
        let rows: Vec<(String, String)> = sqlx::query_as(sql).bind(&t.membership_id).fetch_all(&mut *tx).await?;
        resolved.extend(rows);
    }
    if resolved.is_empty()
        && let (Some(principal), Some(org_uuid)) = (&t.principal_id, org_uuid)
    {
        let mirrored: Vec<(String, String)> = sqlx::query_as(
            "UPDATE iam_members SET status = 'removed', updated_at = now() \
             WHERE principal_id = $1 AND org_uuid = $2 RETURNING org, actor",
        )
        .bind(principal)
        .bind(org_uuid)
        .fetch_all(&mut *tx)
        .await?;
        let tokens: Vec<(String, String)> = sqlx::query_as(
            "SELECT org, actor FROM access_tokens WHERE principal_id = $1 \
             AND org IN (SELECT org FROM iam_members WHERE org_uuid = $2)",
        )
        .bind(principal)
        .bind(org_uuid)
        .fetch_all(&mut *tx)
        .await?;
        resolved.extend(mirrored.into_iter().chain(tokens));
        if !resolved.is_empty() {
            tracing::warn!(
                "IAM removed membership {}, which we never saw; resolved by principal {principal} to {resolved:?}",
                t.membership_id
            );
        }
    }
    let orgs: Vec<String> = resolved.iter().map(|(org, _)| org.clone()).collect();
    for (org, actor) in &resolved {
        remove_actor(tx, org, actor).await?;
    }
    Ok(orgs)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    const SECRET: &str = "whs_stubstubstubstubstubstubstubstubstubstubabc";
    const OLD_SECRET: &str = "whs_oldoldoldoldoldoldoldoldoldoldoldoldoldoabc";
    const TEST_KEY: &str = "TkTkTkTkTkTkTkTkTkTkTkTkTkTkTkTk";

    /// One full row as IAM delivers it: `bot:tos` in `tos` with the `ops` tag, membership version 4.
    fn bot_row() -> Value {
        json!({
            "membership": {"id": "01a0702c-6520-77c1-9c5b-2aecd65f88d1", "status": "active", "version": 4,
                "removed_at": null, "tags": [{"id": "01a0702a-75a0-7901-85da-acb13c3b3dda", "name": "ops"}]},
            "organization": {"id": "01a0702a-6f88-79f2-9cc1-aaa7d91388fd", "org_id": "tos", "name": "Team of Silicons",
                "status": "active", "version": 1},
            "principal": {"principal_id": "01a0702c-6520-77c1-9c5b-2ad0df6f4da4", "public_id": "bot:tos",
                "type": "silicon", "status": "active"},
            "resource": {"id": "01a0702c-6520-77c1-9c5b-2aecd65f88d1", "principal_id": "01a0702c-6520-77c1-9c5b-2ad0df6f4da4",
                "principal_type": "silicon", "status": "active", "type": "organization_membership", "version": 4}
        })
    }

    /// The real removal shape captured from IAM: a bare tombstone, no member sub-objects.
    fn tombstone_row() -> Value {
        json!({"authorization": "removed", "resource": {"id": "01a0702c-6520-77c1-9c5b-2aecd65f88d1",
            "principal_id": "01a0702c-6520-77c1-9c5b-2ad0df6f4da4", "principal_type": "silicon", "status": "removed",
            "type": "organization_membership", "version": 5}})
    }

    fn metadata(event_type: &str) -> Value {
        json!({"spec_version": "1.0", "event_id": "01a0703a-97ff-7f91-9947-66a001765e42", "event_type": event_type,
               "occurred_at": "2026-09-05T06:21:23.282422Z", "organization_id": "01a0702a-6f88-79f2-9cc1-aaa7d91388fd",
               "aggregate": {"type": "organization_membership", "id": "01a0702c-6520-77c1-9c5b-2aecd65f88d1", "version": 4}})
    }

    fn production(event_type: &str, members: Value) -> Vec<u8> {
        let mut event = metadata(event_type);
        event["data"] = json!({"changed_fields": ["membership.tags"], "current": {"members": members}});
        event.to_string().into_bytes()
    }

    fn testing(event_type: &str, key: &str, members: Value) -> Vec<u8> {
        json!({"test": {"testing_key": key, "metadata": metadata(event_type),
               "data": {"changed_fields": [], "current": {"members": members}}}})
        .to_string()
        .into_bytes()
    }

    fn headers(secret: &str, ts: i64, body: &[u8]) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-silicon-iam-event-id", "01a0703a-97ff-7f91-9947-66a001765e42".parse().unwrap());
        h.insert("x-silicon-iam-timestamp", ts.to_string().parse().unwrap());
        h.insert("x-silicon-iam-key-version", "1".parse().unwrap());
        h.insert("x-silicon-iam-signature", crate::webhooks::sign(secret, ts, body).parse().unwrap());
        h
    }

    fn secrets(test_key: Option<&'static str>) -> Secrets<'static> {
        Secrets { current: SECRET, previous: Some(OLD_SECRET), test_key }
    }

    fn bot() -> Member {
        Member {
            org: "tos".into(),
            org_uuid: Some("01a0702a-6f88-79f2-9cc1-aaa7d91388fd".into()),
            org_name: Some("Team of Silicons".into()),
            actor: "bot:tos".into(),
            kind: Kind::Silicon,
            membership_id: "01a0702c-6520-77c1-9c5b-2aecd65f88d1".into(),
            principal_id: Some("01a0702c-6520-77c1-9c5b-2ad0df6f4da4".into()),
            status: "active".into(),
            tags: Some(vec!["ops".into()]),
            version: 4,
        }
    }

    #[test]
    fn a_production_delivery_verifies_and_reads_its_member_rows() {
        let now = Utc::now().timestamp();
        let body = production("organization.membership.updated.v1", json!([bot_row()]));
        let event = verify(&headers(SECRET, now, &body), &body, &secrets(None)).unwrap();
        assert_eq!(event.id, Uuid::parse_str("01a0703a-97ff-7f91-9947-66a001765e42").unwrap());
        assert_eq!(event.event_type, "organization.membership.updated.v1");
        assert_eq!(event.org_uuid.as_deref(), Some("01a0702a-6f88-79f2-9cc1-aaa7d91388fd"), "the envelope's org");
        assert_eq!((event.members, event.tombstones.len(), event.rows), (vec![bot()], 0, 1));
    }

    #[test]
    fn a_removal_tombstone_is_read_where_a_full_row_would_be() {
        let now = Utc::now().timestamp();
        let body = production("organization.silicon.removed.v1", json!([tombstone_row()]));
        let event = verify(&headers(SECRET, now, &body), &body, &secrets(None)).unwrap();
        assert!(event.members.is_empty(), "a tombstone is not a member row");
        assert_eq!(event.rows, 1, "but it is a readable row, so nothing is warned about as lost");
        assert_eq!(
            event.tombstones,
            vec![Tombstone {
                membership_id: "01a0702c-6520-77c1-9c5b-2aecd65f88d1".into(),
                principal_id: Some("01a0702c-6520-77c1-9c5b-2ad0df6f4da4".into()),
                status: "removed".into(),
                version: 5,
            }]
        );
    }

    #[test]
    fn a_testing_delivery_needs_this_deployments_key() {
        let now = Utc::now().timestamp();
        let body = testing("organization.membership.updated.v1", TEST_KEY, json!([bot_row()]));
        let event = verify(&headers(SECRET, now, &body), &body, &secrets(Some(TEST_KEY))).unwrap();
        assert_eq!(event.members, vec![bot()], "the same rows, read from inside `test`");
        let refused = |key: Option<&'static str>| verify(&headers(SECRET, now, &body), &body, &secrets(key));
        assert_eq!(refused(None).unwrap_err(), Refusal::BadTestingKey, "no key configured: not our environment");
        assert_eq!(refused(Some("KkKkKkKkKkKkKkKkKkKkKkKkKkKkKkKk")).unwrap_err(), Refusal::BadTestingKey);
    }

    #[test]
    fn a_production_envelope_carries_no_key_to_check_even_in_a_testing_deployment() {
        let now = Utc::now().timestamp();
        let body = production("organization.membership.updated.v1", json!([bot_row()]));
        assert!(verify(&headers(SECRET, now, &body), &body, &secrets(Some(TEST_KEY))).is_ok());
    }

    #[test]
    fn a_tampered_body_or_wrong_secret_does_not_verify_and_the_previous_secret_still_does() {
        let now = Utc::now().timestamp();
        let body = production("organization.membership.updated.v1", json!([bot_row()]));
        let h = headers(SECRET, now, &body);
        let mut tampered = body.clone();
        let last = tampered.len() - 2;
        tampered[last] ^= 1;
        assert_eq!(verify(&h, &tampered, &secrets(None)).unwrap_err(), Refusal::BadSignature);
        let other = headers("whs_otherotherotherotherotherotherotherotherabc", now, &body);
        assert_eq!(verify(&other, &body, &secrets(None)).unwrap_err(), Refusal::BadSignature);
        assert!(verify(&headers(OLD_SECRET, now, &body), &body, &secrets(None)).is_ok(), "rotation drains");
        let only_current = Secrets { current: SECRET, previous: None, test_key: None };
        assert_eq!(verify(&headers(OLD_SECRET, now, &body), &body, &only_current).unwrap_err(), Refusal::BadSignature);
    }

    #[test]
    fn a_stale_timestamp_is_refused() {
        let now = Utc::now().timestamp();
        let body = production("organization.membership.updated.v1", json!([bot_row()]));
        assert_eq!(
            verify(&headers(SECRET, now - 600, &body), &body, &secrets(None)).unwrap_err(),
            Refusal::StaleTimestamp
        );
    }

    #[test]
    fn a_verified_body_that_is_not_an_event_is_a_bad_envelope() {
        let now = Utc::now().timestamp();
        let body = b"{\"hello\": \"world\"}".to_vec();
        // Correctly signed, so the failure is the envelope shape and not the signature.
        assert_eq!(verify(&headers(SECRET, now, &body), &body, &secrets(None)).unwrap_err(), Refusal::BadEnvelope);
    }

    #[test]
    fn a_member_row_and_a_tombstone_are_told_apart() {
        let mut carbon = bot_row();
        carbon["principal"]["public_id"] = json!("alice");
        carbon["membership"]["tags"] = json!([]);
        let m = member(&carbon).unwrap();
        assert_eq!((m.kind, m.actor.as_str(), m.tags), (Kind::Carbon, "alice", Some(vec![])), "the kind is the colon");
        carbon["membership"].as_object_mut().unwrap().remove("tags");
        assert_eq!(member(&carbon).unwrap().tags, None, "a row without tags discloses none, it does not clear them");
        assert!(tombstone(&bot_row()).is_some(), "a full row also carries a resource, but member() wins first");
        for missing in ["principal", "organization", "membership"] {
            let mut row = bot_row();
            row.as_object_mut().unwrap().remove(missing);
            assert!(member(&row).is_none(), "without {missing} the row is not a full member");
        }
        assert!(member(&tombstone_row()).is_none() && tombstone(&tombstone_row()).is_some());
    }
}
