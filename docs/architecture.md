# Architecture

AgentPulse watches AI coding agents running on this Mac. It reads the files the agents
already write, measures their processes, and shows the result in a native window. It makes
no network calls and stores no conversation content.

## Data flow

```text
 agent files                     collector thread                         UI thread (GPUI)
┌────────────────────┐   poll   ┌──────────────────────────────┐ snapshot ┌──────────────────┐
│ ~/.claude/projects │ ───────▶ │ adapter  → PollBatch          │ ───────▶ │ views render the │
│ ~/.claude/sessions │          │ storage  → idempotent upsert  │ channel  │ latest snapshot  │
└────────────────────┘          │ cost     → price per response │          │ cat animators    │
 OS process table  ───sample──▶ │ process  → tree CPU / RSS     │          └──────────────────┘
                                │ core     → status, rates      │
                                └───────────────┬──────────────┘
                                                ▼
                                   SQLite (sessions, usage_events,
                                           ingest_files)
```

The UI never reads agent files. It receives a `Snapshot` and draws it. Collectors know
nothing about GPUI, so they are tested headless and `agentpulse --dump` prints a snapshot
as JSON with no window.

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `agentpulse-core` | Domain types, the `AgentAdapter` trait, pure rules (status, active time, rolling rate) | serde |
| `agentpulse-telemetry` | Claude Code adapter: transcript tailing, live-session registry | core |
| `agentpulse-cost` | Price table (TOML) and per-response cost | core |
| `agentpulse-process` | Process sampling through libproc, PID-reuse check, tree attribution | core, libc |
| `agentpulse-storage` | SQLite schema, migrations, idempotent ingestion, totals | core, rusqlite |
| `agentpulse-animation` | Original cat sprites and the animation state machine | core |
| `agentpulse-platform-macos` | AppKit bridge: menu-bar status item, Reduce Motion | objc2 |
| `agentpulse-app` | Collector loop, app hub, GPUI window, menu-bar cat, widget export, the `agentpulse` binary | all of the above, gpui |

The handoff brief proposed nine crates. One is not created yet because it would be
empty: `agentpulse-ui`. The UI is two screens and lives in the app crate until it is worth
splitting.

Outside the Rust workspace, `macos/` holds the Swift widget extension and `scripts/bundle.sh`
assembles and signs `AgentPulse.app`.

## Decisions

### GPUI pinned to a Zed commit
`gpui` and `gpui_platform` come from `zed-industries/zed` at
`27bbbc1cc480a7500cec122f120e796d1fb8500c`. The crates.io release (0.2.2) predates the
current API by 13 months, and the only crates.io packaging of the current API is a
third-party mirror. Both sources were built and launched on macOS 27 before choosing. The
rev is the single place to bump; `Cargo.lock` pins everything else.

`runtime_shaders` is enabled so the build does not need Xcode's separately downloaded Metal
Toolchain. Shaders compile at launch instead. For a distributed release build, install the
toolchain and drop the feature.

Unstable surface used: window and titlebar options, `canvas` painting, `cx.spawn` async
closures, window appearance and activation observers. See `repository-audit.md` for the
list and the gaps (no status item, no Reduce Motion wiring, no release-build window capture).

### Threads and a channel, no async runtime
One collector thread does blocking file and SQLite I/O and sends snapshots over a channel.
The UI awaits the channel, so it wakes when there is something new and not on a timer.
Tokio is not a dependency: nothing here is concurrent I/O. It can be added when an adapter
needs it.

### Adapters are polled and stateless about progress
`AgentAdapter::poll(offsets, since)` returns a `PollBatch`. Offsets live in SQLite and are
written in the same transaction as the data they cover, so a crash cannot skip or repeat
records. Adapters report errors and the collector carries on with the last good data; a
broken or missing agent never takes the window down.

### Every measurement says how it is known
`Reading::Measured`, `Reading::Estimated` and `Reading::Unavailable` are distinct in the
types and in the UI. Costs are always `Estimated`. CPU is `None` until two samples exist.
Sessions with no verified local process have `Unavailable` resources and no PID.

### Idempotent token accounting
A usage event is keyed by `(provider, event_id)`, where `event_id` is the API response id.
It belongs to the session named in the record itself, not to the file it was found in,
because Claude Code copies history between session files.
Re-reading a file, reading duplicate records, or seeing the same response in two files all
converge on one row. On conflict the larger usage wins, so order does not matter.
Session totals are sums over rows, never running counters.

### Prices are data
`pricing.toml` is embedded as the default and replaced wholesale by
`~/Library/Application Support/AgentPulse/pricing.toml` when that file exists. Cost is
computed per response at ingestion (long-context tiers and fast mode are per-request) and
stored, so later price changes do not rewrite history. A model with no price yields no
cost; there is no default rate.

### Process sampling uses libproc directly, not sysinfo
`sysinfo` 0.40 was tried first and dropped. On macOS it leaves a process's CPU reading at
its last non-zero value when the process goes idle, which would overstate agent CPU
indefinitely, and one sample cost 24 ms. Calling `proc_listallpids`, `proc_pidinfo` and
`proc_pid_rusage` directly costs about 3 ms, reads exactly zero for an idle process, and
matches `ps` for tree size and memory.

### Process identity is PID plus start time
A live session is attached to a process only when the PID exists and its OS start time
matches what the agent recorded. Each process is attributed to its nearest agent ancestor,
so nested agents never count a process twice.

### The app outlives its window
A hub entity, not tied to any window, owns the snapshot stream, the menu-bar cat and the
status item. The window observes the hub and holds only view state. Closing the window
leaves the collector and the menu bar running; "Open AgentPulse" in the menu or a Dock
click brings a window back, and only Quit ends the process. The window and the menu bar
read the same snapshot, so they cannot disagree.

### Menu bar through AppKit, kept cheap
GPUI has no status item, so `agentpulse-platform-macos` wraps `NSStatusItem`. The cat is a
template image (macOS tints it for light and dark menu bars) at a fixed width, so nothing in
the menu bar shifts when its pose changes. A still pose runs no timer. Motion is capped at
5 frames a second (at 10 a sprinting cat cost the app 3.5 to 3.7% of one core, at 5 it costs
2.7%), frames are cached images, and menu rows are applied only when the menu opens.
Most of the remaining per-frame cost is AppKit mirroring the status window.

### Widgets read a file, not the app
A WidgetKit extension is a sandboxed process that cannot talk to the Rust app. When run
from inside the bundle, the app writes a small `widget.json` (counts, display strings, the
cat's sprite rows; never a path or session id) and asks WidgetKit to reload, sparingly,
through a one-function Swift dylib it `dlopen`s. The extension reads that one directory
through a read-only sandbox exception. An app group container was the first choice and was
abandoned: without a provisioning profile macOS 27 denies the unsandboxed app access to it
with no prompt. `macos/README.md` has the details and the App Store caveat.

### Animation follows telemetry, not a clock
A cat's state comes from session status. Its speed comes from a smoothed rolling token
rate. Timers run only while something is animating and the window is active; with Reduce
Motion or animation disabled the cat changes pose on state changes and nothing else runs.

## Storage

`~/Library/Application Support/AgentPulse/agentpulse.db` (directory 0700, file 0600, WAL).
Override the directory with `AGENTPULSE_DATA_DIR`.

| Table | Contents |
|---|---|
| `sessions` | id, provider, project path, model, first and last activity, estimated active time, last-error flag |
| `usage_events` | one row per API response: model, time, five token buckets, estimated cost |
| `ingest_files` | byte offset per transcript file |

Schema changes are numbered migrations tracked in `PRAGMA user_version`. A database from a
newer AgentPulse is refused, not downgraded.

Not stored: prompts, responses, tool inputs or outputs, file contents, API keys. The parser
does not deserialize those fields at all.

## What is deliberately not built yet

Codex and OpenCode adapters, resource history tables, retention pruning, GPU telemetry,
settings UI, alerts, a notarized distributable. Each is listed with its phase in
`roadmap.md`.
