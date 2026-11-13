# AgentPulse

Watch your agents work. A native macOS app that shows what the AI coding agents on this
Mac are doing right now: tokens, estimated cost, runtime, CPU and memory, with a pixel cat
per agent whose behaviour follows the agent's real activity.

This is the first vertical slice: **Claude Code** sessions, two screens (Overview and
Sessions). See [docs/roadmap.md](docs/roadmap.md) for what is and is not built.

![Overview](docs/screenshots/dark/overview.png)

## What it does

- Finds Claude Code sessions, including subagents, and tails their transcripts.
- Counts tokens once per API response (Claude Code logs each response several times).
- Estimates cost from an editable price table. Costs are always labelled as estimates.
- Maps each running session to its process by PID and start time, and measures the CPU and
  resident memory of its whole process tree.
- Shows status from the agent's own state: active, waiting, idle, completed, failed.
- Keeps history in a local SQLite database.
- Animates one cat per live agent: sitting when idle, looking around when waiting, running
  when working, and a cartoon-style sprint (spinning legs, speed lines, dust) under heavy
  output.

Everything stays on this machine. AgentPulse makes no network calls and never reads or
stores prompt, response or file content. Values it cannot measure are shown as unavailable,
not as zero.

## Requirements

- macOS 14 or later on Apple Silicon (developed on macOS 27).
- Rust (stable) and Xcode command line tools.

## Build and run

```sh
cargo run --release -p agentpulse-app          # the app
cargo run --release -p agentpulse-app -- --dump    # one snapshot as JSON, no window
cargo run --release -p agentpulse-app -- --bench   # collector overhead over 20 s
cargo test --workspace
```

The first build fetches GPUI from the Zed repository at a pinned commit and takes a few
minutes.

## Configuration

| Setting | How |
|---|---|
| Data directory | `AGENTPULSE_DATA_DIR` (default `~/Library/Application Support/AgentPulse`) |
| Claude Code directory | `CLAUDE_CONFIG_DIR` (default `~/.claude`) |
| Prices | Put a `pricing.toml` in the data directory; it replaces the built-in table. Format: [crates/agentpulse-cost/pricing.toml](crates/agentpulse-cost/pricing.toml) |
| Animation off | `AGENTPULSE_ANIMATION=off`, or macOS Reduce Motion |
| Animation speed | `AGENTPULSE_ANIMATION_SPEED=1.5` (0.25 to 3) |
| Log level | `AGENTPULSE_LOG=debug` |

## Documentation

- [docs/repository-audit.md](docs/repository-audit.md): what was learned from related projects and GPUI.
- [docs/architecture.md](docs/architecture.md): crates, data flow and decisions.
- [docs/telemetry-spec.md](docs/telemetry-spec.md): what is read, how each number is derived, and how sure it is.
- [docs/roadmap.md](docs/roadmap.md): status and next steps.
