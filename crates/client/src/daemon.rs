//! The machine's one ingest daemon: accepts lines from any number of `SpaceClient`s over a unix
//! socket, spools them, and ships the oldest unacked lines as one in-flight batch over a
//! WebSocket, stamping the sampled metadata on the way out. Whoever holds `<home>/daemon.lock` is
//! the daemon; a `SpaceClient` runs this in-process when nobody does, and then its `flush` waits
//! on the same spool for the server's acks. The socket is `<home>/daemon.sock` when that fits a
//! socket address, else a short path under the temp dir ([`socket_path`]); the spool and the lock
//! always live in the home.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use std::{fmt, thread};

use space_station_shared::limits::BATCH_MAX;
use space_station_shared::secrets::sha256_hex;
use space_station_shared::wire::{Ack, Code, Entry, Metadata, System};
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
/// `<temp dir>/space-station-<hash of home>.sock`, so a long `SPACE_STATION_HOME` costs nothing
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

/// Create `<home>` (0700) and take `flock(LOCK_EX | LOCK_NB)` on `<home>/daemon.lock`: `None`
/// when someone else holds it. The lock lives as long as the returned file.
pub(crate) fn try_lock(home: &Path) -> io::Result<Option<File>> {
    fs::create_dir_all(home)?;
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
    let file =
        OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(home.join("daemon.lock"))?;
    // SAFETY: flock takes an open descriptor and flags; no memory is handed over.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock { Ok(None) } else { Err(err) }
}

/// Unlink a stale socket, bind the daemon's socket, make it 0600. Only the lock holder calls this.
pub(crate) fn bind(home: &Path) -> io::Result<UnixListener> {
    let path = socket_path(home);
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| named(&path, e))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
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
    let mut sampler = Sampler::new(&config.home);
    let mut backoff = Duration::from_secs(1);
    loop {
        let error = match connect_ws(&url) {
            Ok(mut ws) => {
                log(format_args!("connected to {url}"));
                backoff = Duration::from_secs(1);
                session(&mut ws, shared, &mut sampler, on_error, log)
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
    request.headers_mut().insert(
        tungstenite::http::header::USER_AGENT,
        concat!("space-station/", env!("CARGO_PKG_VERSION")).parse().expect("a package version is ASCII"),
    );
    let (ws, _) = tungstenite::client_tls(request, tcp).map_err(|e| Error::Ws(e.to_string()))?;
    socket.set_read_timeout(Some(POLL))?;
    Ok(ws)
}

/// One connection: one batch in flight, acked within 30 s; a ping every 20 s whose pong must
/// arrive before the next one. Returns the error that ended it.
fn session(ws: &mut Ws, shared: &Shared, sampler: &mut Sampler, on_error: &Hook, log: &Log) -> Error {
    let mut in_flight: Option<(Batch, Instant)> = None;
    let (mut last_ping, mut awaiting_pong) = (Instant::now(), false);
    loop {
        if in_flight.is_none() && lock(&shared.spool).pending() {
            sampler.refresh();
            match lock(&shared.spool).batch(BATCH_MAX, |m| sampler.stamp(m)) {
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

/// Metadata sampled at send time: the machine once, cpu/ram/disk/gpu at most once a second.
struct Sampler {
    sys: sysinfo::System,
    disks: sysinfo::Disks,
    /// Index in `disks` of the mount holding the home directory.
    disk: Option<usize>,
    nvidia: bool,
    system: System,
    sampled: Instant,
    cpu_pct: Option<f32>,
    gpu_pct: Option<f32>,
    ram_pct: Option<f32>,
    disk_free_mb: Option<u64>,
}

impl Sampler {
    /// Reads the machine and takes the first sample, which costs one `MINIMUM_CPU_UPDATE_INTERVAL`
    /// because CPU usage is a delta between two refreshes.
    fn new(home: &Path) -> Sampler {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let home = home.canonicalize().unwrap_or_default();
        let disk = disks
            .list()
            .iter()
            .enumerate()
            .filter(|(_, d)| home.starts_with(d.mount_point()))
            .max_by_key(|(_, d)| d.mount_point().as_os_str().len())
            .map(|(i, _)| i);
        let system = System {
            hostname: sysinfo::System::host_name().unwrap_or_default(),
            os: sysinfo::System::long_os_version().unwrap_or_default(),
            arch: sysinfo::System::cpu_arch(),
            cpu: sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
            cores: sysinfo::System::physical_core_count().unwrap_or(sys.cpus().len()) as u32,
            ram_mb: sys.total_memory() / (1024 * 1024),
        };
        let nvidia = Command::new("nvidia-smi").arg("--version").output().is_ok();
        let mut sampler = Sampler {
            sys,
            disks,
            disk,
            nvidia,
            system,
            sampled: Instant::now(),
            cpu_pct: None,
            gpu_pct: None,
            ram_pct: None,
            disk_free_mb: None,
        };
        thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sampler.sample();
        sampler
    }

    /// Take a fresh sample when the last one is over a second old.
    fn refresh(&mut self) {
        if self.sampled.elapsed() >= Duration::from_secs(1) {
            self.sample();
        }
    }

    fn stamp(&self, m: &mut Metadata) {
        m.system = Some(self.system.clone());
        (m.cpu_pct, m.gpu_pct, m.ram_pct, m.disk_free_mb) =
            (self.cpu_pct, self.gpu_pct, self.ram_pct, self.disk_free_mb);
    }

    fn sample(&mut self) {
        self.sampled = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.cpu_pct = Some(self.sys.global_cpu_usage());
        let total = self.sys.total_memory();
        self.ram_pct = (total > 0).then(|| self.sys.used_memory() as f32 / total as f32 * 100.0);
        if let Some(disk) = self.disk.map(|i| &mut self.disks.list_mut()[i]) {
            disk.refresh_specifics(sysinfo::DiskRefreshKind::nothing().with_storage());
            self.disk_free_mb = Some(disk.available_space() / (1024 * 1024));
        }
        if self.nvidia {
            let output = Command::new("nvidia-smi")
                .args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"])
                .output();
            let gpus: Vec<f32> = output
                .map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect())
                .unwrap_or_default();
            self.gpu_pct = (!gpus.is_empty()).then(|| gpus.iter().sum::<f32>() / gpus.len() as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SpaceClient, test_home};
    use serde_json::json;
    use space_station_shared::wire::{Batch as Frame, Rejection};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::sync::mpsc;
    use uuid::Uuid;

    const KEY: &str = "table-orders-0123456789abcdef0123456789abcdef";

    /// A WebSocket server on 127.0.0.1 that hands every frame to `reply`, sends back the ack it
    /// returns, and drops the connection when it returns `None`. Frames are also passed out.
    #[allow(clippy::result_large_err)]
    fn server(reply: impl Fn(&Frame) -> Option<Ack> + Send + 'static) -> (String, mpsc::Receiver<Frame>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut ws = tungstenite::accept_hdr(
                    stream.unwrap(),
                    |request: &tungstenite::handshake::server::Request, response| {
                        assert_eq!(
                            request.headers()["user-agent"],
                            concat!("space-station/", env!("CARGO_PKG_VERSION"))
                        );
                        Ok(response)
                    },
                )
                .unwrap();
                loop {
                    let text = match ws.read() {
                        Ok(Message::Text(text)) => text,
                        Ok(_) => continue,
                        Err(_) => break,
                    };
                    let frame: Frame = serde_json::from_str(&text).unwrap();
                    let ack = reply(&frame);
                    let _ = tx.send(frame);
                    match ack {
                        Some(ack) => ws.send(Message::text(serde_json::to_string(&ack).unwrap())).unwrap(),
                        None => break,
                    }
                }
            }
        });
        (url, rx)
    }

    fn client(home: &Path, url: &str) -> (SpaceClient, Arc<Mutex<Vec<Error>>>) {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        let client = SpaceClient::builder(KEY)
            .home(home)
            .url(url)
            .on_error(move |e| sink.lock().unwrap().push(e))
            .build()
            .unwrap();
        (client, errors)
    }

    fn cursor(home: &Path) -> u64 {
        fs::read_to_string(home.join("spool.cursor")).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    /// Poll `spool.cursor` for up to five seconds until it reaches `at_least`.
    fn wait_cursor(home: &Path, at_least: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let cursor = cursor(home);
            if cursor >= at_least || Instant::now() > deadline {
                return cursor;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// A daemon of `home` in a thread of its own, shipping to `url`: what another process on the
    /// machine looks like to a client.
    fn foreground_daemon(home: &Path, url: &str) {
        let (lock, listener, shared) =
            (try_lock(home).unwrap().unwrap(), bind(home).unwrap(), Shared::open(home).unwrap());
        let config = Config { home: home.to_path_buf(), url: url.into() };
        thread::spawn(move || serve(config, Arc::new(|_| {}), Arc::new(|_| {}), lock, listener, shared));
    }

    #[test]
    fn second_locker_fails_until_the_first_lets_go() {
        let home = test_home();
        let first = try_lock(&home).unwrap().unwrap();
        assert!(try_lock(&home).unwrap().is_none());
        drop(first);
        assert!(try_lock(&home).unwrap().is_some());
        assert_eq!(fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(home.join("daemon.lock")).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn bind_unlinks_a_stale_socket() {
        let home = test_home();
        drop(bind(&home).unwrap());
        assert!(home.join("daemon.sock").exists(), "a dropped listener leaves its socket file behind");
        let _listener = bind(&home).unwrap();
        assert_eq!(fs::metadata(home.join("daemon.sock")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(connect(&home).is_ok());
    }

    #[test]
    fn the_socket_stays_in_a_short_home_and_moves_to_the_temp_dir_for_a_long_one() {
        let short = test_home();
        assert_eq!(socket_path(&short), short.join("daemon.sock"));

        let long = short.join("x".repeat(120));
        let socket = socket_path(&long);
        assert!(long.join("daemon.sock").as_os_str().len() >= SOCKET_PATH_MAX, "the home itself is too long");
        assert!(socket.starts_with(std::env::temp_dir()), "{}", socket.display());
        assert!(socket.as_os_str().len() < SOCKET_PATH_MAX, "{}", socket.display());
        assert_eq!(socket, socket_path(&long), "the same home always gets the same socket");
        assert_ne!(socket, socket_path(&short.join("y".repeat(120))), "another home gets another socket");
    }

    #[test]
    fn a_long_home_gets_a_daemon_whose_spool_stays_in_the_home() {
        let (url, frames) = server(|f| Some(Ack::ok(f.batch_id)));
        let home = test_home().join("h".repeat(120));
        let (client, errors) = client(&home, &url);
        client.record(json!({"n": 1}));
        assert!(client.flush());
        frames.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(cursor(&home), 1, "acked; the spool and its cursor live in the long home");
        assert!(home.join("spool.jsonl").exists() && !home.join("daemon.sock").exists());
        let status = status(&home).unwrap();
        assert!(status.running && status.socket == socket_path(&home) && !status.socket.starts_with(&home));
        assert!(errors.lock().unwrap().is_empty(), "{:?}", errors.lock().unwrap());
    }

    #[test]
    fn a_socket_path_the_os_refuses_is_named_with_its_length_and_the_limit() {
        let path = PathBuf::from(format!("/{}", "p".repeat(200)));
        let err = UnixListener::bind(&path).map_err(|e| named(&path, e)).unwrap_err().to_string();
        assert!(err.starts_with(&path.display().to_string()), "{err}");
        assert!(err.contains("201 bytes") && err.contains(&format!("under {SOCKET_PATH_MAX}")), "{err}");
        let other = named(Path::new("/x"), io::Error::new(io::ErrorKind::NotFound, "gone"));
        assert_eq!(other.to_string(), "gone", "only a refused path is renamed");
    }

    #[test]
    fn run_refuses_when_another_daemon_holds_the_lock() {
        let home = test_home();
        let _held = try_lock(&home).unwrap().unwrap();
        let result = run(Config { home, url: "ws://127.0.0.1:1".into() });
        assert!(matches!(result, Err(Error::Io(e)) if e.kind() == io::ErrorKind::AddrInUse));
    }

    #[test]
    fn connect_tries_every_resolved_address_and_names_the_one_that_refused() {
        let (url, _frames) = server(|f| Some(Ack::ok(f.batch_id)));
        let port = url.rsplit(':').next().unwrap();
        // The listener is IPv4-only and `localhost` resolves `::1` first on a dual-stack host.
        connect_ws(&format!("ws://localhost:{port}/api/ws/ingest")).unwrap();
        let refused = connect_ws("ws://127.0.0.1:1/api/ws/ingest").unwrap_err().to_string();
        assert!(refused.contains("127.0.0.1:1"), "{refused}");
    }

    #[test]
    fn ws_url_maps_http_to_ws_and_adds_the_ingest_path() {
        assert_eq!(
            ws_url("https://backend.spacestation.teamofsilicons.com/"),
            "wss://backend.spacestation.teamofsilicons.com/api/ws/ingest"
        );
        assert_eq!(ws_url("http://localhost:8080"), "ws://localhost:8080/api/ws/ingest");
        assert_eq!(ws_url("ws://127.0.0.1:9"), "ws://127.0.0.1:9/api/ws/ingest");
    }

    #[test]
    fn sampler_fills_system_and_live_fields() {
        let sampler = Sampler::new(&test_home());
        let mut m: Metadata =
            serde_json::from_value(json!({"record_id": Uuid::new_v4(), "table_id": "t", "event_ts_ms": 0})).unwrap();
        sampler.stamp(&mut m);
        let system = m.system.unwrap();
        assert!(system.cores > 0 && system.ram_mb > 0 && !system.arch.is_empty());
        assert!(m.cpu_pct.is_some() && m.ram_pct.is_some_and(|r| r > 0.0) && m.disk_free_mb.is_some_and(|d| d > 0));
        assert_eq!(m.gpu_pct.is_some(), sampler.nvidia);
    }

    #[test]
    fn batch_is_acked_cursor_advances_and_metadata_is_stamped() {
        let (url, frames) = server(|f| Some(Ack::ok(f.batch_id)));
        let home = test_home();
        let (client, errors) = client(&home, &url);
        client.record(json!({"order": 1}));
        assert!(client.flush());
        let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
        let entry = &frame.records[0];
        assert_eq!(
            (entry.key.as_str(), entry.metadata.table_id.as_str(), &entry.record["order"]),
            (KEY, "orders", &json!(1))
        );
        let m = &entry.metadata;
        assert!(m.system.is_some() && m.cpu_pct.is_some() && m.ram_pct.is_some() && m.disk_free_mb.is_some());
        assert_eq!(wait_cursor(&home, 1), 1);
        assert!(errors.lock().unwrap().is_empty());
    }

    // ── flush: a one-shot program delivers ──────────────────────────────────────────────────

    #[test]
    fn flush_returns_only_once_the_server_acked_when_the_daemon_runs_in_this_process() {
        let (url, _frames) = server(|f| Some(Ack::ok(f.batch_id)));
        let home = test_home();
        let (client, errors) = client(&home, &url);
        for n in 1..=3 {
            client.record(json!({"n": n}));
        }
        assert!(client.flush());
        assert_eq!(cursor(&home), 3, "acked before flush returned, without anyone waiting for the daemon");
        assert!(errors.lock().unwrap().is_empty());
        assert!(client.flush(), "nothing new to wait for");
    }

    #[test]
    fn dropping_the_client_delivers_what_it_recorded() {
        let (url, _frames) = server(|f| Some(Ack::ok(f.batch_id)));
        let home = test_home();
        let (client, _) = client(&home, &url);
        client.record(json!({"n": 1}));
        drop(client);
        assert_eq!(cursor(&home), 1, "Drop waited for the ack, as a one-shot program needs");
    }

    #[test]
    fn flush_says_false_when_the_ack_does_not_come_within_its_timeout_and_the_spool_keeps_the_lines() {
        let (url, _frames) = server(|f| {
            thread::sleep(Duration::from_millis(1500));
            Some(Ack::ok(f.batch_id))
        });
        let home = test_home();
        let client = SpaceClient::builder(KEY)
            .home(&home)
            .url(&url)
            .flush_timeout(Duration::from_millis(300))
            .on_error(|_| {})
            .build()
            .unwrap();
        client.record(json!({"n": 1}));
        let started = Instant::now();
        assert!(!client.flush(), "nothing was acked within 300 ms");
        assert!(started.elapsed() < Duration::from_secs(1), "the wait is bounded: {:?}", started.elapsed());
        assert_eq!(cursor(&home), 0);
        assert!(fs::read_to_string(home.join("spool.jsonl")).unwrap().contains("\"n\":1"), "spooled all the same");
        assert_eq!(wait_cursor(&home, 1), 1, "and the ack lands later");
    }

    #[test]
    fn flush_with_a_daemon_of_another_process_returns_once_its_spool_has_the_lines() {
        let home = test_home();
        foreground_daemon(&home, "ws://127.0.0.1:1");
        let (client, errors) = client(&home, "ws://127.0.0.1:1");
        for n in 1..=2 {
            client.record(json!({"n": n}));
        }
        let started = Instant::now();
        assert!(client.flush(), "the other daemon's spool outlives this process");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(cursor(&home), 0, "nothing is acked: nobody answers on that port");
        let spool = fs::read_to_string(home.join("spool.jsonl")).unwrap();
        assert_eq!(spool.lines().count(), 2);
        assert!(errors.lock().unwrap().is_empty(), "the other daemon's troubles are its own");
    }

    #[test]
    fn rejections_reach_on_error_except_duplicates_and_the_cursor_still_advances() {
        let (url, frames) = server(|f| {
            let rejected = f.records.iter().filter_map(|r| {
                let code = match r.record["n"].as_u64()? {
                    1 => Code::Duplicate,
                    2 => Code::Unauthorized,
                    _ => return None,
                };
                Some(Rejection { record_id: r.metadata.record_id, code, reason: "test".into() })
            });
            Some(Ack::rejected(f.batch_id, rejected.collect()))
        });
        let home = test_home();
        let (client, errors) = client(&home, &url);
        for n in 1..=3 {
            client.record(json!({"n": n}));
        }
        assert!(client.flush());
        assert_eq!(wait_cursor(&home, 3), 3);
        let received: Vec<Entry> = frames.try_iter().flat_map(|f| f.records).collect();
        assert_eq!(received.len(), 3);
        let second = received.iter().find(|r| r.record["n"] == 2).unwrap();
        let errors = errors.lock().unwrap();
        assert!(
            matches!(errors.as_slice(), [Error::Rejected { record_id, code: Code::Unauthorized, .. }] if *record_id == second.metadata.record_id),
            "{errors:?}"
        );
    }

    #[test]
    fn after_a_disconnect_the_batch_is_resent_under_a_new_id_with_new_records() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let (url, frames) = server(move |f| (counter.fetch_add(1, Relaxed) > 0).then(|| Ack::ok(f.batch_id)));
        let home = test_home();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        // A short flush timeout: the resend happens after the 1 s backoff, and the new record must
        // be recorded before then to travel with it.
        let client = SpaceClient::builder(KEY)
            .home(&home)
            .url(&url)
            .flush_timeout(Duration::from_millis(200))
            .on_error(move |e| sink.lock().unwrap().push(e))
            .build()
            .unwrap();
        client.record(json!({"n": 1}));
        assert!(!client.flush(), "the first frame is dropped by the server, so nothing is acked yet");
        let first = frames.recv_timeout(Duration::from_secs(5)).unwrap();
        client.record(json!({"n": 2}));
        let second = frames.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_ne!(first.batch_id, second.batch_id);
        assert_eq!(second.records.iter().map(|r| r.record["n"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(second.records[0].metadata.record_id, first.records[0].metadata.record_id);
        assert_eq!(wait_cursor(&home, 2), 2);
        assert!(errors.lock().unwrap().iter().any(|e| matches!(e, Error::Ws(_))), "the disconnect was reported");
    }
}
