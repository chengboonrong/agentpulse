# Qwen Code adapter

Date: 2026-11-22. Part 2 of issue #59, first new agent. The owner said "dive in and make
your picks", so every call below is the builder's.

## Why Qwen Code before Gemini CLI

The issue names Gemini CLI first. The build Mac has Gemini CLI 0.51.0 installed but no
session data (`~/.gemini/tmp` is empty), and its first run hung on sign-in. Qwen Code
0.14.5 is a fork of Gemini CLI and has 40+ real sessions here. That lets the adapter be
checked against real files instead of guessed from docs. Gemini CLI comes next, once a real
session exists to check against; its chat files have a different format (one JSON file per
session under `tmp/<hash>/chats/`), so it is not covered by this adapter.

## What Qwen Code stores (verified against 0.14.5)

- Root: `$QWEN_RUNTIME_DIR` if set and non-empty, else `~/.qwen` (from `Storage.getRuntimeBaseDir`
  in the bundled CLI). A missing root or `projects/` folder is not an error.
- `projects/<sanitised cwd>/chats/<session id>.jsonl`, one file per session, append-only,
  written with `appendFileSync`, so the file is not held open between lines. The folder name
  replaces every non-alphanumeric character with `-`, which can't be decoded, so the working
  folder comes from each record's `cwd` field.
- Every line is a JSON object with `uuid` (unique), `parentUuid`, `sessionId`, `timestamp`
  (ISO 8601), `type`, `cwd`, `version` and `gitBranch`. Types: `user`, `assistant`,
  `tool_result` and `system` (subtype `ui_telemetry`).
- An `assistant` line has `model` and `usageMetadata`: `promptTokenCount`,
  `candidatesTokenCount`, `thoughtsTokenCount`, `cachedContentTokenCount`, `totalTokenCount`.
  On this Mac, in 516 of 517 lines total = prompt + candidates, so candidates already include
  thoughts. In the remaining line thoughts are 0. Cached is never more than prompt.
- Content lives in `message.parts` (text, thoughts, function calls and responses) and
  `toolCallResult.resultDisplay` (whole file contents before and after an edit). It is never named.
- `ui_telemetry` lines repeat each API response (`qwen-code.api_response`, 571 of them against
  517 assistant lines) and log tool calls and API errors.

## Decisions

- **Provider.** `Provider::Qwen`, key `qwen-code`, label "Qwen Code", last in `Provider::ALL`.
- **Privacy.** Lines are parsed into structs naming only `uuid`, `sessionId`, `timestamp`,
  `type`, `cwd`, `model` and the four token counts. serde skips everything else. Only
  `projects/*/chats/*.jsonl` is opened; symlinks are not followed. No line text is stored
  or logged.
- **Tokens.** One event per `assistant` line that has `usageMetadata`, id `<sessionId>:<uuid>`.
  cache read = `cachedContentTokenCount`; input = `promptTokenCount` − cached (saturating);
  output = `candidatesTokenCount`; cache write = 0. `thoughtsTokenCount` is not added (already
  counted). `ui_telemetry` lines are not used for tokens: they duplicate the assistant lines.
- **Sessions.** Session id is the file stem. Start is the first line's time; every line's
  time is activity. The model is the newest assistant line's `model`; the project path is
  the first line's `cwd` (storage keeps a session's first path).
- **Live state.** `live_sessions` returns nothing for now. Qwen runs as `node` and AgentPulse
  reads only `p_comm`, so a folder match could claim any Node process. The core's transcript-age
  rule applies instead (active within 30 s, ended after 5 min of silence, as Completed).
- **Cost.** Unpriced. Every local session uses Qwen OAuth (`auth_type` `qwen-oauth`, model
  alias `coder-model`), which has no list price. Cost shows as unavailable, never as $0.
- **Cat.** Default coat `lilac` (solid). `palette(Provider::Qwen)` copies its colours, so
  the existing default-look test covers it.
- **Tailing.** Reuse `tail::Tailer` like the Claude Code adapter, with the same budgets.

## Known gaps

- No process, CPU or memory for Qwen sessions, and no orphan clean-up for them. The next step
  is reading argv (`KERN_PROCARGS2`) to match `node …/qwen-code/cli.js`, then pairing by cwd as
  OpenCode does.
- A Qwen session waiting on the user for more than 5 minutes shows as ended until it writes again.
- Never Waiting: permission prompts aren't distinguished.
- Tokens from subagent (`task` tool) calls appear only in `ui_telemetry`, so they aren't counted.
- A `runtimeBaseDir` set in Qwen's settings file (not the env var) is not followed.
- Plan limits aren't read.

## Acceptance

- Fixture test in `crates/agentpulse-telemetry/tests/qwen.rs` with synthetic lines (no real
  content). It checks the token buckets and event ids, that `ui_telemetry` adds nothing, that
  a re-poll from the stored offset adds no duplicates, and that a half-written last line is
  read on the next poll, along with session id, path, model and start time.
- The adapter appears in the Settings Agents card ("Not detected" when `~/.qwen` is missing)
  and is hidden by the same switch.
- Running the app on this Mac lists the Qwen sessions with tokens.
- `cargo test --workspace`, clippy `-D warnings` and `cargo fmt --check` are clean.
