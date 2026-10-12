//! The machine's one ingest daemon: accepts lines from any number of `SpaceClient`s over a unix
//! socket, spools them, and ships the oldest unacked lines as one in-flight batch over a
//! WebSocket. Whoever holds `<home>/daemon.lock` is
//! the daemon; a `SpaceClient` runs this in-process when nobody does, and then its `flush` waits
//! on the same spool for the server's acks. The socket is `<home>/daemon.sock` when that fits a
//! socket address, else a short path under the temp dir ([`socket_path`]); the spool and the lock
//! always live in the home.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::net::UnixListener;
#[cfg(unix)]
pub(crate) use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use std::{fmt, thread};
#[cfg(windows)]
use uds_windows::UnixListener;
#[cfg(windows)]
pub(crate) use uds_windows::UnixStream;

use space_station_shared::limits::BATCH_MAX;
use space_station_shared::secrets::sha256_hex;
use space_station_shared::wire::{Ack, Code, Entry};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Bytes, Message, WebSocket, stream::MaybeTlsStream};

use crate::spool::{Batch, Spool};
use crate::{Error, Hook, lock};

const PING_EVERY: Duration = Duration::from_secs(20);
const ACK_TIMEOUT: Duration = Duration::from_secs(30);
/// How often an idle send loop looks at the spool; also the socket read timeout.
const POLL: Duration = Duration::from_millis(50);
/// A unix socket path must be shorter than `sun_path`: 108 bytes on Linux, 104 on macOS and the
/// BSDs. The daemon keeps its socket under the smaller of the two so one home works everywhere.
pub const SOCKET_PATH_MAX: usize = 104;

pub struct Config {
    pub home: PathBuf,
    pub url: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { home: crate::default_home(), url: crate::default_url() }
    }
}

/// What a daemon says as it works — up, connected, sent, acked, reconnecting. The foreground
/// daemon prints it; the one inside a `SpaceClient` keeps quiet, because a library does not write
/// to its host's stderr.
pub(crate) type Log = Arc<dyn Fn(fmt::Arguments<'_>) + Send + Sync>;

/// The spool and the signal fired after every ack, shared by the threads of one daemon — and,
/// when that daemon runs inside a `SpaceClient`'s process, by that client's `flush`.
pub(crate) struct Shared {
    pub spool: Mutex<Spool>,
    acked: Condvar,
}

impl Shared {
    pub fn open(home: &Path) -> io::Result<Arc<Shared>> {
        Ok(Arc::new(Shared { spool: Mutex::new(Spool::open(home)?), acked: Condvar::new() }))
    }

    /// Whether every line in the spool is acked by `deadline`.
    pub fn wait_acked(&self, deadline: Instant) -> bool {
        let mut spool = lock(&self.spool);
        while spool.pending() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
            spool = self.acked.wait_timeout(spool, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
        true
    }

    fn ack(&self, batch: &Batch) -> io::Result<()> {
        let acked = lock(&self.spool).ack(batch);
        self.acked.notify_all();
        acked
    }
}

/// Where the daemon of `home` listens: `<home>/daemon.sock` when that fits a socket address, else
/// `<temp dir>/space-station-<hash of home>.sock`, so a long `SPACE_STATION_HOME` (or `SILICON_HOME`) costs nothing
/// but the socket's location. Processes sharing a home share a temp dir, as one user's do.
pub fn socket_path(home: &Path) -> PathBuf {
    let path = home.join("daemon.sock");
    if path.as_os_str().len() < SOCKET_PATH_MAX {
        return path;
    }
    let hash = &sha256_hex(&home.to_string_lossy())[..16];
    std::env::temp_dir().join(format!("space-station-{hash}.sock"))
}

/// `e`, naming the path and the limit when the path itself is what the OS refused.
fn named(path: &Path, e: io::Error) -> io::Error {
    match e.kind() {
        io::ErrorKind::InvalidInput => {
            let len = path.as_os_str().len();
            let why =
                format!("{}: {e} ({len} bytes; a unix socket path must be under {SOCKET_PATH_MAX})", path.display());
            io::Error::new(e.kind(), why)
        }
        _ => e,
    }
}

/// A client's end of the daemon's socket.
pub(crate) fn connect(home: &Path) -> io::Result<UnixStream> {
    let path = socket_path(home);
    UnixStream::connect(&path).map_err(|e| named(&path, e))
}

/// Whether a daemon is listening on this machine's socket, and how many spooled records it has
/// not had acked: the highest seq in the spool minus the acked cursor. Answers without a
/// credential and without a network, because both questions are about this machine alone.
pub fn status(home: &Path) -> Result<crate::DaemonStatus, Error> {
    #[derive(serde::Deserialize)]
    struct Seq {
        seq: u64,
    }
    let read = |name: &str| fs::read_to_string(home.join(name));
    let cursor: u64 = read("spool.cursor").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let last = match File::open(home.join("spool.jsonl")) {
        Ok(file) => BufReader::new(file).lines().map_while(Result::ok).filter(|l| !l.is_empty()).last(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::Io(e)),
    };
    let last = last.and_then(|l| serde_json::from_str::<Seq>(&l).ok()).map_or(cursor, |l| l.seq);
    let running = connect(home).is_ok();
    let (home, socket) = (home.to_path_buf(), socket_path(home));
    Ok(crate::DaemonStatus { running, unacked: last.saturating_sub(cursor), home, socket })
}

/// Run the daemon in the foreground until the process ends, logging every event to stderr.
/// Fails when another daemon holds the lock or the home is unusable.
pub fn run(config: Config) -> Result<(), Error> {
    let lock = try_lock(&config.home)?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::AddrInUse, format!("another daemon holds {}/daemon.lock", config.home.display()))
    })?;
    let listener = bind(&config.home)?;
    let shared = Shared::open(&config.home)?;
    let on_error: Hook = Arc::new(|e: Error| eprintln!("spacestation daemon: {e}"));
    let log: Log = Arc::new(|what| eprintln!("spacestation daemon: {what}"));
    serve(config, on_error, log, lock, listener, shared);
    Ok(())
}

/// Create a private home and take the exclusive OS file lock: `None` when someone else
/// holds it. The lock is released automatically when its file is dropped or the process exits.
pub(crate) fn try_lock(home: &Path) -> io::Result<Option<File>> {
    crate::local::private_dir(home)?;
    let file = crate::local::open_private(
        &home.join("daemon.lock"),
        OpenOptions::new().write(true).create(true).truncate(false),
    )?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Unlink a stale socket, bind the daemon's socket, make it 0600. Only the lock holder calls this.
pub(crate) fn bind(home: &Path) -> io::Result<UnixListener> {
    let path = socket_path(home);
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| named(&path, e))?;
    crate::local::private(&path, false)?;
    Ok(listener)
}

/// Accept clients forever, one thread each, while another thread ships the spool. `_lock` is
/// held until the process ends.
pub(crate) fn serve(
    config: Config,
    on_error: Hook,
    log: Log,
    _lock: File,
    listener: UnixListener,
    shared: Arc<Shared>,
) {
    {
        let s = lock(&shared.spool);
        let (home, cursor, skipped) = (config.home.display(), s.cursor(), s.skipped);
        log(format_args!("up at {home}, cursor {cursor}, {skipped} unparseable lines skipped"));
    }
    let (shipping, hook, log) = (shared.clone(), on_error.clone(), log.clone());
    thread::spawn(move || ship(&shipping, &config, &hook, &log));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let (shared, hook) = (shared.clone(), on_error.clone());
                thread::spawn(move || receive(stream, &shared.spool, &hook));
            }
            Err(e) => on_error(Error::Io(e)),
        }
    }
}

/// One client: every complete line is an `Entry`; the loop ends when the client hangs up.
fn receive(stream: UnixStream, spool: &Mutex<Spool>, on_error: &Hook) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let appended = match serde_json::from_str::<Entry>(&line) {
            Ok(entry) => lock(spool).append(&entry).map(drop),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("unreadable line from a client: {e}"))),
        };
        if let Err(e) = appended {
            on_error(Error::Io(e));
        }
    }
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

/// Connect, ship until the socket fails, reconnect with backoff 1 s → 30 s. Forever.
fn ship(shared: &Shared, config: &Config, on_error: &Hook, log: &Log) {
    let url = ws_url(&config.url);
    let mut backoff = Duration::from_secs(1);
    loop {
        let error = match connect_ws(&url) {
            Ok(mut ws) => {
                log(format_args!("connected to {url}"));
                backoff = Duration::from_secs(1);
                session(&mut ws, shared, on_error, log)
            }
            Err(e) => e,
        };
        on_error(error);
        log(format_args!("reconnecting in {backoff:?}"));
        thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// `https://host` → `wss://host/api/ws/ingest`; a `ws(s)://` base only gets the path.
fn ws_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = base.strip_prefix("http").map_or_else(|| base.to_string(), |rest| format!("ws{rest}"));
    format!("{base}/api/ws/ingest")
}

/// TCP with a 10 s connect timeout, then TLS and the WebSocket handshake; afterwards the socket
/// reads time out every `POLL` so the send loop can watch the spool between frames. Every address
/// the host resolves to is tried in turn — a dual-stack `localhost` hands out `::1` first and an
/// IPv4-only server is behind it — and the error that ends the last attempt names its address.
fn connect_ws(url: &str) -> Result<Ws, Error> {
    let uri: tungstenite::http::Uri = url.parse().map_err(|e| Error::Ws(format!("{url}: {e}")))?;
    let host = uri.host().ok_or_else(|| Error::Ws(format!("{url}: no host")))?.trim_matches(['[', ']']);
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
    let mut refused = None;
    let tcp = (host, port)
        .to_socket_addrs()?
        .find_map(|addr| {
            TcpStream::connect_timeout(&addr, Duration::from_secs(10))
                .map_err(|e| refused = Some(io::Error::new(e.kind(), format!("{addr}: {e}"))))
                .ok()
        })
        .ok_or_else(|| refused.map_or_else(|| Error::Ws(format!("{host} did not resolve")), Error::Io))?;
    tcp.set_read_timeout(Some(ACK_TIMEOUT))?;
    tcp.set_write_timeout(Some(ACK_TIMEOUT))?;
    let socket = tcp.try_clone()?;
    let mut request = url.into_client_request().map_err(|e| Error::Ws(e.to_string()))?;
    // The production gateway requires a User-Agent; omit versions and device information.
    request
        .headers_mut()
        .insert(tungstenite::http::header::USER_AGENT, tungstenite::http::HeaderValue::from_static("space-station"));
    let (ws, _) = tungstenite::client_tls(request, tcp).map_err(|e| Error::Ws(e.to_string()))?;
    socket.set_read_timeout(Some(POLL))?;
    Ok(ws)
}

/// One connection: one batch in flight, acked within 30 s; a ping every 20 s whose pong must
/// arrive before the next one. Returns the error that ended it.
fn session(ws: &mut Ws, shared: &Shared, on_error: &Hook, log: &Log) -> Error {
    let mut in_flight: Option<(Batch, Instant)> = None;
    let (mut last_ping, mut awaiting_pong) = (Instant::now(), false);
    loop {
        if in_flight.is_none() && lock(&shared.spool).pending() {
            match lock(&shared.spool).batch(BATCH_MAX) {
                Err(e) => return Error::Io(e),
                Ok(None) => {}
                Ok(Some(mut batch)) => {
                    batch.errors.drain(..).for_each(|e| on_error(e));
                    if batch.count == 0 {
                        if let Err(e) = shared.ack(&batch) {
                            return Error::Io(e);
                        }
                        continue;
                    }
                    if let Err(e) = ws.send(Message::text(std::mem::take(&mut batch.frame))) {
                        return Error::Ws(e.to_string());
                    }
                    log(format_args!("sent batch {} with {} records", batch.id, batch.count));
                    in_flight = Some((batch, Instant::now()));
                }
            }
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                let Ok(ack) = serde_json::from_str::<Ack>(&text) else {
                    return Error::Ws(format!("unreadable ack: {text}"));
                };
                match in_flight.take() {
                    Some((batch, _)) if batch.id == ack.batch_id => {
                        if let Err(e) = settle(&batch, ack, shared, on_error, log) {
                            return Error::Io(e);
                        }
                    }
                    other => in_flight = other,
                }
            }
            Ok(Message::Pong(_)) => awaiting_pong = false,
            Ok(Message::Close(_)) => return Error::Ws("server closed the connection".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Error::Ws(e.to_string()),
        }
        if let Some((batch, sent)) = &in_flight
            && sent.elapsed() > ACK_TIMEOUT
        {
            return Error::Ws(format!("no ack for batch {} within {ACK_TIMEOUT:?}", batch.id));
        }
        if last_ping.elapsed() > PING_EVERY {
            if awaiting_pong {
                return Error::Ws("missed pong".into());
            }
            if let Err(e) = ws.send(Message::Ping(Bytes::new())) {
                return Error::Ws(e.to_string());
            }
            (last_ping, awaiting_pong) = (Instant::now(), true);
        }
    }
}

/// Apply an ack: duplicates are silently accepted, every other rejection is reported, and the
/// cursor moves past the whole batch either way. A frame-level code cannot be fixed by resending.
fn settle(batch: &Batch, ack: Ack, shared: &Shared, on_error: &Hook, log: &Log) -> io::Result<()> {
    if let Some(code) = ack.code {
        on_error(Error::Ws(format!("batch {} refused as {code:?}, its {} records are dropped", batch.id, batch.count)));
    }
    let rejected = ack.rejected.len();
    for r in ack.rejected {
        if r.code != Code::Duplicate {
            on_error(Error::Rejected { record_id: r.record_id, code: r.code, reason: r.reason });
        }
    }
    log(format_args!("ack {}: {} accepted, {rejected} rejected", batch.id, batch.count.saturating_sub(rejected)));
    shared.ack(batch)
}

#[cfg(test)]
mod tests {
    use super::connect_ws;
    use std::net::TcpListener;
    use std::thread;
    use tungstenite::handshake::server::{Request, Response};

    #[test]
    fn websocket_handshake_supplies_generic_user_agent_for_gateway_compatibility() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            tungstenite::accept_hdr(stream, |request: &Request, response: Response| {
                assert_eq!(request.headers().get("user-agent").unwrap(), "space-station");
                Ok(response)
            })
            .unwrap();
        });

        connect_ws(&format!("ws://{addr}/api/ws/ingest")).unwrap();
        server.join().unwrap();
    }
}
