//! Account authentication through Silicon Accounts. The server owns and refreshes the upstream
//! session; this client holds only the stable Space Station session and its absolute expiry.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::shared::secrets;
use crate::{Error, api};

/// How long the loopback listener waits for a connected browser to finish its request.
const REDIRECT_TIMEOUT: Duration = Duration::from_secs(10);
/// What marks a session secret as a terminal's, exactly as the backend mints it.
const SESSION: &str = "sscli-";

/// One credential. Its prefix says what it is.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    bearer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at_unix: Option<i64>,
}

impl Auth {
    /// A carbon or a silicon signed in from a terminal: the `sscli-` session the backend minted,
    /// holds and refreshes. [`login`] and [`exchange`] are the two ways to get one.
    pub fn session(sscli: &str) -> Auth {
        Self::bearer(sscli)
    }

    /// A `spacewindow-` access token, as processors and dev servers use.
    pub fn access_token(token: &str) -> Auth {
        Self::bearer(token)
    }

    /// An `apikey-` key, acting for the account inside its scopes.
    pub fn api_key(key: &str) -> Auth {
        Self::bearer(key)
    }

    fn bearer(bearer: &str) -> Auth {
        Auth { bearer: bearer.trim().into(), expires_at: None, expires_at_unix: None }
    }

    /// The server session's absolute expiration, when this credential came from sign-in.
    pub fn expires_at(&self) -> Option<&str> {
        self.expires_at.as_deref()
    }

    /// Whether the server-issued absolute session lifetime has ended, without a network call.
    pub fn is_expired(&self) -> bool {
        self.expires_at_unix.is_some_and(|expires| {
            SystemTime::now().duration_since(UNIX_EPOCH).is_ok_and(|now| expires <= now.as_secs() as i64)
        })
    }

    /// What this is, in words, revealing none of it.
    pub fn describe(&self) -> &'static str {
        match self.bearer.split(['-', '_']).next() {
            Some("sscli") => "a session",
            Some("spacewindow") => "an access token",
            Some("apikey") => "an API key",
            _ => "a bearer token",
        }
    }

    /// The `Authorization: Bearer` value.
    pub(crate) fn token(&self) -> &str {
        &self.bearer
    }

    /// Whether the backend holds a session row for this, which `logout` can end.
    pub(crate) fn is_session(&self) -> bool {
        self.bearer.starts_with(SESSION)
    }
}

/// Printing an `Auth` prints what it is, never what it holds.
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Auth({})", self.describe())
    }
}

/// Spend an app-bound short-lived token from `silicon-accounts login --app spacestation -q`.
/// The server exchanges it with Accounts; a Silicon's STK never goes to Space Station.
pub fn exchange(url: &str, slt: &str) -> Result<Auth, Error> {
    let slt = slt.trim();
    if !slt.starts_with("slt_") || slt.len() <= 4 || secrets::find_secret(slt).is_some() {
        return Err(Error::Local("expected an slt_ token from `silicon-accounts login --app spacestation -q`".into()));
    }
    session(url, json!({"slt": slt}))
}

fn session(url: &str, body: serde_json::Value) -> Result<Auth, Error> {
    let minted: Minted = api::call(&api::agent(), &api::origin(url)?, "POST", "/auth/session", None, Some(&body))?;
    Ok(Auth { bearer: minted.session_token, expires_at: minted.expires_at, expires_at_unix: minted.expires_at_unix })
}

#[derive(Deserialize)]
struct Minted {
    #[serde(alias = "token")]
    session_token: String,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    expires_at_unix: Option<i64>,
}

/// Sign in a carbon through the browser using a loopback callback. The server exchanges the
/// Accounts authorization code with PKCE and redirects a one-time handoff to this listener.
/// Nothing is stored; callers persist the returned session until expiry or explicit logout.
pub fn login(url: &str, visit: impl FnOnce(&str)) -> Result<Auth, Error> {
    let origin = api::origin(url)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let nonce = Uuid::new_v4().simple().to_string();
    visit(&format!("{origin}/api/auth/login?cli={port}&state={nonce}"));
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(REDIRECT_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let query = line.split(' ').nth(1).and_then(|t| t.split_once('?')).map(|(_, q)| q).unwrap_or_default();
    let outcome = match (param(query, "code"), param(query, "state")) {
        (_, state) if state != Some(nonce.as_str()) => {
            Err(Error::Local("the sign-in reply carried another state".into()))
        }
        (Some(code), _) => session(&origin, json!({"code": code, "state": nonce})),
        (None, _) => Err(Error::Local("the sign-in reply carried no one-time code".into())),
    };
    match &outcome {
        Ok(_) => answer(&mut stream, "Signed in", "You can close this tab and go back to the terminal."),
        Err(_) => answer(&mut stream, "Not signed in", "The sign-in could not finish. Start again from the terminal."),
    }
    outcome
}

/// One query parameter. Both halves of the login redirect are nonces or URL-safe tokens, so
/// nothing needs decoding.
fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

/// One small page, and then the socket closes: the browser's half of the flow is over.
fn answer(stream: &mut TcpStream, heading: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Space Station</title>\
         <body style=\"font:16px/1.5 system-ui;margin:5rem auto;max-width:28rem\"><h1>{heading}</h1><p>{message}</p>"
    );
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n";
    let _ = write!(stream, "{head}Content-Length: {}\r\n\r\n{body}", body.len());
}
