//! Gemini CLI adapter: tails only `tmp/*/chats/**/*.jsonl`.
//!
//! Privacy: structs name only metadata and token counts. Content is skipped
//! by serde; no line text is stored or logged. Symlinks are not followed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentpulse_core::{
    AgentAdapter, FileCursor, LiveSession, PollBatch, Provider, SessionDelta, TokenUsage,
    UsageEvent,
};
use serde::Deserialize;

use crate::tail::{Groups, Tailer, TranscriptFile, read_lines, ts_ms};

pub struct GeminiAdapter {
    root: PathBuf,
    tail: Tailer,
    files: HashMap<PathBuf, FileState>,
    warned_tmp: bool,
}

impl GeminiAdapter {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tail: Tailer::new(),
            files: HashMap::new(),
            warned_tmp: false,
        }
    }

    /// `$GEMINI_CLI_HOME/.gemini` if set and non-empty, else `~/.gemini`.
    pub fn default_root() -> Option<PathBuf> {
        root_from(
            std::env::var_os("GEMINI_CLI_HOME"),
            std::env::var_os("HOME"),
        )
    }

    pub fn with_rescan_interval(mut self, d: Duration) -> Self {
        self.tail.rescan_interval = d;
        self
    }

    pub fn with_read_budget(mut self, bytes: u64) -> Self {
        self.tail.read_budget = bytes;
        self
    }
}

fn root_from(runtime: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    runtime
        .filter(|v| !v.is_empty())
        .or_else(|| home.filter(|v| !v.is_empty()))
        .map(|h| PathBuf::from(h).join(".gemini"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    session_id: Option<String>,
    start_time: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    r#type: Option<String>,
    model: Option<String>,
    tokens: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
    thoughts: Option<u64>,
    tool: Option<u64>,
}

#[derive(Default)]
struct FileState {
    offset: u64,
    session_id: Option<String>,
}

fn is_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

// Re-check every component under the root, including on cached tailer reads.
fn allowed(root: &Path, path: &Path) -> bool {
    is_dir(root)
        && path.strip_prefix(root).is_ok_and(|relative| {
            let mut p = root.to_path_buf();
            let mut components = relative.components().peekable();
            while let Some(c) = components.next() {
                p.push(c);
                if components.peek().is_some() {
                    if !is_dir(&p) {
                        return false;
                    }
                } else {
                    return fs::symlink_metadata(&p).is_ok_and(|m| m.is_file());
                }
            }
            false
        })
}

fn discover(root: &Path, warned_tmp: &mut bool) -> std::io::Result<Groups> {
    let projects = root.join("tmp");
    if !is_dir(root) || !is_dir(&projects) {
        return Ok(vec![]);
    }
    let mut groups: BTreeMap<String, Vec<TranscriptFile>> = BTreeMap::new();
    let projects = match fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound && !*warned_tmp {
                tracing::warn!(path = %projects.display(), error = %e, "cannot read Gemini tmp directory");
                *warned_tmp = true;
            }
            return Ok(vec![]);
        }
    };
    for project in projects.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let chats = project.path().join("chats");
        if !is_dir(&chats) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&chats) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_file()) {
                add_file(&mut groups, entry.path(), None);
            } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let parent = entry.file_name().to_string_lossy().into_owned();
                let Ok(children) = fs::read_dir(entry.path()) else {
                    continue;
                };
                for child in children.flatten() {
                    if child.file_type().is_ok_and(|t| t.is_file()) {
                        add_file(&mut groups, child.path(), Some(&parent));
                    }
                }
            }
        }
    }
    for files in groups.values_mut() {
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(groups.into_iter().collect())
}

fn add_file(
    groups: &mut BTreeMap<String, Vec<TranscriptFile>>,
    path: PathBuf,
    parent: Option<&str>,
) {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return;
    };
    if path.extension().is_none_or(|x| x != "jsonl")
        || name.contains(".unreadable-")
        || name.contains(".tmp-")
    {
        return;
    }
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    groups
        .entry(parent.unwrap_or(stem).to_string())
        .or_default()
        .push(TranscriptFile {
            path,
            is_main: parent.is_none(),
        });
}

fn project_path(root: &Path, f: &TranscriptFile) -> Option<String> {
    let chats = if f.is_main {
        f.path.parent()?
    } else {
        f.path.parent()?.parent()?
    };
    let path = chats.parent()?.join(".project_root");
    if !allowed(root, &path) {
        return None;
    }
    let (file, _, _) = crate::tail::open_transcript(&path, None).ok()?;
    let mut value = String::new();
    file.take(4096).read_to_string(&mut value).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

// With no output, restore only the source session metadata.
fn fold_line(
    line: &[u8],
    sid: &str,
    main: bool,
    state: &mut FileState,
    out: Option<(&mut BTreeMap<String, Acc>, &mut Vec<UsageEvent>)>,
) {
    let Ok(r) = serde_json::from_slice::<Record>(line) else {
        return;
    };
    if r.r#type.is_none() {
        if let Some(id) = r.session_id.filter(|s| !s.is_empty()) {
            state.session_id = Some(id);
        }
        if let Some(ts) = r.start_time.as_deref().and_then(ts_ms)
            && let Some((accs, _)) = out
        {
            let session = if main {
                state.session_id.as_deref().unwrap_or(sid)
            } else {
                sid
            };
            accs.entry(session.to_string())
                .or_default()
                .timestamps
                .push(ts);
        }
        return;
    }
    let Some(ts) = r.timestamp.as_deref().and_then(ts_ms) else {
        return;
    };
    let source_id = state.session_id.as_deref().unwrap_or(sid);
    let session = if main { source_id } else { sid };
    let event = if r.r#type.as_deref() == Some("gemini") {
        if let (Some(u), Some(id)) = (r.tokens, r.id) {
            let cached = u.cached.unwrap_or(0);
            Some(UsageEvent {
                event_id: format!("gemini:{source_id}:{id}"),
                session_id: session.to_string(),
                model: r.model.clone().unwrap_or_default(),
                ts_ms: ts,
                usage: TokenUsage {
                    input: u
                        .input
                        .unwrap_or(0)
                        .saturating_add(u.tool.unwrap_or(0))
                        .saturating_sub(cached),
                    cache_read: cached,
                    output: u
                        .output
                        .unwrap_or(0)
                        .saturating_add(u.thoughts.unwrap_or(0)),
                    ..Default::default()
                },
                fast: false,
                us_inference: false,
                unknown_tier: false,
                recorded_usd: None,
            })
        } else {
            None
        }
    } else {
        None
    };
    let Some((accs, events)) = out else {
        return;
    };
    let acc = accs.entry(session.to_string()).or_default();
    acc.timestamps.push(ts);
    if main
        && r.r#type.as_deref() == Some("gemini")
        && let Some(model) = r.model.filter(|s| !s.is_empty())
        && acc.model.as_ref().is_none_or(|(t, _)| ts >= *t)
    {
        acc.model = Some((ts, model));
    }
    if let Some(event) = event {
        events.push(event);
    }
}

#[derive(Default)]
struct Acc {
    timestamps: Vec<i64>,
    cwd: Option<String>,
    model: Option<(i64, String)>,
}

impl AgentAdapter for GeminiAdapter {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    fn poll(
        &mut self,
        offsets: &HashMap<String, FileCursor>,
        since_ms: i64,
    ) -> anyhow::Result<PollBatch> {
        let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
        let mut events = vec![];
        // A symlinked root is followed (as Grok does); nothing below it is.
        let root = &fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let mut discovered = None;
        let (files, more) = self.tail.poll(
            &HashSet::new(),
            offsets,
            since_ms,
            || {
                let groups = discover(root, &mut self.warned_tmp)?;
                discovered = Some(
                    groups
                        .iter()
                        .flat_map(|(_, files)| files.iter().map(|f| f.path.clone()))
                        .collect::<HashSet<_>>(),
                );
                Ok(groups)
            },
            |f| allowed(root, &f.path),
            |sid, f, handle, start, size| {
                if start == 0 || self.files.get(&f.path).is_some_and(|s| s.offset != start) {
                    self.files.remove(&f.path);
                }
                let seed = start > 0 && !self.files.contains_key(&f.path);
                let state = self.files.entry(f.path.clone()).or_default();
                if state.session_id.is_none() {
                    state.session_id = f
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(str::to_string);
                }
                state.offset = u64::MAX;
                if seed {
                    handle.seek(SeekFrom::Start(0))?;
                    let mut line = Vec::new();
                    BufReader::new(handle.take(start.min(64 * 1024)))
                        .read_until(b'\n', &mut line)?;
                    fold_line(&line, sid, f.is_main, state, None);
                }
                let end = read_lines(handle, start, Some(size), |line| {
                    fold_line(line, sid, f.is_main, state, Some((&mut accs, &mut events)));
                })?;
                state.offset = end;
                let session = if f.is_main {
                    state.session_id.as_deref().unwrap_or(sid)
                } else {
                    sid
                };
                if let Some(acc) = accs.get_mut(session)
                    && acc.cwd.is_none()
                {
                    acc.cwd = project_path(root, f);
                }
                Ok(end)
            },
        )?;
        if let Some(paths) = discovered {
            self.files.retain(|path, _| paths.contains(path));
        }
        // The collector counts batch events; re-appends must appear only once per batch.
        let mut event_ids = HashSet::new();
        events.retain(|event| event_ids.insert(event.event_id.clone()));
        events.sort_by(|a, b| (a.ts_ms, &a.event_id).cmp(&(b.ts_ms, &b.event_id)));
        let sessions = accs
            .into_iter()
            .map(|(session_id, mut acc)| {
                acc.timestamps.sort_unstable();
                SessionDelta {
                    session_id,
                    project_path: acc.cwd,
                    model: acc.model.map(|(_, model)| model),
                    timestamps_ms: acc.timestamps,
                    last_error: None,
                    recorded_cost: None,
                }
            })
            .collect();
        Ok(PollBatch {
            provider: Provider::Gemini,
            files,
            events,
            sessions,
            more,
            rate_limits: None,
        })
    }

    fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dir_wins_when_set_and_non_empty() {
        let s = |v: &str| Some(OsString::from(v));
        assert_eq!(
            root_from(s("/q"), s("/home/x")),
            Some(PathBuf::from("/q/.gemini"))
        );
        assert_eq!(
            root_from(s(""), s("/home/x")),
            Some(PathBuf::from("/home/x/.gemini"))
        );
        assert_eq!(
            root_from(None, s("/home/x")),
            Some(PathBuf::from("/home/x/.gemini"))
        );
        assert_eq!(root_from(None, s("")), None);
        assert_eq!(root_from(None, None), None);
    }
}
