//! Discovery and incremental reading shared by the adapters: a cached file
//! list whose recently modified (hot) files are polled every time, a full
//! rescan at most every [`RESCAN_INTERVAL`], and a read budget per poll.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agentpulse_core::FileCursor;
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
    /// no scan has seen forces one rescan. `read(group, file, start)` consumes
    /// complete lines from `start` and returns the offset past the last one.
    /// Returns the advanced cursors and whether the read budget cut the poll
    /// short. A missing root (discovery's `NotFound`) is an empty poll.
    pub fn poll(
        &mut self,
        live: &HashSet<String>,
        offsets: &HashMap<String, u64>,
        since_ms: i64,
        discover: impl FnOnce() -> std::io::Result<Groups>,
        mut read: impl FnMut(&str, &TranscriptFile, u64) -> std::io::Result<u64>,
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
                let Ok(meta) = fs::metadata(&f.path) else {
                    tracing::warn!(path = %key, "cannot stat transcript");
                    continue;
                };
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
                let start = match offsets.get(&key) {
                    Some(&o) if o == size => continue,
                    Some(&o) if o < size => o,
                    Some(_) => 0, // truncated or replaced
                    None => {
                        let mtime = mtime.map_or(0, |d| d.as_millis() as i64);
                        if mtime < since_ms && !live.contains(&sid) {
                            continue;
                        }
                        0
                    }
                };
                match read(&sid, &f, start) {
                    Ok(end) if end > start => {
                        bytes += end - start;
                        cursors.push(FileCursor {
                            path: key,
                            offset: end,
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
    path: &Path,
    start: u64,
    end: Option<u64>,
    mut each: impl FnMut(&[u8]),
) -> std::io::Result<u64> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let limit = end.map_or(u64::MAX, |e| e.saturating_sub(start));
    let mut r = BufReader::with_capacity(256 * 1024, f.take(limit));
    let mut buf = Vec::new();
    let mut offset = start;
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
