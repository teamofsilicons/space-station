//! What the terminal sees. The CLI adds nothing to the package but this: lists become aligned
//! columns, everything else becomes JSON (pretty on a terminal, compact in a pipe), a one-time
//! secret goes to stdout alone with its warning on stderr, and a failure is `error: code:
//! message` — the code being the thing to branch on, here and in whatever reads this output —
//! with, when the server answered 401 whatever code it chose, what that means for the credential
//! this run acted with.

use std::io::{self, IsTerminal};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;
use space_station::Error;

/// The way in, said wherever nobody is signed in.
pub const SIGN_IN: &str = "run `spacestation login --org <org>`, or `spacestation auth <slt> --org <org>` with a \
                           short-lived token from the iam CLI";
/// A 401 on the stored session: it is over, and has just been forgotten.
pub const EXPIRED: &str = "the stored session is over and has been forgotten (its org is kept); run `spacestation \
                           login`, or `spacestation auth <slt>` with a short-lived token from the iam CLI";
/// A 401 with `--api-key`: no session to sign in to, only scopes to stay inside.
pub const KEY_REFUSED: &str = "an api key reaches only what its scopes allow — `tables`: tables ls, tables overview, \
                               query; `notifications`: notifications ls, get, events — and acts for no one, so \
                               nothing else answers it; `spacestation keys ls` as a signed-in user shows its scopes";
/// A 401 with `--access-token`.
pub const TOKEN_REFUSED: &str = "an access token stops resolving once it is rotated or its actor leaves the org; \
                                 `spacestation token show` as a signed-in user prints the live one";

/// A column: its header, and the key or dotted path (`def.name`) it reads from each row.
pub type Col = (&'static str, &'static str);

/// One value as JSON on stdout. Nothing at all for an empty answer, so `rm` prints nothing.
pub fn json(v: &impl Serialize) -> Result<(), Error> {
    let text = render(&value(v)?, io::stdout().is_terminal());
    if !text.is_empty() {
        println!("{text}");
    }
    Ok(())
}

/// A list as aligned columns, or as its JSON when `--json` asked for it.
pub fn rows(v: &impl Serialize, cols: &[Col], as_json: bool) -> Result<(), Error> {
    if as_json {
        return json(v);
    }
    print!("{}", table(&value(v)?, cols));
    Ok(())
}

/// A secret goes to stdout alone, so `> key.txt` captures just it; the note goes to stderr.
pub fn secret(note: &str, value: &str) {
    eprintln!("{note}; treat it like a password");
    println!("{value}");
}

/// Hand a link to the desktop's browser; `false` when there is nothing to hand it to.
pub fn open(link: &str) -> bool {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(link).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
}

/// What a failure reads like on stderr: `error: <code>: <message>`, plus `refused` — what a 401
/// means for the credential in use — when the server refused the credential itself. A 401 is
/// final, since the backend holds the session and nothing here can mend one.
pub fn fail(e: &Error, refused: &str) -> String {
    let code = code(e);
    let message = match e {
        Error::Api { message, .. } | Error::Local(message) | Error::Transport(message) | Error::Ws(message) => {
            message.clone()
        }
        Error::Io(e) => e.to_string(),
        other => other.to_string(),
    };
    let how = match e {
        Error::Api { status: 401, .. } => format!("\n{refused}"),
        _ => String::new(),
    };
    format!("error: {code}: {message}{how}")
}

/// The snake_case code of a failure: the server's own where there is one, and the kind of
/// failure where there is not.
pub fn code(e: &Error) -> &str {
    match e {
        Error::Api { code, .. } => code,
        Error::Local(_) => "local",
        Error::Transport(_) => "transport",
        Error::Io(_) => "io",
        Error::Ws(_) => "websocket",
        Error::InvalidKey => "invalid_key",
        Error::SizeExceeded { .. } => "size_exceeded",
        Error::QueueFull => "queue_full",
        Error::Rejected { .. } => "rejected",
    }
}

/// How a JSON value prints: indented for a human, one line for a pipe, empty for `null`.
pub fn render(v: &Value, tty: bool) -> String {
    match (v, tty) {
        (Value::Null, _) => String::new(),
        (v, true) => serde_json::to_string_pretty(v).unwrap_or_default(),
        (v, false) => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Rows of objects as aligned columns under their headers; a value that is not there reads `-`.
pub fn table(rows: &Value, cols: &[Col]) -> String {
    let rows = rows.as_array().map(Vec::as_slice).unwrap_or_default();
    let header = cols.iter().map(|(label, _)| label.to_string()).collect::<Vec<_>>();
    let lines: Vec<Vec<String>> = std::iter::once(header)
        .chain(
            rows.iter().map(|row| cols.iter().map(|(_, path)| cell(path.split('.').fold(row, |v, k| &v[k]))).collect()),
        )
        .collect();
    let widths: Vec<usize> = (0..cols.len()).map(|i| lines.iter().map(|l| l[i].len()).max().unwrap_or(0)).collect();
    let mut out = String::new();
    for line in lines {
        let padded: Vec<String> = line.iter().zip(&widths).map(|(c, w)| format!("{c:<w$}")).collect();
        out.push_str(padded.join("  ").trim_end());
        out.push('\n');
    }
    out
}

/// A window, or a list of them, without the code of its version: what `get`, `ls`, `create` and
/// `edit` answer, the code being `windows code`'s to print.
pub fn summary(v: &impl Serialize) -> Result<Value, Error> {
    let mut v = value(v)?;
    let windows = match &mut v {
        Value::Array(items) => items.iter_mut().collect(),
        one => vec![one],
    };
    for window in windows {
        if let Some(version) = window.get_mut("version").and_then(Value::as_object_mut) {
            version.remove("processor");
            version.remove("renderer");
        }
    }
    Ok(v)
}

/// How a JSON value reads in a column: strings bare, lists comma-joined, missing as `-`.
fn cell(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(cell).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

/// A typed answer as JSON to print. Only a value the package could not have sent can fail here.
fn value(v: &impl Serialize) -> Result<Value, Error> {
    serde_json::to_value(v).map_err(|e| Error::Io(e.into()))
}
