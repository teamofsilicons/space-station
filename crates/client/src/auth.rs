//! Who you are, as a value. `Auth` holds one credential and nothing about where it came from or
//! where it goes: the `sscli-` session the backend minted for a terminal, a `spacewindow-` access
//! token or an `apikey-` key. It reads no file, no environment and no browser; the caller builds
//! it and may serialize it verbatim. No IAM credential has a place here: a person or a silicon
//! hands Space Station a *short-lived token*, the backend exchanges it once at IAM and holds the
//! Application session it gets back, so nothing on this side ever rotates. Two things touch the
//! network, and both return an `Auth::session`: [`exchange`], which spends a short-lived token,
//! and [`login`], the loopback half of a browser sign-in, which catches one and spends it the
//! same way.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

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
#[derive(Clone, Serialize, Deserialize)]
pub struct Auth {
    bearer: String,
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

    /// An `apikey-` key, acting for the org inside its scopes.
    pub fn api_key(key: &str) -> Auth {
        Self::bearer(key)
    }

    fn bearer(bearer: &str) -> Auth {
        Auth { bearer: bearer.trim().into() }
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

/// A short-lived token, spent. `slt` is what `iam login --app-id` or `iam silicon-login --app-id`
/// printed — two minutes old at most and good for one exchange — and `org` the organization the
/// session is bound to. `POST {url}/api/auth/session {slt, org}` has the backend exchange it at
/// IAM, open the session and answer `{token: sscli-…}`, which comes back as `Auth::session`.
/// Anything shaped like a credential is refused before it leaves the machine: Space Station never
/// takes one, only a token minted to be handed over.
pub fn exchange(url: &str, slt: &str, org: &str) -> Result<Auth, Error> {
    let slt = slt.trim();
    if slt.is_empty() || secrets::find_secret(slt).is_some() {
        let why = "not a short-lived token: `iam login --app-id` or `iam silicon-login --app-id` prints one; \
                   a credential is never handed over";
        return Err(Error::Local(why.into()));
    }
    let body = json!({"slt": slt, "org": org});
    let minted: Minted = api::call(&api::agent(), &api::origin(url)?, "POST", "/auth/session", None, Some(&body))?;
    Ok(Auth::session(&minted.token))
}

/// `{"token": "sscli-…"}`, the answer to an exchange.
#[derive(Deserialize)]
struct Minted {
    token: String,
}

/// A carbon, through the browser (RFC 8252 loopback). Binds `127.0.0.1:0`, hands
/// `{url}/api/auth/login?org={org}&cli={port}&state={nonce}` to `visit` — which opens or prints
/// the link and returns at once — then serves the one request the backend redirects there with
/// `?slt=…&state=…`, checks the state, spends the short-lived token through [`exchange`], answers
/// a small page and returns the `sscli-` session, bound to `org`. Nothing is stored.
pub fn login(url: &str, org: &str, visit: impl FnOnce(&str)) -> Result<Auth, Error> {
    let origin = api::origin(url)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let nonce = Uuid::new_v4().simple().to_string();
    visit(&format!("{origin}/api/auth/login?org={org}&cli={port}&state={nonce}"));
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(REDIRECT_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let query = line.split(' ').nth(1).and_then(|t| t.split_once('?')).map(|(_, q)| q).unwrap_or_default();
    let outcome = match (param(query, "slt"), param(query, "state")) {
        (_, state) if state != Some(nonce.as_str()) => {
            Err(Error::Local("the sign-in reply carried another state".into()))
        }
        (Some(slt), _) => exchange(&origin, slt, org),
        (None, _) if param(query, "token").is_some() => Err(Error::Local(
            "the sign-in reply carried a session token instead of a short-lived one: this Space Station is older \
             than this client and its session was not accepted"
                .into(),
        )),
        (None, _) => Err(Error::Local("the sign-in reply carried no short-lived token".into())),
    };
    match &outcome {
        Ok(_) => answer(&mut stream, "Signed in", "You can close this tab and go back to the terminal."),
        Err(e) => answer(&mut stream, "Not signed in", &format!("{e}. Start again from the terminal.")),
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
