# Notifications: agent waiting, finished, failed

Date: 2026-11-23. Issue #65. The owner chose to build the whole pipeline in phases.

## Where things stand

Nothing reaches the phone as a notification today:
- The Mac never calls the relay's `push`.
- The relay only sends Live Activity pushes, and the phone has no Live Activity.
- The phone has no push entitlement and sends no token.

So the pipeline comes first, and the page that controls it second.

## Phases

1. **Relay, Mac and protocol** (this PR, all testable in CI).
2. **Phone** (next PR):
   - the `aps-environment` entitlement and asking for notification permission;
   - registering and sending the alert token;
   - sending the user's choices;
   - a Notification Service Extension that decrypts each alert;
   - the Notifications page.
3. **Real-device check** (needs the owner): an APNs key as relay secrets, the push capability
   in the Apple developer portal, and a physical iPhone.

## Decisions

- **Events.** Three kinds, each for a session the snapshot shows. An event fires only
  once the new state has held for `HOLD` = 10 s of snapshot time (`taken_at_ms`), and
  once per stretch:
  - `waiting`: a known session moves into Waiting and is still Waiting 10 s later (urgent,
    APNs priority 10). The hold keeps prompts answered at the desk from pushing, and it
    saves the 12 urgent pushes an hour for when you've walked away.
  - `finished`: a known session that was Active becomes Idle or Completed and isn't Active
    or Waiting again for 10 s. This is "the agent finished its turn and is waiting for
    your next prompt", as Orca's agent-finished notice is. A process exit is no longer
    required.
  - `failed`: a known session moves from live to Failed and is still Failed 10 s later.
  - A session seen for the first time never fires: it only records its state. This covers
    the first snapshot after launch, a new session, and an agent switched from hidden back
    to shown (hidden agents' sessions aren't in the snapshot, so they never notify).
  - A single tick of a different status (a torn read, a missed process check) can't fire,
    because nothing fires before the hold.
  - Agents whose status comes only from transcript age (Qwen Code) report `finished` after
    each 30 s pause in writing. That is a known ceiling of their status, not of the alerts.
- **The choice lives on the phone, the filtering on the Mac.** iOS can't drop a delivered
  alert without a special entitlement, so the Mac must not send what the user turned off.
  - The phone tells each paired Mac through the encrypted `up` channel, with a new message
    `{"seq", "type": "notify", "waiting": bool, "finished": bool, "failed": bool}`. The
    relay sees only ciphertext.
  - The Mac keeps the latest choices per pairing, saved with the pairing. Until a `notify`
    arrives, the Mac sends nothing for that pairing. So phones and Macs from before this
    change never get alerts they didn't ask for.
  - One set of choices for all of a phone's Macs: the phone sends the same message to each.
    Per-Mac choices can come later.
- **Alert contents stay end-to-end encrypted.**
  - Per pairing, the Mac sends one push per batch:
    `{"seq", "type": "alert", "events": [{"kind", "agent", "project"}, …]}`. The relay
    allows one `POST push` a second per pairing, so a push per event would lose all but
    the first.
  - It is sealed with a new HPKE direction, `alert` (`info` = `"agentpulse/1 alert
    <pairing id>"`), with its own `seq` counter per pairing. A push can't be opened as a
    state, and it doesn't move the `down` seq the app and widgets guard.
  - `agent` is the provider label (such as "Claude Code") and `project` the folder name, as
    the payload already carries.
  - The relay forwards the ciphertext as `m`. The phone's extension decrypts it and writes
    the text.
  - The relay can see only two things: the plaintext `urgent` flag (set when any event in
    the batch is `waiting`) and the ciphertext's length. Both are accepted, as `PUT down`'s
    length already is.
- **Relay.**
  - `PUT push-tokens` accepts an optional `alert` token beside `activity` and `start`.
  - `POST push` accepts `event: "alert"`, sent with `apns-push-type: alert` and
    `apns-topic: <APNS_TOPIC>`. The body is
    `{"aps": {"alert": {"title": "AgentPulse", "body": "An agent needs your attention"},
    "mutable-content": 1, "sound": "default"}, "m": "<message>"}`.
  - The fixed alert text shows only if the extension can't decrypt.
  - `apns-expiration` is now + 300 s: an alert that can't be delivered within 5 minutes is
    dropped, not shown late.
  - Priority 10 when `urgent`, else 5. The hourly limits are shared with the Live Activity
    (60 regular, 12 urgent).
  - A 410, or a 400 with reason `BadDeviceToken` or `DeviceTokenNotForTopic`, deletes the
    token that was used and answers 410.
  - Any other APNs failure answers 502 with APNs' reason. APNs' own statuses never reach
    the Mac, because the Mac reads a 401 or 404 from the relay as "the pairing is gone".
- **Mac push failures.**
  - 409 (no alert token), 410 (token gone) and 501 (relay without APNs) turn alerts off for
    that pairing until its next `notify` message. A phone that set up notifications again
    sends one.
  - 429 and 502 drop that batch.
  - No answer (network error or timeout), or a pairing still in relay backoff, skips that
    pairing for this batch. Pushes then can't hold up the phones worker for longer than
    one request per pairing.
  - No retry queue: an alert arriving late is worse than none.
- **Sending.** The collector hands each tick's events to the phone link, which seals and
  posts one push per pairing.
- **Direct connections don't change anything.** Pushes always go through the relay and
  APNs, because the phone may be locked or in another app.

## Phase 2 notes (the phone)

- `PUT push-tokens` replaces all of a pairing's tokens, so the phone sends `alert` on every
  PUT.
- It PUTs the token before it sends `notify`. Otherwise the first event gets 409 and alerts
  stay off until the next `notify`.
- It sends `notify` again after every `PUT push-tokens` and with each `seen`, so alerts a
  409, 410 or 501 turned off (a token rotation, a relay redeploy) come back on their own.
- The alert carries no pairing id: the extension tries each paired Mac's key until one opens.
- iOS shows any alert the extension was woken for. So when an alert's `seq` isn't newer than
  the last one seen (a relay replaying an old ciphertext), the extension shows the fixed
  fallback text instead of the old content.

## Acceptance (this PR)

- **Relay tests:**
  - `push-tokens` with `alert` is stored and validated like the others;
  - `push` with `event: "alert"` sends the headers and body above to the fake APNs, with
    priority 10 for urgent and 5 otherwise;
  - 409 without an alert token; 410, `BadDeviceToken` and `DeviceTokenNotForTopic` remove
    it and answer 410; other APNs failures answer 502; the shared limits apply;
  - `apns-expiration` is set for alerts.
- **Swift tests (`link`):** a message sealed with direction `alert` opens with `alert` and
  fails with `down` and `up`. The Mac's bridge accepts `alert`.
- **Rust tests:**
  - event detection: each kind fires once after the 10 s hold and not before;
  - a one-tick blip fires nothing;
  - first-seen sessions (launch, new, unhidden) fire nothing;
  - Active to Idle held 10 s gives `finished`; Active to Idle to Active within 10 s gives
    nothing;
  - a `notify` message is stored, persisted and survives reloading the phones state;
  - several events in one batch reach each pairing as one sealed push, with only the
    kinds its choices allow and `urgent` only when a `waiting` is among them. The fake
    relay enforces the one-write-a-second limit;
  - no `notify` means no push; 409/410/501 stop pushes until the next `notify`; 429/502 and
    no answer drop only that batch;
  - each pairing's `alert` seq increases.
- `docs/mobile.md`:
  - the `notify` message;
  - the `alert` direction and its seq;
  - the relay's `alert` token and event;
  - the Mac client's alert rules.
- Existing checks pass: `cargo test --workspace`, clippy, fmt, `swift test --package-path
  link`, relay `npm test` and `npm run typecheck`, and Android unit tests (untouched).
