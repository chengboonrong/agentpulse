---
version: 1
slug: "crates-agentpulse-app-src-ui-rs"
primary_target: "crates/agentpulse-app/src/ui.rs"
related_targets: ["crates/agentpulse-app/src/ui/charts.rs","crates/agentpulse-app/src/ui/cats.rs","crates/agentpulse-app/src/menubar.rs","crates/agentpulse-app/src/widget_export.rs","macos"]
---

# AgentPulse window, menu and widgets

Scope: the whole native app surface: Overview, Sessions, session detail, Usage, Cats, the menu-bar menu, and the desktop widgets. Mode: Operate.

Visitor: one developer running several coding agents, glancing for a few seconds between tasks. Job: see who needs them, who is working and how hard, and what today has roughly cost; then drill into one session or the 30-day history. Constraints: PRODUCT.md (measured vs estimated vs unavailable, metadata only, native controls, cats central).

## Direction contract

THESIS: The agents are a cast, not rows. Every live agent is a character cell, its cat on a pastel stage with a name plate and one honest line about what it is doing, ranked so the one that needs you leads. Refuses the category default: a stat-card strip over a grey data table.

OWN-WORLD: Warm cream ground, lilac sidebar; dark appearance is deep plum with the same ribbons. Each cell is a rounded white card with a keyline around a pastel stage (peach for Claude Code, periwinkle for Codex). Ribbons are the only saturated colour, one per state: pink needs you, orange sprinting, blue working, grey idle, pale grey asleep; ended cats print desaturated. SF Pro Rounded, heavy for name plates and headline figures, tabular digits everywhere. Charts reuse the agent and ribbon colours on cream.

STORY: In two seconds the developer finds the pink-ribboned cell, reads it, and acts; the cats' own motion says how hard each agent works; today's estimate sits labelled as an estimate. A click on a cell opens that session's profile.

FIRST VIEWPORT: Native sidebar (Overview, Sessions, Usage, Cats). Header: "Roster" heavy rounded 26pt, pill counts (needs you in pink, working, resting), today's ≈ cost and Codex limit at the right. Below, a grid of agent cells in whole-cell columns (3 at 1440, fewer when narrow), ranked needs-you, sprinting, working, idle, ended: 96px stage with the animated cat, name plate plus ribbon, one-line note, facts row (≈ cost, tokens, agent). Signature move: the ranked roster, where the cell that needs you takes its ribbon colour as keyline and halo, and a cat gives one small bounce when its cell changes rank (off when animation is off).

FORM: challenger 6 of the ordered list, character goods catalog roster (pop-culture-shelf-character-goods-catalog), chosen by the owner over the assigned direction; seed:85934f20d89c1f58.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Other surfaces in the same world

- Sessions: native table kept (not sortable yet); each row leads with a small cat and its ribbon.
- Session detail: a profile header (large stage, plate, ribbon, facts) above the existing charts.
- Usage: cream cards holding the charts, agent colours as series.
- Cats: coat swatches become roster cells; the preview is the agent's profile stage.
- Menu-bar menu: native NSMenu, untouched in form. Widgets: small roster cells (stage, plate, ribbon).

## Unresolved

- Whether the bounce on rank change reads as delight or noise at a glance; judge in the running app.
