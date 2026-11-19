# Landing Page Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-skin and restructure the existing unpublished `site/` landing page (pale sage, running cat on the window, seven sections, one-click DMG download) and make every release carry a version-less `AgentPulse.dmg`.

**Architecture:** Plain static `site/index.html` + `site/style.css` + images, deployed by the existing `.github/workflows/site.yml`. A stdlib-only Python check (`scripts/check-site.py`) pins the page's rules (local assets exist, no third-party requests, download URL, no version text, reduced-motion stills, section order, contrast of colour tokens) and runs in CI beside the lychee link check. The release change is a copy step in `scripts/bundle.sh` plus one line in the release checklist.

**Tech Stack:** HTML, CSS, Python 3 stdlib (check), Pillow + `cwebp` (one-off image prep, not committed tooling), headless Google Chrome (verification), bash, `gh`.

**Spec:** `docs/superpowers/specs/2026-11-19-landing-page-redesign-design.md`

## Global Constraints

- Static HTML and CSS in `site/`, no build step, no framework, no JavaScript.
- No third-party requests: system fonts only (`ui-rounded, "SF Pro Rounded", system-ui, …`), all images local, no cookies, no analytics.
- Page ground pale sage `#E3EADF`; secondary surfaces `#F4F7F2`; ink `#2B2430`; muted text `#4E5249`.
- Dark mode follows the system: ground about `#1A211B`, cards about `#242C24`, light text; every text token ≥ 4.5:1 on its surfaces.
- Every Download button: `https://github.com/chengboonrong/agentpulse/releases/latest/download/AgentPulse.dmg`. "Release notes" → `https://github.com/chengboonrong/agentpulse/releases/latest`; "All releases" → `https://github.com/chengboonrong/agentpulse/releases`.
- No version number printed anywhere on the page.
- Agents named exactly: Claude Code, Codex CLI, OpenCode, Grok CLI.
- Costs always called estimates. CPU card: "about 1% of one CPU core and 13 MB, measured with three live agents". Never "1.5%".
- No gradients, no big-number KPI tiles; fact cards are short text cards.
- `prefers-reduced-motion: reduce` stops the runner and shows a still for every animated sprite.
- Works from 320 px wide with no horizontal scroll; 16 px side gutter on phones.
- Keep: SEO/OG tags, canonical `https://agentpulse.snitco.com/`, the accuracy table, install/requirements/privacy copy, "not affiliated" line, "This page sets no cookies and loads nothing from anywhere else."
- Work on branch `landing-redesign`; commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Reduced motion on the sprite tiles:** APNGs animate regardless of CSS, so each animated `<img>` needs a `<picture>` with a `(prefers-reduced-motion: reduce)` still source; a user with reduced motion expects nothing moving. Pinned by `test_every_animated_sprite_has_a_still` (Task 2).
2. **Phone width (320–375 px):** the four cat tiles, the alternating rows, the six fact cards and the accuracy table must reflow with no sideways scroll. Pinned by the 375 px and 320 px renders in Task 5.
3. **Dark appearance:** the plum dark screenshots must stand apart from the dark sage page and every text token must stay readable. Pinned by `test_text_tokens_meet_contrast` covering both token blocks (Task 2) and the dark renders (Task 5).
4. **Download link before the asset exists:** the one-click URL 404s until `AgentPulse.dmg` is on the latest release, which also fails lychee. Pinned by the `curl` check and ordering in Task 6.
5. **A broken or missing image after renaming assets:** pinned by `test_local_references_exist` (Task 2).

---

### Task 1: Version-less DMG in the release pipeline

**Files:**
- Modify: `scripts/bundle.sh` (the `if [[ $RELEASE == 1 ]]` block inside the DMG section, after `spctl --assess --type open …`; and the two closing `echo` lines of the release block)
- Modify: `macos/README.md` (release checklist, step 3)
- Also commit: `docs/superpowers/specs/2026-11-19-landing-page-redesign-design.md`, this plan, `.gitignore` (already edited: ignores `.superpowers/`)

- [ ] **Step 1: Branch**

Run: `git switch -c landing-redesign`

- [ ] **Step 2: In `bundle.sh`, after the Gatekeeper assessment of the stapled image, copy it**

`cp "$IMAGE" "$OUT/AgentPulse.dmg"` (byte-identical, so the stapled ticket and signature come along). Change the closing message to name `AgentPulse-$VERSION.dmg, AgentPulse.dmg, AgentPulse-$VERSION.zip and appcast.xml`.

- [ ] **Step 3: In `macos/README.md` step 3, add the file to the upload list and say why**

Command becomes `… target/bundle/AgentPulse-<version>.{dmg,zip} target/bundle/AgentPulse.dmg target/bundle/appcast.xml`, with one sentence: the landing page's Download button links to `releases/latest/download/AgentPulse.dmg`.

- [ ] **Step 4: Verify**

Run: `bash -n scripts/bundle.sh && grep -c 'AgentPulse.dmg' scripts/bundle.sh macos/README.md`
Expected: no syntax error; each file count ≥ 1.

- [ ] **Step 5: Commit**

```bash
git add scripts/bundle.sh macos/README.md .gitignore docs/superpowers/specs/2026-11-19-landing-page-redesign-design.md docs/superpowers/plans/2026-11-19-landing-page-redesign.md
git commit -m "Ship a version-less AgentPulse.dmg with every release"
```

---

### Task 2: Site check script, wired into CI

**Files:**
- Create: `scripts/check-site.py` (stdlib only; `html.parser`, `re`, `pathlib`, `unittest`)
- Modify: `.github/workflows/site.yml` (`links` job: add `- run: python3 scripts/check-site.py` after checkout)

**Interfaces:**
- Produces: `python3 scripts/check-site.py` exits 0 when `site/` follows the rules, non-zero with a failing test name otherwise. Task 4 must make it pass. Section ids it expects, in this order: `cats`, `answers`, `menubar`, `private`, `agents`, `download`. Colour tokens it reads from `site/style.css`: `--ground`, `--card`, `--ink`, `--muted`, `--link`, in the light `:root` block and the `@media (prefers-color-scheme: dark)` `:root` block.

- [ ] **Step 1: Write the tests (`unittest.TestCase`, run via `unittest.main()`)**

```python
DMG = "https://github.com/chengboonrong/agentpulse/releases/latest/download/AgentPulse.dmg"

def test_local_references_exist(self):
    # every img src, source srcset entry (descriptor stripped), link href that is not http(s)/mailto/#
    # resolves to a file under site/
    self.assertEqual(missing, [])

def test_no_third_party_requests(self):
    # img/src, source/srcset, link rel in (stylesheet, icon, apple-touch-icon, preload), script/src:
    # none starts with http:// or https://   (og:image / canonical meta and <a href> navigations are allowed)
    self.assertEqual(external, [])
    self.assertNotIn("<script", html)

def test_download_buttons_are_one_click(self):
    hrefs = [a.href for a in anchors if "download" in a.classes]
    self.assertGreaterEqual(len(hrefs), 2)
    self.assertTrue(all(h == DMG for h in hrefs))

def test_no_version_number(self):
    self.assertIsNone(re.search(r"\bv?\d+\.\d+\.\d+\b", visible_text))

def test_every_animated_sprite_has_a_still(self):
    # for each <img> whose src is an APNG (file has an acTL chunk), its parent <picture> has a
    # <source media="(prefers-reduced-motion: reduce)"> whose srcset file is not an APNG
    self.assertEqual(unguarded, [])

def test_sections_in_order(self):
    self.assertEqual([i for i in section_ids if i in EXPECTED], ["cats","answers","menubar","private","agents","download"])

def test_cpu_claim(self):
    self.assertIn("13 MB", visible_text)
    self.assertNotIn("1.5%", visible_text)

def test_text_tokens_meet_contrast(self):
    # WCAG relative-luminance ratio, for each of the two token blocks:
    # (ink, ground), (ink, card), (muted, ground), (muted, card), (link, ground), (link, card) >= 4.5
    self.assertEqual(failures, [])
```

- [ ] **Step 2: Run it against today's page**

Run: `python3 scripts/check-site.py`
Expected: FAIL in `test_download_buttons_are_one_click` (links point at `releases/latest`), `test_sections_in_order`, `test_cpu_claim`; `test_local_references_exist` and `test_no_third_party_requests` PASS.

- [ ] **Step 3: Add the CI step to the `links` job in `site.yml`**

- [ ] **Step 4: Commit** (the check is expected to fail until Task 4; do not open the PR yet)

```bash
git add scripts/check-site.py .github/workflows/site.yml
git commit -m "Check the landing page's rules in CI"
```

---

### Task 3: Page images

**Files (all under `site/img/`):**
- Create: `cat-windup.png`, `cat-sprinting.png`, `cat-skid.png` (copies of `assets/cats/anim-{windup,sprinting,skid}.png`; `cat-running.png` already equals `anim-running.png`)
- Create: `cat-windup-still.png`, `cat-sprinting-still.png`, `cat-skid-still.png` (frame 0 of each APNG via Pillow, saved as a plain PNG)
- Create: `light-detail.webp`, `light-detail@2x.webp` from `docs/screenshots/light/session-detail.png` (2240×1440 is @2x; 1x is 1120×720), same quality as the existing WebPs
- Create: `widget-small-{light,dark}.webp`, `widget-medium-{light,dark}.webp` from `docs/screenshots/widgets/data-{small,medium}-{light,dark}.png`
- Create: `menubar-{sleeping,lookingaround,running}.png` copied from `docs/screenshots/menubar/`
- Modify: `og.png` re-rendered at 1200×630 with the same composition as today (icon, "Watch your agents work", the agents line, running cat on a stage) on `#E3EADF`; render a throwaway HTML in the scratchpad with `"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless --screenshot=og.png --window-size=1200,630 --hide-scrollbars file://…`; do not commit the HTML.

- [ ] **Step 1: Produce the files above**

- [ ] **Step 2: Verify**

Run: `python3 -I -c "from PIL import Image; import glob; [print(f, Image.open(f).size, getattr(Image.open(f),'n_frames',1)) for f in sorted(glob.glob('site/img/*'))]"`
Expected: the three new `cat-*.png` have `n_frames > 1`, every `*-still.png` has 1, `og.png` is `(1200, 630)`, `light-detail@2x.webp` is `(2240, 1440)`. Read `og.png` and confirm the ground is sage, not cream.

- [ ] **Step 3: Commit**

```bash
git add site/img
git commit -m "Landing page images: cat states, session detail, widgets, menu bar, sage og image"
```

---

### Task 4: Rebuild `index.html` and `style.css`

**Files:**
- Modify: `site/index.html`, `site/style.css`

**Interfaces:**
- Consumes: Task 2's ids, token names and rules; Task 3's file names.

- [ ] **Step 1: Restyle tokens in `style.css`**

Light `:root`: `--ground: #E3EADF; --card: #F4F7F2; --ink: #2B2430; --muted: #4E5249;` keyline/rule in a darker sage (about `#C9D3C4`); keep `--work`, `--idle`, `--sprint` for the table tags; pick `--link` that passes on both sage surfaces. Dark block: ground about `#1A211B`, card about `#242C24`, ink `#F3ECF5`, muted and link chosen to pass. Remove the hero `.stage` rule.

- [ ] **Step 2: Hero**

Keep icon `<picture>`, `h1` "Watch your agents work", lede, `.download` (text "Download for Mac", href = DMG URL), `.fine` "Free and open source · macOS 14 or later on Apple Silicon · signed and notarized", `.honest`. Below it, a `.window` block: the Overview `<picture>` (light/dark sources as today) with the running cat `<picture>` (still under reduced motion) absolutely positioned on the screenshot's top edge, animated left→right across the window width with a CSS `@keyframes` translate (about 9 s, linear, infinite), `animation: none` under `prefers-reduced-motion: reduce`. `image-rendering: pixelated` on sprites.

- [ ] **Step 3: Sections 2–7 in this order and with these ids**

| id | Heading (draft copy; may be tightened, not made less exact) | Content |
|---|---|---|
| `cats` | One cat per agent. Read its mood, not a dashboard. | 4 tiles (wind-up: a burst is starting; running: working; sprinting: heavy output; skid: just stopped), each a `<picture>` with its still; then today's Cats figure and caption |
| `answers` | (no h2 needed visually; give it an accessible one) Three rows alternating text/image: "Is anything stuck?" → Overview with today's roster caption; "What's it costing?" → Usage with today's usage caption; "How hard is it working?" → `light-detail.webp`: "Open any session to see its context size per request, its running estimated cost, CPU and memory." |
| `menubar` | Close the window. The cat stays in your menu bar. | the three menu-bar PNGs; "A cat follows overall activity next to the number of active agents. Desktop widgets show today's figures." Widgets as `<picture>` light/dark |
| `private` | Your prompts never leave your Mac. AgentPulse never reads them. | six cards: No account, no API key · Metadata only · One network request (the update check; you can turn it off) · About 1% of one CPU core (and 13 MB, measured with three live agents) · Counts checked (token totals matched an independent count exactly) · Open source (MIT or Apache 2.0); then today's "What it reads, and how sure each number is" table and its "more" link; then today's privacy paragraphs |
| `agents` | Works with the agents you already run | four pills in the Global Constraints order; "Subagents included. Nothing to install or configure in the agents." |
| `download` | Try it in a minute. | second `.download` "Download AgentPulse" (DMG URL); fine print "Signed and notarized by Apple · macOS 14 or later on Apple Silicon · Release notes · All releases"; today's Install steps (step 1 says "Download the DMG" linking the DMG URL), Requirements, the App Store sentence; FAQ as `<details>`: "Does it work on Intel Macs?" — "Not yet. It needs an Apple Silicon Mac."; "Are the costs exact?" — "No. For Claude Code and Codex CLI they're estimates at API list prices, and the app always says so; OpenCode and Grok CLI record their own charge."; "How do updates work?" — today's update sentence |

Then today's footer unchanged.

- [ ] **Step 4: Layout**

Content width `min(968px, 100% - 32px)`. Tiles: 4 columns ≥ 760 px, 2 below. Fact cards: 3 columns ≥ 760 px, 1 column below 520 px. Answer rows: two columns ≥ 760 px, alternating sides; stacked below. Below-the-fold images `loading="lazy"` with `width`/`height` set.

- [ ] **Step 5: Run the check**

Run: `python3 scripts/check-site.py`
Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add site/index.html site/style.css
git commit -m "Landing page: pale sage, cat on the window, seven sections, one-click download"
```

---

### Task 5: Browser verification (Claude runs this; sandboxed agents cannot launch Chrome)

- [ ] **Step 1: Render**

Serve with `python3 -m http.server -d site 8765`, then headless Chrome screenshots of `http://localhost:8765/` at 1440×900 full page, 375×812 and 320×700, each in light and dark (`--force-dark-mode` plus `--blink-settings=preferredColorScheme=0` for dark, `=1` for light) and once with `--force-prefers-reduced-motion`.

- [ ] **Step 2: Check each image**

Expected: no horizontal scrollbar or clipped content at 375 and 320; the window screenshot clearly separated from the page ground in both schemes; the cat on the window's top edge; tiles, rows, cards and table reflowed; reduced-motion render shows stills.

- [ ] **Step 3: Network**

`test_no_third_party_requests` covers the markup; also confirm the stylesheet loads nothing external.
Run: `grep -nE 'url\(|@import' site/style.css`
Expected: no output.

- [ ] **Step 4: Fix rounds**

Anything wrong goes back to Task 4 (or 3), then re-run `check-site.py` and this task. Commit fixes with a message naming what changed.

---

### Task 6: Publish (outward-facing: each step needs the owner's go-ahead)

- [ ] **Step 1: Ask the owner, then attach `AgentPulse.dmg` to v0.0.10**

```bash
# a fresh, empty directory in the session scratchpad, not the repo
D="$(mktemp -d)" && cd "$D"
gh release download v0.0.10 -R chengboonrong/agentpulse -p 'AgentPulse-0.0.10.dmg'
cp AgentPulse-0.0.10.dmg AgentPulse.dmg
gh release upload v0.0.10 AgentPulse.dmg -R chengboonrong/agentpulse
```

Run: `curl -sIL https://github.com/chengboonrong/agentpulse/releases/latest/download/AgentPulse.dmg | grep -E '^HTTP|content-length' | tail -2`
Expected: final `HTTP/2 200` and a content length equal to `AgentPulse-0.0.10.dmg`'s size.

- [ ] **Step 2: Owner turns on Pages** (Settings → Pages → Source: GitHub Actions). Confirm: `gh api repos/chengboonrong/agentpulse/pages -q .build_type` prints `workflow`.

- [ ] **Step 3: With the owner's OK, push and open the PR**

`git push -u origin landing-redesign` and `gh pr create` with a short body ending in `🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Expected: the Site workflow's `links` job (lychee + `check-site.py`) passes.

- [ ] **Step 4: After merge,** the `deploy` job succeeds. Hand the owner the remaining checklist: DNS `CNAME agentpulse → chengboonrong.github.io`, Pages custom domain `agentpulse.snitco.com`, Enforce HTTPS, repo website field. Verify once DNS resolves: `curl -sI https://agentpulse.snitco.com/ | head -1` → `HTTP/2 200`.
