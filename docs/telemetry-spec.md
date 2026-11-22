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
| Plan limit (used %, window, reset) | Measured | Codex `token_count` records; Claude Code's status line payload, only when the user installs the status line command from the README |
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

### Plan limits
Claude Code writes no plan-limit data to disk: transcripts carry none. It passes the plan's
windows only to the `statusLine` command, as `rate_limits` in the JSON on its stdin, e.g.
`{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{…}}`, with
`resets_at` in Unix seconds ([status line docs](https://code.claude.com/docs/en/statusline)).
`rate_limits` comes only for Pro, Max and Ultra plans, after a session's first response, and
each window can be missing.
- AgentPulse never edits Claude Code's settings. The README gives a `statusLine` command for
  the user to install. It saves the whole payload to `agentpulse-rate-limits.json` under the root, by
  write-then-rename through a per-process temporary file, only when the payload contains `"rate_limits"`, and passes the
  payload unchanged to the user's own status line command.
- Each poll reads that file. Only `rate_limits.five_hour` and `.seven_day`, each
  `used_percentage` and `resets_at`, are deserialized. The rest of the payload (model, working
  directory, cost, `spend_limit`…) is skipped unread. A missing or malformed file means no
  reading, not an error.
- The reading's time is the file's modification time, so it is as current as Claude Code's
  last status line update. Claude Code sends no window length, so `five_hour` is a 300-minute
  window and `seven_day` a 10080-minute one, named "5h" and "weekly" like Codex's.
- The status line docs list no per-model weekly windows (`seven_day_opus`,
  `seven_day_sonnet`, which the OAuth usage API has), so none are read. AgentPulse does not
  call that API.
- No credential is read and no host is contacted. Out-of-date handling and the "as of" note
  are the same as for Codex.

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
- Claude Code's limits come from elsewhere: see [Claude Code plan limits](#plan-limits).

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

## Grok CLI

Root: `$GROK_HOME`, else `~/.grok`. A missing root or `sessions/` folder is not an error;
Grok simply shows no sessions. Verified against grok 1.0.88.

### Source files and privacy
- `sessions/<percent-encoded cwd>/<session id>/`: one folder per session. The folder name
  is the session id; its parent's name decodes exactly to the working folder
  (`%2FUsers%2Fx%2Fp` → `/Users/x/p`), so no file is read for it.
- Only two files per session are ever opened: `usage.json` and `events.jsonl`. Discovery
  yields no other name, and symlinks are not followed. The session folder also holds the
  conversation (`chat_history.jsonl`, `updates.jsonl`), `system_prompt.txt`,
  `prompt_context.json`, `tool_definitions.json` and `summary.json`; none is opened.
- `events.jsonl` (append-only, one JSON object per line) is parsed into a struct naming
  only `ts` and `type`; every other field (tool names, MCP targets, error messages) is
  skipped unread.
- Of `usage.json` only `updatedAt` and, per entry of `turns`, `turnNumber`, `endedAt`,
  `primaryModelId` and `modelUsage.<model>` (`inputTokens`, `outputTokens`,
  `cachedReadTokens`, `cacheCreationTokens`, `costUsdTicks`) are read. The `session`
  totals are not read.

| File | Read as | Use |
|---|---|---|
| `events.jsonl` `turn_started`, `turn_ended` | `ts`, `type`, `model_id` | Busy or not, model before the first turn ends |
| `events.jsonl` `permission_requested`, `permission_resolved` | `ts`, `type` | Waiting or not |
| `events.jsonl` any other line | `ts` | Activity, session start |
| `usage.json` | fields above | Tokens, cost, model |

### Token accounting
- `usage.json` is rewritten whole at the end of each turn and only grows. It is read in
  full whenever its size changes; one file caught mid-write does not parse, so its cursor
  is not advanced and the next poll retries.
- One event per turn and model in `modelUsage`. Event id is `<session id>:<turnNumber>:<model>`,
  so re-reading earlier turns upserts the same rows; time is `endedAt` (else `updatedAt`).
  Entries with no tokens are skipped.
- `inputTokens` includes `cachedReadTokens`, and `outputTokens` includes `reasoningTokens`
  (both checked: per-call prompt and completion tokens summed exactly to them). Buckets:
  cache read = `cachedReadTokens`; input = `inputTokens` − cached, saturating at 0;
  output = `outputTokens`; cache write (5 min) = `cacheCreationTokens` (0 in every local
  file). `reasoningTokens` is not read.
- The session's model is the newest turn's `primaryModelId`. Until the first turn ends there
  is no `usage.json`, so meanwhile the model is `turn_started.model_id` (the model chosen,
  e.g. `grok-4.7`, where `usage.json` later names the billed one, `grok-4.7-build`).
- Each turn's end time also counts as activity, so a session whose `events.jsonl` is
  missing still shows.
- `turn_started.turn_number` (0-based) is not matched to `usage.json`'s 1-based
  `turnNumber`; events are not used for tokens at all.

### Live sessions and status
- A session is live when a `grok` process holds its `events.jsonl` open (Grok keeps it
  open for writing for the whole run). The installer's `grok` is a symlink to a versioned
  binary, so the kernel may name the process `grok` or after the target, cut to 16 bytes
  (`grok-1.0.88-maco`); both match. The loose match is safe because only processes
  holding a log under the Grok home count. Same libproc scan as Codex, at most every 5 s,
  limited to logs under the adapter's Grok home. `active_sessions.json` is not used: it
  read `[]` while sessions were running.
- Status of a live session: Waiting if its latest `permission_requested` has no later
  `permission_resolved`, `turn_started` or `turn_ended` and was made at least 1 s ago; else
  Active if its latest `turn_started` has no later `turn_ended`; else Idle. Grok logs a
  permission request for every tool call, auto-approved ones included: all 4,091 on the
  audit machine resolved within 6 ms, so the 1 s floor keeps them from showing as Waiting.
- A `grok` process holding several sessions' logs is shared, with the same rules as Codex
  (no resources, live only with activity within the last hour).
- Tailing resumes mid-file, so each log's busy and waiting state is kept across polls and
  re-learned from the earlier lines after a restart. Without an open log the generic
  transcript-age rule applies; a Grok session ends as Completed, never Failed.

### Cost
Grok's own per-turn charge, not an estimate: `costUsdTicks` / 1e10 (xAI bills in ticks of
US$1e-10, see docs.x.ai cost tracking). It replaces the price table for that event, so no
Grok rows are in `pricing.toml`. A model entry without `costUsdTicks` falls back to the
price table, where it is unpriced.

### Known gaps
- Sandbox `strict` and custom profiles run through Grok's daemon keep a separate store
  (`~/.grok/daemon/run/<name>`); its layout is unverified and it is not read.
- Plan limits are not read.
- Tokens and cost arrive when a turn ends: a long turn shows its model and activity but no
  new tokens until then.
- One permission request is tracked at a time; if parallel tool calls ever overlap their
  prompts, the first answer ends Waiting early.
- Discovery skips symlinks and each read re-checks just before opening; a file swapped
  for a symlink in between could still be followed.

## Qwen Code

Root: `$QWEN_RUNTIME_DIR` if set and non-empty, else `~/.qwen`. A missing root or
`projects/` folder is not an error; Qwen simply shows no sessions. A symlinked root is
followed; nothing below it is. Verified format:
Qwen Code 0.14.5.

### Source files and privacy
- Only `projects/<sanitised cwd>/chats/<session id>.jsonl` is opened: one append-only
  JSONL file per session. Symlinked files and folders are skipped, including on cached
  reads. No settings, credentials or other runtime files are opened.
- The session id is the file stem. The sanitised project folder cannot be decoded, so
  the project path comes from the first record's `cwd` (storage keeps a session's first path).
- Records are deserialized into metadata-only structs: `uuid`, `sessionId`, `timestamp`,
  `type`, `cwd`, `model` and `usageMetadata` token counts (`promptTokenCount`,
  `candidatesTokenCount`, `cachedContentTokenCount`). All content fields are skipped
  by serde; no line text is stored or logged. Malformed lines are skipped.
- Every record's timestamp counts as activity; the first timestamp starts the session.
  The model comes from the newest assistant record. Tailing uses the shared transcript
  reader and budgets, resumes at stored offsets and retries incomplete final lines.

### Token accounting
- One event per `assistant` record with `usageMetadata`, keyed by `<sessionId>:<uuid>`.
- Cache read = `cachedContentTokenCount`; input = `promptTokenCount` minus cached,
  saturating at zero; output = `candidatesTokenCount`; cache writes = zero.
- Candidates already include thoughts, so `thoughtsTokenCount` is not added.
  `totalTokenCount` is not used.
- `system` / `ui_telemetry` records repeat API responses and add no tokens.

### Live sessions and status
- No live sessions or process resources are reported. Qwen runs as `node`, and matching
  only its process name and working directory could claim an unrelated Node process.
- The core's transcript-age rule applies: active within 30 seconds, ended after 5 minutes
  of silence, as Completed. Qwen sessions never show Waiting or Failed.

### Cost
Unpriced: local sessions use Qwen OAuth (`qwen-oauth`) with model alias `coder-model`,
which has no list price. No charge or price is supplied; cost is unavailable, never $0.

### Known gaps
- No process, CPU, memory or orphan clean-up. Matching Qwen's Node argv is future work.
- Waiting on the user for more than 5 minutes shows as ended until another record arrives.
  Permission prompts are not distinguished.
- Subagent (`task` tool) tokens appear only in `ui_telemetry` and are not counted.
- A `runtimeBaseDir` set in Qwen's settings, rather than the environment, is not followed.
- Plan limits are not read.
- As with Grok, a component swapped for a symlink between its check and opening the file
  could still be followed.

## OpenCode

Store: `$XDG_DATA_HOME/opencode/opencode.db`, else `~/.local/share/opencode/opencode.db`
(the XDG path on macOS too). A missing file is not an error; OpenCode simply shows no
sessions. Verified against opencode 1.19.3.

### Source and privacy
- One SQLite database in WAL mode, opened read-only (250 ms busy timeout, reopened after an
  error). OpenCode keeps it open while running.
- It also holds every prompt, reply, tool call and diff: `message.data`, `part.data` and the
  session's `title`, `summary_diffs`, `revert`, `permission` and `metadata`. None of these is
  ever selected. Every value is pulled inside SQLite with `json_extract` / `json_type`
  naming only `role`, `modelID`, `time.completed`, `tokens.*` and `cost`, so content never
  leaves SQLite. Of `error` only whether it is set is read (`json_type IS NOT NULL`); of
  `part` only `time_updated`. A unit test checks that no query names a `data` column outside
  those functions.

| Table | Read | Use |
|---|---|---|
| `session` | `id`, `parent_id`, `directory`, `time_created`, `time_updated` | Root session, folder, live match |
| `message` | `id`, `session_id`, `time_created`, `time_updated`, the JSON fields above | Tokens, cost, model, activity, error |
| `part` | `session_id`, `time_updated` | Busy recency |

### Token accounting
- One event per assistant message with `time.completed` set; event id is the message id,
  time is `time.completed`. Messages with no tokens are skipped.
- `tokens = {input, output, reasoning, cache: {read, write}}` are disjoint (where OpenCode
  records `total`, it equals their sum). Buckets: input = `input`; output = `output` +
  `reasoning` (reasoning is billed as output); cache read = `cache.read`; cache write
  (5 min) = `cache.write`.
- A subagent session (`parent_id` set) counts toward its root session, as in Codex. The
  session's folder is the root's `directory`; its model is the root's newest response's
  `modelID`; its error flag is whether that response has an `error`.
- Activity: every message's `time_created`, plus each `time.completed`.
- Incremental: the cursor is a `message.time_updated` watermark in ms (stored as the store
  path's offset, so the dump's "bytes read" figure is meaningless for OpenCode). A message
  updated later is read again under the same id and upserted. The first run reads messages
  updated since the backfill start. At most 5,000 messages per poll; an unchanged store
  (size and time of the database and its WAL) is not queried.

### Cost
OpenCode's own per-message `cost` replaces the price table when it is a number. Free and
local models record 0. A message without `cost` falls back to the price table.

### Live sessions and status
- Every `opencode` process (TUI and `opencode acp`) holds the one database open, so open
  files cannot tell sessions apart. The process's working folder does: at most every 5 s,
  libproc gives each `opencode` process's cwd (`PROC_PIDVNODEPATHINFO`, start time
  verified). Its session is the newest root session with `directory` equal to that folder
  and written to (session or message `time_updated`) since the process started. Two
  processes resolving to one session: the newer process keeps it.
- Status: Active when the session's newest response has no `time.completed` and no error
  and the session or one of its subagents wrote a message or part within the last 2 min;
  else Idle. An interrupted response stays unfinished for good, hence the recency rule.
  Never Waiting: permission prompts are not stored.

### Known gaps
- One session per process: `opencode serve` / `web` hosting several folders is not
  attributed, and a process whose folder differs from the session's `directory` is not
  matched.
- Sessions older than the backfill start are not read until they change.

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

An event that carries the agent's own charge (Grok's `costUsdTicks`, OpenCode's `cost`) uses that figure
instead of the price table.

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

## Network

Without a paired phone, the update check is AgentPulse's only network request made on its
own: an HTTPS GET of
`https://api.github.com/repos/chengboonrong/agentpulse/releases?per_page=10` through an
ephemeral NSURLSession (no cookies or cache kept). Its headers are `User-Agent:
AgentPulse/<version>`, `Accept: application/vnd.github+json` and `Accept-Language: en` (so
the Mac's language is not sent), plus the standard `Accept-Encoding` that macOS adds; there
is no body. GitHub sees the IP address, as with any connection. The reply is the public
release list; AgentPulse reads each release's tag, page URL, notes, draft flag and date,
ignores drafts and releases whose page is not under
`https://github.com/chengboonrong/agentpulse/releases/` (or contains `..` or `%2e`), and
compares the newest version with its own. It runs every 2 hours while "Check for updates
automatically" is on (the default), at the next data refresh if a check became due while the
Mac slept, and whenever "Check Now" is clicked, on or off. A failed check keeps any update
already found. Nothing is downloaded or installed by itself: a newer release shows a notice.

Clicking "Install Update" (the notice or Settings) or the menu's "Update Available" item loads
Sparkle 2, which is not started before that click; for a release GitHub marks as a
pre-release, which Sparkle's feed never has, it opens the release's page instead. Sparkle then makes its own HTTPS requests through an NSURLSession with
cookies refused (`HTTPCookieStorage` accept policy "never"): a GET of
`https://github.com/chengboonrong/agentpulse/releases/latest/download/appcast.xml`, and, only
after the user confirms in Sparkle's window, a GET of the zip the appcast names
(`https://github.com/chengboonrong/agentpulse/releases/download/v<version>/AgentPulse-<version>.zip`).
GitHub redirects both to `release-assets.githubusercontent.com`. Headers: `User-Agent:
AgentPulse/<version>`, `Accept-Language: en`, the standard `Accept-Encoding`, and `Accept:
application/rss+xml,*/*;q=0.1` for the appcast or `Accept: */*` for the zip; no body. The
app's `Info.plist` turns off Sparkle's scheduled checks (`SUEnableAutomaticChecks`),
automatic installs (`SUAllowsAutomaticUpdates`) and system profiling (`SUSendProfileInfo`,
and `SUEnableSystemProfiling` so it is never offered), so it never checks by itself and
sends no profile; the appcast carries no release notes, so no notes page is fetched. Sparkle
refuses an appcast whose EdDSA signature does not match the app's `SUPublicEDKey`
(`SURequireSignedFeed`), checks the zip's EdDSA signature against that key before
extracting it (`SUVerifyUpdateBeforeExtraction`), then checks that the new app's code
signature is valid. A refused update shows Sparkle's error and the running version stays.

The phone link (docs/mobile.md) is opt-in and makes no request until a phone is being paired
or is paired. It talks only to the relay: the AgentPulse relay at `https://relay.snitco.com`
unless a relay address is set in Phones (in the sidebar), and then only that one; a pairing keeps
the relay it was made on. Over HTTPS (plain HTTP only for `localhost`), through NSURLSession
with `User-Agent: AgentPulse/<version>`, `Accept: application/json` and `Accept-Language:
en`. Requests: `POST /v1/pairings` to create a pairing (the relay key, if one is saved, only
here and only to the own relay); `GET
/v1/pairings/<id>/up` every 2 s while a QR code is shown, then after each publish and every
30 s; `PUT /v1/pairings/<id>/down` with the state when the widget payload changes (at most
every 10 s, else every 5 minutes); `DELETE /v1/pairings/<id>` when a pairing is removed,
cancelled, declined or its QR code expires. Bodies hold random
pairing ids, token hashes and ciphertext only: the state is the widget payload (live
sessions' folder names, counts, today's tokens and cost, the output rate, the cat), sealed
with HPKE for that phone, so the relay sees its size and timing, not its content; the relay
sees the IP address, as with any connection. "Direct connection" (on by default) adds a
local-network listener while a phone is paired and has a state to receive: Bonjour
`_agentpulse._tcp`, instance "AgentPulse", TXT `v=1`, and over each connection the same
state messages and a heartbeat every 30 s, encrypted with keys from the pairing.
