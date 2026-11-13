# Roadmap

Status as of 2026-11-13. "Done" means built, tested and run against real data on the
development machine (macOS 27.0, Apple Silicon). "Built, not exercised" means the code
exists and compiles but the condition was never produced to watch it work.

## Measured results

| Check | Result |
|---|---|
| Automated tests | 178 pass across 8 crates; clippy clean with warnings denied |
| Token totals vs an independent count | Exact match on all 6 sessions of the last 24 h |
| Codex token totals vs an independent Python count | All four buckets equal on all 76 Codex sessions the app listed (243 rollouts, 5,224 usage records), including both sessions active in the last 24 h; estimated cost equal within 1e-6, 367 long-context requests, none unpriced |
| Codex open-rollout scan | Process walk about 9 ms and descriptor scan of 39 `codex` processes about 5 ms, cold; run once every 5 s |
| Collector overhead with Codex (`--bench`, 20 s) | 1.56% of one core, against 1.21 to 1.35% for the same machine without the Codex adapter |
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
- Done: workspace, GPUI window, sidebar with Overview, Sessions, Usage, Cats and Settings,
  light and dark themes following the system, logging, headless `--dump` and `--bench`.
- Done: the Cat Roster (0.0.4). The Overview is a ranked grid of cards, one per agent, the
  one that needs you first; the header counts who needs you, who is working and who is
  resting beside today's estimated cost. Today's token split is at the top of the Usage
  screen. The output rate chart is our own canvas component with a hover readout (see
  `repository-audit.md`, section E).
- Done: Usage screen (see Phase 5) and Settings screen.
- Built, not verified by a person: keyboard use of the sidebar (Tab to an item, Enter or
  Space to open it).
- Not started: Agents and Insights screens.

## Phase 2 — Agent monitoring: Claude Code and Codex done
- Done: sessions and subagents, tokens, duration, status, estimated cost, SQLite history.
- Done: Claude Code's own recorded session cost (`cost-state`) beside the estimate, in the
  Sessions table and under the estimate in Overview rows; "—" until Claude Code writes one,
  "+?" when it had no price for a model. Migration 2 stores it and backfills existing installs.
- Done: Codex CLI adapter. Rollouts under `$CODEX_HOME` (or `~/.codex`) are tailed for
  `token_usage_record` lines only, subagent threads count toward their root session,
  live sessions come from the rollouts `codex` processes hold open, and OpenAI prices are
  in the table. The collector takes a list of adapters with errors per adapter; sessions
  are keyed by provider and id (the schema already had a provider column, so no
  migration). Rows name their agent ("Codex · ~/project"), as does the menu bar.
- Built, not exercised: the Active state of a Codex session (no Codex task ran while it was
  checked), reading uncompressed rollouts from `archived_sessions` (the local archives are
  all `.jsonl.zst`, which are not read), and cache writes (every local record had 0).
- Next: OpenCode. Rules are in `telemetry-spec.md`.

## Phase 3 — Resources: live process metrics done
- Done: PID + start-time correlation, process-tree CPU and resident memory per live session.
- Next: `resource_samples` table, per-session CPU/RAM history, process-tree view.
- Done: system-wide GPU from IORegistry `PerformanceStatistics` / `"Device Utilization %"`
  (undocumented key, accepted by the owner), shown as "GPU n% whole Mac" and in `--dump`,
  "unavailable" when the key cannot be read. About 0.05 ms per read. Never per agent.

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
- Done: heavy CPU also makes the cat sprint, only while the agent is Active (process-tree
  CPU smoothed like the token rate, on above 100% of one core, off below 60%). Window cats
  use their session's CPU; the menu-bar cat the sum over Active agents. The widget's still
  cat still follows the token rate only.
- Next: calibrate the sprint thresholds (30 tok/s and 100% CPU on; 20 tok/s and 60% off)
  against recorded sessions.
- Done: menu-bar companion. A template-image cat for overall activity, the active-agent
  count, and a menu with today's figures, the live sessions, Open and Quit. Read back from
  the running app through Accessibility; Quit confirmed.
- Done: small and medium WidgetKit widgets in a signed `AgentPulse.app`
  (`scripts/bundle.sh`). The extension registers with macOS and the app writes live data
  for it.
- Not verified by a person: the menu-bar icon's actual pixels in the menu bar, a real click
  on it, closing the window and reopening it from the menu or Dock, and the widget placed
  on a desktop. No screen capture was available and synthetic clicks were ruled out.
- Done: coats. Each agent's cat can wear one of 47 coats (the 46 of the coat catalogue in
  seven groups, plus "Cloud", the OpenCode default) and one of 7 eye colours or the coat's
  own, picked on a Cats screen (agent switch, live sit and run preview, grouped swatches).
  Saved to `settings.json` in the data directory on every change; no file renders exactly
  as before (tested per pixel against the old palettes; overview captured with animation
  off and its cat column compared pixel for pixel with the previous build). Patterns are
  computed per pixel from the sprite position and the frame's body box, the same maths as
  the catalogue's prototype renderer: all 46 coats' sitting and four running frames and the
  7 eye swatches match the catalogue PNGs exactly (one-off comparison), and every frame of
  every state is pinned by checksums from the prototype. The widget's cat wears the coat of
  the agent behind the overall cat (Claude Code's when none). That is a deliberate change:
  the widget's cat used to be ginger always, so with default settings a Codex-driven widget
  now shows Codex's default dark cat. Patterns are baked into the
  payload rows as extra characters, so the Swift side is unchanged (rendered once through
  `widget-preview` with the Galaxy coat). The menu-bar cat stays a template image.
- Built, not verified: keyboard use of the Cats screen (Tab / Shift-Tab between controls,
  Enter or Space to pick; focus ring) and the VoiceOver labels (tabs, radio buttons named
  after each coat and eye colour, through GPUI's AccessKit support). No keystrokes or screen
  reader were driven. Mouse clicks on the screen were not exercised either; the screenshots
  are rendered with a settings file written beforehand. Enter and Space need no handler of
  our own: GPUI turns them into a click on the focused element (`gpui/src/elements/div.rs`,
  keyboard activation, with its own tests).
- Patterns use sprite coordinates, so when the body moves a row between run frames the
  patches stay put and the body moves under them, as in the catalogue.
- Menu-bar cost: a still pose adds nothing measurable. The menu-bar cat is capped at 5 frames
  a second. With the sprint forced on and the window open but inactive, the whole app used
  2.7% of one core at 5 fps against 3.5 to 3.7% at 10 fps. That is still above the 2% goal;
  the collector alone is about 1.3%.

## Phase 5 — Analytics and distribution: partly done
- Done: Usage screen. Daily estimated cost for 30 days stacked by agent, with no-data days
  apart from zero days and unpriced requests counted; cache efficiency per day and an
  estimated saving from caching; today's sessions on a local-time timeline using the
  active-time rule; Codex's plan limit (used percent, window, reset, plan) read from
  `token_count` events, Claude Code shown as having none. Session detail, opened from the
  Sessions table: context size per request with the long-context threshold, and the
  cumulative estimated cost. Screenshot mode captures both. The 30-day cost per day and
  today's per-agent cost matched an independent `sqlite3` query exactly, and the caching
  saving an independent Python computation, on the development machine.
- Built, not exercised: the day tooltips (no pointer was used; only stills were captured),
  a secondary Codex limit window and the out-of-date state (local readings had neither).
- Next: usage by model and project; trends beyond 30 days.
- Retention setting and pruning.
- Done: Settings screen. Animation on or off and its speed (0.5× to 2×), saved in
  `settings.json` beside the coats; the `AGENTPULSE_ANIMATION` variables still win, and
  Reduce Motion still stops the cats. Rendered through screenshot mode; clicks and keyboard
  use not exercised by a person.
- Alerts and thresholds.
- Distribution outside the App Store (owner's choice, 2026-11-13). Done:
  `scripts/bundle.sh` signs with the hardened runtime and builds a DMG; `--release` signs
  with Developer ID, notarizes and staples. Waiting on: a Developer ID Application
  certificate for the team and a notarytool keychain profile. Until then the DMG is
  development-signed and another Mac blocks it until the user allows it in System
  Settings > Privacy & Security.
- Not pursued: the Mac App Store. It requires the app sandbox, and a sandboxed build
  tested on macOS 27 sees no other processes (`proc_listallpids` returns 0,
  `proc_pid_rusage` fails) and cannot open `~/.claude` without a user grant, so per-agent
  CPU and memory, and the CPU sprint, would be lost.
- App icon. Build-time shaders need Xcode's Metal Toolchain.

## Phase 6 — Beyond the window: updates notice done (owner request, 2026-11-13)
- App logo. Design an original mark (no RunCat likeness) and ship it as the app icon, the
  DMG icon and the README header. It needs light, dark and tinted variants for macOS 26+
  icons, and a template version that reads at menu-bar size.
- Dynamic Island on the Mac. macOS has no public Dynamic Island or notch API. Options to
  research before building:
  - a borderless window drawn around the notch, as open-source notch apps do. Read their
    approach only; some are GPL, so none of their code is copied;
  - an iPhone Live Activity (see the next item), which would give a real Dynamic Island on
    the phone.
  Either way the cat and the active-agent count are the content. Macs without a notch need
  a fallback, which can simply be the menu bar.
- Mobile app and widgets through a relay, in the manner of Orca's connect. An iPhone app with
  home-screen widgets and a Live Activity shows live sessions, today's tokens and cost, and
  the cat, fed from the Mac through a relay. This changes the "works offline, never uploads"
  rule, so it must be:
  - off by default and opt-in;
  - metadata only: the same fields as the widget file, with no prompts, paths optional;
  - end-to-end encrypted, with pairing by QR code, so the relay sees only ciphertext;
  - multi-device (owner request, 2026-11-13): one phone pairs with several Macs and shows
    each Mac's agents under its name, and one Mac pairs with several phones or iPads. Every
    pairing has its own key, is listed by device name and last seen time on both sides, and
    can be removed alone, which stops that device at once without touching the others.
  It needs the paid Apple Developer Program for TestFlight or the App Store. Still open:
  who runs the relay (self-hosted or third party) and whether a local-network-only mode is
  enough for a first version.
- Done: new updates notice. Tell the user when a newer AgentPulse release exists, in the
  window and the menu-bar menu, with its version, its release notes and a link to download
  it. It is AgentPulse's only network request, and it is:
  - on by default, checking every 2 hours (owner decision, 2026-11-13), with a setting to
    turn automatic checks off and a manual "Check for updates" that works either way;
  - documented in the README and the telemetry spec as the app's only network request;
  - one request for the latest release on the project's GitHub page, sending nothing about
    the user or their agents;
  - a notice only: it never downloads or installs anything by itself.
  It reads `api.github.com/repos/chengboonrong/agentpulse/releases` (not `/latest`, which
  skips pre-releases) with an ephemeral session and the User-Agent `AgentPulse/<version>`.
  One real check against GitHub succeeded (up to date at 0.0.4); the "available" state was
  rendered from a local file through screenshot mode. The menu item and its click were not
  exercised by a person.
  A self-updating app (Sparkle, MIT licensed) needs a notarized, Developer ID signed build
  and a signed update feed, so it waits for the notarized DMG in Phase 5.

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
- Codex has no waiting-on-you state and no failed-session detection.
- Codex in VS Code and the app-server keep old rollouts open in long-running processes. In
  a process that hosts several sessions, a session counts as live only if it had activity
  in the last hour (before this rule, 75 idle threads showed as live on the development
  machine); a thread idle longer in such a process shows as ended even while it is still
  open. Those sessions have no CPU or memory figure, and such a process is counted for no
  session.
- A Codex session is seen starting or ending up to 5 s late.
- Compressed Codex archives (`.jsonl.zst`) are not read.
- The widget's data file has no agent field; its contract is unchanged, so widgets do not
  say which agent a session belongs to.

## Open decisions for the owner
- Phase 6 relay: who hosts it, and whether a local-network-only first version is enough.
- Decided 2026-11-13: the undocumented GPU key is acceptable (whole Mac
  only), heavy CPU triggers the sprint while Active, Claude Code's own cost is shown
  beside the estimate, and distribution is Developer ID rather than the App Store.
- Decided 2026-11-13: the window loads the undocumented system font name
  `.AppleSystemUIFontRounded` (SF Pro Rounded, all weights) for the Cat Roster design. If
  macOS drops the name, GPUI falls back to the system font.
