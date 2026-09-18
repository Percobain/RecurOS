//! Append-only JSONL shards: `log/<machine-id>/<YYYY-MM>.jsonl`.
//!
//! Crash-safety contract: a record is either a complete `\n`-terminated line
//! or it is not there. A torn final line (power loss mid-write) is skipped on
//! read with a warning, and the next append terminates it first so the new
//! record is never glued onto garbage.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ctx_core::Record;
use thiserror::Error;

use crate::home::CtxHome;

#[derive(Debug, Error)]
pub enum ShardError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("serialising record: {0}")]
    Serialise(#[from] serde_json::Error),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ShardError + '_ {
    move |source| ShardError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Path of the shard this machine writes to for records made at `now`.
pub fn shard_path(home: &CtxHome, machine: &str, now: DateTime<Utc>) -> PathBuf {
    home.log_dir()
        .join(machine)
        .join(format!("{}.jsonl", now.format("%Y-%m")))
}

/// Append one record to this machine's current shard and fsync it.
///
/// Takes an exclusive OS file lock for the duration so that two local
/// processes (CLI and daemon, say) can't interleave bytes. The lock is
/// advisory and only guards local writers; cross-machine safety comes from
/// sharding, not locking.
pub fn append(
    home: &CtxHome,
    machine: &str,
    record: &Record,
    now: DateTime<Utc>,
) -> Result<PathBuf, ShardError> {
    let path = shard_path(home, machine, now);
    let dir = path.parent().unwrap_or(home.log_dir().as_path()).to_owned();
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;

    let mut line = serde_json::to_string(record)?;
    line.push('\n');

    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(&path)
        .map_err(io_err(&path))?;
    file.lock().map_err(io_err(&path))?;

    let result = (|| {
        if ends_without_newline(&mut file)? {
            // Terminate a torn line left by a crash so it stays a separate,
            // skippable line instead of corrupting this record.
            file.write_all(b"\n")?;
        }
        file.write_all(line.as_bytes())?;
        file.sync_data()
    })();
    let _ = file.unlock();
    result.map_err(io_err(&path))?;
    Ok(path)
}

fn ends_without_newline(file: &mut File) -> io::Result<bool> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

/// All shard files under `log/`, sorted, as paths relative to the home root
/// using forward slashes (stable keys across platforms).
pub fn list_shards(home: &CtxHome) -> Result<Vec<String>, ShardError> {
    let log = home.log_dir();
    let mut out = Vec::new();
    let entries = match fs::read_dir(&log) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io_err(&log)(e)),
    };
    for writer in entries {
        let writer = writer.map_err(io_err(&log))?;
        if !writer.file_type().map_err(io_err(&log))?.is_dir() {
            continue;
        }
        let wdir = writer.path();
        for f in fs::read_dir(&wdir).map_err(io_err(&wdir))? {
            let f = f.map_err(io_err(&wdir))?;
            let name = f.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl") {
                out.push(format!(
                    "log/{}/{}",
                    writer.file_name().to_string_lossy(),
                    name
                ));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Result of reading a shard from a byte offset.
#[derive(Debug, Default)]
pub struct ReadOutcome {
    pub records: Vec<Record>,
    /// Offset just past the last complete line. Resume from here next time;
    /// an incomplete trailing line is not consumed.
    pub next_offset: u64,
    /// Human-readable problems (bad lines). Never fatal.
    pub warnings: Vec<String>,
    /// True if the file is shorter than `offset`: it was rewritten or
    /// truncated behind our back, so incremental state is invalid.
    pub shrunk: bool,
}

/// Read complete records from `rel` (relative to the home) starting at `offset`.
pub fn read_from(home: &CtxHome, rel: &str, offset: u64) -> Result<ReadOutcome, ShardError> {
    let path = home.root().join(rel);
    let mut file = File::open(&path).map_err(io_err(&path))?;
    let len = file.metadata().map_err(io_err(&path))?.len();
    if len < offset {
        return Ok(ReadOutcome {
            next_offset: offset,
            shrunk: true,
            ..Default::default()
        });
    }
    file.seek(SeekFrom::Start(offset)).map_err(io_err(&path))?;
    let mut buf = Vec::with_capacity((len - offset) as usize);
    file.read_to_end(&mut buf).map_err(io_err(&path))?;

    let mut out = ReadOutcome {
        next_offset: offset,
        ..Default::default()
    };
    let mut pos = 0usize;
    while pos < buf.len() {
        let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') else {
            out.warnings.push(format!(
                "{rel}: incomplete final line at byte {} (torn write?); skipped",
                offset + pos as u64
            ));
            break;
        };
        let line = &buf[pos..pos + nl];
        let line_offset = offset + pos as u64;
        pos += nl + 1;
        out.next_offset = offset + pos as u64;

        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        match serde_json::from_slice::<Record>(line) {
            Ok(Record::Claim(c)) if !c.verify_cid() => out.warnings.push(format!(
                "{rel}: claim {} at byte {line_offset} does not match its cid (edited by hand?); skipped",
                c.id
            )),
            Ok(r) => out.records.push(r),
            Err(e) => out
                .warnings
                .push(format!("{rel}: unreadable line at byte {line_offset}: {e}; skipped")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_core::{Claim, ClaimDraft, Kind};
    use ulid::Ulid;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-18T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn rec(text: &str) -> Record {
        Record::Claim(
            Claim::from_draft(ClaimDraft::new(Kind::Fact, text), Ulid::new(), now()).unwrap(),
        )
    }

    fn setup() -> (tempfile::TempDir, CtxHome) {
        let dir = tempfile::tempdir().unwrap();
        let home = CtxHome::at(dir.path());
        (dir, home)
    }

    #[test]
    fn append_then_read() {
        let (_d, home) = setup();
        append(&home, "m1", &rec("a"), now()).unwrap();
        append(&home, "m1", &rec("b"), now()).unwrap();
        append(&home, "m2", &rec("c"), now()).unwrap();
        let shards = list_shards(&home).unwrap();
        assert_eq!(shards, vec!["log/m1/2026-09.jsonl", "log/m2/2026-09.jsonl"]);
        let r = read_from(&home, &shards[0], 0).unwrap();
        assert_eq!(r.records.len(), 2);
        assert!(r.warnings.is_empty());

        // Incremental: nothing new past next_offset.
        let again = read_from(&home, &shards[0], r.next_offset).unwrap();
        assert!(again.records.is_empty());
    }

    #[test]
    fn torn_final_line_is_skipped_then_repaired_by_next_append() {
        let (_d, home) = setup();
        let path = append(&home, "m1", &rec("good"), now()).unwrap();
        let rel = "log/m1/2026-09.jsonl";

        // Simulate a crash mid-write.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(br#"{"rec":"claim","id":"01J"#).unwrap();
        drop(f);

        let r = read_from(&home, rel, 0).unwrap();
        assert_eq!(r.records.len(), 1);
        assert_eq!(r.warnings.len(), 1);
        let resume = r.next_offset;

        append(&home, "m1", &rec("after crash"), now()).unwrap();
        let r2 = read_from(&home, rel, resume).unwrap();
        assert_eq!(r2.records.len(), 1, "new record must survive the torn line");
        assert_eq!(r2.warnings.len(), 1, "torn line reported as unreadable");
    }

    #[test]
    fn tampered_claim_is_skipped() {
        let (_d, home) = setup();
        let path = append(&home, "m1", &rec("original"), now()).unwrap();
        let s = fs::read_to_string(&path)
            .unwrap()
            .replace("original", "tampered");
        fs::write(&path, s).unwrap();
        let r = read_from(&home, "log/m1/2026-09.jsonl", 0).unwrap();
        assert!(r.records.is_empty());
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn shrunk_file_is_detected() {
        let (_d, home) = setup();
        append(&home, "m1", &rec("x"), now()).unwrap();
        let r = read_from(&home, "log/m1/2026-09.jsonl", 10_000).unwrap();
        assert!(r.shrunk);
    }
}
