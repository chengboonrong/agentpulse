# Pairing Guide Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Execution in this repo:** Codex implements each task (`codex-companion task --write`).
> Codex does not commit: Claude reads the diff, runs the task's checks, and commits.

**Goal:** Move pairing out of the Mac's Settings into a Phones screen in the sidebar.
When nothing is paired, that screen walks the user through pairing in three steps. The
phone apps' empty state points to the new screen.

**Architecture:** A new `Screen::Phones` in the GPUI app (`crates/agentpulse-app`)
renders `settings::phones_screen`, built from the existing Phones card's pieces. The card
leaves the Settings screen. The App Store QR code is a committed constant made by a
Swift script (CoreImage, the same generator the bridge uses). The script decodes the
code back to check it, so the app makes no network call and needs no bridge for it.

**Tech Stack:** Rust + GPUI (Mac app), SwiftUI (iOS), Jetpack Compose (Android), Swift
script (QR generation).

**Spec:** `docs/superpowers/specs/2026-11-22-pairing-guide-design.md`

## Global Constraints

- No request before the user pairs: the App Store QR code is a constant in the binary,
  and the link opens only on click (`cx.open_url`).
- App Store link, exactly: `https://apps.apple.com/app/agentpulse/id6835137307`.
- Mac site, exactly: `agentpulse.snitco.com`.
- Sidebar order: Overview · Sessions · Usage · Clean-up · Cats · Phones · Settings.
  Clean-up landed on main after the spec; Phones still goes directly after Cats.
- The pairing protocol, the pairing QR code, the confirm-the-code step and all Phones
  behaviour stay the same. Only where they render and their surrounding copy change.
- CI gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `./gradlew :app:testDebugUnitTest :app:assembleDebug` (from
  `android/`).
- Copy style: plain sentences, the existing button names exactly ("Pair a Phone",
  "Pair a Mac"), no exclamation marks.

## Review Focus

1. Without the app bundle (`cargo run`, `v.available == false`), the Phones screen shows
   only "Phones need AgentPulse.app (run scripts/bundle.sh)." No steps and no QR code.
   *Test: Task 2, `guide_hidden_without_bridge`.*
2. A pairing started from the guide, then cancelled or expired, brings the steps back.
   The guide depends only on the current view, not on history.
   *Test: Task 2, `guide_follows_pairings_and_pending`.*
3. Removing the last paired phone brings the steps back.
   *Test: Task 2, `guide_follows_pairings_and_pending`.*
4. Relay and pairing errors (`hub.phone_error`, `v.error`) still show on the Phones
   screen after the move. *Check: Task 2 screenshot step, `phones-error.png`.*
5. The App Store QR code and the pairing QR code never render together, and their
   element ids and accessible labels differ. *Test: Task 2, `guide_hidden_while_pairing`;
   ids are pinned in Task 2, Step 3.*

---

### Task 1: App Store QR constant

**Files:**
- Create: `scripts/app-store-qr.swift`
- Modify: `crates/agentpulse-app/src/ui/settings.rs` (new const and fn near `QR_SIDE`)
- Test: `crates/agentpulse-app/src/ui/settings.rs` (`#[cfg(test)] mod tests`; create the
  module if the file has none)

**Interfaces:**
- Produces: `pub(super) const APP_STORE_URL: &str` and `pub(super) fn app_store_qr() -> Qr`
  (`crate::phones::Qr { size, dark }`), with the same module layout as the bridge's
  scale-1 output, including CoreImage's 1-module margin.

- [ ] **Step 1: Write `scripts/app-store-qr.swift`**
  - Run with `swift scripts/app-store-qr.swift`.
  - It encodes `APP_STORE_URL` with `CIQRCodeGenerator`, correction level `M` (CoreImage's
    default; match whatever `macos/LinkBridge/LinkBridge.swift` sets).
  - It reads back one value per module: dark when red < 128.
  - It decodes that bitmap with `CIDetector(ofType: CIDetectorTypeQRCode)` at 10× scale,
    and exits non-zero unless the decoded text equals the URL.
  - It prints a Rust `const APP_STORE_QR: &str` with one line per row: `#` for dark,
    `.` for light.

- [ ] **Step 2: Run it**

  Run: `swift scripts/app-store-qr.swift`
  Expected: exit 0, then a square block of `#`/`.` rows.

- [ ] **Step 3: Write the failing tests**

```rust
#[test]
fn app_store_qr_is_square_and_has_finder_patterns() {
    let qr = app_store_qr();
    // A QR symbol is 21 + 4k modules; CoreImage adds a 1-module margin on each side.
    assert!(qr.size >= 23 && (qr.size - 2 - 21) % 4 == 0, "size {}", qr.size);
    assert_eq!(qr.dark.len(), qr.size * qr.size);
    // Top-left finder: the corner just inside the 1-module margin is dark,
    // the margin itself is light.
    let at = |x: usize, y: usize| qr.dark[y * qr.size + x];
    assert!(!at(0, 0));
    assert!(at(1, 1) && at(7, 1) && at(1, 7));
    assert!(at(qr.size - 2, 1) && at(1, qr.size - 2));
}

#[test]
fn app_store_url_is_the_listing() {
    assert_eq!(APP_STORE_URL, "https://apps.apple.com/app/agentpulse/id6835137307");
}
```

- [ ] **Step 4: Run them to see them fail**

  Run: `cargo test -p agentpulse-app app_store`
  Expected: FAIL, because `app_store_qr` is not defined.

- [ ] **Step 5: Paste the script's const and add `app_store_qr()`**

  `app_store_qr()` parses `APP_STORE_QR`: rows split on `'\n'`, blank lines skipped, `#`
  is dark. Put a header comment on the const: "Generated by
  `swift scripts/app-store-qr.swift`; rerun if APP_STORE_URL changes."

- [ ] **Step 6: Run the tests**

  Run: `cargo test -p agentpulse-app app_store`
  Expected: PASS.

- [ ] **Step 7: Claude reviews the diff and commits**
  `Mac app: App Store QR code for the pairing guide, generated and checked by a script`

---

### Task 2: Phones screen on the Mac

**Files:**
- Modify: `crates/agentpulse-app/src/ui.rs`
  - `enum Screen` (~426): add `Phones` after `Cats`.
  - Sidebar (~1151): `self.nav("Phones", Screen::Phones, t, cx)` between Cats and Settings.
  - Content match (~1170): a `(Screen::Phones, _)` arm that builds `PhoneControls` exactly
    as the Settings arm does and calls `settings::phones_screen`. The Settings arm then
    calls `settings::screen` without `phone`.
  - Screenshot driver (~805-830): for the phone fixtures, set `this.screen = Screen::Phones`
    and reset `page_scroll` to the top, instead of `scroll_to_item(4)`.
  - `phone_fixtures` (~825): make it return 5 entries. Rename `phones-no-relay.png` to
    `phones-empty.png` (its view has no phones: the guide). Add `phones-error.png`: the
    paired fixture's view with `error: Some("Cannot reach the relay: timed out".into())`.
- Modify: `crates/agentpulse-app/src/ui/settings.rs`
- Test: `crates/agentpulse-app/src/ui/settings.rs` `mod tests`

**Interfaces:**
- Consumes: `APP_STORE_URL` and `app_store_qr()` from Task 1.
- Produces:
  - `pub(super) fn phones_screen(hub: &Hub, entity: &Entity<Hub>, phone: PhoneControls, t: Theme, scroll: &gpui::ScrollHandle) -> impl IntoElement`
  - `fn guide_shown(v: &crate::phones::View) -> bool`
  - `settings::screen` without the `phone: PhoneControls` parameter.

- [ ] **Step 1: Write the failing tests**

```rust
fn view() -> crate::phones::View {
    crate::phones::View { available: true, ..Default::default() }
}

#[test]
fn guide_hidden_without_bridge() {
    assert!(!guide_shown(&crate::phones::View::default()));
}

#[test]
fn guide_follows_pairings_and_pending() {
    assert!(guide_shown(&view()));
    let paired = crate::phones::View { phones: vec![/* one Phone, as in phone_fixtures */], ..view() };
    assert!(!guide_shown(&paired));
    // Removing the last phone, or a pairing ending, is the same view as before: guide back.
    assert!(guide_shown(&view()));
}

#[test]
fn guide_hidden_while_pairing() {
    let pending = crate::phones::View { pending: Some(/* Started, as in phone_fixtures */), ..view() };
    assert!(!guide_shown(&pending));
}
```

  Build the `Phone` and `Started` values the way `phone_fixtures` in `ui.rs` builds them.
  If the field types make that awkward, add a small `#[cfg(test)]` constructor.

- [ ] **Step 2: Run them to see them fail**

  Run: `cargo test -p agentpulse-app guide_`
  Expected: FAIL, because `guide_shown` is not defined.

- [ ] **Step 3: Implement**
  - `guide_shown(v)` is `v.available && v.phones.is_empty() && v.pending.is_none()`.
  - `phones_screen` scrolls like `settings::screen` (`id("phones")`, `heading("Phones")`).
    Its content, top to bottom:
    - **Not available:** only the existing "Phones need AgentPulse.app (run
      scripts/bundle.sh)." line. Nothing else.
    - **Guide card** (when `guide_shown`):
      - Title "Get AgentPulse on your iPhone or iPad".
      - Three numbered rows with these exact strings:
        1. "Install AgentPulse from the App Store." Shows the App Store QR (`qr_code`
           with id `"app-store-qr"`, aria label `"App Store QR code"`) and a link button
           "Open App Store" (id `"open-app-store"`) that calls `cx.open_url(APP_STORE_URL)`.
        2. "Click Pair a Phone." Shows the existing Pair a Phone button, its label
           ("Pairing…" while starting) and its handler unchanged.
        3. "On your phone, tap Pair a Mac, scan the code, and check that both screens
           show the same code."
    - **Pairing card** (when not `guide_shown` and available): the current card body.
      That is the errors, the `pairing_sheet` or the Pair a Phone button, and the
      paired list. Under the list goes one secondary line, "Another device? Get
      AgentPulse from the App Store.", with an inline "Open App Store" link button.
    - **Connection card** (when available): title "Connection". It holds the relay row,
      the relay note, the relay key row, the Direct connection switch and
      `PHONES_PRIVACY`, moved as they are today.
    - **Errors:** in the guide state, `hub.phone_error` and `v.error` render inside the
      guide card, above step 2, in the same failed colour as today.
  - Split `phones_card` into these pieces rather than copying them. Delete the parts
    nothing uses.
  - `qr_code(qr, id, label)` takes the element id and accessible label as parameters. The
    pairing sheet passes `"pair-qr"` / `"Pairing QR code"`, as today.
  - `settings::screen` drops `phones_card` and the `phone` parameter.
  - Spacing, type, numbering style and QR size follow `DESIGN.md` and the existing cards.
    The App Store QR can be smaller than `QR_SIDE`; it only needs to scan from a phone
    held up to the screen.

- [ ] **Step 4: Run the tests and gates**

  Run: `cargo test -p agentpulse-app && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
  Expected: all pass.

- [ ] **Step 5: Screenshots (Claude runs this; it needs a display)**

  Run: `cargo run -p agentpulse-app --features screenshot -- --screenshot <dir>`
  Expected:
  - `phones-empty.png` shows the guide: three steps, the App Store QR code, Pair a Phone.
  - `phones-qr.png` shows the pairing sheet and no App Store QR code.
  - `phones-confirm.png` and `phones-paired.png` show the list, the "Another device?"
    line, and Connection.
  - `phones-error.png` shows the error text.
  - `settings.png` / `settings-end.png` have no Phones card.
  - The App Store QR in `phones-empty.png` scans to the listing with a phone.
  - Check light and dark.

- [ ] **Step 6: Claude reviews the diff and commits**
  `Mac app: Phones gets its own screen, with a three-step guide until a phone is paired`

---

### Task 3: Phone apps' empty state

**Files:**
- Modify: `ios/App/MacsView.swift:56`
- Modify: `android/app/src/main/kotlin/io/github/chengboonrong/agentpulse/Screens.kt:183`

**Interfaces:** none. Copy only.

- [ ] **Step 1: Replace the one-line description on both platforms with this exact text**

```
1. Install AgentPulse on your Mac from agentpulse.snitco.com.
2. Open it, choose Phones in the sidebar, and click Pair a Phone.
3. Tap Pair a Mac below and scan the code.
```

  - iOS: one `Text` with the three lines joined by `"\n"`, left-aligned
    (`.multilineTextAlignment(.leading)`), inside the existing `ContentUnavailableView`
    description.
  - Android: the same string with `"\n"` in the existing `Message(...)` call. If `Message`
    centres the text, left-align it.
  - Keep the title "No Macs" and the "Pair a Mac" button unchanged.

- [ ] **Step 2: Build both**

  Run (iOS, Claude):
  `xcodegen generate --spec ios/project.yml --project ios && xcodebuild -project ios/AgentPulsePhone.xcodeproj -scheme AgentPulse -destination 'generic/platform=iOS Simulator' build`
  Run (Android): `cd android && ./gradlew --console=plain :app:testDebugUnitTest :app:assembleDebug`
  Expected: both succeed.

- [ ] **Step 3: Look at it (Claude)**

  Launch the iOS app in a simulator with no pairings (`ios/README.md`, the `empty`
  fixture). The three lines read left-aligned above Pair a Mac, in light and dark.

- [ ] **Step 4: Claude reviews the diff and commits**
  `Phone apps: No Macs gives the three pairing steps and names the Mac's Phones screen`

---

### Task 4: Docs that name "Settings, Phones"

**Files (modify):** `README.md:130`, `docs/mobile.md:35,94,173,219`,
`docs/telemetry-spec.md:476`, `relay/README.md:98`.

- [ ] **Step 1: Point each mention at the Phones screen**

  "Settings, Phones" becomes "Phones (in the sidebar)" on first mention in a file and
  "Phones" after that. `docs/mobile.md:35` and `:219` describe the "Phones card". Change
  them to "the Phones screen", with relay and direct connection under "Connection".

- [ ] **Step 2: Check nothing is left**

  Run: `grep -rn "Settings, Phones\|Settings, then Phones\|Phones card\|\"Phones\" card" --exclude-dir=target --exclude-dir=node_modules --exclude-dir=.git . | grep -v docs/superpowers`
  Expected: nothing printed, apart from code comments in `ui.rs`/`hub.rs` that say
  "Phones card". Reword those to "Phones screen" as well.

- [ ] **Step 3: Claude reviews the diff and commits**
  `Docs: pairing lives on the Phones screen`
