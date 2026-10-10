//! The daemon's on-disk queue: `spool.jsonl` holds one `{"seq","key","metadata","record"}` line
//! per record, `spool.cursor` the last acked seq. Appends and acks come from different threads
//! and share one mutex around the whole `Spool`; batches are read straight from the file, so
//! nothing is held in memory twice.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use space_station_shared::limits::SPOOL_TRUNCATE_BYTES;
use space_station_shared::wire::{Code, Entry, Metadata};
use uuid::Uuid;

use crate::Error;

/// One spool line: the entry as the client sent it, plus its seq.
#[derive(Serialize, Deserialize)]
struct Line<E> {
    seq: u64,
    #[serde(flatten)]
    entry: E,
}

pub struct Spool {
    file: File,
    home: PathBuf,
    /// Bytes in the file.
    len: u64,
    next_seq: u64,
    /// Last acked seq, mirrored in `spool.cursor`.
    cursor: u64,
    /// Byte offset of the first unacked line.
    read_pos: u64,
    /// Lines that did not parse, skipped at boot or while batching.
    pub skipped: u64,
    /// A fully acked spool bigger than this is truncated.
    pub truncate_at: u64,
}

/// One frame ready to send. `errors` are entries too big for any frame: dropped, reported, and
/// acked along with the batch.
pub struct Batch {
    pub id: Uuid,
    pub frame: String,
    pub count: usize,
    pub errors: Vec<Error>,
    last_seq: u64,
    end: u64,
}

impl Spool {
    /// Open `<home>/spool.jsonl` and `<home>/spool.cursor`; `next_seq` is one past the larger of
    /// the cursor and the last parseable seq, and a torn tail line is sealed with a newline.
    pub fn open(home: &Path) -> io::Result<Spool> {
        let file = crate::local::open_private(
            &home.join("spool.jsonl"),
            OpenOptions::new().read(true).write(true).create(true).truncate(false),
        )?;
        let cursor =
            fs::read_to_string(home.join("spool.cursor")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let mut spool = Spool {
            len: file.metadata()?.len(),
            file,
            home: home.to_path_buf(),
            next_seq: cursor + 1,
            cursor,
            read_pos: 0,
            skipped: 0,
            truncate_at: SPOOL_TRUNCATE_BYTES,
        };
        let (mut reader, mut line, mut pos, mut first_unacked, mut torn) =
            (BufReader::new(&spool.file), String::new(), 0, None, false);
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            torn = !line.ends_with('\n');
            match serde_json::from_str::<Line<Entry>>(&line) {
                Ok(parsed) => {
                    if parsed.seq > cursor && first_unacked.is_none() {
                        first_unacked = Some(pos);
                    }
                    spool.next_seq = spool.next_seq.max(parsed.seq + 1);
                }
                Err(_) => spool.skipped += 1,
            }
            pos += n as u64;
        }
        spool.read_pos = first_unacked.unwrap_or(pos);
        if torn {
            spool.file.seek(SeekFrom::End(0))?;
            spool.file.write_all(b"\n")?;
            spool.len += 1;
        }
        Ok(spool)
    }

    /// Append one line; write only, no fsync. A torn write is rolled back so nothing glues.
    pub fn append(&mut self, entry: &Entry) -> io::Result<u64> {
        let seq = self.next_seq;
        let mut line = serde_json::to_vec(&Line { seq, entry })?;
        line.push(b'\n');
        // One daemon and its spool mutex serialize writes. A read/write handle also permits
        // truncation on Windows, where an append-only handle cannot set_len.
        self.file.seek(SeekFrom::End(0))?;
        if let Err(e) = self.file.write_all(&line) {
            let _ = self.file.set_len(self.len);
            return Err(e);
        }
        self.len += line.len() as u64;
        self.next_seq += 1;
        Ok(seq)
    }

    /// The oldest unacked lines, any key, as one frame of at most `max` bytes; `stamp` fills each
    /// entry's sampled metadata before it is measured. `None` when nothing is waiting.
    pub fn batch(&mut self, max: usize, mut stamp: impl FnMut(&mut Metadata)) -> io::Result<Option<Batch>> {
        let id = Uuid::new_v4();
        let mut b = Batch {
            id,
            frame: format!("{{\"batch_id\":\"{id}\",\"records\":["),
            count: 0,
            errors: Vec::new(),
            last_seq: self.cursor,
            end: self.read_pos,
        };
        let mut reader = BufReader::new(&self.file);
        reader.seek(SeekFrom::Start(self.read_pos))?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            let Ok(Line { seq, mut entry }) = serde_json::from_str::<Line<Entry>>(&line) else {
                self.skipped += 1;
                b.end += n as u64;
                continue;
            };
            stamp(&mut entry.metadata);
            let text = serde_json::to_string(&entry)?;
            let separator = usize::from(b.count > 0);
            if b.frame.len() + separator + text.len() + 2 > max {
                if b.count > 0 {
                    break;
                }
                let reason = format!("{} bytes alone exceed the {max} byte frame limit", text.len());
                b.errors.push(Error::Rejected {
                    record_id: entry.metadata.record_id,
                    code: Code::SizeExceeded,
                    reason,
                });
            } else {
                if separator == 1 {
                    b.frame.push(',');
                }
                b.frame.push_str(&text);
                b.count += 1;
            }
            b.last_seq = seq;
            b.end += n as u64;
        }
        b.frame.push_str("]}");
        Ok((b.end > self.read_pos).then_some(b))
    }

    /// Advance the cursor past `batch`; a fully acked spool over `truncate_at` is emptied.
    pub fn ack(&mut self, batch: &Batch) -> io::Result<()> {
        self.cursor = batch.last_seq;
        self.read_pos = batch.end;
        write_private(&self.home.join("spool.cursor"), self.cursor.to_string().as_bytes())?;
        if self.read_pos >= self.len && self.len > self.truncate_at {
            self.file.set_len(0)?;
            self.len = 0;
            self.read_pos = 0;
        }
        Ok(())
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Whether any line waits beyond the cursor.
    pub fn pending(&self) -> bool {
        self.read_pos < self.len
    }
}

/// Replace `path` with `bytes` atomically, mode 0600.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    crate::local::open_private(&tmp, OpenOptions::new().write(true).create(true).truncate(true))?.write_all(bytes)?;
    fs::rename(tmp, path)
}
