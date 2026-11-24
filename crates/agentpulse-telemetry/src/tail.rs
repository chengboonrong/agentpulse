//! Discovery and incremental reading shared by the adapters: a cached file
//! list whose recently modified (hot) files are polled every time, a full
//! rescan at most every [`RESCAN_INTERVAL`], and a read budget per poll.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agentpulse_core::{FileCursor, FileIdentity};
use chrono::DateTime;

const READ_BUDGET: u64 = 64 * 1024 * 1024;
/// Full directory walk + stat of every transcript at most this often; a new
/// file of a running session can be noticed this late.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);
/// Files modified within this window at the last full scan are polled every
/// time; older ones wait for the next full scan (one more rescan interval of delay).
const HOT_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub(crate) struct TranscriptFile {
    pub path: PathBuf,
    pub is_main: bool,
}

/// Group id (a session or thread) → its files, as discovery found them.
pub(crate) type Groups = Vec<(String, Vec<TranscriptFile>)>;

struct Cached {
    sid: String,
    file: TranscriptFile,
    hot: bool,
}

struct Scan {
    taken: Instant,
    files: Vec<Cached>,
    sids: HashSet<String>,
    /// Live ids the last full scan looked for and did not find.
    missing: HashSet<String>,
}

pub(crate) struct Tailer {
    scan: Option<Scan>,
    /// The previous poll stopped at its budget: keep scanning everything.
    backfilling: bool,
    pub rescan_interval: Duration,
    pub read_budget: u64,
    pub full_scans: u64,
}

impl Tailer {
    pub fn new() -> Self {
        Self {
            scan: None,
            backfilling: false,
            rescan_interval: RESCAN_INTERVAL,
            read_budget: READ_BUDGET,
            full_scans: 0,
        }
    }

    /// Reads what was appended to each file since `offsets`. Groups in `live`
    /// are polled every time and read however old their files are; a live id
    /// no scan has seen forces one rescan. `read` receives the verified handle,
    /// start offset and size, consumes complete lines, and returns the offset
    /// past the last one. Unchanged live files are passed through for state recovery.
    /// Returns the advanced cursors and whether the read budget cut the poll
    /// short. A missing root (discovery's `NotFound`) is an empty poll.
    pub fn poll(
        &mut self,
        live: &HashSet<String>,
        offsets: &HashMap<String, FileCursor>,
        since_ms: i64,
        discover: impl FnOnce() -> std::io::Result<Groups>,
        allowed: impl Fn(&TranscriptFile) -> bool,
        mut read: impl FnMut(&str, &TranscriptFile, &mut File, u64, u64) -> std::io::Result<u64>,
    ) -> anyhow::Result<(Vec<FileCursor>, bool)> {
        if let Some(s) = &mut self.scan {
            s.missing.retain(|i| live.contains(i));
        }
        let full = self.backfilling
            || self.scan.as_ref().is_none_or(|s| {
                s.taken.elapsed() >= self.rescan_interval
                    || live
                        .iter()
                        .any(|l| !s.sids.contains(l) && !s.missing.contains(l))
            });
        let scan_start = Instant::now();
        let groups: Groups = if full {
            match discover() {
                Ok(g) => g,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.scan = None;
                    self.backfilling = false;
                    return Ok((vec![], false));
                }
                Err(e) => return Err(e.into()),
            }
        } else {
            let mut g: Groups = vec![];
            for c in self.scan.iter().flat_map(|s| &s.files) {
                if !(c.hot || live.contains(&c.sid)) {
                    continue;
                }
                match g.last_mut() {
                    Some((sid, files)) if *sid == c.sid => files.push(c.file.clone()),
                    _ => g.push((c.sid.clone(), vec![c.file.clone()])),
                }
            }
            g
        };
        let mut cached = vec![];
        let mut cursors = vec![];
        let (mut bytes, mut more) = (0u64, false);
        for (sid, files) in groups {
            for f in files {
                let key = f.path.to_string_lossy().into_owned();
                // Symlinked transcripts are not read (the open refuses them too).
                let Ok(meta) = fs::symlink_metadata(&f.path) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                // Compressed rollouts are immutable, including when their
                // thread also has an open plain rollout.
                let immutable = f.path.extension().is_some_and(|e| e == "zst");
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
                if full {
                    cached.push(Cached {
                        sid: sid.clone(),
                        hot: live.contains(&sid)
                            || SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .zip(mtime)
                                .is_some_and(|(now, m)| now.saturating_sub(m) <= HOT_WINDOW),
                        file: f.clone(),
                    });
                }
                if !offsets.contains_key(&key) {
                    let mtime = mtime.map_or(0, |d| d.as_millis() as i64);
                    if mtime < since_ms && !live.contains(&sid) {
                        continue;
                    }
                }
                let identity = Some(identity_of(&meta));
                // Live adapters may need to rebuild parser state after a restart,
                // even when no new bytes are available.
                if (immutable || !live.contains(&sid))
                    && offsets
                        .get(&key)
                        .is_some_and(|c| c.offset == size && c.identity == identity)
                {
                    continue;
                }
                // After the cheap skips: an adapter's own check may cost several lstats.
                if !allowed(&f) {
                    continue;
                }
                if (immutable || !live.contains(&sid))
                    && offsets
                        .get(&key)
                        .is_some_and(|c| c.identity.is_none() && c.offset == size)
                {
                    cursors.push(FileCursor {
                        path: key,
                        offset: size,
                        identity,
                    });
                    continue;
                }
                let Ok((mut handle, meta, start)) = open_transcript(&f.path, offsets.get(&key))
                else {
                    tracing::warn!(path = %key, "cannot open transcript");
                    continue;
                };
                let size = meta.len();
                let identity = Some(identity_of(&meta));
                // Any archive read starts over and charges its full compressed
                // size to the budget, even if the saved offset was nonzero.
                let start = if immutable { 0 } else { start };
                match read(&sid, &f, &mut handle, start, size) {
                    Ok(end)
                        if end > start
                            || (end == start
                                && start == size
                                && offsets.get(&key).is_some_and(|c| {
                                    c.identity != identity || c.offset != end
                                })) =>
                    {
                        bytes += end - start;
                        cursors.push(FileCursor {
                            path: key,
                            offset: end,
                            identity,
                        });
                    }
                    Ok(_) => {}
                    Err(_) => tracing::warn!(path = %key, "cannot read transcript"),
                }
            }
            if bytes >= self.read_budget {
                more = true;
                break;
            }
        }
        self.backfilling = more;
        if full {
            self.full_scans += 1;
            let sids: HashSet<String> = cached.iter().map(|c| c.sid.clone()).collect();
            self.scan = Some(Scan {
                taken: scan_start,
                missing: live.difference(&sids).cloned().collect(),
                sids,
                files: cached,
            });
        }
        Ok((cursors, more))
    }
}

/// Calls `each` with every complete, non-blank line from `start` up to `end`
/// (or the end of the file); returns the offset past the last one.
pub(crate) fn read_lines(
    f: &mut File,
    start: u64,
    end: Option<u64>,
    each: impl FnMut(&[u8]),
) -> std::io::Result<u64> {
    f.seek(SeekFrom::Start(start))?;
    let limit = end.map_or(u64::MAX, |e| e.saturating_sub(start));
    read_stream(f.take(limit), each).map(|bytes| start + bytes)
}

/// The complete-line loop for plain transcripts and decoded archive streams.
pub(crate) fn read_stream(reader: impl Read, mut each: impl FnMut(&[u8])) -> std::io::Result<u64> {
    let mut r = BufReader::with_capacity(256 * 1024, reader);
    let mut buf = Vec::new();
    let mut offset = 0;
    loop {
        buf.clear();
        let n = r.read_until(b'\n', &mut buf)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            return Ok(offset);
        }
        offset += n as u64;
        let line = buf.trim_ascii();
        if !line.is_empty() {
            each(line);
        }
    }
}

pub(crate) fn ts_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

pub(crate) fn identity_of(meta: &Metadata) -> FileIdentity {
    FileIdentity {
        birth: meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        ino: meta.ino(),
    }
}

/// Verify and read the same open file, even if its path is renamed meanwhile.
/// In-place rewrites that keep the inode are outside this cursor's scope.
pub(crate) fn open_transcript(
    path: &Path,
    cursor: Option<&FileCursor>,
) -> std::io::Result<(File, Metadata, u64)> {
    // A FIFO must not block the collector, and a symlink swapped in after the
    // checks must not be followed.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let start = cursor.map_or(0, |c| {
        if c.identity.is_some_and(|i| i != identity_of(&meta)) || meta.len() < c.offset {
            0
        } else {
            c.offset
        }
    });
    Ok((file, meta, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_archive_skips_callback_and_restarts_with_full_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl.zst");
        fs::write(&path, b"archive").unwrap();
        let mut tail = Tailer::new();
        let live = HashSet::from(["thread".to_string()]);
        let mut reads = 0;
        let mut poll = |tail: &mut Tailer, offsets: &HashMap<String, FileCursor>| {
            tail.poll(
                &live,
                offsets,
                0,
                || {
                    Ok(vec![(
                        "thread".into(),
                        vec![TranscriptFile {
                            path: path.clone(),
                            is_main: true,
                        }],
                    )])
                },
                |_| true,
                |_, _, _, start, size| {
                    reads += 1;
                    assert_eq!(start, 0);
                    Ok(size)
                },
            )
            .unwrap()
        };
        let (first, _) = poll(&mut tail, &HashMap::new());
        let offsets = HashMap::from([(first[0].path.clone(), first[0].clone())]);
        fs::write(&path, b"corrupt").unwrap(); // Same identity and size.
        assert!(poll(&mut tail, &offsets).0.is_empty());
        fs::write(&path, b"larger archive").unwrap();
        tail.read_budget = 10; // Full compressed size exceeds it; the delta does not.
        let (changed, more) = poll(&mut tail, &offsets);
        assert_eq!(changed[0].offset, 14);
        assert!(more);
        assert_eq!(reads, 2);
    }

    #[test]
    fn non_regular_transcripts_are_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo.jsonl");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let mut tail = Tailer::new();
        let (cursors, more) = tail
            .poll(
                &HashSet::new(),
                &HashMap::new(),
                0,
                || {
                    Ok(vec![(
                        "session".into(),
                        vec![TranscriptFile {
                            path: fifo.clone(),
                            is_main: true,
                        }],
                    )])
                },
                |_| true,
                |_, _, _, _, _| panic!("non-regular file was read"),
            )
            .unwrap();
        assert!(cursors.is_empty());
        assert!(!more);
        assert!(open_transcript(&fifo, None).is_err());
        let target = dir.path().join("target");
        fs::write(&target, b"line\n").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert!(open_transcript(&link, None).is_err());
    }

    #[test]
    fn rename_after_open_reads_the_verified_file_and_bounds_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript");
        std::fs::write(&path, b"old\n").unwrap();
        let (mut file, meta, start) = open_transcript(&path, None).unwrap();
        // An append after metadata is left for the next poll.
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"later\n")
            .unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"new\n").unwrap();
        std::fs::rename(replacement, &path).unwrap();
        let mut lines = vec![];
        let end = read_lines(&mut file, start, Some(meta.len()), |l| {
            lines.push(l.to_vec())
        })
        .unwrap();
        assert_eq!(lines, [b"old".to_vec()]);
        assert_eq!(end, 4);
        assert_ne!(
            identity_of(&meta),
            identity_of(&File::open(path).unwrap().metadata().unwrap())
        );
    }
}
