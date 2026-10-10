//! Private, atomically replaced account sessions. Reads and writes share an OS file lock;
//! transient server or network errors never delete the stored credential.

use std::io::Write;
use std::path::Path;
use std::{fs, io};

use serde::{Deserialize, Serialize};
use space_station::{Auth, Error, Identity};

use crate::out;

/// `auth.json`: the credential, its expiry, and cached account identity.
#[derive(Clone, Serialize, Deserialize)]
pub struct Stored {
    #[serde(flatten)]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// What to say when nothing is stored.
pub fn not_signed_in() -> Error {
    Error::Local(format!("not signed in: {}", out::SIGN_IN))
}

/// Whatever was last stored.
pub fn load(home: &Path) -> Result<Stored, Error> {
    read(home).ok_or_else(not_signed_in)
}

/// Replace the file with what `f` makes of its current content, under the lock.
pub fn update(home: &Path, f: impl FnOnce(Option<Stored>) -> Result<Stored, Error>) -> Result<(), Error> {
    let _lock = lock(home)?;
    write(home, &f(read(home))?)
}

/// Delete the stored credential; nothing stored is nothing to delete.
pub fn forget(home: &Path, expected: Option<&Stored>) -> Result<(), Error> {
    let _lock = lock(home)?;
    if expected.is_some_and(|expected| read(home).is_none_or(|current| current.auth != expected.auth)) {
        return Ok(());
    }
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
        let mut file =
            space_station::local::open_private(&tmp, fs::OpenOptions::new().write(true).create(true).truncate(true))?;
        file.write_all(&serde_json::to_vec(stored)?)?;
        drop(file);
        fs::rename(&tmp, &path)
    };
    go().map_err(|e| local(e, &path))
}

/// An exclusive OS lock on `auth.lock`, released when its file is dropped. Keep it separate
/// from `auth.json`, which is replaced on each write.
fn lock(home: &Path) -> Result<fs::File, Error> {
    space_station::local::private_dir(home).map_err(|e| local(e, home))?;
    let path = home.join("auth.lock");
    let file =
        space_station::local::open_private(&path, fs::OpenOptions::new().write(true).create(true).truncate(false))
            .map_err(|e| local(e, &path))?;
    file.lock().map_err(|e| local(e, &path))?;
    Ok(file)
}

fn local(e: io::Error, path: &Path) -> Error {
    Error::Local(format!("{}: {e}", path.display()))
}
