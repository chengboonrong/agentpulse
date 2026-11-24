# Gemini CLI adapter

Date: 2026-11-24. Issue #59. Implements the supplied Gemini CLI 0.70 storage design,
using the Qwen adapter's Tailer, metadata-only serde parsing and transcript-age status.

## Source and privacy

Root: `$GEMINI_CLI_HOME/.gemini` when set and non-empty, otherwise `~/.gemini`.
Only `tmp/*/chats/*.jsonl`, `tmp/*/chats/*/*.jsonl` and `tmp/*/.project_root`
are opened. The latter supplies the project path from at most 4096 bytes, trimmed of whitespace. Missing or
unreadable project roots yield no path. Symlinked roots follow the shared adapter
convention; symlinks below the root are skipped, including on cached reads.
Legacy `.json`, `.unreadable-*` and `.tmp-*` files are ignored.

Record structs name only `sessionId`, `startTime`, `id`, `timestamp`, `type`,
`model` and `tokens` (input, output, cached, thoughts, tool). Content is skipped by
serde, never stored or logged. Mutation records (`$set`, `$patch`, `$rewindTo`) are ignored.

## Decisions

- Provider: `Gemini`, key `gemini-cli`, label "Gemini CLI", last in `Provider::ALL`.
  Default coat: `galaxy`, with its palette. Settings detects the adapter's default root.
- Session id: metadata `sessionId`, falling back to the file stem. Subagent files
  under `chats/<parent>/` contribute tokens and activity to that parent session.
- Start: metadata `startTime`. Every message timestamp is activity; the newest main-file Gemini
  message supplies the model. Project path comes only from `.project_root`.
- One event per Gemini message with non-null tokens, id `gemini:<source session id>:<message id>`.
  The source session id is the subagent’s own id for subagent files. Re-appends are
  deduplicated within each batch; storage deduplicates event ids across polls.
  Restarting mid-file reads only the first line (at most 64 KiB) to restore the source session id.
- Cache read = cached; input = input + tool − cached (saturating); output = output +
  thoughts (candidates exclude thoughts); cache write = 0. Total is not used.
- No live sessions: Gemini runs as `node`. Transcript age means active within 30 s,
  Completed after 5 min of silence. Cost is unavailable: OAuth free tier is unpriced,
  with no Gemini pricing rows.
- Shared Tailer budgets and since window; incomplete final lines wait for the next poll.

## Known gaps

- Tokens queued when the last message already has tokens attach to the next `gemini`
  message and are lost if none follows. API calls the CLI does not record in the chat
  file are not seen.
- Legacy whole-session `.json` files are skipped until Gemini converts them to `.jsonl`.
- No process match, CPU, memory or orphan cleanup.
- Never Waiting; a silent permission prompt eventually shows Completed.
- API-key users are also unpriced.
- Plan limits are not read.
- Real-session reading is built, not exercised: this Mac has no signed-in Gemini sessions.
- As with Qwen, a directory swapped to a symlink between checking and opening remains
  outside the shared tailer's race protection; final file opens refuse symlinks.

## Acceptance

Synthetic integration tests cover token buckets, repeated ids, ignored mutations and
non-Gemini records, parent attribution, metadata, discovery boundaries, symlinks,
stored offsets, restart recovery and partial writes. Unit tests cover root env handling.
Workspace fmt, clippy with warnings denied, and tests verify integration. No new crates.
