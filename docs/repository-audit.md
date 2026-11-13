# Repository audit

Audited 2026-11-13 from shallow clones of each repository (source, not READMEs), the
crates.io and GitHub APIs, and the Claude Code data on the development machine
(macOS 27.0, Apple Silicon, Claude Code 2.1.322). Anything marked *inferred* was not
read in source.

## Summary of decisions this audit drives

| Question | Decision | Why |
|---|---|---|
| Which GPUI? | See [GPUI](#gpui) — pinned, exact | crates.io `gpui` is 13 months behind `main`; `main` breaks daily |
| Reuse system-pulse code? | No. Ideas only | GPL-3.0-or-later; AgentPulse is not GPL |
| How to dedup Claude tokens? | Key on `message.id`, field-wise max | None of the three audited tools does this correctly for our data |
| Session ↔ process mapping? | `~/.claude/sessions/<pid>.json` + PID start time | No audited tool does macOS correlation |
| Prices? | Config file, validated against Claude Code's own cost records | All three tools hardcode tables in source |
| GPU? | System-wide only, later phase, labelled | Per-process GPU needs undocumented keys; power/temps need private APIs |

## A. system-pulse (`eas4ai/system-pulse`)

- Exists, not archived. 13 stars, created 2026-09-07, last push 2026-09-11. About 18k lines
  of app Rust on top of inherited gpui-component history. Its README says Linux is the
  primary verification platform and that Apple hardware validation is still pending.
- **License: GPL-3.0-or-later** (`Cargo.toml`, `COPYING`). No code is copied into AgentPulse.
- GPUI source: not `gpui` from crates.io and not a Zed git rev. It uses `gpui-pre` /
  `gpui-pre-platform` `=0.3.2`, a third-party crates.io republish of Zed `main`, plus a
  vendored, patched `gpui_macos` and a vendored, patched `sysinfo`.
- Structure: one long-lived `ApplicationView` entity plus a dock of panels
  (`src/workspace.rs`, 1880 lines).
- Sampling: a dedicated `std::thread` with a `Condvar` (`crates/collectors/src/service.rs`).
  The UI **polls** it on a fixed 100 ms timer (`src/live/window_lifetime.rs:60`) and calls
  `cx.notify()` on every accepted snapshot. A second 250 ms timer drives the tray icon.
- Collectors: `sysinfo` for CPU/RAM. Apple GPU through `libloading` over IOReport, IOKit
  and SMC (private APIs, aarch64 only).
- Charts: custom `canvas(...)` elements using `PathBuilder` and `window.paint_path`
  (`src/screen_charts.rs:206-286`). Tray icon: a hand-drawn 32×32 RGBA buffer.
- Themes: gpui-component's `Theme::change`, driven by an app setting, not OS appearance.

**Taken as patterns:** sampler on its own thread; charts as `canvas` paths; icon as an
in-memory pixel buffer.
**Deliberately not taken:** the always-on 100 ms UI poll (AgentPulse pushes snapshots over
a channel and wakes the UI only when one arrives); vendored framework patches; private GPU APIs.

## B. Agent telemetry tools

| | ccdash | session-observer | tokenomics-viewer |
|---|---|---|---|
| Language / license | Go / MIT | JS / MIT | JS + Swift / ISC |
| Last push | 2026-11-08 | 2026-11-07 | 2026-10-12 |
| Agents | Claude, Codex, OpenCode | Claude, Codex, Qwen (no OpenCode) | Claude, Codex, OpenCode, others |
| Honors `CLAUDE_CONFIG_DIR` | No | Yes | Yes |
| Claude dedup | **None** (file + line only) | Adjacent-record delta by `message.id` | `requestId \|\| uuid`, first wins |
| `<synthetic>` records | Priced at $0 | Not filtered | Skipped (zero tokens) |
| Codex tokens | `last_token_usage` only | `last_token_usage` | Diffs `total_token_usage`, reset detection |
| OpenCode | `opencode.db` SQLite | — | `opencode*.db` SQLite |
| Lifecycle | Hook files + 5 min stale rule | File mtime (30 min / 5 min) | None |
| Process correlation | `/proc`, **Linux only** | None | None |
| Prices | Go tables + config override | JS regex table | JS tables with 5m/1h split |

Findings that matter for AgentPulse:

- **Claude Code** writes one `assistant` record per content block, and every record of the
  same API response repeats the same `message.usage`. ccdash sums every line, so it
  overcounts. session-observer's adjacent-only delta breaks when ids interleave or repeat
  across files.
- Subagent transcripts live in `projects/<slug>/<sessionId>/subagents/agent-*.jsonl` and are
  real consumption. tokenomics-viewer and ccdash both include them.
- **Codex CLI**: `~/.codex/sessions/**/rollout-*.jsonl` (honor `CODEX_HOME`), `token_count`
  events. Cached input is a *subset* of input. tokenomics-viewer's cumulative-diff with
  reset detection is the reference to follow.
- **OpenCode**: SQLite at `~/.local/share/opencode/opencode.db` (honor `XDG_DATA_HOME`),
  `message.data.tokens.{input,output,reasoning,cache.read,cache.write}`. Only count
  messages with `time.completed` (ccdash does; tokenomics-viewer does not).
- session-observer calls `chatgpt.com/backend-api/...` with the user's Codex credentials to
  read rate limits. AgentPulse makes no network calls.
- None of the three uses a shared price feed. All hardcode prices in source.

### Verified on local Claude Code data

Measured across the 60 most recent transcripts on this machine:

- 18,540 `assistant` records shared a `message.id` with an earlier record. Summing per
  line would roughly double the totals (411 of 782 records in the 12 newest files were
  repeats). In that sample the repeated usage was identical; across all transcripts a
  later review found 527 responses written first with partial and then with full usage,
  so the larger value must win.
- Across all transcripts, 69 response ids appear in two session files (copied history).
  Both copies carry the same `sessionId` field, naming the session that produced them.
- `<synthetic>` records carry zero tokens.
- Usage fields present: `input_tokens`, `output_tokens`, `cache_read_input_tokens`,
  `cache_creation_input_tokens`, `cache_creation.ephemeral_5m_input_tokens`,
  `cache_creation.ephemeral_1h_input_tokens`, `speed`, `inference_geo`.
- `~/.claude/sessions/<pid>.json` exists per running process and holds `pid`, `sessionId`,
  `cwd`, `procStart`, `startedAt`, `status`, `statusUpdatedAt`. The CLI binary uses the
  status values `busy`, `idle` and `waiting`. This gives exact session ↔ process mapping
  and an observed activity state, with no heuristics.
- Claude Code writes `cost-state` records with its own `totalCostUSD` and per-model token
  counts. Recomputing one from the published price table reproduced Claude Code's figure
  exactly (Sonnet 5.5: $0.5160846 both ways), which validates the price file and shows that
  thinking tokens are already inside `output_tokens`.
- Volume: 302 transcript files and 1.75 GB modified in the last 7 days (5.1 GB in total),
  so ingestion has to be incremental with persisted offsets.

## C. macmon (`vladkens/macmon`)

MIT, last push 2026-11-11, 2.1k stars, also published as a library crate (v0.11.0).

| Metric | API | Class |
|---|---|---|
| CPU load | `host_processor_info`, `hw.perflevelN.*` sysctls | Documented |
| RAM, swap | `host_statistics64`, `sysctl(VM_SWAPUSAGE)` | Documented |
| Per-process CPU/RSS | `proc_pid_rusage`, `proc_pidinfo` | Documented |
| GPU utilisation | IOReport `GPU Stats` residency | **Private library** (`libIOReport`) |
| CPU/GPU frequency, watts | IOReport + pmgr `voltage-states` | **Private** |
| Temperatures | `IOHIDEventSystemClient*`, SMC keys | **Private symbols** / SMC |
| GPU memory | IORegistry `AGXAccelerator` → `PerformanceStatistics` | Public calls, undocumented key |
| Per-process GPU time | IORegistry `AppUsage[].accumulatedGPUTime` | Public calls, undocumented key |

- No root needed for the normal paths. *Inferred:* IOReport and SMC will not work under App Sandbox.
- macmon's README says macOS 27 broke the IOReport "Energy Model" path (it now needs a
  private Apple entitlement). That is the maintenance cost of private APIs, on the exact OS
  this project targets.
- A system-wide GPU figure is readable with public IOKit calls only:
  `IOServiceMatching("IOAccelerator")` → `PerformanceStatistics` → `"Device Utilization %"`.
  The key is present on this machine but is not a documented contract. `IOAccelerator` and
  `AGXAccelerator` resolve to the same single node here (`AGXAcceleratorG13X`); the earlier
  75 vs 0 was two reads at different moments, not two nodes. The value changes moment to
  moment, so AgentPulse reads it once per sample.
- The SDK's `rusage_info` structs carry no GPU time field. There is no documented
  per-process GPU measurement.

**For AgentPulse:** CPU and RAM come from documented APIs. GPU is system-wide only, behind
a fallback that reports "unavailable", and is never attributed to an agent. Local GPU use
says nothing about remote inference and the UI must not suggest it does.

## D. GPUI

- Zed `main` audited at `27bbbc1cc480a7500cec122f120e796d1fb8500c` (2026-11-12). The `gpui`
  crate is Apache-2.0.
- crates.io `gpui` is **0.2.2, published 2025-10-22**. It is the old single-crate layout.
  `main` still calls itself 0.2.2 but has since split into `gpui`, `gpui_platform`,
  `gpui_macos` and others, and only `gpui` is published. Code written for one layout does
  not compile on the other (`Application::new()` vs `gpui_platform::application()`).
- `gpui-pre*` on crates.io is a third-party mirror of `main`. Not used: it adds a
  publisher to trust for no technical gain over a git rev.
- Several GPUI commits land per day and the README promises breaking changes.

APIs AgentPulse relies on (paths under `crates/gpui/src`):

| Need | API |
|---|---|
| Window, title bar | `cx.open_window(WindowOptions { titlebar: Some(TitlebarOptions { appears_transparent, traffic_light_position, .. }), .. })` |
| Custom drawing | `canvas(prepaint, paint)`, `window.paint_quad`, `PathBuilder` + `window.paint_path` |
| In-memory images | `img(ImageSource::Render(Arc<RenderImage>))`, BGRA frames |
| Timers | `cx.spawn(async move \|this, cx\| ...)` + `background_executor().timer(..)` |
| Appearance | `window.appearance()`, `cx.observe_window_appearance` |
| Visibility | `window.is_window_active()`, `cx.observe_window_activation` |
| Frame throttling | Built in for inactive windows, Low Power Mode and thermal pressure |

Gaps:

- **No menu-bar status item.** No `NSStatusItem` or tray code anywhere in `gpui`,
  `gpui_platform` or `gpui_macos`. The menu-bar companion needs a small AppKit bridge.
- **Reduce Motion is not wired up on macOS.** `App::reduce_motion()` exists but nothing
  sets it from the system. AgentPulse reads
  `NSWorkspace.accessibilityDisplayShouldReduceMotion` itself.
- **No window snapshot in release builds.** `Window::render_to_image()` is test-only.

## Licensing strategy

- AgentPulse code: original, licensed MIT OR Apache-2.0, which is compatible with every
  dependency below.
- GPUI (Apache-2.0), libc (MIT OR Apache-2.0), rusqlite (MIT, bundles public-domain SQLite): fine.
- system-pulse (GPL-3.0-or-later): read for ideas, no code or assets copied.
- ccdash, session-observer (MIT), tokenomics-viewer (ISC), macmon (MIT): parsing rules
  re-derived and verified against local data, no code copied.
- Cat sprites: drawn for this project. RunCat is an inspiration for the idea only.
