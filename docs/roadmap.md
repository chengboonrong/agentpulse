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
- Done: layout redesign. The live agents lead the Overview as rows with large cats; totals
  moved to a status line and a Today block in the sidebar; the rate chart has real axes.
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
- Done: menu-bar companion. A template-image cat for overall activity, the active-agent
  count, and a menu with today's figures, the live sessions, Open and Quit. Read back from
  the running app through Accessibility; Quit confirmed.
- Done: small and medium WidgetKit widgets in a signed `AgentPulse.app`
  (`scripts/bundle.sh`). The extension registers with macOS and the app writes live data
  for it.
- Not verified by a person: the menu-bar icon's actual pixels in the menu bar, a real click
  on it, closing the window and reopening it from the menu or Dock, and the widget placed
  on a desktop. No screen capture was available and synthetic clicks were ruled out.
- Menu-bar cost: a still pose adds nothing measurable. The menu-bar cat is capped at 5 frames
  a second. With the sprint forced on and the window open but inactive, the whole app used
  2.7% of one core at 5 fps against 3.5 to 3.7% at 10 fps. That is still above the 2% goal;
  the collector alone is about 1.3%.

## Phase 5 — Analytics and distribution: not started
- Usage by agent, model and project; trends; cache efficiency.
- Retention setting and pruning.
- Settings screen (animation speed and on/off are environment variables today).
- Alerts and thresholds.
- App icon, Developer ID signing and notarization for distribution (the current bundle is
  development-signed for this Mac). Build-time shaders need Xcode's Metal Toolchain.
- App Store route for widgets: an app group with provisioning profiles, replacing the
  sandbox exception.

## Known limits of this slice
- Widgets are snapshots: WidgetKit budgets reloads, so a widget can lag the app and always
  shows its "as of" time.
- The widget data path is fixed and ignores `AGENTPULSE_DATA_DIR`.
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
- Whether the undocumented IORegistry GPU key is acceptable in a production build.
- Whether "heavy" for the cartoon sprint should include CPU load, or stay token rate only.
- Whether to show Claude Code's own recorded cost next to the estimate.
