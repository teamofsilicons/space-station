//! The recording half: `SpaceClient::record` stamps metadata, sanitises, and hands the line to a
//! sender thread that writes it to the local daemon's unix socket, starting that daemon
//! in-process when nobody on this machine runs one. Never blocks, never panics; every failure is
//! an `on_error` event. `flush` (and `Drop`) is the one wait: for the server's acks when the
//! daemon runs in this process — a one-shot program would otherwise exit with its records
//! unsent — or for the spool of the daemon another process runs, which outlives this one.

use crate::daemon::UnixStream;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::shared::limits::QUEUE_CAPACITY;
use crate::shared::wire::{Entry, Metadata};
use crate::{Error, Hook, daemon, default_home, default_url, lock, shared};

/// How long `flush` waits for acks unless `Builder::flush_timeout` says otherwise.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// How long past its deadline `flush` lets the sender's verdict arrive.
const GRACE: Duration = Duration::from_millis(100);

/// `SpaceClient::builder(key).url(..).home(..).flush_timeout(..).on_error(..).build()`. Defaults:
/// `default_url()`, `default_home()`, 5 s, and errors printed to stderr.
pub struct Builder {
    key: String,
    home: PathBuf,
    url: String,
    flush_timeout: Duration,
    on_error: Hook,
}

impl Builder {
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = home.into();
        self
    }

    /// How long `flush` and `Drop` may wait for the server to ack what was recorded, when the
    /// daemon runs inside this process. With a daemon in another process they return as soon as
    /// the lines are in its spool, whatever this says.
    pub fn flush_timeout(mut self, timeout: Duration) -> Self {
        self.flush_timeout = timeout;
        self
    }

    pub fn on_error(mut self, f: impl Fn(Error) + Send + Sync + 'static) -> Self {
        self.on_error = Arc::new(f);
        self
    }

    pub fn build(self) -> Result<SpaceClient, Error> {
        let table_id = shared::secrets::parse_table_key(&self.key).ok_or(Error::InvalidKey)?.to_string();
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let flushed = Arc::new((Mutex::new((0, false)), Condvar::new()));
        let sender = Sender {
            rx,
            home: self.home,
            url: self.url,
            on_error: self.on_error.clone(),
            queued: queued.clone(),
            flushed: flushed.clone(),
            sock: None,
            daemon: None,
        };
        std::thread::spawn(move || sender.run());
        Ok(SpaceClient {
            key: self.key,
            table_id,
            tx,
            queued,
            flushes: AtomicU64::new(0),
            flushed,
            flush_timeout: self.flush_timeout,
            on_error: self.on_error,
        })
    }
}

/// One table key. Share it behind an `Arc`; `Drop` flushes.
pub struct SpaceClient {
    key: String,
    table_id: String,
    tx: mpsc::Sender<Msg>,
    queued: Arc<AtomicUsize>,
    flushes: AtomicU64,
    /// The last flush turn the sender finished, and whether everything was acked by then.
    flushed: Arc<(Mutex<(u64, bool)>, Condvar)>,
    flush_timeout: Duration,
    on_error: Hook,
}

enum Msg {
    Line(String),
    Flush(u64, Instant),
}

impl SpaceClient {
    pub fn new(table_key: &str) -> Result<SpaceClient, Error> {
        Self::builder(table_key).build()
    }

    pub fn builder(table_key: &str) -> Builder {
        let on_error: Hook = Arc::new(|e: Error| eprintln!("space-station: {e}"));
        let (home, url) = (default_home(), default_url());
        Builder { key: table_key.to_string(), home, url, flush_timeout: FLUSH_TIMEOUT, on_error }
    }

    /// Queue one record. Never blocks; a record that cannot be sent is dropped and reported.
    pub fn record(&self, value: impl serde::Serialize) {
        if let Err(e) = self.enqueue(value) {
            (self.on_error)(e);
        }
    }

    fn enqueue(&self, value: impl serde::Serialize) -> Result<(), Error> {
        let mut record = serde_json::to_value(value).map_err(io::Error::from)?;
        if !record.is_object() {
            return Err(Error::Local("record must be a JSON object".into()));
        }
        shared::sanitize::record(&mut record).map_err(|e| Error::SizeExceeded { bytes: e.bytes })?;
        let event_ts_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let metadata = Metadata {
            record_id: Uuid::new_v4(),
            table_id: self.table_id.clone(),
            event_ts_ms,
            system: None,
            cpu_pct: None,
            gpu_pct: None,
            ram_pct: None,
            disk_free_mb: None,
        };
        let mut line =
            serde_json::to_string(&Entry { key: self.key.clone(), metadata, record }).map_err(io::Error::from)?;
        line.push('\n');
        if self.queued.fetch_add(1, Relaxed) >= QUEUE_CAPACITY {
            self.queued.fetch_sub(1, Relaxed);
            return Err(Error::QueueFull);
        }
        self.tx
            .send(Msg::Line(line))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "sender thread is gone"))?;
        Ok(())
    }

    /// Block until everything recorded so far has been acked by the server — when the daemon
    /// runs in this process — or is in the spool of the daemon another process runs, which
    /// outlives this one; `true` when that happened within `flush_timeout`. `false` means the
    /// lines are spooled at best and the next daemon on the machine ships them.
    pub fn flush(&self) -> bool {
        let turn = self.flushes.fetch_add(1, Relaxed) + 1;
        let deadline = Instant::now() + self.flush_timeout;
        if self.tx.send(Msg::Flush(turn, deadline)).is_err() {
            return false;
        }
        let (done, wake) = &*self.flushed;
        let mut done = lock(done);
        while done.0 < turn {
            let Some(left) = (deadline + GRACE).checked_duration_since(Instant::now()) else { return false };
            done = wake.wait_timeout(done, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
        done.1
    }
}

impl Drop for SpaceClient {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The thread between the channel and the daemon's socket.
struct Sender {
    rx: mpsc::Receiver<Msg>,
    home: PathBuf,
    url: String,
    on_error: Hook,
    queued: Arc<AtomicUsize>,
    flushed: Arc<(Mutex<(u64, bool)>, Condvar)>,
    sock: Option<UnixStream>,
    /// The daemon this process elected and runs, when it did: its spool is what a flush waits on.
    daemon: Option<Arc<daemon::Shared>>,
}

impl Sender {
    fn run(mut self) {
        while let Ok(msg) = self.rx.recv() {
            match msg {
                Msg::Line(line) => {
                    self.write(line.as_bytes());
                    self.queued.fetch_sub(1, Relaxed);
                }
                Msg::Flush(turn, deadline) => {
                    let acked = self.hangup() && self.acked(deadline);
                    let (done, wake) = &*self.flushed;
                    *lock(done) = (turn, acked);
                    wake.notify_all();
                }
            }
        }
        self.hangup();
    }

    /// Write one line, reconnecting (and electing a daemon) until it is on the socket.
    fn write(&mut self, line: &[u8]) {
        let mut backoff = Duration::from_millis(50);
        loop {
            let Some(sock) = self.sock.as_mut() else {
                match self.connect() {
                    Ok(sock) => self.sock = Some(sock),
                    Err(e) => {
                        // Nobody listening right after a lost election means the winner is still
                        // binding: wait quietly. Anything else is worth a report.
                        if !nobody_listening(&e) {
                            (self.on_error)(Error::Io(e));
                        }
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(1));
                    }
                }
                continue;
            };
            match sock.write_all(line) {
                Ok(()) => return,
                Err(e) => {
                    self.sock = None;
                    (self.on_error)(Error::Io(e));
                }
            }
        }
    }

    /// Connect to the daemon's socket. When nobody is listening, hold the election: the lock
    /// holder binds the socket and runs the daemon on a thread in this process; a loser retries.
    fn connect(&mut self) -> io::Result<UnixStream> {
        match daemon::connect(&self.home) {
            Err(e) if nobody_listening(&e) => {}
            other => return other,
        }
        if let Some(lock) = daemon::try_lock(&self.home)? {
            let listener = daemon::bind(&self.home)?;
            let shared = daemon::Shared::open(&self.home)?;
            let config = daemon::Config { home: self.home.clone(), url: self.url.clone() };
            let (hook, spool) = (self.on_error.clone(), shared.clone());
            let quiet: daemon::Log = Arc::new(|_| {});
            std::thread::spawn(move || daemon::serve(config, hook, quiet, lock, listener, spool));
            self.daemon = Some(shared);
        }
        daemon::connect(&self.home)
    }

    /// Half-close the socket and wait for the daemon to close its side, which it does only after
    /// it has read and spooled every line: `true` means "on disk", not "in a buffer".
    fn hangup(&mut self) -> bool {
        let Some(sock) = self.sock.take() else { return true };
        let _ = sock.set_read_timeout(Some(Duration::from_secs(1)));
        let _ = sock.shutdown(Shutdown::Write);
        matches!((&sock).read(&mut [0]), Ok(0))
    }

    /// Whether everything spooled is acked: waited for, up to `deadline`, when the daemon runs in
    /// this process; taken as given when another process holds the spool, which outlives this one.
    fn acked(&self, deadline: Instant) -> bool {
        self.daemon.as_ref().is_none_or(|shared| shared.wait_acked(deadline))
    }
}

/// ENOENT or ECONNREFUSED: no daemon is bound to the socket.
fn nobody_listening(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}
