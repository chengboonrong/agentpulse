//! Qwen Code adapter: tails only `projects/*/chats/*.jsonl`.
//!
//! Privacy: structs name only metadata and token counts. Content is skipped
//! by serde; no line text is stored or logged. Symlinks are not followed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentpulse_core::{
    AgentAdapter, FileCursor, LiveSession, PollBatch, Provider, SessionDelta, TokenUsage,
    UsageEvent,
};
use serde::Deserialize;

use crate::tail::{Groups, Tailer, TranscriptFile, read_lines, ts_ms};

pub struct QwenAdapter {
    root: PathBuf,
    tail: Tailer,
}

impl QwenAdapter {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tail: Tailer::new(),
        }
    }

    /// `$QWEN_RUNTIME_DIR` if set and non-empty, else `~/.qwen`.
    pub fn default_root() -> Option<PathBuf> {
        root_from(
            std::env::var_os("QWEN_RUNTIME_DIR"),
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
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".qwen"))
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    uuid: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>,
    r#type: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    usage_metadata: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
    cached_content_token_count: Option<u64>,
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

fn discover(root: &Path) -> std::io::Result<Groups> {
    let projects = root.join("projects");
    if !is_dir(root) || !is_dir(&projects) {
        return Ok(vec![]);
    }
    let mut groups: BTreeMap<String, Vec<TranscriptFile>> = BTreeMap::new();
    for project in fs::read_dir(projects)?.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let chats = project.path().join("chats");
        if !is_dir(&chats) {
            continue;
        }
        let Ok(files) = fs::read_dir(chats) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if !file.file_type().is_ok_and(|t| t.is_file())
                || path.extension().is_none_or(|x| x != "jsonl")
            {
                continue;
            }
            let Some(sid) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            groups
                .entry(sid.to_string())
                .or_default()
                .push(TranscriptFile {
                    path,
                    is_main: true,
                });
        }
    }
    for files in groups.values_mut() {
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(groups.into_iter().collect())
}

#[derive(Default)]
struct Acc {
    timestamps: Vec<i64>,
    cwd: Option<String>,
    model: Option<(i64, String)>,
}

impl AgentAdapter for QwenAdapter {
    fn provider(&self) -> Provider {
        Provider::Qwen
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
        let (files, more) = self.tail.poll(
            &HashSet::new(),
            offsets,
            since_ms,
            || discover(root),
            |f| allowed(root, &f.path),
            |sid, _, handle, start, size| {
                read_lines(handle, start, Some(size), |line| {
                    let Ok(r) = serde_json::from_slice::<Record>(line) else {
                        return;
                    };
                    let Some(ts) = r.timestamp.as_deref().and_then(ts_ms) else {
                        return;
                    };
                    let acc = accs.entry(sid.to_string()).or_default();
                    acc.timestamps.push(ts);
                    // Storage keeps a session's first path, so the first `cwd` is the one used.
                    if acc.cwd.is_none() {
                        acc.cwd = r.cwd.filter(|s| !s.is_empty());
                    }
                    if r.r#type.as_deref() != Some("assistant") {
                        return;
                    }
                    if let Some(model) = r.model.as_ref().filter(|s| !s.is_empty())
                        && acc.model.as_ref().is_none_or(|(t, _)| ts >= *t)
                    {
                        acc.model = Some((ts, model.clone()));
                    }
                    let (Some(u), Some(uuid), Some(session_id)) =
                        (r.usage_metadata, r.uuid, r.session_id)
                    else {
                        return;
                    };
                    let cached = u.cached_content_token_count.unwrap_or(0);
                    events.push(UsageEvent {
                        event_id: format!("{session_id}:{uuid}"),
                        session_id: sid.to_string(),
                        model: r.model.unwrap_or_default(),
                        ts_ms: ts,
                        usage: TokenUsage {
                            input: u.prompt_token_count.unwrap_or(0).saturating_sub(cached),
                            cache_read: cached,
                            output: u.candidates_token_count.unwrap_or(0),
                            ..Default::default()
                        },
                        fast: false,
                        us_inference: false,
                        unknown_tier: false,
                        recorded_usd: None,
                    });
                })
            },
        )?;
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
            provider: Provider::Qwen,
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
        assert_eq!(root_from(s("/q"), s("/home/x")), Some(PathBuf::from("/q")));
        assert_eq!(
            root_from(s(""), s("/home/x")),
            Some(PathBuf::from("/home/x/.qwen"))
        );
        assert_eq!(
            root_from(None, s("/home/x")),
            Some(PathBuf::from("/home/x/.qwen"))
        );
        assert_eq!(root_from(None, s("")), None);
        assert_eq!(root_from(None, None), None);
    }
}
