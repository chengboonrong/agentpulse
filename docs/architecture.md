# Architecture

AgentPulse watches AI coding agents running on this Mac. It reads the files the agents
already write, measures their processes, and shows the result in a native window. It makes
no network calls and stores no conversation content.

## Data flow

```text
 agent files                     collector thread                         UI thread (GPUI)
┌────────────────────┐   poll   ┌──────────────────────────────┐ snapshot ┌──────────────────┐
│ ~/.claude/projects │ ───────▶ │ adapters → PollBatch each     │ ───────▶ │ views render the │
│ ~/.claude/sessions │          │ storage  → idempotent upsert  │ channel  │ latest snapshot  │
│ ~/.codex/sessions  │          │ cost     → price per response │          │ cat animators    │
└────────────────────┘          │ process  → tree CPU / RSS     │          └──────────────────┘
 OS process table  ───sample──▶ │ core     → status, rates      │
 (+ codex open files)           │                               │
                                └───────────────┬──────────────┘
                                                ▼
                                   SQLite (sessions, usage_events,
                                           ingest_files)
```

The UI never reads agent files or the database. It receives a `Snapshot` and draws it.
The one message the other way is which session's detail is open: the window sends that key
over a channel, the collector wakes, queries that session's requests and carries them in
the following snapshots. Collectors know nothing about GPUI, so they are tested headless
and `agentpulse --dump` prints a snapshot as JSON with no window.

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `agentpulse-core` | Domain types, the `AgentAdapter` trait, pure rules (status, active time, rolling rate) | serde |
| `agentpulse-telemetry` | Claude Code and Codex adapters, and the file tailing they share (`tail.rs`: cached file list, hot files, 5 s rescan, read budget) | core, process |
| `agentpulse-cost` | Price table (TOML) and per-response cost | core |
| `agentpulse-process` | Process sampling through libproc, PID-reuse check, tree attribution, open files of a process | core, libc |
| `agentpulse-storage` | SQLite schema, migrations, idempotent ingestion, totals | core, rusqlite |
| `agentpulse-animation` | Original cat sprites, coats and eye colours (`coats.rs`), and the animation state machine | core |
| `agentpulse-platform-macos` | AppKit bridge: menu-bar status item, Reduce Motion; whole-Mac GPU through IOKit | objc2 |
| `agentpulse-app` | Collector loop, app hub, GPUI window, menu-bar cat, widget export, the `agentpulse` binary | all of the above, gpui |

The handoff brief proposed nine crates. One is not created yet because it would be
empty: `agentpulse-ui`. The UI lives in the app crate: `ui.rs` holds the window, Overview
and Sessions, `ui/charts.rs` the Usage screen and the session detail, and `ui/cats.rs` the
Cats screen. Charts are drawn by hand (divs for bars that carry a tooltip, `canvas` for lines
and dense series); there is no chart library.

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
records. The collector holds a list of adapters (Claude Code, and Codex when a Codex root
can be named) and polls each one in turn. Errors are kept per adapter: one failing adapter
keeps its last good data and reports its error, and the others carry on. A missing agent
directory is not an error. A broken or missing agent never takes the window down.

Sessions and live-session meters are keyed by `(provider, session id)`, so two agents can
never merge rows even if their ids collide.

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

Codex writes no registry, so a Codex session is live when a `codex` process holds its
rollout open (found through libproc's file-descriptor calls). The process is attached only
when it holds exactly one root session; a process hosting several sessions still bounds the
process tree but is counted for none of them, so its CPU and memory are never split by
guesswork or counted twice.

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

### Coats are a palette plus a per-pixel pattern
A `Look` (coat and eye colour) paints a sprite frame into RGBA. Flat coats only swap the
palette; patterned ones recolour body, shade and belly pixels from the pixel's position and
the frame's body box, deterministically, so every frame of every state works and nothing
flickers. Each agent's look is kept in `settings.json` in the data directory (`settings.rs`),
shared by the window and the widget exporter. The widget payload stays one colour per
character: pattern colours get extra characters in the rows.

### Animation follows telemetry, not a clock
A cat's state comes from session status. Its speed comes from a smoothed rolling token
rate; a high token rate or, while Active, a busy process tree makes it sprint. Timers run only while something is animating and the window is active; with Reduce
Motion or animation disabled the cat changes pose on state changes and nothing else runs.

## Storage

`~/Library/Application Support/AgentPulse/agentpulse.db` (directory 0700, file 0600, WAL).
Override the directory with `AGENTPULSE_DATA_DIR`.

| Table | Contents |
|---|---|
| `sessions` | id, provider, project path, model, first and last activity, estimated active time, last-error flag, Claude Code's own cost record (USD, as-of time, unknown-model flag) |
| `usage_events` | one row per API response: model, time, five token buckets, estimated cost |
| `ingest_files` | byte offset per transcript file |

Next to it, `settings.json` holds each agent's cat coat and eye colour (written on change,
through a temp file and rename). A missing file means the defaults; a corrupt file or an
unknown id falls back to the default with a warning.

Schema changes are numbered migrations tracked in `PRAGMA user_version`. A database from a
newer AgentPulse is refused, not downgraded. Migration 2 added the recorded-cost columns and
cleared `ingest_files`, so an existing install re-reads its recent transcripts once to pick up
cost records it had already passed (usage upserts are idempotent; about 0.1 s). The recorded
cost is upserted only by a record with a later as-of time, so re-reading changes nothing.

Not stored: prompts, responses, tool inputs or outputs, file contents, API keys. The parser
does not deserialize those fields at all.

### Usage queries are cached in two parts
The Usage screen needs 30 days of per-day aggregates. Past days change rarely, so they are
queried once indexing has finished, then at local midnight and at most every 10 minutes.
Today's part (per-day totals since midnight, cache reads, the timeline) is a cheap query on
the `ts` index and runs with the session rows, after new data or every 30 s. The existing
indexes cover every query, so no migration was added.

## What is deliberately not built yet

The OpenCode adapter, resource history tables, retention pruning, per-agent GPU,
settings UI, alerts, a notarized distributable. Each is listed with its phase in
`roadmap.md`.
