//! Notifications: an always-on subscription that ends in a message instead of a view. This file
//! owns the shape — the definition, its versions, the subscription set — and the routes the app,
//! the CLI and the dev server call. `engine` fires them under the lease, `deliver` takes one
//! event to its recipients, `cron` reads a `{schedule}` trigger. A definition is checked once,
//! when it is saved, with the saver's identity — down to asking ClickHouse whether its sql can
//! yield `{dedup_key, text, metadata}` at all; the engine re-resolves nobody at run time.

pub mod cron;
pub mod deliver;
pub mod engine;

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

pub use engine::Engine;

use crate::access::{self, Entry};
use crate::http::auth::{Auth, Principal};
use crate::http::{ApiError, AppState, Json, Path};
use crate::iam::{Identity, Kind};
use crate::store::ChError;
use crate::{query, sql};
use cron::Cron;

/// The columns every row of a notification's sql must carry.
const SHAPE: [&str; 3] = ["dedup_key", "text", "metadata"];

/// `delay` never waits longer than an hour, `cooldown` never silences for more than a month.
const DELAY_MAX: u64 = 3_600_000;
const COOLDOWN_MAX: u64 = 30 * 86_400_000;

/// A notification definition: the JSON of `UNDERSTANDING.md` minus `recipients`, which is a
/// subscription set and not versioned. Stored in `notification_versions.def` with its defaults
/// filled in, so a stored version says exactly what it does.
#[derive(Serialize, Deserialize, Clone)]
pub struct Def {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub triggers: Vec<Trigger>,
    pub sql: String,
    #[serde(default = "default_delay")]
    pub delay: String,
    #[serde(default = "default_cooldown")]
    pub cooldown: String,
    #[serde(default)]
    pub access: Vec<String>,
}

/// A row landing in a table, or the clock.
#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum Trigger {
    Table {
        table: String,
        #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
        where_: Option<String>,
    },
    Schedule {
        schedule: String,
    },
}

fn yes() -> bool {
    true
}

fn default_delay() -> String {
    "2s".into()
}

fn default_cooldown() -> String {
    "10m".into()
}

impl Def {
    /// The `(table, where)` of every table trigger.
    pub fn tables(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.triggers.iter().filter_map(|t| match t {
            Trigger::Table { table, where_ } => Some((table.as_str(), where_.as_deref())),
            Trigger::Schedule { .. } => None,
        })
    }

    pub fn schedules(&self) -> impl Iterator<Item = &str> {
        self.triggers.iter().filter_map(|t| match t {
            Trigger::Schedule { schedule } => Some(schedule.as_str()),
            Trigger::Table { .. } => None,
        })
    }
}

/// `^[0-9]+(ms|s|m|h|d)$` as milliseconds; `None` for anything else, overflow included.
pub fn duration_ms(text: &str) -> Option<u64> {
    let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let scale = match &text[digits.len()..] {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/notifications", get(list).post(create))
        .route("/orgs/{org}/notifications/{id}", get(show).put(update).delete(remove))
        .route("/orgs/{org}/notifications/{id}/events", get(events))
        .route("/orgs/{org}/notifications/{id}/subscribe", post(subscribe).delete(unsubscribe))
        .route("/orgs/{org}/notifications/{id}/test", post(test))
}

/// The org and the notification a path names; the org is already the `Auth`'s.
type Id = Path<(String, Uuid)>;

/// One notification as every client reads it: the mutable row plus its current definition.
#[derive(sqlx::FromRow, Serialize)]
struct Row {
    id: Uuid,
    def: Value,
    recipients: Value,
    enabled: bool,
    created_by: String,
    created_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT n.id, v.def, n.recipients, n.enabled, n.created_by, n.created_at FROM notifications n \
                      JOIN notification_versions v ON v.id = n.current_version";

impl Row {
    fn access_list(&self) -> Vec<String> {
        access::list(&self.def["access"])
    }

    fn recipients_list(&self) -> Vec<String> {
        access::list(&self.recipients)
    }

    fn def(&self) -> Result<Def, ApiError> {
        serde_json::from_value(self.def.clone()).map_err(|e| ApiError::internal("bad_definition", e))
    }
}

/// An actor the access list names, or an API key holding `notifications`, which reads the org.
fn readable(auth: &Auth, row: &Row) -> bool {
    match &auth.0 {
        Principal::Actor(me) => access::matches(me, &row.access_list()),
        Principal::Org { .. } => true,
    }
}

async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<Row>>, ApiError> {
    auth.allow("notifications")?;
    let sql = format!("{SELECT} WHERE n.org = $1 ORDER BY n.created_at");
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(auth.org()).fetch_all(&state.store.pg).await?;
    Ok(Json(rows.into_iter().filter(|row| readable(&auth, row)).collect()))
}

/// The notification, if this caller may read it.
async fn one(state: &AppState, auth: &Auth, id: Uuid) -> Result<Row, ApiError> {
    auth.allow("notifications")?;
    let sql = format!("{SELECT} WHERE n.org = $1 AND n.id = $2");
    let row: Option<Row> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(auth.org()).bind(id).fetch_optional(&state.store.pg).await?;
    let row = row.ok_or_else(|| ApiError::not_found("notification"))?;
    if readable(auth, &row) { Ok(row) } else { Err(ApiError::forbidden()) }
}

/// The notification, if this caller may change it: `one` is already that test for an actor, and
/// an API key is not one.
async fn mine(state: &AppState, auth: &Auth, id: Uuid) -> Result<Row, ApiError> {
    auth.actor()?;
    one(state, auth, id).await
}

async fn show(State(state): State<AppState>, auth: Auth, Path((_, id)): Id) -> Result<Json<Row>, ApiError> {
    Ok(Json(one(&state, &auth, id).await?))
}

/// `{def, recipients?}`: on a PUT an absent `recipients` keeps the current subscribers.
#[derive(Deserialize)]
struct Body {
    def: Def,
    recipients: Option<Vec<String>>,
}

async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<Body>,
) -> Result<(StatusCode, Json<Row>), ApiError> {
    let me = auth.actor()?;
    let recipients = body.recipients.unwrap_or_default();
    let def = validate(&state, me, body.def, &recipients, &[]).await?;
    let id = Uuid::new_v4();
    let mut tx = state.store.pg.begin().await?;
    sqlx::query("INSERT INTO notifications (id, org, created_by, cursors) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(&me.org)
        .bind(&me.id)
        .bind(watermarks(&state, &me.org, &def).await?)
        .execute(&mut *tx)
        .await?;
    save(&mut tx, id, &def, recipients, &me.id, None).await?;
    tx.commit().await?;
    engine::changed();
    Ok((StatusCode::CREATED, Json(one(&state, &auth, id).await?)))
}

/// Editing writes a new version and points `current_version` at it; enabling one again restarts
/// it from the current watermarks.
async fn update(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Id,
    Json(body): Json<Body>,
) -> Result<Json<Row>, ApiError> {
    let row = mine(&state, &auth, id).await?;
    let me = auth.actor()?;
    let current = row.recipients_list();
    let recipients = body.recipients.unwrap_or_else(|| current.clone());
    let def = validate(&state, me, body.def, &recipients, &current).await?;
    let restart = match def.enabled && !row.enabled {
        true => Some(watermarks(&state, &me.org, &def).await?),
        false => None,
    };
    let mut tx = state.store.pg.begin().await?;
    save(&mut tx, id, &def, recipients, &me.id, restart).await?;
    tx.commit().await?;
    engine::changed();
    Ok(Json(one(&state, &auth, id).await?))
}

/// A notification takes its versions and its whole event history with it, by `ON DELETE CASCADE`.
/// The `current_version` pointer into `notification_versions` needs no separate step: it is a
/// `NO ACTION` foreign key, checked at the end of the statement, by which time the row that held
/// the pointer is gone as well. `engine::changed()` retires its worker.
async fn remove(State(state): State<AppState>, auth: Auth, Path((_, id)): Id) -> Result<StatusCode, ApiError> {
    mine(&state, &auth, id).await?;
    sqlx::query("DELETE FROM notifications WHERE id = $1").bind(id).execute(&state.store.pg).await?;
    engine::changed();
    Ok(StatusCode::NO_CONTENT)
}

/// A new version, current the moment it lands, plus the parts of the row that are not versioned.
/// `restart` sets the cursors; `None` leaves them where they were.
async fn save(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    def: &Def,
    recipients: Vec<String>,
    by: &str,
    restart: Option<Value>,
) -> Result<(), ApiError> {
    let version = Uuid::new_v4();
    sqlx::query("INSERT INTO notification_versions (id, notification, def, created_by) VALUES ($1, $2, $3, $4)")
        .bind(version)
        .bind(id)
        .bind(json!(def))
        .bind(by)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE notifications SET enabled = $2, recipients = $3, current_version = $4, \
         cursors = COALESCE($5, cursors) WHERE id = $1",
    )
    .bind(id)
    .bind(def.enabled)
    .bind(Value::from(recipients))
    .bind(version)
    .bind(restart)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Where a fresh, or newly re-enabled, notification starts: the watermark of every table it
/// triggers on, so it never fires on history.
async fn watermarks(state: &AppState, org: &str, def: &Def) -> Result<Value, ApiError> {
    let mut cursors = serde_json::Map::new();
    for (table, _) in def.tables() {
        cursors.insert(table.to_owned(), state.store.watermarks.get(org, table).await?.into());
    }
    Ok(Value::Object(cursors))
}

#[derive(sqlx::FromRow, Serialize)]
struct Event {
    id: i64,
    dedup_key: String,
    text: String,
    metadata: Value,
    created_at: DateTime<Utc>,
}

/// The stored events, newest first. Append only: storing one is what "read" means.
async fn events(State(state): State<AppState>, auth: Auth, Path((_, id)): Id) -> Result<Json<Vec<Event>>, ApiError> {
    one(&state, &auth, id).await?;
    let sql = "SELECT id, dedup_key, text, metadata, created_at FROM notification_events WHERE notification = $1 \
               ORDER BY id DESC LIMIT 200";
    Ok(Json(sqlx::query_as(sql).bind(id).fetch_all(&state.store.pg).await?))
}

/// Subscribing adds the caller to the recipients; a silicon needs the webhook its events would
/// go to before it can.
async fn subscribe(State(state): State<AppState>, auth: Auth, Path((_, id)): Id) -> Result<Json<Value>, ApiError> {
    let row = mine(&state, &auth, id).await?;
    let me = auth.actor()?;
    let sql = "SELECT count(*) FROM webhooks WHERE org = $1 AND actor = $2";
    let webhooks: i64 = sqlx::query_scalar(sql).bind(&me.org).bind(&me.id).fetch_one(&state.store.pg).await?;
    if me.kind == Kind::Silicon && webhooks == 0 {
        let message = "a silicon is delivered to its own webhook: set one first (spacestation webhook set <url>)";
        return Err(invalid("no_delivery_webhook", message));
    }
    recipients(&state, id, row, &format!("@{}", me.id), true).await
}

async fn unsubscribe(State(state): State<AppState>, auth: Auth, Path((_, id)): Id) -> Result<Json<Value>, ApiError> {
    let row = mine(&state, &auth, id).await?;
    let entry = format!("@{}", auth.actor()?.id);
    recipients(&state, id, row, &entry, false).await
}

/// The subscription set with `entry` added or removed; never versioned, so one UPDATE.
async fn recipients(
    state: &AppState,
    id: Uuid,
    row: Row,
    entry: &str,
    subscribed: bool,
) -> Result<Json<Value>, ApiError> {
    let mut recipients = row.recipients_list();
    recipients.retain(|r| r != entry);
    if subscribed {
        recipients.push(entry.to_owned());
    }
    sqlx::query("UPDATE notifications SET recipients = $2 WHERE id = $1")
        .bind(id)
        .bind(Value::from(recipients.clone()))
        .execute(&state.store.pg)
        .await?;
    engine::changed();
    Ok(Json(json!({"recipients": recipients})))
}

/// Strikes `entry` (`@actor` or `webhook:{id}`) from every recipients list in `org`: what a
/// removal from IAM and a deleted webhook both do, so no list names a recipient that no longer
/// exists. Returns how many notifications changed; the caller rebuilds the engine when any did.
pub async fn forget_recipient(tx: &mut PgConnection, org: &str, entry: &str) -> sqlx::Result<u64> {
    let sql = "UPDATE notifications SET recipients = recipients - $2::text WHERE org = $1 AND recipients ? $2";
    Ok(sqlx::query(sql).bind(org).bind(entry).execute(tx).await?.rows_affected())
}

/// Runs the sql now over `(cursors, watermark]` and moves nothing: what the test button, the CLI
/// and `space-station-dev notify` show. Org-scoped, exactly as the engine will run it.
async fn test(State(state): State<AppState>, auth: Auth, Path((_, id)): Id) -> Result<Json<Value>, ApiError> {
    let def = mine(&state, &auth, id).await?.def()?;
    let cursors: Value = sqlx::query_scalar("SELECT cursors FROM notifications WHERE id = $1")
        .bind(id)
        .fetch_one(&state.store.pg)
        .await?;
    let last: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT max(created_at) FROM notification_events WHERE notification = $1")
            .bind(id)
            .fetch_one(&state.store.pg)
            .await?;
    let restrict = engine::restrict(&def, &serde_json::from_value(cursors).unwrap_or_default());
    let mut result = json!({"rows": [], "last_trigger_at": last});
    match query::run(&state, auth.org(), None, &def.sql, &restrict).await {
        Ok(run) => result["rows"] = run["rows"].clone(),
        Err(e) => result["error"] = e.message.into(),
    }
    Ok(Json(result))
}

fn invalid(code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
}

/// Everything that must hold before a definition is stored, checked with the saver's identity:
/// every table it names must be visible to them, the sql and every trigger must parse, ClickHouse
/// must accept the sql and see the three columns in it, the durations must fit, and the
/// recipients must stay inside the access list. The creator is appended to `access`, so every
/// later check is one membership test.
async fn validate(
    state: &AppState,
    me: &Identity,
    def: Def,
    recipients: &[String],
    current: &[String],
) -> Result<Def, ApiError> {
    if def.name.trim().is_empty() || def.name.len() > 200 {
        return Err(invalid("invalid_name", "a notification name is 1 to 200 characters"));
    }
    access::validate(&def.access)?;
    for (what, text, max, limit) in
        [("delay", &def.delay, DELAY_MAX, "1h"), ("cooldown", &def.cooldown, COOLDOWN_MAX, "30d")]
    {
        match duration_ms(text) {
            None => return Err(invalid("invalid_duration", format!("{what} {text:?} is not like \"2s\" or \"10m\""))),
            Some(ms) if ms > max => return Err(invalid("invalid_duration", format!("{what} is at most {limit}"))),
            Some(_) => {}
        }
    }
    if def.triggers.is_empty() {
        return Err(invalid("invalid_trigger", "a notification needs at least one trigger"));
    }
    let tables = access::visible_tables(state, me).await?;
    let visible = |table: &str| tables.iter().any(|t| t == table);
    let plan = sql::plan(&def.sql, &visible).map_err(|e| invalid("invalid_sql", e.to_string()))?;
    shape(state, &me.org, &plan).await?;
    for trigger in &def.triggers {
        match trigger {
            Trigger::Table { table, where_ } => sql::trigger_plan(table, where_.as_deref(), &visible)
                .map(drop)
                .map_err(|e| invalid("invalid_trigger", e.to_string()))?,
            Trigger::Schedule { schedule } => Cron::parse(schedule)
                .map(drop)
                .ok_or_else(|| invalid("invalid_cron", format!("{schedule:?} is not a 5-field UTC cron expression")))?,
        }
    }
    let access = access::with_actor(def.access, &me.id);
    let mut webhooks = Vec::new();
    for entry in recipients {
        if !recipient_in(entry, &access, current, &me.id) {
            return Err(invalid("recipients_not_in_access", format!("{entry:?} is not in the access list")));
        }
        if let Entry::Webhook(id) = Entry::parse(entry) {
            let id =
                Uuid::parse_str(id).map_err(|_| invalid("unknown_webhook", format!("{entry:?} is not a webhook")))?;
            webhooks.push(id);
        }
    }
    webhooks.sort();
    webhooks.dedup();
    let sql = "SELECT count(*) FROM webhooks WHERE org = $1 AND id = ANY($2) AND actor IS NULL";
    let known: i64 = sqlx::query_scalar(sql).bind(&me.org).bind(&webhooks).fetch_one(&state.store.pg).await?;
    if known != webhooks.len() as i64 {
        return Err(invalid("unknown_webhook", "a webhook recipient does not exist in this org"));
    }
    Ok(Def { access, ..def })
}

/// One ClickHouse round trip at save time, so a notification that could never fire is refused
/// rather than failing on every trigger: `DESCRIBE` of the rendered query names the columns it
/// would yield without reading a row, and every one of `dedup_key`, `text`, `metadata` must be
/// among them. ClickHouse refusing the query outright (an unknown function, say) is `invalid_sql`
/// with its own words.
async fn shape(state: &AppState, org: &str, plan: &sql::Plan) -> Result<(), ApiError> {
    let describe = format!("DESCRIBE ({}) FORMAT JSONEachRow", plan.sql(org, &BTreeMap::new()));
    let columns = match state.store.ch.query_org(org, &describe).await {
        Ok(columns) => columns,
        Err(ChError::Http { message, .. }) => return Err(invalid("invalid_sql", message)),
        Err(e) => return Err(e.into()),
    };
    let missing: Vec<&str> =
        SHAPE.into_iter().filter(|want| !columns.iter().any(|c| c["name"].as_str() == Some(want))).collect();
    match missing.is_empty() {
        true => Ok(()),
        false => Err(invalid(
            "invalid_sql_shape",
            format!("the sql must return {}; missing: {}", SHAPE.join(", "), missing.join(", ")),
        )),
    }
}

/// Is `entry` a recipient this saver may set? `recipients ⊆ access`, as far as one process can
/// know it: an `@actor` must be named by the access list itself, be the saver (who passed it to
/// get here), or already be a recipient — someone who matched a tag is on the list because
/// `subscribe` tested their identity, and nobody here can re-resolve another actor's tags. A
/// `webhook:{id}` only has to belong to the org: a webhook is a thing, not a member, and the
/// saver's own access is what stands behind it. A tag is not a recipient; delivery is per actor.
fn recipient_in(entry: &str, access: &[String], current: &[String], saver: &str) -> bool {
    match Entry::parse(entry) {
        Entry::Actor(actor) => {
            !actor.is_empty()
                && (actor == saver || access.iter().any(|e| e == entry) || current.iter().any(|e| e == entry))
        }
        Entry::Webhook(id) => !id.is_empty(),
        Entry::Tag(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn durations_are_an_integer_and_one_unit() {
        assert_eq!(duration_ms("500ms"), Some(500));
        assert_eq!(duration_ms("2s"), Some(2_000));
        assert_eq!(duration_ms("10m"), Some(600_000));
        assert_eq!(duration_ms("1h"), Some(DELAY_MAX));
        assert_eq!(duration_ms("30d"), Some(COOLDOWN_MAX));
        assert_eq!(duration_ms("0s"), Some(0));
        for text in
            ["", "2", "s", "ms", "-2s", "2.5s", "2 s", " 2s", "2sec", "2S", "1e3s", "2π", "99999999999999999999d"]
        {
            assert_eq!(duration_ms(text), None, "{text:?} is not a duration");
        }
    }

    #[test]
    fn a_definition_keeps_its_defaults_and_both_trigger_shapes() {
        let def: Def = serde_json::from_value(json!({
            "name": "Big order", "sql": "SELECT 1", "access": ["ops"],
            "triggers": [{"table": "orders", "where": "record.x::Float64 > 1"}, {"schedule": "*/5 * * * *"}]
        }))
        .unwrap();
        assert_eq!((def.enabled, def.delay.as_str(), def.cooldown.as_str()), (true, "2s", "10m"));
        assert_eq!(def.tables().collect::<Vec<_>>(), [("orders", Some("record.x::Float64 > 1"))]);
        assert_eq!(def.schedules().collect::<Vec<_>>(), ["*/5 * * * *"]);
        let stored = json!(def);
        assert_eq!(
            stored["triggers"],
            json!([{"table": "orders", "where": "record.x::Float64 > 1"}, {"schedule": "*/5 * * * *"}])
        );
        assert!(stored.get("description").is_none(), "an absent description stays absent");
        let full = json!({"name": "Big order", "description": "over 100", "enabled": false, "sql": "SELECT 1",
            "triggers": [{"schedule": "0 * * * *"}], "delay": "500ms", "cooldown": "1d", "access": ["@alice"]});
        let def: Def = serde_json::from_value(full.clone()).unwrap();
        assert_eq!(json!(def), full, "what a client sends round-trips field for field");
    }

    #[test]
    fn recipients_stay_inside_the_access_list() {
        let access = strings(&["@alice", "ops"]);
        let subscribed = strings(&["@bob"]);
        assert!(recipient_in("@alice", &access, &[], "carol"), "named by the list");
        assert!(recipient_in("@carol", &access, &[], "carol"), "the saver, who passed the list to get here");
        assert!(recipient_in("@bob", &access, &subscribed, "carol"), "already subscribed, through a tag");
        assert!(!recipient_in("@bob", &access, &[], "carol"), "another actor's tags are not ours to resolve");
        assert!(recipient_in("webhook:abc", &[], &[], "carol"), "a webhook is a thing, not a member");
        assert!(!recipient_in("ops", &access, &[], "carol"), "a tag is not a recipient");
        assert!(!recipient_in("@", &access, &[], "carol"));
        assert!(!recipient_in("webhook:", &access, &[], "carol"));
    }
}
