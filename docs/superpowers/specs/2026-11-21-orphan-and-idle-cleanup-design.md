# Orphan and idle process clean-up

Date: 2026-11-21. Status: approved by the owner on 2026-11-21 (builder check done). Part of issue #60.

## Intent

Agents leave processes behind: `node`, `node_repl`, `python`, MCP servers such as the Spline
bridge, and dev servers they started. When the agent ends, these keep running and hold memory,
which pushes the Mac into swap. The owner's main problem is these **orphans**. Agent sessions
that sit idle for a long time are a smaller, related problem.

AgentPulse already watches every agent process and its children. It should use that to warn about
leftovers after a few hours, and to clean them up automatically after a day.

This design covers only the process part of #60. Worktrees, the Homebrew/uv/npm caches and the
simulator, emulator and VM checks are manual actions in the same issue, designed separately.

## Goals

- Track every process an agent starts, so that after the agent ends AgentPulse knows which
  leftover processes came from it.
- Two user settings, one clock for orphans and idle sessions alike:
  - **Warn after** (default 3 hours): flag the process in the app and send a macOS notification.
    Nothing is touched.
  - **Clean after** (default 24 hours): end orphans automatically.
- An orphan's clock starts when its agent ended. An idle agent session's clock starts at its last
  activity.
- **Idle agent sessions are warned about only, never cleaned automatically** (owner's decision,
  2026-11-21). Codex and OpenCode never report Waiting, so a session on an approval prompt looks
  Idle. Codex and Grok can also run several sessions in one shared process (the Codex app, an IDE),
  so killing one would kill them all. The user ends idle sessions from the Clean-up list.
- Every automatic clean is shown afterwards: a notification and an entry in a short in-app log
  (what, which agent session, memory freed, when).
- Suggest a clean-up when swap or memory pressure crosses a threshold (owner's decision). This is
  only a suggestion: it opens the manual clean-up and never cleans by itself.

This reverses part of issue #60, which said nothing is ever killed without confirmation. The
owner decided on 2026-11-21 that orphans past `clean_after` are cleaned automatically. The issue
will be updated to match once this design is approved.

## Non-goals

- Automatic cleaning of anything other than processes. Worktrees, caches, simulators and VMs stay
  manual and need confirmation every time.
- Detecting the origin of processes AgentPulse never saw start. They appear in the manual check
  only.
- Working while AgentPulse is closed.

## Anti-goals (a failure even if it technically works)

- Killing a process the user started themselves, or one belonging to a live agent session that is
  Active.
- Killing the wrong process because a PID was reused.
- Automatically killing any agent session. An Idle session may really be waiting on a prompt
  (Codex and OpenCode cannot tell), or may share its process with other sessions.
- Cleaning silently, with no record of what was ended.
- A noticeable rise in collector overhead (about 1% of one core today).

## Current state

- `agentpulse-process` samples every process (PID, parent PID, start time, CPU, RSS) into a
  `ProcTable`. `attribute()` walks each process up to its nearest agent root, so the children of
  live agents are already known on every sample.
- `Collector::tick` (`crates/agentpulse-app/src/collector.rs`) samples the table only while at
  least one agent is live (`table = None` otherwise). Orphans outlive their agent, so the clean-up
  needs its own slower sample when no agent is live.
- On macOS, when a parent exits its children are moved to `launchd` (parent PID 1). The link to
  the agent is lost at that moment, which is why it has to be recorded while the agent is alive.
- Session status (`agentpulse-core`): Active, Idle (process alive, not working), Waiting (blocked
  on the user), Completed, Failed. There is no "idle since" time. `last_activity_ms` is the
  nearest equivalent.
- Storage (`agentpulse-storage`) has an ordered `MIGRATIONS` list in SQLite. Two migrations have
  been added after the first.
- Settings live in `settings.json` in the data directory (`crates/agentpulse-app/src/settings.rs`)
  and have a Settings screen (`ui/settings.rs`).
- The app is not sandboxed (the Mac App Store was ruled out for this reason), so it can signal
  processes owned by the same user.
- The app does not send macOS notifications yet.

## Approaches considered

All of them run inside the existing collector, which already samples parent PIDs, so tracking
lineage costs no extra sampling.

1. **Lineage kept in memory only.** No migration, but any AgentPulse restart (update, reboot,
   quit) forgets every pending orphan. A 24-hour clock almost always spans a restart, so the
   automatic clean would rarely fire.
2. **Lineage saved in SQLite (chosen).** One new table and migration. The clock survives
   restarts. Each row stores PID and start time, so a reused PID never matches.
3. A separate scheduled helper that also runs while the app is closed. Ruled out: it can't see
   which processes an agent started, because that has to be watched live.

## Design

### Lineage

- On each process sample with live agents, record every descendant of a verified agent root,
  keyed by (PID, start time), with the provider and session ID of its root. The agent processes
  themselves are recorded too, for the idle-session rule.
- When the root agent is gone (its PID and start time no longer match), set `ended_at` on that
  agent's descendant rows that are still alive. That is when the orphan clock starts.
- A row whose (PID, start time) is no longer in the process table is deleted: the process ended
  on its own.
- New table, roughly: `tracked_processes (pid, start_time_s, provider, session_id, is_agent,
  name, ended_at_ms, warned_at_ms)`, primary key `(pid, start_time_s)`.
- Processes that already existed before AgentPulse first saw their agent are recorded on the
  first sample, like any other.

### Sweep

- Runs about once a minute: on the existing tick when agents are live, and on its own
  low-frequency sample when none are, but only while tracked orphans exist.
- **Orphan** (tracked, not an agent, `ended_at` set): warn when `now - ended_at >= warn_after`,
  clean when `>= clean_after`.
- **Idle or Waiting agent session**: warned about after `warn_after`, counted from
  `last_activity_ms`. Never cleaned automatically. A session that is Active is not warned about.
  When the user ends one from the list and it is a shared process, the confirmation names every
  session that process holds.
- Clean means SIGTERM, then SIGKILL if the process is still alive after a grace period. Before
  each signal, check that the PID's start time still matches.
- A process whose agent is live again is never cleaned. This covers an agent root that became a
  child of a new agent.
- (Builder check, my call) **Always warn first.** A process is cleaned only if its warning was
  shown at least 1 hour earlier. If AgentPulse was closed past `clean_after`, the first sweep
  warns and the clean follows an hour later. For an agent that ended while AgentPulse was not
  running, `ended_at` is the time AgentPulse first saw it gone.
- (Builder check, my call) **Lineage is per process, not per session.** Resuming a session in a
  new process (`claude --resume`) does not adopt the old process's orphans. They stay orphans,
  because the process that started them is gone.
- (Builder check, my call) **"Keep" exempts a process** for the rest of its life (saved by PID
  and start time). It covers a dev server the user wants to keep running. There is no list of
  process names that are never cleaned: the warning plus Keep covers those cases.
- (Builder check, my call) **Processes that detach before a sample sees them** (`nohup … &`
  from a shell that exits at once, or double-forking daemons) are not tracked. This is an
  accepted gap: they appear in the manual list below. Sampling more often would cost overhead
  against the 1% budget.

### Manual process list (in this build)

- (Builder check, my call) The Clean-up list also shows untracked leftover candidates: `node`,
  `node_repl`, `python` and Spline processes owned by the user whose parent is `launchd`
  (PID 1). Each has a "why it's here" label (tracked orphan, idle session, or untracked
  leftover). Untracked leftovers are only ever ended by the user, after confirmation.
- The swap and memory-pressure suggestion opens this list.

### Settings

- In `settings.json`: `cleanup.warn_after_hours` (default 3), `cleanup.clean_after_hours`
  (default 24), and `cleanup.auto_clean` (default on). With auto clean off, only the warning is
  given.
- `clean_after` cannot be set lower than `warn_after`.
- The values appear on the Settings screen.

### What the user sees

- A warning: a macOS notification, plus the process in a Clean-up list in the app with its name,
  agent session, age since its agent ended, and memory. From there it can be ended now or left.
- After an automatic clean: a notification and an entry in the clean-up log.
- Swap and memory pressure suggestion: a notification that opens the manual clean-up.

## Left to the builder

- Where the Clean-up list and log sit in the UI (a new screen, or part of Overview or Sessions),
  following DESIGN.md.
- The SIGTERM grace period, and how long the log is kept.
- How notifications are sent (UserNotifications through the bundled app), including the
  permission request and behaviour when permission is denied.
- The swap and memory-pressure thresholds for the suggestion, and how often it may repeat.
- Exact table shape and indexes.

## Acceptance

- Unit tests with a fake process table: an agent starts children, the agent exits, the children
  are reparented to PID 1, and they are warned about and then cleaned at the configured times.
- A reused PID with a different start time is never signalled.
- No agent session (Active, Idle or Waiting) is ever signalled automatically.
- Pending orphans survive an AgentPulse restart (database test).
- `--bench` collector overhead stays at about 1% of one core with clean-up on.
- A manual check on a real Mac: start a Claude Code session that runs `python -m http.server`,
  quit Claude Code, and with the settings set to minutes, see the warning and then the automatic
  clean.
