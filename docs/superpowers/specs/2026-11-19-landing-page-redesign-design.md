# Landing page redesign

Date: 2026-11-19. Status: draft for the owner's approval (builder check done).

## Intent

AgentPulse is about to be released publicly. The landing page is where a developer who runs AI
coding agents on a Mac finds the app (search, social posts, AI search answers), understands what
it does, trusts it, and downloads it. The one action that matters is **Download**. The app is
free and open source: no pricing, no sign-up, no email capture.

The owner's five jobs for the page: discoverability, value proposition, credibility,
conversion (download), and being the official home for downloads, release information and
platform requirements.

## Current state

A landing page already exists and is not live:

- `site/index.html` and `site/style.css` (PR #48, #50), images in `site/img/`.
- `.github/workflows/site.yml` checks every link with lychee on PRs and deploys `site/` to
  GitHub Pages on pushes to `main`.
- Canonical URL `https://agentpulse.snitco.com/`. GitHub Pages is not enabled on the repo yet
  (the Pages API returns 404), so nothing is published.
- Its ground colour is cream `#FFF6E8`, the same as the app window, so the screenshots blend into
  the page. This is the owner's main complaint.

Worth keeping from it: the SEO and Open Graph tags and `og.png`, light/dark support, the
reduced-motion still cat, the "how sure each number is" table, the install, requirements and
privacy copy (all checked against the code), the "not affiliated" line, and the promise that the
page sets no cookies and loads nothing from anywhere else.

## Approaches considered

1. **Rework the existing page (chosen).** Re-skin `site/` in pale sage, move the cat onto the
   window, add the missing sections, fold the existing copy into the new order. Smallest change;
   keeps verified copy and working infrastructure.
2. Replace it with a fresh page built from the mockup. Cleaner, but discards the accuracy table,
   dark mode, still-cat fallback and checked copy, and redoes the meta tags.
3. Keep it and only publish. Fastest, but keeps the cream-on-cream problem the owner rejected.

The owner chose 1.

## Decisions made with the owner

- **Layout: classic Mac app page with the cats (mockup "B + A, option 1").** Centred app icon,
  headline, lede, Download button, fine print; then a large app-window screenshot with one
  ginger cat running left to right along its top edge in a loop.
- **Page background: pale sage `#E3EADF`.** Chosen so the cream window and lilac sidebar never
  blend into the page. Secondary surfaces (cards, tiles) use a lighter sage `#F4F7F2`.
- **Section order** (approved as mocked up):
  1. Hero (above).
  2. The cats: "One cat per agent. Read its mood, not a dashboard." Four animated tiles:
     wind-up (a burst is starting), running (working), sprinting (heavy output),
     skid (just stopped).
  3. Three questions it answers, alternating text and screenshot rows:
     *Is anything stuck?* → Overview; *What's it costing?* → Usage;
     *How hard is it working?* → Session detail.
  4. Menu bar and widgets: the menu-bar cat states and the small and medium widgets;
     "Close the window. The cat stays in your menu bar."
  5. Private and light: six short fact cards (no account or API key; metadata only; one
     network request, the update check, which can be turned off; CPU cost; counts verified
     against an independent count; open source, MIT or Apache 2.0).
  6. Works with: Claude Code, Codex CLI, OpenCode, Grok CLI. Subagents included, no setup in
     the agents.
  7. Download and FAQ: second Download button, "signed and notarized", requirements, release
     links, and a short FAQ (Intel Macs, are costs exact, how updates work).
  Then the existing footer.
- **Free and open source, no account.** No pricing section, no forms.
- **Hosting: GitHub Pages via the existing workflow, at `agentpulse.snitco.com`.**
- **Download is one click** to a version-less DMG on the latest release (see constraints and
  the release pipeline change).

## Constraints

- Static HTML and CSS in `site/`, no build step, no framework, no JavaScript needed (the
  running cat is a CSS animation of the existing APNG sprite).
- No third-party requests: system fonts only, all images local. No cookies, no analytics.
- Brand: colours and type follow `DESIGN.md` (plum ink `#2B2430`, SF Pro Rounded via
  `ui-rounded`). The pixel cats stay central and original. Must not look like a generic SaaS
  page (no gradients, no big-number KPI tiles); the fact cards in section 5 are short text
  cards, not metrics tiles.
- Honest numbers, as in the app: costs are always called estimates; every factual claim must be
  traceable to `README.md`, `PRODUCT.md`, `docs/roadmap.md` ("Measured results") or the
  telemetry spec. The CPU card uses the collector row ("about 1% of one CPU core and 13 MB,
  measured with three live agents"), not the 1.5% whole-app row: the roadmap notes that row was
  probably taken with the window in the background and the animation paused, and that animated,
  foreground CPU has not been measured.
- Accessibility: WCAG AA contrast on sage (`#4E5249` muted text on `#E3EADF` is 6.5:1; plum ink
  is 12.2:1), keyboard focus visible, alt text on every image, `prefers-reduced-motion` stops
  the runner and swaps animated sprites for stills.
- Works from 320 px wide with no horizontal scroll; 16 px side gutter on phones.
- **One-click download (owner's decision).** Every Download button links to
  `https://github.com/chengboonrong/agentpulse/releases/latest/download/AgentPulse.dmg`, so the
  click starts the download. This needs a version-less `AgentPulse.dmg` attached to every
  release, beside the existing `AgentPulse-<version>.dmg` (kept, since older links and notes
  use it). See "Release pipeline change" below. "Release notes" and "All releases" links still
  go to `releases/latest` and `releases`.
- No version number is printed on the page: with no build step it would go stale every release.
- The lychee link check in `site.yml` must keep passing.

## Goals, non-goals, anti-goals

- **Goals:** a visitor understands the app within the first screen, trusts it from facts not
  testimonials, and can download in one click from the top or the bottom of the page. Search
  engines and link previews get an accurate title, description and image.
- **Non-goals:** pricing, sign-up, newsletter, blog, docs site, changelog page (the GitHub
  releases page is the changelog), Intel or App Store builds, the phone app (not released;
  the existing privacy paragraph mentioning it stays).
- **Anti-goals:** a page background that matches the app window; anything that reads as a toy
  before it reads as a tool; claims the repo can't back up; any request to a third party.

## Calls I made (the owner can overrule)

- **Dark mode keeps working.** The page follows the system appearance as it does today. In dark
  mode the ground becomes a deep sage-black (about `#1A211B`, cards about `#242C24`) with light
  text, so the plum dark screenshots still stand apart from the page. Exact values are the
  builder's, subject to AA contrast.
- **Existing copy is reused, not rewritten,** where a section already has it: the roster and
  usage figcaptions feed section 3; the accuracy table sits in section 5 under the fact cards;
  the privacy paragraphs sit in section 5; install steps, requirements and "why not the App
  Store" sit in section 7. New copy is only written where nothing exists.
- **The Cats screenshot** (coat picker) stays, placed in section 2 under the four state tiles.
- **Hero cat:** the existing `cat-running.png` / `cat-still.png` pair drives the runner; the
  separate "stage" box in today's hero goes away.
- The `.superpowers/` brainstorm folder is added to `.gitignore`.

## Left to the builder

- Exact spacing, type sizes and breakpoints within `DESIGN.md` and the constraints above.
- Image preparation: converting the new screenshots (session detail, widgets, menu-bar states)
  to WebP with @2x variants like the existing ones, sizes, `loading="lazy"` below the fold.
  Sources: `docs/screenshots/{light,dark,widgets,menubar}/`, sprites in `assets/cats/`.
- Still frames for the wind-up, sprint and skid sprites under reduced motion (extract the first
  frame, or use one shared still).
- Whether `og.png` needs regenerating for the sage look (it should match the page).
- Final wording of new headings and FAQ answers, within the honesty constraint.

## Release pipeline change

- `scripts/bundle.sh --release`: after the DMG is notarized and stapled, also write a copy
  named `target/bundle/AgentPulse.dmg` (byte-identical, same signature and staple).
- `macos/README.md` release checklist, step 3: upload `target/bundle/AgentPulse.dmg` too.
- The current release v0.0.10 gets the same file once, by downloading its
  `AgentPulse-0.0.10.dmg` and uploading it again as `AgentPulse.dmg`
  (`gh release upload v0.0.10`). This publishes to GitHub, so it is done only after the owner
  confirms, and before the site PR is merged: the lychee link check fails until the file exists.

## Publishing (owner's steps, outside the repo)

1. GitHub repo Settings → Pages → Source: **GitHub Actions**.
2. DNS for `snitco.com`: a `CNAME` record `agentpulse` → `chengboonrong.github.io`.
3. Settings → Pages → Custom domain `agentpulse.snitco.com`, then **Enforce HTTPS** once the
   certificate is issued.
4. Set the repo's website field to the same URL.

Order: Pages must be turned on (step 1) before the site PR merges to `main`, or the first deploy
run fails. Steps 2 and 3 can follow.

## Acceptance checks

- `site.yml` link check passes on the PR.
- Rendered in a browser at 1440 px and 375 px, light and dark: no horizontal scroll, the
  window screenshot clearly separated from the page background, the runner cat moving along the
  window's top edge, and stopped under reduced motion.
- Contrast of every text colour against its background is at least 4.5:1 (3:1 for large text).
- Every figure and claim on the page can be pointed to in the repo's docs.
- No request leaves the page's own origin (checked in the browser's network panel); the
  Download link is a navigation to GitHub, not a page request.
- `curl -sIL .../releases/latest/download/AgentPulse.dmg` ends in a 200 for a DMG.
