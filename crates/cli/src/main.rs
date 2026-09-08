//! The `spacestation` command: one clap tree over the `space-station` crate, plus the state the
//! package refuses to hold. Every arm reads arguments, calls one method on the package, and hands
//! the answer to `out`, so a capability the crate does not have cannot be typed here.
//!
//! What lives in this crate: the tree, the flags, the files an argument names, the links that
//! let a terminal hand off to the web app, and — in `store` — `~/.space-station/auth.json`: the
//! one signed-in session and the org it is bound to. Nothing here ever prompts or takes a
//! credential: a person signs in through the browser, or a person or a silicon hands over the
//! short-lived token the `iam` CLI minted, and the backend keeps the session it exchanges it for.

mod out;
mod store;
#[cfg(test)]
mod tests;

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::{env, fs, process};

use clap::{Parser, Subcommand};
use serde_json::Value;
use space_station::{Auth, Def, Error, Key, Space, SpaceClient, WindowOutput, daemon};

use out::Col;
use store::Stored;

const TABLES: &[Col] = &[
    ("ID", "id"),
    ("RECORDS", "records"),
    ("WATERMARK", "watermark"),
    ("CREATED_BY", "created_by"),
    ("ACCESS", "access"),
];
const WINDOWS: &[Col] =
    &[("ID", "id"), ("NAME", "name"), ("VERSION", "version.name"), ("CREATED_BY", "created_by"), ("ACCESS", "access")];
const NOTIFICATIONS: &[Col] = &[
    ("ID", "id"),
    ("NAME", "def.name"),
    ("ENABLED", "enabled"),
    ("RECIPIENTS", "recipients"),
    ("CREATED_BY", "created_by"),
];
const WEBHOOKS: &[Col] = &[("ID", "id"), ("URL", "url"), ("CREATED_BY", "created_by"), ("CREATED_AT", "created_at")];
const KEYS: &[Col] =
    &[("ID", "id"), ("SCOPES", "scopes"), ("CREATED_BY", "created_by"), ("LAST_USED_AT", "last_used_at")];
const ORGS: &[Col] = &[("ID", "id"), ("NAME", "name")];

/// The globals that also come from the environment are read by hand, not through clap's `env`:
/// clap counts a value it took from a variable as given and then draws it into every usage line
/// as if it were required (`windows run --org <ORG> <ID>`), where `[OPTIONS]` is the truth.
#[derive(Parser)]
#[command(name = "spacestation", version, about = "Space Station from the terminal: all of the space-station crate")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// The org to work in, and the one a new session is bound to; else $SPACE_STATION_ORG, else the stored one
    #[arg(long, global = true)]
    org: Option<String>,
    /// Print the JSON of a list instead of columns
    #[arg(long, global = true)]
    json: bool,
    /// Act as this apikey- key (else $SPACE_STATION_API_KEY) rather than the stored credential
    #[arg(long, global = true)]
    api_key: Option<String>,
    /// Act as this spacewindow- access token (else $SPACE_STATION_ACCESS_TOKEN) rather than the stored credential
    #[arg(long, global = true)]
    access_token: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign in through the browser: opens IAM, keeps the session Space Station redirects back, bound to --org
    Login {
        /// Print the link instead of opening it
        #[arg(long)]
        no_browser: bool,
    },
    /// Sign in with a short-lived token from `iam login --app-id 'tos>spacestation' --org <org>` or
    /// `iam silicon-login --app-id 'tos>spacestation'`; never prompts
    Auth {
        /// The short-lived token; `-` reads it from stdin; else $SPACE_STATION_TOKEN
        #[arg(value_name = "SLT")]
        token: Option<String>,
    },
    /// Forget the stored credential, and end a terminal session at the server
    Logout,
    /// Who this credential is in this org: id, kind, org and tags
    Whoami,
    /// The orgs you belong to
    Orgs,
    /// Work in this org from now on
    Use { org: String },
    /// Tables: ls, get, create, access, rotate, rm, overview
    #[command(subcommand)]
    Tables(Tables),
    /// Space windows: ls, get, code, create, edit, rm, versions, publish, run, json, tool, open
    #[command(subcommand)]
    Windows(Windows),
    /// Notifications: ls, get, create, edit, rm, events, subscribe, unsubscribe, test
    #[command(subcommand)]
    Notifications(Notifications),
    /// The org's webhooks: ls, create, rm
    #[command(subcommand)]
    Webhooks(Webhooks),
    /// This silicon's own delivery webhook: set, rm
    #[command(subcommand)]
    Webhook(Webhook),
    /// API keys: ls, create, rm
    #[command(subcommand)]
    Keys(Keys),
    /// The access token processors use: show, rotate
    #[command(subcommand)]
    Token(Token),
    /// Run a read-only SQL query over the org's tables
    Query { sql: String },
    /// Send one JSON record with a table key; DATA defaults to stdin
    Record {
        /// The JSON object, or `-` to read it from stdin
        #[arg(default_value = "-")]
        data: String,
        /// The table key; else $SPACE_STATION_TABLE_KEY
        #[arg(long)]
        table_key: Option<String>,
    },
    /// What went wrong server-side for this org
    Errors,
    /// The ingest daemon: run (the default), status
    Daemon {
        #[command(subcommand)]
        cmd: Option<Daemon>,
    },
}

#[derive(Subcommand)]
enum Tables {
    /// Every table you may see, with its records and its watermark
    Ls,
    /// One table
    Get { id: String },
    /// Create a table and print its key once
    Create {
        id: String,
        /// @actors and tags that may use it, comma separated
        #[arg(long, value_delimiter = ',')]
        access: Vec<String>,
    },
    /// Replace who may use the table
    Access {
        id: String,
        #[arg(long, value_delimiter = ',', required = true)]
        access: Vec<String>,
    },
    /// Replace the table key and print the new one once
    Rotate { id: String },
    /// Delete the table; its records go too
    Rm { id: String },
    /// Counts, the busiest tables and the ingest lag
    Overview {
        /// 1m 5m 15m 1h 5h 1d 7d 30d
        #[arg(long, default_value = "5h")]
        window: String,
    },
}

#[derive(Subcommand)]
enum Windows {
    /// Every window you may open, with the name of the version it runs
    Ls,
    /// One window and the version it runs, without the code (see `code`)
    Get { id: String },
    /// The processor and renderer of the version a window runs
    Code { id: String },
    /// Create a window; the name is what the UI shows, under 20 characters
    Create {
        name: String,
        /// @actors and tags that may open it, comma separated
        #[arg(long, value_delimiter = ',')]
        access: Vec<String>,
    },
    /// Rename a window, change who may open it, or both
    Edit {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_delimiter = ',')]
        access: Option<Vec<String>>,
    },
    /// Delete the window with its versions and its state
    Rm { id: String },
    /// Every published version, with its code
    Versions { id: String },
    /// Upload a named version, refusing code that carries a secret
    Publish {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        processor: PathBuf,
        #[arg(long)]
        renderer: PathBuf,
    },
    /// Run the published processor locally in Node until the window goes idle
    Run { id: String },
    /// The cached SiliconJSON and its metadata
    Json { id: String },
    /// Run one processor tool against the cached SiliconJSON; ARGS is a JSON object
    Tool { id: String, name: String, args: String },
    /// Print the page that renders this window, and open it
    Open { id: String },
}

#[derive(Subcommand)]
enum Notifications {
    /// Every notification you may see
    Ls,
    /// One notification, its definition and its subscribers
    Get { id: String },
    /// Create one from a JSON file: the definition itself, or {def, recipients?}; you are the recipient unless it says
    Create { file: PathBuf },
    /// Replace the definition from such a file; absent recipients keeps the current ones
    Edit { id: String, file: PathBuf },
    /// Delete the notification with its versions and its events
    Rm { id: String },
    /// What it has fired, newest first
    Events { id: String },
    /// Add yourself to its recipients
    Subscribe { id: String },
    /// Take yourself off its recipients
    Unsubscribe { id: String },
    /// Run its SQL now, over everything since its cursors, without advancing them
    Test { id: String },
}

#[derive(Subcommand)]
enum Webhooks {
    /// Every webhook of the org
    Ls,
    /// Add a webhook and print its signing secret once
    Create { url: String },
    /// Delete a webhook; notifications delivering to it stop
    Rm { id: String },
}

#[derive(Subcommand)]
enum Webhook {
    /// Set this silicon's delivery webhook and print its signing secret once
    Set { url: String },
    /// Remove this silicon's delivery webhook; its notifications stop arriving
    Rm,
}

#[derive(Subcommand)]
enum Keys {
    /// Every api key of the org and when it was last used
    Ls,
    /// Create an api key and print it once
    Create {
        #[arg(long, value_delimiter = ',', required = true)]
        scopes: Vec<String>,
    },
    /// Delete an api key; it stops resolving at once
    Rm { id: String },
}

#[derive(Subcommand)]
enum Token {
    /// Print the access token and when it was last used
    Show,
    /// Replace it; the old one stops resolving at once
    Rotate,
}

#[derive(Subcommand)]
enum Daemon {
    /// Run the ingest daemon in the foreground
    Run,
    /// Whether one is listening on this machine, where, and how much it still owes the server
    Status,
}

/// Which credential a run acts with, for what a 401 means afterwards.
enum Acting {
    /// The stored session: a 401 ends it, so it is forgotten and the next command says so.
    Stored,
    ApiKey,
    AccessToken,
    /// Signing in, out, or the daemon: no credential acted.
    Nobody,
}

fn main() {
    let mut cli = Cli::parse();
    cli.org = setting(cli.org, "SPACE_STATION_ORG");
    cli.api_key = setting(cli.api_key, "SPACE_STATION_API_KEY");
    cli.access_token = setting(cli.access_token, "SPACE_STATION_ACCESS_TOKEN");
    let acting = match (&cli.cmd, &cli.api_key, &cli.access_token) {
        (
            Cmd::Login { .. }
            | Cmd::Auth { .. }
            | Cmd::Logout
            | Cmd::Use { .. }
            | Cmd::Daemon { .. }
            | Cmd::Record { .. },
            ..,
        ) => Acting::Nobody,
        (_, Some(_), _) => Acting::ApiKey,
        (_, None, Some(_)) => Acting::AccessToken,
        (_, None, None) => Acting::Stored,
    };
    let home = space_station::default_home();
    if let Err(e) = run(cli, &home) {
        let refused = match (acting, &e) {
            (Acting::Stored, Error::Api { status: 401, .. }) if store::expire(&home).is_ok() => out::EXPIRED,
            (Acting::ApiKey, _) => out::KEY_REFUSED,
            (Acting::AccessToken, _) => out::TOKEN_REFUSED,
            _ => out::SIGN_IN,
        };
        eprintln!("{}", out::fail(&e, refused));
        process::exit(1);
    }
}

/// A flag, else its variable; a blank from either is absent — `set -a; . ./.env` exports
/// `SPACE_STATION_ACCESS_TOKEN=` bare, and that must not become an empty bearer or org.
fn setting(flag: Option<String>, var: &str) -> Option<String> {
    flag.or_else(|| env::var(var).ok()).filter(|v| !v.trim().is_empty())
}

fn run(cli: Cli, home: &Path) -> Result<(), Error> {
    let Cli { cmd, org, json: as_json, api_key, access_token } = cli;
    let url = space_station::default_url();
    // The credential this run acts with, and the org it works in: `--org` (or $SPACE_STATION_ORG)
    // above the one stored, which only the stored credential has.
    let space = || -> Result<Space, Error> {
        let (auth, org) = match (&api_key, &access_token) {
            (Some(key), _) => (Auth::api_key(key), org.clone()),
            (None, Some(token)) => (Auth::access_token(token), org.clone()),
            (None, None) => {
                let stored = store::load(home)?;
                (stored.auth, org.clone().or(stored.org))
            }
        };
        let space = Space::new(&url, auth)?;
        Ok(match org {
            Some(org) => space.org(org),
            None => space,
        })
    };
    match cmd {
        Cmd::Login { no_browser } => {
            let org = bound(home, org)?;
            let auth = space_station::login(&url, &org, |link| {
                if no_browser || !out::open(link) {
                    eprintln!("open this to sign in:\n{link}")
                }
            })?;
            signed_in(home, &url, auth, org)?
        }
        Cmd::Auth { token } => {
            let org = bound(home, org)?;
            let slt = match setting(token, "SPACE_STATION_TOKEN").as_deref() {
                Some("-") => stdin()?,
                Some(token) => token.to_string(),
                None => {
                    return Err(Error::Local("no token: pass one, `-` for stdin, or set $SPACE_STATION_TOKEN".into()));
                }
            };
            signed_in(home, &url, space_station::exchange(&url, &slt, &org)?, org)?
        }
        Cmd::Logout => {
            let ended = store::load(home).and_then(|s| Space::new(&url, s.auth)?.logout());
            store::forget(home)?;
            match ended {
                Ok(()) => eprintln!("the stored credential is gone"),
                Err(e) => {
                    eprintln!("the stored credential is gone; the server session was not ended ({})", out::code(&e))
                }
            }
        }
        Cmd::Whoami => {
            match (&api_key, &access_token) {
                (Some(key), _) => {
                    eprintln!("using: {} from --api-key / $SPACE_STATION_API_KEY", Auth::api_key(key).describe())
                }
                (None, Some(token)) => {
                    let what = Auth::access_token(token).describe();
                    eprintln!("using: {what} from --access-token / $SPACE_STATION_ACCESS_TOKEN")
                }
                (None, None) => {
                    if let Ok(stored) = store::load(home) {
                        eprintln!("stored: {}", stored.auth.describe());
                    }
                }
            }
            out::json(&space()?.me()?)?
        }
        Cmd::Orgs => out::rows(&space()?.orgs()?, ORGS, as_json)?,
        Cmd::Use { org } => {
            store::update(home, |s| Ok(Stored { org: Some(org.clone()), ..s.ok_or_else(store::not_signed_in)? }))?;
            eprintln!("working in {org}")
        }

        Cmd::Tables(Tables::Ls) => out::rows(&space()?.tables()?, TABLES, as_json)?,
        Cmd::Tables(Tables::Get { id }) => out::json(&space()?.table(&id)?)?,
        Cmd::Tables(Tables::Create { id, access }) => once("table key", &space()?.create_table(&id, &refs(&access))?),
        Cmd::Tables(Tables::Access { id, access }) => out::json(&space()?.set_table_access(&id, &refs(&access))?)?,
        Cmd::Tables(Tables::Rotate { id }) => {
            let key = space()?.rotate_table_key(&id)?;
            out::secret("the new table key, shown once; the old one is already dead", &key.value)
        }
        Cmd::Tables(Tables::Rm { id }) => {
            space()?.delete_table(&id)?;
            eprintln!("table {id} deleted; its records follow, on ClickHouse's own clock")
        }
        Cmd::Tables(Tables::Overview { window }) => out::json(&space()?.overview(&window)?)?,

        Cmd::Windows(Windows::Ls) => out::rows(&out::summary(&space()?.windows()?)?, WINDOWS, as_json)?,
        Cmd::Windows(Windows::Get { id }) => {
            let space = space()?;
            out::json(&out::summary(&space.window(&id)?)?)?;
            page(&space, &id)
        }
        Cmd::Windows(Windows::Code { id }) => match space()?.window(&id)?.version {
            Some(version) => out::json(&version)?,
            None => return Err(Error::Local(format!("window {id} has no published version yet"))),
        },
        Cmd::Windows(Windows::Create { name, access }) => {
            let space = space()?;
            let window = space.create_window(&name, &refs(&access))?;
            out::json(&out::summary(&window)?)?;
            page(&space, &window.id)
        }
        Cmd::Windows(Windows::Edit { id, name, access }) => {
            let space = space()?;
            let named = access.as_deref().map(refs);
            out::json(&out::summary(&space.update_window(&id, name.as_deref(), named.as_deref())?)?)?;
            page(&space, &id)
        }
        Cmd::Windows(Windows::Rm { id }) => {
            space()?.delete_window(&id)?;
            eprintln!("window {id} deleted, with its versions and its state")
        }
        Cmd::Windows(Windows::Versions { id }) => out::json(&space()?.versions(&id)?)?,
        Cmd::Windows(Windows::Publish { id, name, processor, renderer }) => {
            let space = space()?;
            let (processor, renderer) = (read(&processor)?, read(&renderer)?);
            out::json(&space.publish(&id, &name, &processor, &renderer)?)?;
            page(&space, &id)
        }
        Cmd::Windows(Windows::Run { id }) => {
            let space = space()?;
            // `not_found` before a link and before Node: the package checks too, for its own callers.
            space.window(&id)?;
            page(&space, &id);
            space.run_window(&id, &home.join("runtime"), |line| match line {
                WindowOutput::Json(line) => println!("{line}"),
                WindowOutput::Diagnostic(line) => eprintln!("{line}"),
            })?
        }
        Cmd::Windows(Windows::Json { id }) => out::json(&space()?.window_state(&id)?)?,
        Cmd::Windows(Windows::Tool { id, name, args }) => {
            let args = match serde_json::from_str::<Value>(&args) {
                Ok(args @ Value::Object(_)) => args,
                _ => return Err(Error::Local(r#"args must be a JSON object, like '{"order_id": "o_9"}'"#.into())),
            };
            out::json(&space()?.window_tool(&id, &home.join("runtime"), &name, &args)?)?
        }
        Cmd::Windows(Windows::Open { id }) => {
            let link = space()?.window_url(&id)?;
            println!("{link}");
            if !out::open(&link) {
                eprintln!("there is no browser here to open it with")
            }
        }

        Cmd::Notifications(Notifications::Ls) => out::rows(&space()?.notifications()?, NOTIFICATIONS, as_json)?,
        Cmd::Notifications(Notifications::Get { id }) => out::json(&space()?.notification(&id)?)?,
        Cmd::Notifications(Notifications::Create { file }) => {
            let space = space()?;
            let (def, recipients) = definition(&file)?;
            let recipients = match recipients {
                Some(recipients) => recipients,
                None => vec![format!("@{}", space.me()?.id)],
            };
            out::json(&space.create_notification(&def, &refs(&recipients))?)?
        }
        Cmd::Notifications(Notifications::Edit { id, file }) => {
            let (def, recipients) = definition(&file)?;
            let named = recipients.as_deref().map(refs);
            out::json(&space()?.update_notification(&id, &def, named.as_deref())?)?
        }
        Cmd::Notifications(Notifications::Rm { id }) => {
            space()?.delete_notification(&id)?;
            eprintln!("notification {id} deleted, with its versions and its events")
        }
        Cmd::Notifications(Notifications::Events { id }) => out::json(&space()?.events(&id)?)?,
        Cmd::Notifications(Notifications::Subscribe { id }) => out::json(&space()?.subscribe(&id)?)?,
        Cmd::Notifications(Notifications::Unsubscribe { id }) => out::json(&space()?.unsubscribe(&id)?)?,
        Cmd::Notifications(Notifications::Test { id }) => out::json(&space()?.test_notification(&id)?)?,

        Cmd::Webhooks(Webhooks::Ls) => out::rows(&space()?.webhooks()?, WEBHOOKS, as_json)?,
        Cmd::Webhooks(Webhooks::Create { url }) => once("webhook signing secret", &space()?.create_webhook(&url)?),
        Cmd::Webhooks(Webhooks::Rm { id }) => {
            space()?.delete_webhook(&id)?;
            eprintln!("webhook {id} deleted")
        }
        Cmd::Webhook(Webhook::Set { url }) => {
            once("signing secret of this silicon's webhook", &space()?.set_silicon_webhook(&url)?)
        }
        Cmd::Webhook(Webhook::Rm) => {
            space()?.delete_silicon_webhook()?;
            eprintln!("this silicon's delivery webhook is gone")
        }

        Cmd::Keys(Keys::Ls) => out::rows(&space()?.api_keys()?, KEYS, as_json)?,
        Cmd::Keys(Keys::Create { scopes }) => once("api key", &space()?.create_api_key(&refs(&scopes))?),
        Cmd::Keys(Keys::Rm { id }) => {
            space()?.delete_api_key(&id)?;
            eprintln!("api key {id} deleted")
        }

        Cmd::Token(Token::Show) => {
            let token = space()?.access_token()?;
            let last = token.last_used_at.unwrap_or_else(|| "never".into());
            out::secret(&format!("the access token, last used {last}"), &token.token)
        }
        Cmd::Token(Token::Rotate) => {
            let token = space()?.rotate_access_token()?;
            out::secret("the new access token; the old one is already dead", &token.token)
        }

        Cmd::Query { sql } => out::json(&space()?.query(&sql, &Default::default())?)?,
        Cmd::Record { data, table_key } => {
            let key = setting(table_key, "SPACE_STATION_TABLE_KEY").ok_or_else(|| {
                Error::Local("a table key is required: --table-key or $SPACE_STATION_TABLE_KEY".into())
            })?;
            let data = if data == "-" { stdin()? } else { data };
            let value = serde_json::from_str::<Value>(&data)
                .map_err(|e| Error::Local(format!("record must be valid JSON: {e}")))?;
            let (errors, received) = std::sync::mpsc::channel();
            let client = SpaceClient::builder(&key)
                .url(&url)
                .home(home)
                .on_error(move |e| {
                    let _ = errors.send(e);
                })
                .build()?;
            client.record(value);
            let flushed = client.flush();
            let mut connection_error = None;
            for error in received.try_iter() {
                match error {
                    Error::Io(_) | Error::Ws(_) | Error::Transport(_) => connection_error = Some(error),
                    _ => return Err(error),
                }
            }
            if !flushed {
                return Err(connection_error.unwrap_or_else(|| {
                    Error::Transport(
                    "delivery was not confirmed before the timeout; the daemon will retry records saved in its spool"
                        .into(),
                )
                }));
            }
            eprintln!("record delivered to the server or saved by the running daemon");
        }
        Cmd::Errors => out::json(&space()?.dev_errors()?)?,
        Cmd::Daemon { cmd } => match cmd.unwrap_or(Daemon::Run) {
            Daemon::Run => daemon::run(daemon::Config { home: home.to_path_buf(), url: url.clone() })?,
            Daemon::Status => out::json(&daemon::status(home)?)?,
        },
    }
    Ok(())
}

/// The org a new session is bound to: `--org` (or $SPACE_STATION_ORG), else the one stored —
/// which outlives the session it was stored with.
fn bound(home: &Path, org: Option<String>) -> Result<String, Error> {
    let missing =
        || Error::Local("no org: a session is bound to one; pass --org <org> or set $SPACE_STATION_ORG".into());
    org.or_else(|| store::org(home)).ok_or_else(missing)
}

/// Store the session just minted with the org it is bound to, and say where the pages it unlocks
/// live. The link is a courtesy: signing in worked whether or not the app answers, so a failure
/// is named, not raised.
fn signed_in(home: &Path, url: &str, auth: Auth, org: String) -> Result<(), Error> {
    eprintln!("signed in to {org}: {}", auth.describe());
    store::update(home, |_| Ok(Stored { auth: auth.clone(), org: Some(org) }))?;
    match Space::new(url, auth)?.app_url() {
        Ok(app) => eprintln!("the app: {app}"),
        Err(e) => eprintln!("the app: unknown ({})", out::code(&e)),
    }
    Ok(())
}

/// The page that renders this window, on stderr — graphs, live views and video belong to the web
/// app, and this is how a terminal hands off to it. A courtesy too: the answer is already out.
fn page(space: &Space, id: &str) {
    match space.window_url(id) {
        Ok(link) => eprintln!("view: {link}"),
        Err(e) => eprintln!("view: unknown ({})", out::code(&e)),
    }
}

/// A secret the server will not show again, named by what the server called it.
fn once(what: &str, key: &Key) {
    let note = match &key.id {
        Some(id) => format!("the {what} for {id}, shown once"),
        None => format!("the {what}, shown once"),
    };
    out::secret(&note, &key.value)
}

/// All of stdin, trimmed: how a token arrives without touching the command line or a prompt.
fn stdin() -> Result<String, Error> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    match text.trim() {
        "" => Err(Error::Local("nothing on stdin".into())),
        token => Ok(token.to_string()),
    }
}

/// A notification file: `{"def": {…}, "recipients": [...]}`, or the definition alone as the docs
/// show it. Absent recipients means the creator on `create` and the current subscribers on `edit`.
fn definition(path: &Path) -> Result<(Def, Option<Vec<String>>), Error> {
    let at = |e: serde_json::Error| Error::Local(format!("{}: {e}", path.display()));
    let file: Value = serde_json::from_str(&read(path)?).map_err(at)?;
    let (def, recipients) = match file.get("def") {
        Some(def) => (def.clone(), file.get("recipients").cloned().unwrap_or(Value::Null)),
        None => (file, Value::Null),
    };
    let shape = |e: serde_json::Error| {
        let expected = "expected {def, recipients?}, or the definition {name, triggers, sql, …} itself";
        Error::Local(format!("{}: {e}; {expected}", path.display()))
    };
    Ok((serde_json::from_value(def).map_err(shape)?, serde_json::from_value(recipients).map_err(at)?))
}

fn read(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|e| Error::Local(format!("{}: {e}", path.display())))
}

/// `--access a,b` as the package takes it.
fn refs(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}
