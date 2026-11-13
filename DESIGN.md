---
name: AgentPulse
description: An offline observatory for AI coding agents, where each session is a pixel cat on its own stage.
colors:
  cream-ground: "#FFF6E8"
  lilac-sidebar: "#F1EAF3"
  warm-hover: "#F8EAD4"
  sand-border: "#E8DACB"
  lilac-selected: "#E3D5E8"
  plum-ink: "#2B2430"
  dusk-muted: "#66596C"
  card-white: "#FFFFFF"
  biscuit-keyline: "#EBDCC7"
  stage-peach: "#FFE9DC"
  stage-periwinkle: "#E4E8F3"
  stage-ended: "#F1EEF2"
  need-halo: "#FFD6E5"
  ribbon-need-pink: "#D92D6A"
  ribbon-sprint-orange: "#B9520A"
  ribbon-work-blue: "#2F6FE0"
  ribbon-idle-grey: "#6E737B"
  ribbon-fail-red: "#B4423E"
  ribbon-ended-pale: "#E5E1E8"
  ribbon-ended-ink: "#5A4F60"
  on-ribbon: "#FFFFFF"
  warning-amber: "#B9520A"
  error-red: "#B4423E"
  banner-cream: "#FFF3E0"
  banner-gold: "#E6C46E"
  series-claude: "#D06A2E"
  series-codex: "#5F6FCB"
  dark-plum-ground: "#1F1823"
  dark-plum-sidebar: "#2A2030"
  dark-hover: "#30263A"
  dark-border: "#3E3245"
  dark-selected: "#45384E"
  dark-ink: "#F3ECF5"
  dark-muted: "#B8ACBF"
  dark-card: "#2B2231"
  dark-keyline: "#40344A"
  dark-stage-peach: "#4A3329"
  dark-stage-periwinkle: "#2E3452"
  dark-stage-ended: "#342C39"
  dark-need-halo: "#5C2440"
  dark-ribbon-ended-pale: "#4A4150"
  dark-ribbon-ended-ink: "#E0D8E4"
  dark-warning-amber: "#E59A55"
  dark-error-red: "#F0857C"
  dark-banner: "#3A2F17"
  dark-banner-gold: "#6E5826"
  dark-series-claude: "#F0A070"
  dark-series-codex: "#A3AEF0"
typography:
  headline:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "26px"
    fontWeight: 800
    fontFeature: "\"tnum\" 1"
  figure:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "22px"
    fontWeight: 800
    fontFeature: "\"tnum\" 1"
  plate:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "17px"
    fontWeight: 800
    fontFeature: "\"tnum\" 1"
  row-title:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "15px"
    fontWeight: 700
    fontFeature: "\"tnum\" 1"
  body:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "13px"
    fontWeight: 400
    fontFeature: "\"tnum\" 1"
  label:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "12px"
    fontWeight: 400
    fontFeature: "\"tnum\" 1"
  ribbon:
    fontFamily: ".AppleSystemUIFontRounded, SF Pro Rounded, system-ui"
    fontSize: "11px"
    fontWeight: 700
    lineHeight: "17px"
    fontFeature: "\"tnum\" 1"
rounded:
  control: "8px"
  inset: "10px"
  stage: "14px"
  card: "18px"
  halo: "21px"
  full: "9999px"
spacing:
  xs: "4px"
  s: "8px"
  m: "12px"
  l: "16px"
  xl: "24px"
  group: "32px"
components:
  roster-cell:
    backgroundColor: "{colors.card-white}"
    textColor: "{colors.plum-ink}"
    rounded: "{rounded.card}"
    padding: "14px 16px"
  roster-cell-needs-you:
    backgroundColor: "{colors.card-white}"
    rounded: "{rounded.card}"
    padding: "14px 16px"
  stage-claude:
    backgroundColor: "{colors.stage-peach}"
    rounded: "{rounded.stage}"
    size: "96px"
  stage-codex:
    backgroundColor: "{colors.stage-periwinkle}"
    rounded: "{rounded.stage}"
    size: "96px"
  stage-ended:
    backgroundColor: "{colors.stage-ended}"
    rounded: "{rounded.stage}"
    size: "96px"
  ribbon-need:
    backgroundColor: "{colors.ribbon-need-pink}"
    textColor: "{colors.on-ribbon}"
    typography: "{typography.ribbon}"
    rounded: "{rounded.full}"
    padding: "0 8px"
  ribbon-sprint:
    backgroundColor: "{colors.ribbon-sprint-orange}"
    textColor: "{colors.on-ribbon}"
    typography: "{typography.ribbon}"
    rounded: "{rounded.full}"
    padding: "0 8px"
  ribbon-work:
    backgroundColor: "{colors.ribbon-work-blue}"
    textColor: "{colors.on-ribbon}"
    typography: "{typography.ribbon}"
    rounded: "{rounded.full}"
    padding: "0 8px"
  ribbon-idle:
    backgroundColor: "{colors.ribbon-idle-grey}"
    textColor: "{colors.on-ribbon}"
    typography: "{typography.ribbon}"
    rounded: "{rounded.full}"
    padding: "0 8px"
  ribbon-fail:
    backgroundColor: "{colors.ribbon-fail-red}"
    textColor: "{colors.on-ribbon}"
    typography: "{typography.ribbon}"
    rounded: "{rounded.full}"
    padding: "0 8px"
  ribbon-ended:
    backgroundColor: "{colors.ribbon-ended-pale}"
    textColor: "{colors.ribbon-ended-ink}"
    typography: "{typography.ribbon}"
    rounded: "{rounded.full}"
    padding: "0 8px"
  count-pill:
    backgroundColor: "{colors.card-white}"
    textColor: "{colors.plum-ink}"
    rounded: "{rounded.full}"
    padding: "4px 12px"
  count-pill-need:
    backgroundColor: "{colors.ribbon-need-pink}"
    textColor: "{colors.on-ribbon}"
    rounded: "{rounded.full}"
    padding: "4px 12px"
  chart-card:
    backgroundColor: "{colors.card-white}"
    rounded: "{rounded.card}"
    padding: "20px"
  nav-item:
    textColor: "{colors.dusk-muted}"
    rounded: "{rounded.control}"
    padding: "6px 12px"
  nav-item-hover:
    backgroundColor: "{colors.warm-hover}"
  nav-item-selected:
    backgroundColor: "{colors.lilac-selected}"
    textColor: "{colors.plum-ink}"
  session-row:
    rounded: "{rounded.inset}"
    padding: "8px 0"
  session-row-hover:
    backgroundColor: "{colors.warm-hover}"
  segmented-control:
    backgroundColor: "{colors.lilac-sidebar}"
    rounded: "{rounded.inset}"
    padding: "3px"
  segmented-control-selected:
    backgroundColor: "{colors.card-white}"
    rounded: "{rounded.control}"
    padding: "4px 12px"
  banner:
    backgroundColor: "{colors.banner-cream}"
    typography: "{typography.label}"
    rounded: "{rounded.inset}"
    padding: "8px 12px"
  tooltip:
    backgroundColor: "{colors.card-white}"
    textColor: "{colors.plum-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "4px 8px"
---

# Design System: AgentPulse

## Overview

**Creative North Star: "The Character Goods Shelf"**

AgentPulse presents its agents the way a character-goods catalogue presents its figures: each one on its own little pastel stage, boxed in a white card with a soft keyline, with a heavy rounded name plate and a ribbon that says what state it is in. The ground is warm cream with a lilac sidebar in light appearance and deep plum in dark; the cards, stages and ribbons keep their roles in both. Nothing is glossy, gradient-filled or shadowed; depth is a flat card on a tinted ground.

The system is glance-first. Saturation is rationed to the state ribbons and the chart series, so the eye lands on the one pink cell before it reads a word. Pixel cats carry the state through motion; the text confirms it in one line and a small row of tabular figures. Every figure that is estimated says so (≈, "estimate"), and missing values show as a dash, never a zero.

Density is moderate: whole cells only, never clipped, at most three across, with generous 16px gutters. The app keeps the Mac's own grammar (a full-height sidebar under the traffic lights, system rounded type, native menus) and puts its personality into the cells, not the chrome.

**Key Characteristics:**
- Warm cream ground, lilac sidebar; deep plum in dark appearance.
- White rounded cards with a 2px keyline; no shadows anywhere.
- One saturated ribbon per state; everything else is pastel or ink.
- Pastel stage per agent: peach for Claude Code, periwinkle for Codex, neutral for ended.
- SF Pro Rounded throughout, extra-bold for plates and headline figures, tabular digits everywhere.
- Pixel cats drawn on whole-pixel edges; ended cats print grey and faded.

## Colors

A warm, low-chroma ground of cream, lilac and plum ink, with six state ribbons and two agent series as the only strong colour.

### Primary
- **Needs-You Pink** (ribbon-need-pink): the most important colour in the app. Fills the needs-you ribbon and the needs-you count pill, and becomes the 2px keyline of the cell that needs you, set inside a pale pink halo (need-halo). Same value in light and dark.

### Secondary
- **Sprint Orange** (ribbon-sprint-orange): the sprinting ribbon. Shares its value with Warning Amber in light appearance.
- **Working Blue** (ribbon-work-blue): the working ribbon.
- **Idle Slate** (ribbon-idle-grey): the idle ribbon.
- **Failed Brick** (ribbon-fail-red): the failed ribbon.
- **Ended Pale** (ribbon-ended-pale with ribbon-ended-ink label): the only ribbon without a white label; ended sessions recede.

### Tertiary
- **Claude Ember** (series-claude) and **Codex Indigo** (series-codex): chart series per agent on the Usage and detail charts, lighter in dark appearance (dark-series-claude, dark-series-codex). Codex Indigo also fills the Codex limit bar below 75%.
- **Warning Amber** (warning-amber / dark-warning-amber) and **Error Red** (error-red / dark-error-red): chart warnings only: the long-context threshold line, and the limit bar at 75% and 90%.

### Neutral
- **Cream Ground** (cream-ground / dark-plum-ground): behind every screen.
- **Lilac Sidebar** (lilac-sidebar / dark-plum-sidebar): the sidebar and the track of segmented controls, separated from the ground by a 1px sand-border rule.
- **Card White** (card-white / dark-card): roster cells, chart cards, pills, tooltips.
- **Biscuit Keyline** (biscuit-keyline / dark-keyline): the 2px outline of every card and pill.
- **Stage Peach / Stage Periwinkle / Stage Ended** (and their dark counterparts): the pastel square each cat stands on, chosen by agent.
- **Warm Hover** (warm-hover / dark-hover): hover fill on nav items, rows and chips.
- **Lilac Selected** (lilac-selected / dark-selected): the selected nav item; keyboard focus on a session row.
- **Sand Border** (sand-border / dark-border): hairline rules, table dividers, sidebar edge, chart gridlines, limit-bar track.
- **Plum Ink** (plum-ink / dark-ink): primary text and the keyboard focus ring.
- **Dusk Muted** (dusk-muted / dark-muted): secondary text, axis labels, chart baselines.
- **Banner Cream / Banner Gold** (and dark): the error banner fill and its 1px border.

### Named Rules
**The Ribbon Monopoly Rule.** Saturated colour on the roster belongs to the state ribbons. Stages, cards and grounds stay pastel or neutral; a new element that needs attention earns it with a ribbon, not a new hue.

**The Same Ribbons Rule.** Ribbon fills are identical in light and dark appearance; only the grounds, cards, stages and the ended ribbon change.

**The Tested Contrast Rule.** Text reaches 4.5:1 on every surface it is drawn on, and series, marks and the needs-you keyline reach 3:1 on cards and ground, in both appearances. A unit test enforces it; a new colour joins the test before it ships.

## Typography

**Display Font:** SF Pro Rounded (the system rounded design, `.AppleSystemUIFontRounded`)
**Body Font:** SF Pro Rounded
**Label/Mono Font:** SF Pro Rounded with tabular digits; no monospace face.

**Character:** one friendly rounded family doing all the work, heavy where it names or counts and regular where it explains. Tabular figures are on for the whole window so columns of cost and tokens hold still as they tick.

### Hierarchy
- **Headline** (800, 26px): screen titles (Roster, Usage). Nothing is larger.
- **Figure** (800, 22px): today's estimated cost in the roster header, the coat name on the Cats preview.
- **Plate** (800, 17px): name plates in roster cells and figure values in Usage cards. Session detail uses the plate at headline size.
- **Row Title** (700, 15px): project names in the Sessions table.
- **Body** (400, 13px): the default for the window, cell notes, table figures.
- **Label** (400, 12px): secondary lines, axis labels, legends, facts rows; at 700 for section labels and emphasised figures.
- **Ribbon** (700, 11px, 17px line): state ribbons only, lowercase.

### Named Rules
**The Heavy Names Rule.** Names and headline figures are extra-bold (800); everything that explains them is regular and muted. There is no medium-weight middle tier for headings.

**The Tabular Everywhere Rule.** `tnum` is set on the root view; never turn it off for a figure.

## Layout

A native two-column window: a fixed 176px sidebar running full height under the traffic lights (38px title-bar clearance), and a scrolling content column with 24px side padding, 12px top and 24px bottom. Minimum window 880x560.

The roster is a grid of whole cells: each at least 340px wide, 16px apart, at most three across (three at 1440px, one or two when narrower). Cells never clip; the column count drops instead. Inside a cell, the 96px square stage sits left of a column holding plate, note and facts, 14px apart horizontally and 6px vertically.

Spacing runs on a 4px base: 4, 8, 12, 16, 24, and 32 between major groups. Sections stack at 24px (header to grid, grid to legend, legend to card); Usage cards stack at 16px. The Sessions table drops whole columns (Model, Started, Claude Code's cost record) rather than squeezing them when the window is narrow.

## Elevation & Depth

Flat. There are no shadows in the app or widgets. Depth is tonal: a white (or plum) card with a 2px keyline sits on the cream (or deep plum) ground, and a pastel stage sits inside the card. Emphasis comes from colour, not lift: the cell that needs you swaps its keyline for the pink ribbon colour and gains a 3px pale pink halo around it.

### Named Rules
**The Keyline Not Shadow Rule.** Separation is a 2px keyline on cards, pills and chips, or a 1px rule for dividers. Do not add drop shadows to make something float.

## Shapes

Soft, toy-like rounding in four steps: 8px for controls (nav items, tooltips, segments), 10px for inset surfaces (session rows, banners, segmented-control track, coat swatch stages), 14px for cat stages, 18px for cards (21px for the halo around a card). Ribbons, pills, chips, legend dots and the limit bar are full capsules. Pixel cats are the one deliberately hard-edged form: square pixels at an integer scale snapped to whole points, never smoothed.

## Components

### Roster Cell (signature)
The product's face: one agent as a character on its own stage.
- **Shape:** card radius (18px), 2px keyline, 14px x 16px padding, wrapped in a 3px outer ring that stays transparent unless the cell needs you.
- **Content:** 96px stage (cat drawn at 3x, bottom-aligned at 10% inset) left of a plate (17px extra-bold name plus ribbon), one muted note line, and a facts row (≈ cost and tokens in bold ink, "tok" and the agent name in muted label).
- **Needs you:** keyline becomes Needs-You Pink; the ring fills with the pale pink halo.
- **Ended:** stage turns neutral, the cat sleeps in greyscale at 60% opacity, the name prints muted, the ribbon is Ended Pale.
- **Focus:** keyboard focus fills the 3px ring with ink.
- **Motion:** when a cell changes rank its cat hops once, 6px over 250ms with an ease-out cubic curve; off when animation is off.
- **Order:** needs you, sprinting, working, idle, failed, then the newest three ended.

### Ribbons
- **Style:** capsule, 8px side padding, 11px bold lowercase label, white on the state colour; Ended uses a pale fill with ink label.
- **Use:** beside every name plate, in Sessions rows beside a 2x cat, and in widgets. A legend of 10px dots under the roster names each colour.

### Chips (count pills and eye chips)
- **Count pills:** capsule, 2px keyline on card, 4px x 12px, bold. The needs-you pill fills with Needs-You Pink and white text when anyone needs you.
- **Eye chips (Cats):** capsule on card with a 2px keyline that turns ink when selected (label goes bold); hover fills Warm Hover.

### Cards / Containers
- **Corner Style:** 18px.
- **Background:** Card White / dark card.
- **Shadow Strategy:** none; see Elevation & Depth.
- **Border:** 2px keyline.
- **Internal Padding:** 20px for chart cards, 16px for the Cats preview card.

### Navigation
- **Sidebar:** Lilac Sidebar with a 1px border on the right. Items are 13px muted text, 6px x 12px, 8px radius, 4px apart. Hover fills Warm Hover; the selected item fills Lilac Selected in ink and bold. Focus is a 1px ink border that is always reserved, so focus never shifts layout.
- **Segmented control (Cats agent switch):** Lilac track with a 1px border, 10px radius, 3px padding; the selected segment is a card-white 8px pill in bold.

### Tables (Sessions)
Native-feeling rows separated by 1px rules, 10px gaps between columns, 8px vertical padding, 10px radius on hover (Warm Hover) and focus (Lilac Selected). Each row leads with its cat at 2x and its ribbon; figures are right-aligned; missing values show a muted dash.

### Charts
Hand-drawn on the card: daily bars as stacked segments in the agent series colours with a 1px card-coloured hairline between segments and 2px between days; a 1px muted baseline and 1px sand gridlines; lines at 1.5 to 2px. Days with no stored data show three faint 2px dots instead of a bar, never a zero. The limit bar is an 8px capsule on a sand track, Codex Indigo below 75%, amber from 75%, red from 90%, muted when stale.

### Banner and Tooltip
- **Banner:** Banner Cream with a 1px Banner Gold border, 10px radius, 8px x 12px, 12px text; sits between the title and the content.
- **Tooltip:** card fill, 1px sand border, 8px radius, 4px x 8px, 12px ink text.

### Widgets
SwiftUI, same world in miniature: the cream (light) or deep plum (dark) container ground, rounded system type with monospaced digits, a cat on a 12px continuous-corner stage, a heavy headline with a lead ribbon (needs you leads), and ribbon-led session rows. In tinted or accented rendering the stage drops away, the cat becomes a one-tone silhouette, and ribbons turn into 1px outlined capsules.

## Do's and Don'ts

### Do:
- **Do** give every new state or warning a ribbon from the existing six before considering a new colour.
- **Do** put every agent on its stage colour: peach for Claude Code, periwinkle for Codex, neutral once ended.
- **Do** separate surfaces with a 2px keyline on a flat card; use the 3px halo only for the cell that needs you.
- **Do** keep figures tabular and mark estimates with ≈ or "estimate"; show a muted dash for unknown.
- **Do** draw cats at integer scale on whole-point origins, and print ended cats in greyscale at 60% opacity.
- **Do** add any new text or chart colour to the contrast test, in both appearances.

### Don't:
- **Don't** add drop shadows, gradients or glass to cards, cells or pills.
- **Don't** set any text larger than the 26px screen title.
- **Don't** use a ribbon colour as a background tint for whole surfaces; stages and grounds stay pastel.
- **Don't** clip a roster cell to fit a column; drop a column instead.
- **Don't** convey the agent by cat or stage colour alone; name it in text as well.
