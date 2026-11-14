# AgentPulse relay

The mailbox between a Mac running AgentPulse and the phones paired with it, plus the
gateway that sends Live Activity pushes through Apple (APNs). One Cloudflare Worker with
one Durable Object per pairing. The design, the routes and the limits are in
[docs/mobile.md](../docs/mobile.md) ("Relay API", "Push gateway", "Cryptography").

## What it stores

Per pairing, and nothing else:

- the pairing id (16 random bytes);
- SHA-256 hashes of the Mac's and the phone's relay tokens, and of the pairing secret until
  the phone's hello arrives;
- the latest Mac-to-phone message (`down`) and its `seq`, and up to 32 queued
  phone-to-Mac messages (`up`), all end-to-end encrypted ciphertext;
- the phone's Apple push tokens for the Live Activity and their environment;
- when the pairing was created and last used (recorded at most hourly, so reads don't
  write), and the times of its pushes in the last hour, for the push limit.

A pairing with no hello after 10 minutes, or unused for 30 days, is deleted. `DELETE` by
either side deletes it at once.

Hashes are SHA-256 of the 32 decoded bytes, sent and stored as unpadded base64url.

## Develop

Node 22 or newer.

```sh
npm ci
npm test              # Vitest in the Workers runtime
npm run typecheck     # tsc --noEmit
npm run dev           # wrangler dev on http://localhost:8787
curl localhost:8787/v1/health
```

Without APNs secrets the relay still relays, and `POST .../push` answers 501. To try push
locally, put the four secrets below in `.dev.vars` (git-ignored) as `NAME=value` lines.

## Limits

The limits are in [docs/mobile.md](../docs/mobile.md) ("Relay API"): one write and
four reads a second per pairing, side and route (429 with `Retry-After`), 60 regular and
12 urgent pushes an hour per pairing, counted apart, request bodies of at most 20,000 bytes.

Creating a pairing and posting a hello need no relay token, so a public relay should use
one or both of these. Without them anyone can create pairings; unanswered ones expire
after 10 minutes.

- `RELAY_KEY` (secret): when set, `POST /v1/pairings` needs `Authorization: Bearer
  <RELAY_KEY>`, so only Macs given the key can pair phones.
- `CREATE_LIMITER` (Workers Rate Limiting binding): when bound, create and hello are
  limited per client (`CF-Connecting-IP`: the IPv4 address, or the IPv6 /64, since one host
  can use a whole /64) and answer 429 past the limit. Uncomment
  the `ratelimits` example in `wrangler.jsonc` and pick the limit.

## Push secrets

| Secret | Value |
|---|---|
| `APNS_KEY_P8` | The `.p8` APNs auth key from the Apple Developer account, PEM text |
| `APNS_KEY_ID` | That key's 10-character id |
| `APNS_TEAM_ID` | The Apple Developer team id |
| `APNS_TOPIC` | The iPhone app's bundle id (the relay appends `.push-type.liveactivity`) |

A `.p8` that doesn't import makes `push` answer 500 `{"error": "push not configured"}`.

APNs only accepts a key from the team that signs the iPhone app, so a self-hosted relay can
push only to a build of the app signed by that same team.

## Self-hosting

On a Cloudflare account (Durable Objects with SQLite storage are on the free plan):

```sh
npm ci
npx wrangler login
npx wrangler deploy
# Recommended for a public relay (and see CREATE_LIMITER above):
npx wrangler secret put RELAY_KEY
# Only for Live Activity pushes:
npx wrangler secret put APNS_KEY_P8 < AuthKey_XXXXXXXXXX.p8
npx wrangler secret put APNS_KEY_ID
npx wrangler secret put APNS_TEAM_ID
npx wrangler secret put APNS_TOPIC
```

Then put the Worker's URL (for example `https://agentpulse-relay.<subdomain>.workers.dev`)
in AgentPulse, Settings, Phones.
