# Telemetry specification

What AgentPulse reads, how each number is derived, and how sure it is. Verified against
Claude Code 2.1.322 on macOS 27.0.

## Measurement classes

| Value | Class | Source |
|---|---|---|
| Tokens (input, output, cache write 5m/1h, cache read) | Measured | Agent's own usage records |
| Model | Measured | Agent's records |
| Session start, last activity | Measured | Record timestamps |
| Live state (busy / idle / waiting) | Measured | Agent's live-session registry |
| Process CPU, resident memory | Measured | OS process table |
| Cost | **Estimated** | Tokens × price table |
| Active execution time | **Estimated** | Gaps between records ≤ 60 s |
| Status of a session with no registry entry | **Estimated** | Transcript age (30 s / 5 min) |
| Token rate | Derived | 30 s rolling window over measured tokens |
| GPU utilisation | **Unavailable** | Not collected in this build |
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

## Token rate and animation

Output tokens are recorded when a response completes, so they arrive in bursts. The rate
shown is output tokens over the last 30 s divided by 30. The cat animator smooths it again
(about 2 s time constant) and applies hysteresis before changing gait. Cumulative totals are
never used as a speed.

## Planned adapters

| Agent | Source | Notes from the audit |
|---|---|---|
| Codex CLI | `$CODEX_HOME/sessions/**/rollout-*.jsonl` | `token_count` events are cumulative; diff `total_token_usage` with reset detection. Cached input is a subset of input |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | SQLite `message.data.tokens.*`; count only messages with `time.completed` |
