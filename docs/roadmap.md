# Roadmap

Status as of 2026-11-13. "Done" means built, tested and run against real data on the
development machine (macOS 27.0, Apple Silicon). "Built, not exercised" means the code
exists and compiles but the condition was never produced to watch it work.

## Measured results

| Check | Result |
|---|---|
| Automated tests | 99 pass across 7 crates; clippy clean with warnings denied |
| Token totals vs an independent count | Exact match on all 6 sessions of the last 24 h |
| Estimated cost vs Claude Code's own record | $1.6456 vs $1.6457, and $18.1074 vs $18.1075 |
| Process tree vs `ps` | Same process count, same resident memory |
| Collector overhead (`--bench`, 3 live agents, one busy) | 1.05% of one core, 13 MB |
| Whole app, window open, 20 s | 1.5% of one core, about 100 MB |
| Idle adapter poll | 86 µs; full rescan every 5 s costs about 5 ms |
| First-run backfill | 86 MB of transcripts in 0.08 s |

The whole-app figure was taken with the window launched from a background shell, so it was
probably not the active window and cat animation was paused. Animated, foreground CPU has
not been measured.

## Phase 0 — Research and validation: done
- Repository audit, licensing strategy, architecture, telemetry spec.
- GPUI built and launched from both candidate sources before choosing one.
- Claude Code formats and the price table verified against local data.

## Phase 1 — Foundation: partly done
- Done: workspace, GPUI window, sidebar with Overview and Sessions, light and dark themes
  following the system, logging, headless `--dump` and `--bench`.
- Not started: Agents, Usage & Cost, Timeline, Insights and Settings screens. Keyboard
  navigation in the sidebar.

## Phase 2 — Agent monitoring: Claude Code done
- Done: sessions and subagents, tokens, duration, status, estimated cost, SQLite history.
- Next: Codex adapter, then OpenCode. Rules are in `telemetry-spec.md`. The collector then
  takes a list of adapters and the tables gain a provider column.

## Phase 3 — Resources: live process metrics done
- Done: PID + start-time correlation, process-tree CPU and resident memory per live session.
- Next: `resource_samples` table, per-session CPU/RAM history, process-tree view.
- Next: system-wide GPU from IORegistry `PerformanceStatistics` behind an "unavailable"
  fallback, once the right accelerator node is confirmed. Create
  `agentpulse-platform-macos` for it. The GPU card says "Unavailable" until then.

## Phase 4 — Cats: dashboard cats done
- Done: original sprites for seven states, state machine with smoothing and hysteresis, one
  cat per live session driven by status and rolling token rate. Heavy activity switches to
  a cartoon-style sprint (blurred leg wheel, speed lines, dust) with a wind-up on entry and
  a skid on exit.
- Built, not exercised: Reduce Motion (the system setting was off on the test machine, and
  the code read it correctly as off), `AGENTPULSE_ANIMATION=off`, and the "data collection
  stopped" banner.
- Not verified by eye: animation playback. Frames were reviewed as stills and the timing is
  covered by tests, but nobody has watched the cats move in the window, and the sprint was
  not triggered during a capture.
- Next: calibrate the sprint thresholds (30 tok/s on, 20 off) against recorded sessions;
  decide whether heavy CPU should also trigger the sprint.
- Next: menu-bar companion. GPUI has no status-item support, so this is an `NSStatusItem`
  bridge fed with rasterized frames.

## Phase 5 — Analytics and distribution: not started
- Usage by agent, model and project; trends; cache efficiency.
- Retention setting and pruning.
- Settings screen (animation speed and on/off are environment variables today).
- Alerts and thresholds.
- `.app` bundle, icon, signing, notarization. Needs Xcode's Metal Toolchain so shaders
  compile at build time.

## Known limits of this slice
- Active time is a floor: a long tool call with no intermediate records is not counted.
- A process that starts and exits between two samples (2 s apart) is never seen.
- Processes owned by other users are invisible to the sampler.
- A new subagent transcript of a running session can be noticed up to 5 s late.
- Cost uses API list prices. It does not reflect subscription plans, and small background
  calls that Claude Code does not write to the transcript are missing (about $0.0001 per
  session in the two sessions compared).
- A live session silent for over 24 h whose registry file has no start time shows zero tokens.
- The project column truncates long paths at the default window width.

## Open decisions for the owner
- License for AgentPulse (MIT OR Apache-2.0 suggested). No `LICENSE` file exists yet.
- Whether the undocumented IORegistry GPU key is acceptable in a production build.
- Whether "heavy" for the cartoon sprint should include CPU load, or stay token rate only.
- Whether to show Claude Code's own recorded cost next to the estimate.
