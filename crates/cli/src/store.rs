//! The state this command owns and the package does not: `<home>/auth.json`, holding the one
//! signed-in credential and the org it is bound to (or the one `use` chose). 0700 home, 0600
//! file, written whole through a tmp file and a rename, and every read-modify-write under `flock`
//! on a sibling lock file, so two commands storing at once never lose each other's write. A
//! credential the server has refused for good is dropped from the file and its org kept, so the
//! next command says "not signed in" and the next sign-in needs no `--org`.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::{fs, io};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use space_station::{Auth, Error};

use crate::out;

/// `auth.json`: the credential, exactly as the package serializes it, plus its org.
#[derive(Serialize, Deserialize)]
pub struct Stored {
    #[serde(flatten)]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
}

/// What to say when nothing is stored.
pub fn not_signed_in() -> Error {
    Error::Local(format!("not signed in: {}", out::SIGN_IN))
}

/// Whatever was last stored.
pub fn load(home: &Path) -> Result<Stored, Error> {
    read(home).ok_or_else(not_signed_in)
}

/// The org stored beside the credential — or left behind by `expire` — for the next sign-in.
pub fn org(home: &Path) -> Option<String> {
    let file: Value = serde_json::from_slice(&fs::read(home.join("auth.json")).ok()?).ok()?;
    file.get("org")?.as_str().map(String::from)
}

/// Replace the file with what `f` makes of its current content, under the lock.
pub fn update(home: &Path, f: impl FnOnce(Option<Stored>) -> Result<Stored, Error>) -> Result<(), Error> {
    let _lock = lock(home)?;
    write(home, &f(read(home))?)
}

/// The server refused the stored credential for good: drop it and keep the org.
pub fn expire(home: &Path) -> Result<(), Error> {
    let _lock = lock(home)?;
    write(home, &org(home).map_or(json!({}), |org| json!({"org": org})))
}

/// Delete the stored credential; nothing stored is nothing to delete.
pub fn forget(home: &Path) -> Result<(), Error> {
    match fs::remove_file(home.join("auth.json")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(Error::Io(e)),
        _ => Ok(()),
    }
}

fn read(home: &Path) -> Option<Stored> {
    serde_json::from_slice(&fs::read(home.join("auth.json")).ok()?).ok()
}

/// `auth.json` replaced whole: written to a sibling tmp file, then renamed over.
fn write(home: &Path, stored: &impl Serialize) -> Result<(), Error> {
    let (tmp, path) = (home.join("auth.json.tmp"), home.join("auth.json"));
    let go = || -> io::Result<()> {
        let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(&serde_json::to_vec(stored)?)?;
        fs::rename(&tmp, &path)
    };
    go().map_err(|e| local(e, &path))
}

/// `flock(LOCK_EX)` on `<home>/auth.lock`, held until the file is dropped. A sibling file, not
/// `auth.json` itself: tmp + rename gives `auth.json` a new inode on every write. The home is
/// made 0700 here as the daemon does, because this is where the credential is about to live.
fn lock(home: &Path) -> Result<fs::File, Error> {
    let private = |()| fs::set_permissions(home, fs::Permissions::from_mode(0o700));
    fs::create_dir_all(home).and_then(private).map_err(|e| local(e, home))?;
    let path = home.join("auth.lock");
    let file = fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path);
    let file = file.map_err(|e| local(e, &path))?;
    // SAFETY: `flock` only reads the descriptor and the flags; the descriptor is open and ours.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(local(io::Error::last_os_error(), &path));
    }
    Ok(file)
}

fn local(e: io::Error, path: &Path) -> Error {
    Error::Local(format!("{}: {e}", path.display()))
}
