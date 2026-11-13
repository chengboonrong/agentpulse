# Telemetry specification

What AgentPulse reads, how each number is derived, and how sure it is. Verified against
Claude Code 2.1.322 and Codex CLI 0.164.0 on macOS 27.0.

## Measurement classes

| Value | Class | Source |
|---|---|---|
| Tokens (input, output, cache write 5m/1h, cache read) | Measured | Agent's own usage records |
| Model | Measured | Agent's records |
| Session start, last activity | Measured | Record timestamps |
| Live state (busy / idle / waiting) | Measured | Claude Code's live-session registry; for Codex, the rollouts a `codex` process holds open plus its task records (no waiting state) |
| Process CPU, resident memory | Measured | OS process table |
| Cost | **Estimated** | Tokens × price table |
| Claude Code's own session cost | Measured | The agent's `cost-state` record (per session, for the run that wrote it). Codex has no such record: always "—" |
| Codex plan limit (used %, window, reset) | Measured | Codex `token_count` records; Claude Code has none |
| Active execution time | **Estimated** | Gaps between records ≤ 60 s |
| Status of a session with no registry entry | **Estimated** | Transcript age (30 s / 5 min) |
| Token rate | Derived | 30 s rolling window over measured tokens |
| GPU utilisation, whole Mac | Measured | IORegistry `PerformanceStatistics` (undocumented key) |
| Per-agent GPU | **Unavailable** | No documented macOS API |
| Waiting time, idle time as separate totals | **Unavailable** | Only the current state is observable |

## Claude Code

Root: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.

### Transcripts
- `projects/<slug>/<sessionId>.jsonl`: the session.
- `projects/<slug>/<sessionId>/subagents/*.jsonl`: subagents of that session. Their tokens
  count toward the parent.

One JSON object per line. Fields read:

| Field | Use |
|---|---|
| `type` | `assistant` records carry usage |
| `timestamp` | Activity timeline, event time |
| `cwd` | Project path |
| `isApiErrorMessage` | Failed-session detection |
| `sessionId` | Which session the record belongs to (falls back to the file's id) |
| `message.id`, `requestId`, `uuid` | Event identity, in that order of preference |
| `message.model` | Model; `<synthetic>` records are not API calls and are skipped |
| `message.usage.*` | Tokens, `speed`, `inference_geo` |

No other field is deserialized. In particular `message.content` is never parsed into memory.

### Claude Code's cost record
Claude Code also appends `cost-state` records to the main transcript:
`{"type":"cost-state","sessionId":…,"totalCostUSD":…,"startTime":…,"totalDuration":…,"hasUnknownModelCost":…,"modelUsage":{…}}`.
Only `sessionId`, `totalCostUSD`, `startTime`, `totalDuration` and `hasUnknownModelCost` are
read; `modelUsage` is not.

- It is written at the end of the file when a session ends or saves its state, so a running
  session often has none yet. On the development machine 123 of 127 transcripts had one.
- Records are cumulative: a file can hold several with the same `startTime` and growing
  `totalCostUSD` and `totalDuration`. The figure is current at `startTime + totalDuration`,
  and the one with the latest such time wins, in storage as well as within one read.
- It belongs to the session named in the record, like usage records.
- A record whose `modelUsage` names no model carries $0 whatever the session spent (35 of
  138 records on the development machine), so it is skipped rather than allowed to win.
  Only the number of model names is read; their contents are skipped unread.
- `startTime` is when that Claude Code process started, so the figure covers that run. No
  local session had records from two runs, so whether a resumed run carries the earlier
  total over is unverified.
- It is per session, not per day, so it is shown beside each session's estimate and never
  added to the "today" totals. `hasUnknownModelCost` means Claude Code had no price for a model the
  session used, so its figure is short by that amount; the app marks it `+?`.
- It is the agent's own number and is shown as such ("Claude Code $1.24"), next to
  AgentPulse's estimate, which stays labelled as an estimate. A session without a record
  shows "—".

### Token accounting
- Claude Code writes one `assistant` record per content block, each repeating the response's
  full usage. Events are therefore keyed by `message.id` and stored once.
- A response can be logged first with partial usage and again with full usage; the larger
  record wins.
- Claude Code copies history into other session files (resume, fork). A copied record keeps
  its original `sessionId`, so it is credited to the session that produced it.
- `input_tokens` excludes cached tokens, so the five buckets are disjoint and sum to the total.
- Cache writes use `cache_creation.ephemeral_5m_input_tokens` and `ephemeral_1h_input_tokens`.
  If only the combined `cache_creation_input_tokens` is present, it is treated as 5-minute
  writes (the API default).
- Thinking tokens are already included in `output_tokens`.
- Records with zero tokens are skipped.

### Live sessions
`sessions/<pid>.json`, written by each running Claude Code process:

| Field | Use |
|---|---|
| `pid`, `procStart` | Process identity. `procStart` is `ps -o lstart=` in UTC |
| `sessionId` | Join to the transcript |
| `status` | `busy` → Active, `waiting` → Waiting, `idle` → Idle |
| `startedAt`, `cwd` | Start time and project when the transcript is still empty |

A registry file is trusted only if the PID exists and the OS start time is within 2 s of
`procStart`. Stale files from crashed processes fail that check and are ignored.

## Codex CLI

Root: `$CODEX_HOME`, else `~/.codex`. A missing root is not an error; Codex simply shows no
sessions.

### Rollouts
- `sessions/YYYY/MM/DD/rollout-<timestamp>-<thread id>.jsonl` and `archived_sessions/…`:
  one file per thread. The thread id is the last 36 characters of the file name.
- Moving a rollout to the archive re-reads it under its new path; events are keyed by
  response id, so the totals do not change.
- Compressed archives (`.jsonl.zst`) are not read.

Rollout lines carry prompts, responses, reasoning, tool calls and file contents. The record
type sits in a line's first 256 bytes and is matched there before any parsing. Only these
records are deserialized, into structs that name only the fields below:

| Record | Fields read | Use |
|---|---|---|
| `session_meta` | `timestamp`, `payload.id`, `payload.session_id`, `payload.cwd` | Thread start, its root session, project path |
| `turn_context` | `payload.turn_id`, `payload.model` | Model per turn |
| `event_msg` `thread_settings_applied` | `thread_settings.model`, `thread_settings.service_tier` | Fallback model; pricing tier |
| `event_msg` `task_started`, `task_complete`, `turn_aborted` | type only | Busy or not |
| `event_msg` `token_count` | `timestamp`, `payload.rate_limits.primary` and `.secondary` (`used_percent`, `window_minutes`, `resets_at`), `payload.rate_limits.plan_type` | Plan limit |
| `token_usage_record` | `timestamp`, `session_id`, `turn_id`, `response_id`, `usage.*` | Tokens |

Every other line (messages, tool calls and outputs, reasoning, compaction) contributes only
the timestamp at its start, as activity. A line whose type is not found in
its first bytes is parsed for the metadata fields above rather than dropped.

### Token accounting
- One `token_usage_record` per response. Event id is `response_id`; time is the record's
  timestamp; session is `session_id`, the root session, so subagent threads count toward
  their parent.
- `input_tokens` includes cached input. Buckets: cache read = `cached_input_tokens`;
  input = `input_tokens` − cached, saturating at 0; output = `output_tokens`. Cache writes
  (`cache_write_input_tokens`) stay in input, as OpenAI bills them as normal input; no
  cache-write bucket is filled. Every local record had 0 cache writes.
- The token figures of `token_count` events (`payload.info`) are not read. They repeat the
  same usage cumulatively.
- Model: the `turn_context` of the record's `turn_id`, else the latest
  `thread_settings_applied` model. The session's model shown is the root thread's newest.
- Tailing resumes mid-file, so each rollout's model, tier, session and busy state are kept
  across polls. A rollout first opened at a stored offset (after a restart) has its earlier
  lines re-read for that state only; their usage is not counted again.

### Plan limits
`token_count` events carry the subscription plan's usage, e.g. `{"limit_id":"codex",
"primary":{"used_percent":62.0,"window_minutes":10080,"resets_at":1795138706},
"secondary":null,"plan_type":"prolite",…}` (verified on local rollouts, Codex 0.164.0).
- Only the fields in the table above are deserialized; `credits`, `limit_id` and the rest
  are skipped.
- The newest reading by record timestamp wins, across all rollouts read in this run.
  It is kept in memory only. After a restart it is re-learned from rollouts read from the
  start or seeded (an open rollout, or one resumed mid-file); with no Codex running and
  nothing new to read, there is no reading until Codex writes one.
- The window is named from `window_minutes` (10080 weekly, 300 "5-hour"). A reading whose
  `resets_at` has passed is shown as out of date, not as current.
- Claude Code writes no plan-limit data to disk, so none is shown for it.

### Live sessions and status
- A rollout is live when a process named `codex` holds it open. Open files come from libproc
  (`proc_pidinfo(PROC_PIDLISTFDS)`, then `proc_pidfdinfo(PROC_PIDFDVNODEPATHINFO)` per
  descriptor), checked against the process's start time to rule out PID reuse. The whole
  scan runs at most every 5 s, so a session is seen starting or ending up to 5 s late.
  Only open rollouts under the adapter's Codex home count; one under another `CODEX_HOME`
  is ignored.
- Status of a live session: Active if its latest `task_started` has no later `task_complete`
  or `turn_aborted`, otherwise Idle. Codex records no waiting-on-you state, so a Codex
  session is never Waiting.
- A `codex` process with exactly one root session open is that session's process, with CPU
  and memory of its tree. A process holding several (the VS Code extension and app-server
  run many threads in one process) makes each of them live, but none gets its resources:
  they show unavailable, and the process is counted for no one, including any agent it
  runs under. Such processes keep rollouts from days ago open, so a session there is live
  only if any of its rollouts held by that process has a line timestamped within the last
  hour; otherwise it is treated like any session without an open rollout. A session whose
  process holds only its rollouts stays live however old they are.
- Without an open rollout the generic transcript-age rule applies. Codex has no API-error
  marker that was verified, so a Codex session ends as Completed, never Failed.

### Cost
Rates are OpenAI API list prices (source and date in `pricing.toml`). A request above
272,000 prompt tokens uses the long-context rates. `service_tier` `fast` or `priority` is
billed at 2×; `default` or no tier at standard rates; any other tier is left unpriced. Cache
writes are priced as input (see Token accounting); the cache-write rate is unused.

## Usage charts

All from `usage_events`; costs are the per-response estimates stored at ingestion.

| Chart | Derivation |
|---|---|
| Daily estimated cost | `SUM(cost_usd)` per agent per local day (`date(ts/1000,'unixepoch','localtime')`), last 30 days. Days before an agent's first stored event are "no data", after it a day with no events is a measured $0. Unpriced events are counted and named, never added as $0 |
| Cache efficiency | Per day: cache read ÷ (input + cache writes + cache read), all agents. Net saved ≈ Σ cache-read tokens × (input rate − cache-read rate) − Σ cache-write tokens × (cache-write rate − input rate), per model and write bucket (5 m, 1 h), at standard rates (long-context and fast requests differ), over 30 days. Events without a cost (model or tier unpriced) are skipped, as in the cost chart, and counted. Shown as no data while a day in the window has not been queried yet |
| Today's timeline | One lane per session with events since local midnight. Consecutive API responses at most 60 s apart (`ACTIVE_GAP_MS`) form a solid run; a lone event is a tick. Lanes use usage events only, so they are not the active-time figure. Each lane is labelled with its first request's local time |
| Session detail | Per event: context size = input + cache writes + cache reads, with the model's long-context threshold from the price table when it has one; cumulative estimated cost, unpriced events adding nothing and counted |

A day where AgentPulse was not running and whose transcripts were never read later shows
as $0, not as no data: the database cannot tell the two apart.

## Session status

Evaluated every collector tick by `agentpulse_core::resolve_status`:

1. Verified live process: status is the agent's own state. An alive process is never
   reported as Active unless the agent says it is busy.
2. No verified process: Active if the transcript changed within 30 s, Idle within 5 min,
   otherwise ended.
3. Ended: Failed if the newest main-conversation response was an API error, else Completed.

## Runtime

- **Wall clock**: first record to last record, or to now while the session is live.
- **Active execution (estimated)**: the sum of gaps between consecutive records, across the
  session's main and subagent transcripts, that are at most 60 s. A long tool call or a slow
  response with no intermediate records is undercounted; this is a floor, not an exact figure.

## Cost

`cost = Σ bucket_tokens × rate / 1e6`, per response, times 2.0 for fast mode and 1.1 for
US-only inference. Haiku 5.5 switches to its long-context rates when a request's prompt
exceeds 100,000 tokens. Rates are first-party API list prices; subscription plans and cloud
partners bill differently, which is why the figure is labelled an estimate. Responses from
a model with no price are counted as unpriced and shown as such.

The price table was checked against Claude Code's own `cost-state` records: recomputing a
recorded Sonnet 5.5 figure gave the same value to seven decimal places.

## Processes

- Table sampled with libproc calls (`proc_listallpids`, `proc_pidinfo`, `proc_pid_rusage`)
  only while at least one live session exists.
- CPU is percent of one core since the previous sample, summed over the agent's process
  tree, so it can exceed 100.
- Memory is resident set size summed over the tree. Shared pages are counted in each process
  that maps them, as in Activity Monitor's per-process figure.
- A process belongs to its nearest agent ancestor. Nested agents do not double count.

## GPU

- Whole Mac only: `IOServiceMatching("IOAccelerator")` → `IORegistryEntryCreateCFProperty(…,
  "PerformanceStatistics")` → `"Device Utilization %"`, the highest across accelerators that
  report it (a Mac with two GPUs lists both), rounded and clamped to 0–100.
  Public IOKit calls, read once per collector tick (about 0.05 ms a read on the
  development machine).
- The key is **not a documented contract**. Any missing node, key or unexpected type yields
  "unavailable", never 0.
- It is the GPU use of every process on the Mac. It is never attributed to an agent, and it
  says nothing about remote inference. The UI labels it "whole Mac".

## Token rate and animation

Output tokens are recorded when a response completes, so they arrive in bursts. The rate
shown is output tokens over the last 30 s divided by 30. The cat animator smooths it again
(about 2 s time constant) and applies hysteresis before changing gait. Cumulative totals are
never used as a speed.

A cat also sprints when its agent is Active and its process tree is busy: the same 2 s
smoothing of the tree's CPU, on above 100% of one core, off below 60%. Either the token
latch or the CPU latch makes it sprint; the run speed still follows the token rate. CPU
counts only while the agent is Active and restarts from zero each time, so an idle agent
with a busy background process does not sprint, and an unavailable CPU reading never
triggers it. The menu-bar cat uses the CPU summed over the Active agents that have a reading.

## Planned adapters

| Agent | Source | Notes from the audit |
|---|---|---|
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | SQLite `message.data.tokens.*`; count only messages with `time.completed` |
