//! ClickHouse over HTTP. The admin user from `CLICKHOUSE_URL` bootstraps, inserts and reads
//! watermarks; the `ss_query` user, fenced by its settings profile and the `org_only` row policy,
//! runs everything a user wrote with `SQL_org` as the only setting. Errors say whether a retry
//! can help (`Transport`) or not (`Http`).

use std::fmt;
use std::time::Duration;

use serde_json::Value;
use url::Url;

#[derive(Clone)]
pub struct Clickhouse {
    http: reqwest::Client,
    admin: Access,
    query: Access,
}

#[derive(Clone)]
struct Access {
    url: Url,
    user: String,
    password: String,
}

#[derive(Debug, Clone)]
pub enum ChError {
    /// Could not reach ClickHouse, or it did not answer in time: a retry may succeed.
    Transport(String),
    /// ClickHouse answered and refused; `message` is its exception text.
    Http { status: u16, message: String },
}

impl fmt::Display for ChError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "clickhouse unreachable: {e}"),
            Self::Http { status, message } => write!(f, "clickhouse {status}: {message}"),
        }
    }
}

impl std::error::Error for ChError {}

/// The bootstrap, statement by statement, exactly as docs/ARCHITECTURE.md "Storage" has it; the
/// `ALTER USER` makes an existing `ss_query` converge on the configured password and profile.
const BOOTSTRAP: [&str; 6] = [
    "CREATE TABLE IF NOT EXISTS records (org_id LowCardinality(String), table_id LowCardinality(String), \
     cursor UInt64, record_id UUID, event_ts_ms Int64, registered_ts_ms Int64, metadata JSON, record JSON) \
     ENGINE = MergeTree ORDER BY (org_id, table_id, event_ts_ms) SETTINGS non_replicated_deduplication_window = 100",
    "CREATE SETTINGS PROFILE IF NOT EXISTS ss_query SETTINGS readonly = 1 CONST, allow_ddl = 0 CONST, \
     max_execution_time = 10 CONST, max_result_rows = 100000 CONST, max_result_bytes = 16000000 CONST, \
     result_overflow_mode = 'throw' CONST, max_memory_usage = 2000000000 CONST, \
     output_format_json_quote_64bit_integers = 1 CONST, SQL_org = '' CHANGEABLE_IN_READONLY",
    "CREATE USER IF NOT EXISTS ss_query IDENTIFIED WITH sha256_password BY '{password}' SETTINGS PROFILE 'ss_query'",
    "ALTER USER ss_query IDENTIFIED WITH sha256_password BY '{password}' SETTINGS PROFILE 'ss_query'",
    "GRANT SELECT ON space_station.records TO ss_query",
    "CREATE ROW POLICY IF NOT EXISTS org_only ON space_station.records FOR SELECT \
     USING org_id = getSetting('SQL_org') TO ss_query",
];

impl Clickhouse {
    /// `admin` is `http(s)://user:password@host:port/database`; `ss_query` lives on the same host.
    pub fn new(admin: &Url, query_password: &str) -> Result<Clickhouse, ChError> {
        let database = admin.path().trim_start_matches('/').to_owned();
        let mut url = admin.clone();
        url.set_path("/");
        url.set_query(None);
        url.query_pairs_mut().append_pair("database", &database);
        let bad = |_| ChError::Transport("the ClickHouse URL cannot carry credentials".into());
        url.set_username("").map_err(bad)?;
        url.set_password(None).map_err(bad)?;
        let password = admin.password().unwrap_or_default().to_owned();
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(transport)?;
        Ok(Clickhouse {
            http,
            admin: Access { url: url.clone(), user: admin.username().to_owned(), password },
            query: Access { url, user: "ss_query".into(), password: query_password.to_owned() },
        })
    }

    pub async fn bootstrap(&self, query_password: &str) -> Result<(), ChError> {
        let password = query_password.replace('\\', "\\\\").replace('\'', "\\'");
        for statement in BOOTSTRAP {
            self.post(&self.admin, &[], statement.replace("{password}", &password).into_bytes()).await?;
        }
        Ok(())
    }

    /// One `INSERT … FORMAT JSONEachRow` of `rows`; ClickHouse drops a block whose `token` it
    /// already has, which is what makes a retried flush safe.
    pub async fn insert(&self, rows: Vec<u8>, token: &str) -> Result<(), ChError> {
        let params = [("query", "INSERT INTO records FORMAT JSONEachRow"), ("insert_deduplication_token", token)];
        self.post(&self.admin, &params, rows).await.map(drop)
    }

    /// An admin query with `{name:Type}` placeholders bound from `params`; `sql` names its FORMAT.
    pub async fn query_admin(&self, sql: &str, params: &[(&str, &str)]) -> Result<Vec<Value>, ChError> {
        let params: Vec<(String, &str)> = params.iter().map(|(k, v)| (format!("param_{k}"), *v)).collect();
        let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        rows(&self.post(&self.admin, &params, sql.as_bytes().to_vec()).await?)
    }

    /// A rendered user query as `ss_query` with `SQL_org=org` and nothing else.
    pub async fn query_org(&self, org: &str, sql: &str) -> Result<Vec<Value>, ChError> {
        rows(&self.post(&self.query, &[("SQL_org", org)], sql.as_bytes().to_vec()).await?)
    }

    async fn post(&self, access: &Access, params: &[(&str, &str)], body: Vec<u8>) -> Result<String, ChError> {
        let mut url = access.url.clone();
        url.query_pairs_mut().extend_pairs(params);
        let request = self.http.post(url).basic_auth(&access.user, Some(&access.password)).body(body);
        let res = request.send().await.map_err(transport)?;
        let status = res.status();
        let text = res.text().await.map_err(transport)?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(ChError::Http { status: status.as_u16(), message: message(&text) })
        }
    }
}

fn transport(e: reqwest::Error) -> ChError {
    ChError::Transport(e.without_url().to_string())
}

/// JSONEachRow, one object per line.
fn rows(text: &str) -> Result<Vec<Value>, ChError> {
    let parse = |line| serde_json::from_str(line).map_err(|e| ChError::Http { status: 502, message: e.to_string() });
    text.lines().filter(|l| !l.trim().is_empty()).map(parse).collect()
}

/// The exception text, out of its `{"exception": …}` wrapper and without the version suffix.
fn message(text: &str) -> String {
    let text = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["exception"].as_str().map(str::to_owned))
        .unwrap_or_else(|| text.trim().to_owned());
    match text.find(" (version ") {
        Some(end) => text[..end].to_owned(),
        None => text,
    }
}

/// A 64-bit number the way ClickHouse's JSON formats send it: quoted or not.
pub fn u64_of(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_text_is_unwrapped() {
        let json =
            r#"{"exception": "Code: 47. DB::Exception: Unknown identifier `x` (UNKNOWN_IDENTIFIER) (version 25.8.1)"}"#;
        assert_eq!(message(json), "Code: 47. DB::Exception: Unknown identifier `x` (UNKNOWN_IDENTIFIER)");
        assert_eq!(
            message("Code: 164. DB::Exception: readonly (READONLY) (version 25.8.1 (official build))\n"),
            "Code: 164. DB::Exception: readonly (READONLY)"
        );
        assert_eq!(u64_of(&serde_json::json!("42")), Some(42));
        assert_eq!(u64_of(&serde_json::json!(42)), Some(42));
    }

    #[test]
    fn the_admin_url_keeps_the_database_and_drops_the_credentials() {
        let ch = Clickhouse::new(&"http://dev:secret@localhost:8123/space_station".parse().unwrap(), "qpw").unwrap();
        assert_eq!(ch.admin.url.as_str(), "http://localhost:8123/?database=space_station");
        assert_eq!((ch.admin.user.as_str(), ch.admin.password.as_str()), ("dev", "secret"));
        assert_eq!((ch.query.user.as_str(), ch.query.password.as_str()), ("ss_query", "qpw"));
    }
}
