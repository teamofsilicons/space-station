//! `POST /orgs/{org}/query`: the mirage in one request. Plan the SQL against the caller's visible
//! tables, bound every table to `(from, to]` with `to` defaulting to the watermark now, render,
//! run as `ss_query`, and echo the effective bounds as `watermarks`.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::State;
use axum::routing::post;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json};
use crate::sql::{self, Bounds};

#[derive(Deserialize)]
struct Body {
    sql: String,
    #[serde(default)]
    restrict: BTreeMap<String, Restrict>,
}

#[derive(Deserialize, Default, Clone, Copy)]
pub struct Restrict {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/orgs/{org}/query", post(query))
}

async fn query(State(state): State<AppState>, auth: Auth, Json(body): Json<Body>) -> Result<Json<Value>, ApiError> {
    auth.allow("tables")?;
    let visible = auth.visible_tables(&state).await?;
    Ok(Json(run(&state, auth.org(), visible.as_deref(), &body.sql, &body.restrict).await?))
}

/// `{rows, watermarks}` for `sql` over `visible` tables; `None` means every table in the org
/// (API keys, the notification engine).
pub async fn run(
    state: &AppState,
    org: &str,
    visible: Option<&[String]>,
    sql: &str,
    restrict: &BTreeMap<String, Restrict>,
) -> Result<Value, ApiError> {
    let all: Vec<String>;
    let visible = match visible {
        Some(visible) => visible,
        None => {
            all =
                sqlx::query_scalar("SELECT id FROM tables WHERE org = $1").bind(org).fetch_all(&state.store.pg).await?;
            &all
        }
    };
    let plan = sql::plan(sql, &|table| visible.iter().any(|v| v == table))?;
    let (mut bounds, mut watermarks) = (BTreeMap::new(), serde_json::Map::new());
    for table in plan.tables() {
        let restrict = restrict.get(table).copied().unwrap_or_default();
        let to = match restrict.to {
            Some(to) => to,
            None => state.store.watermarks.get(org, table).await?,
        };
        bounds.insert(table.clone(), Bounds { from: restrict.from, to: Some(to) });
        watermarks.insert(table.clone(), to.into());
    }
    let rows = state.store.ch.query_org(org, &plan.render(org, &bounds)).await?;
    Ok(json!({"rows": rows, "watermarks": watermarks}))
}
