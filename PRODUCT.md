# Product

<!-- impeccable:product-schema 1 -->

## Platform

macos

Native macOS app (Apple Silicon, macOS 14+) built in Rust with GPUI, plus a menu-bar item and WidgetKit widgets. Design explorations may be mocked in HTML, but the shipped UI is native and must follow macOS conventions.

## Users

A developer who runs several AI coding agents (Claude Code, Codex) at once on one Mac. Mostly they glance at AgentPulse for a few seconds: on a side monitor or from the menu bar, to see whether any agent is stuck or waiting, which ones are working, and what the work is costing right now.

## Product Purpose

An offline observatory for AI coding agents: live status, tokens, estimated cost, runtime, CPU, memory and GPU per agent session, with history charts (daily cost, cache efficiency, today's timeline, Codex plan limit, per-session context size). Success is answering "is anything stuck, what's busy, what's it costing" at a glance, and "where did the money go" when asked.

## Positioning

Reads the agents' own local transcripts and the kernel's process data, so it works with no account, no API key and no upload. Each agent session is a pixel cat whose behaviour (sleeping, sitting, looking around, running, sprinting) is driven by measured telemetry, so state is legible without reading numbers.

## Operating Context

- Lives beside terminals and editors running Claude Code / Codex; often left open on a secondary display or reached from the menu bar.
- Sessions can number 1 to ~15 live at once; most are idle at any moment.
- Light and dark appearance both supported, following the system.
- Minimum window 880x560; widgets small and medium; menu bar item uses a monochrome template cat.

## Capabilities and Constraints

- Measured, estimated and unavailable values stay visually distinct; unknown is never shown as zero. Costs are estimates at API list prices and must say so.
- Metadata only: no prompts, code or responses are read, stored or shown.
- Screens today: Overview (live agents), Sessions (last 24 h or running), Session detail, Usage (charts), Cats (coat and eye picker), menu-bar menu, small and medium widgets.
- 47 cat coats and 7 eye colours, chosen per agent.
- Codex has no "waiting" state; Claude Code writes no plan-limit data.

## Brand Commitments

- Original pixel cats are the product's signature and stay central (owner, 2026-11-13). No RunCat likeness.
- Must feel Mac-native: sidebar, system type, system materials, light/dark (owner).
- Must not look like a generic SaaS analytics dashboard (KPI tile grids, gradients) and must not be terminal-dense (owner).

## Evidence on Hand

- Current UI screenshots: `docs/screenshots/{dark,light}/` (overview, sessions, overview-min, overview-empty), `docs/screenshots/menubar/`.
- Cat sprites: `crates/agentpulse-animation/src/sprites.rs`; coat catalogue renders in the session scratchpad.
- No logo exists yet (roadmap Phase 6). No users, testimonials or benchmarks to cite.

## Product Principles

1. Glance first: the most important state (stuck, waiting, busy, cost now) reads in under two seconds.
2. Honest numbers: every figure says whether it is measured or estimated, and gaps stay gaps.
3. The cats carry state; numbers confirm it.
4. Native, quiet, precise: the app sits beside work all day and never competes with it.
