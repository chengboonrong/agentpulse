# Pairing guide: a Phones screen on the Mac

Status: design, approved by the owner 2026-11-22. Implementation by Codex.

## Why

Pairing is a first-time task, but on the Mac it is the last card in Settings (after
Animation and Updates). Nothing else in the Mac app mentions that an iPhone app exists. The
owner: "I don't feel like that UX is intuitive enough for the first time." Now that the
iPhone app is on the App Store, the Mac app is where most people will first hear about it.

## Goals

- A **Phones** entry in the Mac sidebar: Overview · Sessions · Usage · Cats · **Phones** ·
  Settings. Pairing moves there from Settings.
- With nothing paired, the Phones screen is the guide: three numbered steps, with the
  App Store link and QR code for step 1 and the existing Pair a Phone button for step 2.
- The phone apps' empty state ("No Macs") gives the same steps, starting from the phone,
  and names the new place.

## Non-goals

- A walkthrough, welcome pages or onboarding wizard on either app.
- A menu-bar item, Overview tip or banner pointing at Phones. Add one later if the sidebar
  entry turns out not to be enough.
- Any change to the pairing protocol, the QR code or the confirm-the-code step.

## Anti-goals

- Anything that sends a request before the user pairs. Phones stay off by default (see
  `docs/mobile.md`, Rules). The App Store QR code is generated on the Mac, and the link
  opens only when clicked.
- Nagging: nothing reappears or asks again after the user ignores it.

## Approaches

- **A. Own sidebar screen (picked).** It fixes where pairing lives instead of explaining
  that it is hidden, and its empty state does the guide's job. Cost: one more sidebar
  entry for a feature people use rarely.
- B. Keep pairing in Settings and add pointers to it (a one-time Overview tip, a menu-bar
  item that opens Settings with the card highlighted). Smallest change, but pairing stays
  at the bottom of Settings, and the problem returns once the tip is dismissed.
- C. A step-by-step setup guide on both apps. The most work. It repeats a flow that is
  already three steps, and people often skip guides.

The owner left the choice to Claude, which picked A.

## Design

### Mac: Phones screen

Nothing paired and no pairing in progress:

```
Phones

Get AgentPulse on your iPhone or iPad

1  Install AgentPulse from the App Store.      [QR]   Open App Store
2  Click Pair a Phone.                         [ Pair a Phone ]
3  On your phone, tap Pair a Mac, scan the code, and check that
   both screens show the same code.

Connection                                      (relay, relay key, direct)
```

- During pairing, the existing pairing sheet (QR code, then confirm the code) replaces
  steps 2 and 3. Cancel returns to the steps.
- With one or more phones paired: the paired list as it is today, the Pair a Phone button
  for another phone, and Connection below. The three steps are not shown. Step 1's App
  Store link stays as one secondary line, for a second device.
- Connection holds the Phones card's current controls: relay address, relay key, direct
  connection. The Phones card leaves Settings.
- Without the app bundle (`cargo run`), the screen shows today's "Phones need
  AgentPulse.app" line, with no steps.
- App Store link: `https://apps.apple.com/app/agentpulse/id6835137307`.

### iPhone and iPad: "No Macs"

`ios/App/MacsView.swift`, replacing the one-line text:

```
No Macs

1  Install AgentPulse on your Mac from agentpulse.snitco.com.
2  Open it, choose Phones in the sidebar, and click Pair a Phone.
3  Tap Pair a Mac below and scan the code.

[ Pair a Mac ]
```

The Android app's same string (`android/.../Screens.kt:183`) gets the same steps.

### Copy that names "Settings, Phones"

Update the copy to "Phones" (the Mac's sidebar): `README.md:130`, `docs/mobile.md` (35, 94,
173, 219), `docs/telemetry-spec.md:476` and `relay/README.md:98`.

## Left to the builder

- Spacing, type and the QR code's size, following `DESIGN.md` and the existing cards.
- How the App Store QR code is made, as long as it needs no network: the bridge's `qr`
  call, or a QR image stored in the bundle.
- The sidebar icon, if the sidebar uses icons.
- Moving the `phones-*.png` screenshot targets (`crates/agentpulse-app/src/ui.rs`) to the
  new screen, and adding an empty-state target there.
