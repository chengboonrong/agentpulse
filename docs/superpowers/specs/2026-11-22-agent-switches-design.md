# Agent switches: choose which agents AgentPulse shows

Date: 2026-11-22. Part 1 of issue #59. The owner said "dive in and make your picks", so every
call below is the builder's. New adapters (Gemini CLI, Aider, …) are part 2, one at a time,
each after checking what the agent stores locally.

## Intent

Let the user hide an agent they don't care about from everything AgentPulse shows, without
losing its history.

## Decisions

- **Hide only.** Collection continues for hidden agents, which is the safer default named in
  the issue. History stays complete, and switching an agent back on shows everything at once.
  Collector overhead doesn't change.
- **Setting.** In `settings.json`: `"agents": {"hidden": ["codex"]}`, using `Provider::as_str`
  keys. It defaults to empty. An unknown key is ignored and kept on save, so a newer build's
  agent is never lost.
- **Applied at the source.** The collector reads `settings.agents()` on every tick, as it does
  for clean-up, and leaves hidden agents out of the `Snapshot`. Everything reads the snapshot:
  Overview, Sessions, Usage, the menu-bar cat and its count, the widgets and the phones. So
  every surface follows without any change of its own. Changing the switch wakes the
  collector at once, using the same channel End and Keep use.
- **What hiding removes from the snapshot:**
  - that agent's sessions, and with them its CPU and memory in the agents' totals;
  - its share of today's tokens and cost. Totals are queried per agent (`GROUP BY provider`)
    and summed over the shown agents in memory, so a toggle needs no new query;
  - its share of the output rate. Keep one meter per agent and sum the shown ones. Samples
    already in the rate history keep their old value: a known ceiling, since it ages out in
    120 ticks;
  - its entries in the Usage charts' days and today's lanes, and its part of the caching
    saving (`cache_reads_by_model` grouped by provider and model, filtered);
  - its plan limits.
- **Not filtered.**
  - Clean-up: a hidden agent's orphans are still warned about and ended. Processes are
    safety, not display.
  - The session detail a user already has open.
- **Settings screen.** An "Agents" card lists every supported agent with a switch. When the
  agent's data folder (its adapter's default root) doesn't exist, the row says "Not
  detected"; the switch still works. Hiding every agent is allowed; Overview then shows its
  empty state.

## Acceptance

- A collector test with two agents: hiding one removes its sessions, today's tokens and cost,
  rate, Usage days, lanes and limits from the snapshot. Showing it again restores them on the
  next tick.
- A settings test: hidden agents round-trip through `settings.json`, and an unknown key
  survives a save.
- The switch takes effect without a restart.
- `--bench` overhead is unchanged.
