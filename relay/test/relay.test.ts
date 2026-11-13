import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { env, exports } from "cloudflare:workers";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const b64 = (b: Uint8Array) => b.toBase64({ alphabet: "base64url", omitPadding: true });
const unb64 = (s: string) => Uint8Array.fromBase64(s, { alphabet: "base64url" });
const rand = (n: number) => b64(crypto.getRandomValues(new Uint8Array(n)));
const hash = async (s: string) => b64(new Uint8Array(await crypto.subtle.digest("SHA-256", unb64(s))));
const MIN = 60_000;
const DAY = 86_400_000;

// A fake clock that only moves forward, so alarms the relay sets stay in the real future.
let clock = Date.now();
beforeEach(() => {
  vi.spyOn(Date, "now").mockImplementation(() => clock);
});
afterEach(() => {
  vi.restoreAllMocks();
});

interface Opts {
  token?: string;
  body?: unknown;
  headers?: Record<string, string>;
  tick?: boolean;
}
// Each request moves the clock 1 s on, past the write limit, unless `tick` is false.
function req(method: string, path: string, { token, body, headers = {}, tick = true }: Opts = {}) {
  if (tick) clock += 1000;
  if (token) headers = { ...headers, authorization: `Bearer ${token}` };
  const raw = body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body);
  return exports.default.fetch(`https://relay.test${path}`, { method, headers, body: raw });
}

async function create() {
  const p = { id: rand(16), mac: rand(32), phone: rand(32), secret: rand(32) };
  const r = await req("POST", "/v1/pairings", {
    body: { id: p.id, macTokenHash: await hash(p.mac), secretHash: await hash(p.secret) },
  });
  expect(r.status).toBe(201);
  return p;
}

async function hello(p: { id: string; secret: string; phone: string }, message = "hello") {
  return req("POST", `/v1/pairings/${p.id}/hello`, {
    body: { secret: p.secret, phoneTokenHash: await hash(p.phone), message },
  });
}

async function pair() {
  const p = await create();
  expect((await hello(p)).status).toBe(204);
  return p;
}

const stub = (id: string) => env.PAIRINGS.get(env.PAIRINGS.idFromName(id));
const stored = (id: string) => runInDurableObject(stub(id), async (_, state) => state.storage.list());

describe("routing", () => {
  it("answers health with the version", async () => {
    const r = await req("GET", "/v1/health");
    expect(r.status).toBe(200);
    expect(await r.json()).toEqual({ version: expect.any(String) });
  });

  it("404s unknown routes", async () => {
    const id = rand(16);
    for (const path of ["/", "/v1", "/v2/health", "/v1/nope", `/v1/pairings/${id}/nope`, `/v1/pairings/${id}/down/x`, `/v1/pairings/${id}/constructor`]) {
      expect((await req("GET", path)).status, path).toBe(404);
    }
  });

  it("405s wrong methods with Allow", async () => {
    const id = rand(16);
    const cases: [string, string, string][] = [
      ["POST", "/v1/health", "GET"],
      ["GET", "/v1/pairings", "POST"],
      ["GET", `/v1/pairings/${id}`, "DELETE"],
      ["PATCH", `/v1/pairings/${id}/down`, "PUT, GET"],
      ["GET", `/v1/pairings/${id}/push`, "POST"],
    ];
    for (const [method, path, allow] of cases) {
      const r = await req(method, path);
      expect(r.status, `${method} ${path}`).toBe(405);
      expect(r.headers.get("allow")).toBe(allow);
    }
  });

  it("400s malformed bodies and ids", async () => {
    const good = { id: rand(16), macTokenHash: rand(32), secretHash: rand(32) };
    const bad: unknown[] = ["{", "[]", "null", "3", { ...good, id: rand(15) }, { ...good, id: "not base64url!!!!!!!!!" }, { ...good, macTokenHash: "x" }, { ...good, secretHash: undefined }];
    for (const body of bad) {
      expect((await req("POST", "/v1/pairings", { body })).status, JSON.stringify(body)).toBe(400);
    }
    expect((await req("GET", "/v1/pairings/short/down", { token: rand(32) })).status).toBe(400);
  });

  it("413s bodies over the limit, with or without Content-Length", async () => {
    expect((await req("POST", "/v1/pairings", { body: "x".repeat(20_001) })).status).toBe(413);
    const stream = (chunks: string[]) =>
      new ReadableStream<Uint8Array>({
        pull(c) {
          const next = chunks.shift();
          if (next === undefined) c.close();
          else c.enqueue(new TextEncoder().encode(next));
        },
      });
    const post = (body: ReadableStream) => exports.default.fetch("https://relay.test/v1/pairings", { method: "POST", body });
    expect((await post(stream(["x".repeat(10_000), "x".repeat(10_000), "x"]))).status).toBe(413);
    const ok = JSON.stringify({ id: rand(16), macTokenHash: rand(32), secretHash: rand(32) });
    expect((await post(stream([ok.slice(0, 20), ok.slice(20)]))).status).toBe(201);
  });
});

describe("pairing", () => {
  it("creates once", async () => {
    const p = await create();
    const again = await req("POST", "/v1/pairings", { body: { id: p.id, macTokenHash: rand(32), secretHash: rand(32) } });
    expect(again.status).toBe(409);
  });

  it("takes one hello with the right secret, queues it first and forgets the secret", async () => {
    const p = await create();
    expect((await hello({ ...p, secret: rand(32) })).status).toBe(401);
    expect((await hello({ ...p, id: rand(16) })).status).toBe(401);
    expect((await hello(p, "x".repeat(4097))).status).toBe(413);
    expect((await hello(p, "not base64url")).status).toBe(400);
    const bad = await req("POST", `/v1/pairings/${p.id}/hello`, { body: { secret: p.secret, phoneTokenHash: "nope", message: "m" } });
    expect(bad.status).toBe(400);

    expect((await hello(p)).status).toBe(204);
    expect((await hello(p)).status).toBe(409);

    const meta = (await stored(p.id)).get("meta") as Record<string, unknown>;
    expect(meta.secretHash).toBeUndefined();
    expect(meta.phoneTokenHash).toBe(await hash(p.phone));
    expect(JSON.stringify([...(await stored(p.id)).values()])).not.toContain(p.mac);

    expect((await req("POST", `/v1/pairings/${p.id}/up`, { token: p.phone, body: { message: "m1" } })).status).toBe(204);
    const up = await req("GET", `/v1/pairings/${p.id}/up`, { token: p.mac });
    expect(await up.json()).toEqual({ messages: ["hello", "m1"] });
  });

  it("refuses a hello after 10 minutes even before the alarm runs", async () => {
    const p = await create();
    clock += 10 * MIN;
    expect((await hello(p)).status).toBe(401);
  });
});

describe("auth", () => {
  it("answers 401 the same way for every bad token, existing pairing or not", async () => {
    const p = await pair();
    const path = `/v1/pairings/${p.id}/up`;
    const answers = await Promise.all([
      req("GET", path),
      req("GET", path, { headers: { authorization: "Bearer short" } }),
      req("GET", path, { headers: { authorization: p.mac } }),
      req("GET", path, { token: rand(32) }),
      req("GET", path, { token: p.phone }), // the other side's token
      req("GET", `/v1/pairings/${rand(16)}/up`, { token: p.mac }),
    ]);
    for (const r of answers) {
      expect(r.status).toBe(401);
      expect(await r.json()).toEqual({ error: "unauthorized" });
    }
  });

  it("gives the phone nothing before its hello", async () => {
    const p = await create();
    expect((await req("GET", `/v1/pairings/${p.id}/down`, { token: p.phone })).status).toBe(401);
  });
});

describe("down", () => {
  it("keeps the latest state, refuses old seqs and answers 304 to a matching If-None-Match", async () => {
    const p = await pair();
    const path = `/v1/pairings/${p.id}/down`;
    expect((await req("GET", path, { token: p.phone })).status).toBe(204);

    expect((await req("PUT", path, { token: p.mac, body: { seq: 0, message: "s0" } })).status).toBe(204);
    expect((await req("PUT", path, { token: p.mac, body: { seq: 5, message: "s5" } })).status).toBe(204);
    expect((await req("PUT", path, { token: p.mac, body: { seq: 5, message: "again" } })).status).toBe(409);
    expect((await req("PUT", path, { token: p.mac, body: { seq: 4, message: "older" } })).status).toBe(409);

    const r = await req("GET", path, { token: p.phone });
    expect(r.status).toBe(200);
    expect(r.headers.get("etag")).toBe('"5"');
    expect(await r.json()).toEqual({ seq: 5, message: "s5" });
    expect((await req("GET", path, { token: p.phone, headers: { "if-none-match": "5" } })).status).toBe(304);
    expect((await req("GET", path, { token: p.phone, headers: { "if-none-match": '"5"' } })).status).toBe(304);
    expect((await req("GET", path, { token: p.phone, headers: { "if-none-match": "4" } })).status).toBe(200);
  });

  it("validates seq and message", async () => {
    const p = await pair();
    const path = `/v1/pairings/${p.id}/down`;
    for (const body of [{ seq: -1, message: "m" }, { seq: 1.5, message: "m" }, { seq: "3", message: "m" }, { message: "m" }, { seq: 1, message: "a+b" }, { seq: 1 }]) {
      expect((await req("PUT", path, { token: p.mac, body })).status, JSON.stringify(body)).toBe(400);
    }
    expect((await req("PUT", path, { token: p.mac, body: { seq: 1, message: "x".repeat(16_384) } })).status).toBe(204);
    expect((await req("PUT", path, { token: p.mac, body: { seq: 2, message: "x".repeat(16_385) } })).status).toBe(413);
  });
});

describe("up", () => {
  it("queues at most 32 messages and empties on read", async () => {
    const p = await pair();
    const path = `/v1/pairings/${p.id}/up`;
    expect((await req("POST", path, { token: p.phone, body: { message: "x".repeat(4097) } })).status).toBe(413);
    for (let i = 1; i < 32; i++) {
      expect((await req("POST", path, { token: p.phone, body: { message: `m${i}` } })).status).toBe(204);
    }
    const full = await req("POST", path, { token: p.phone, body: { message: "m32" } });
    expect(full.status).toBe(429);
    expect(await full.json()).toEqual({ error: "queue full" });

    const { messages } = await (await req("GET", path, { token: p.mac })).json<{ messages: string[] }>();
    expect(messages).toHaveLength(32);
    expect(messages[0]).toBe("hello");
    expect(messages[31]).toBe("m31");
    expect(await (await req("GET", path, { token: p.mac })).json()).toEqual({ messages: [] });
  });
});

describe("rate limit", () => {
  it("allows one write and four reads a second per side and route", async () => {
    const p = await pair();
    const down = `/v1/pairings/${p.id}/down`;
    const up = `/v1/pairings/${p.id}/up`;
    expect((await req("PUT", down, { token: p.mac, body: { seq: 1, message: "a" } })).status).toBe(204);
    const limited = await req("PUT", down, { token: p.mac, body: { seq: 2, message: "b" }, tick: false });
    expect(limited.status).toBe(429);
    expect(limited.headers.get("retry-after")).toBe("1");
    // Other routes of the same side, and the other side, are separate.
    expect((await req("GET", up, { token: p.mac, tick: false })).status).toBe(200);
    expect((await req("POST", up, { token: p.phone, body: { message: "c" }, tick: false })).status).toBe(204);
    expect((await req("GET", down, { token: p.phone, tick: false })).status).toBe(200);
    const read = await req("GET", down, { token: p.phone, tick: false });
    expect(read.status).toBe(429);
    expect(read.headers.get("retry-after")).toBe("1");
    // Reads get four a second: the app and its widgets share the phone's token.
    clock += 250;
    expect((await req("GET", down, { token: p.phone, tick: false })).status).toBe(200);
    expect((await req("PUT", down, { token: p.mac, body: { seq: 2, message: "b" } })).status).toBe(204);
    expect((await req("GET", down, { token: p.phone, tick: false })).status).toBe(200);
  });
});

describe("storage writes", () => {
  it("doesn't write on reads and persists lastActive at most hourly", async () => {
    const p = await pair();
    let spies: ReturnType<typeof vi.spyOn>[] = [];
    await runInDurableObject(stub(p.id), (_, state) => {
      spies = (["put", "delete", "deleteAll", "setAlarm"] as const).map((m) => vi.spyOn(state.storage, m));
    });
    const lastActive = async () => ((await stored(p.id)).get("meta") as { lastActive: number }).lastActive;
    const before = await lastActive();
    await req("GET", `/v1/pairings/${p.id}/up`, { token: p.mac }); // takes the hello
    for (const s of spies) s.mockClear();

    clock += 30 * MIN;
    expect((await req("GET", `/v1/pairings/${p.id}/down`, { token: p.phone })).status).toBe(204);
    expect((await req("GET", `/v1/pairings/${p.id}/up`, { token: p.mac })).status).toBe(200);
    for (const s of spies) expect(s).not.toHaveBeenCalled();
    expect(await lastActive()).toBe(before);

    clock += 31 * MIN;
    expect((await req("GET", `/v1/pairings/${p.id}/down`, { token: p.phone })).status).toBe(204);
    expect(spies[0]).toHaveBeenCalledTimes(1);
    expect(await lastActive()).toBe(clock);
  });
});

describe("abuse limits on create and hello", () => {
  const createBody = async () => ({ id: rand(16), macTokenHash: rand(32), secretHash: await hash(rand(32)) });

  it("requires RELAY_KEY to create a pairing when it is set", async () => {
    Object.assign(env, { RELAY_KEY: "relay-key" });
    try {
      expect((await req("POST", "/v1/pairings", { body: await createBody() })).status).toBe(401);
      expect((await req("POST", "/v1/pairings", { body: await createBody(), headers: { authorization: "Bearer relay-ke" } })).status).toBe(401);
      expect((await req("POST", "/v1/pairings", { body: await createBody(), headers: { authorization: "Bearer relay-key" } })).status).toBe(201);
    } finally {
      Object.assign(env, { RELAY_KEY: undefined });
    }
  });

  it("applies CREATE_LIMITER by client address to create and hello only", async () => {
    const limit = vi.fn(async ({ key }: RateLimitOptions) => ({ success: key !== "203.0.113.9" }));
    Object.assign(env, { CREATE_LIMITER: { limit } });
    try {
      const blocked = { "cf-connecting-ip": "203.0.113.9" };
      expect((await req("POST", "/v1/pairings", { body: await createBody(), headers: blocked })).status).toBe(429);
      const p = await create();
      const helloBody = { secret: p.secret, phoneTokenHash: await hash(p.phone), message: "hello" };
      expect((await req("POST", `/v1/pairings/${p.id}/hello`, { body: helloBody, headers: blocked })).status).toBe(429);
      expect((await req("POST", `/v1/pairings/${p.id}/hello`, { body: helloBody, headers: { "cf-connecting-ip": "198.51.100.1" } })).status).toBe(204);
      expect(limit.mock.calls.map(([o]) => o.key)).toEqual(["203.0.113.9", "unknown", "203.0.113.9", "198.51.100.1"]);
      await req("GET", `/v1/pairings/${p.id}/up`, { token: p.mac });
      expect(limit).toHaveBeenCalledTimes(4);
    } finally {
      Object.assign(env, { CREATE_LIMITER: undefined });
    }
  });

  it("takes only one of two concurrent hellos", async () => {
    const p = await create();
    const phones = [rand(32), rand(32)];
    const answers = await Promise.all(phones.map((phone) => hello({ ...p, phone })));
    expect(answers.map((r) => r.status).sort()).toEqual([204, 409]);
    const winner = phones[answers.findIndex((r) => r.status === 204)];
    expect(((await stored(p.id)).get("meta") as { phoneTokenHash: string }).phoneTokenHash).toBe(await hash(winner));
  });
});

describe("delete", () => {
  for (const side of ["mac", "phone"] as const) {
    it(`removes everything when the ${side} asks`, async () => {
      const p = await pair();
      expect((await req("PUT", `/v1/pairings/${p.id}/down`, { token: p.mac, body: { seq: 1, message: "a" } })).status).toBe(204);
      expect((await req("DELETE", `/v1/pairings/${p.id}`, { token: rand(32) })).status).toBe(401);
      expect((await req("DELETE", `/v1/pairings/${p.id}`, { token: p[side] })).status).toBe(204);
      expect((await stored(p.id)).size).toBe(0);
      expect(await runInDurableObject(stub(p.id), (_, state) => state.storage.getAlarm())).toBeNull();
      expect((await req("GET", `/v1/pairings/${p.id}/up`, { token: p.mac })).status).toBe(401);
    });
  }
});

describe("expiry", () => {
  it("removes a pairing that got no hello in 10 minutes", async () => {
    const p = await create();
    clock += 5 * MIN;
    expect(await runDurableObjectAlarm(stub(p.id))).toBe(true);
    expect((await stored(p.id)).size).toBe(1); // not due yet: rescheduled
    clock += 5 * MIN;
    expect(await runDurableObjectAlarm(stub(p.id))).toBe(true);
    expect((await stored(p.id)).size).toBe(0);
    expect((await hello(p)).status).toBe(401);
  });

  it("removes a pairing idle for 30 days", async () => {
    const p = await pair();
    clock += 29 * DAY;
    expect((await req("GET", `/v1/pairings/${p.id}/down`, { token: p.phone })).status).toBe(204);
    clock += 2 * DAY;
    expect(await runDurableObjectAlarm(stub(p.id))).toBe(true);
    expect((await req("GET", `/v1/pairings/${p.id}/up`, { token: p.mac })).status).toBe(200);
    clock += 30 * DAY;
    expect(await runDurableObjectAlarm(stub(p.id))).toBe(true);
    expect((await stored(p.id)).size).toBe(0);
  });
});

describe("push", () => {
  const activity = "ab".repeat(40);
  const start = "cd".repeat(40);
  let publicKey: CryptoKey;

  async function withTokens(environment = "development", tokens: object = { activity, start }) {
    const p = await pair();
    const r = await req("PUT", `/v1/pairings/${p.id}/push-tokens`, { token: p.phone, body: { ...tokens, environment } });
    expect(r.status).toBe(204);
    return p;
  }
  const push = (p: { id: string; mac: string }, body: object) => req("POST", `/v1/pairings/${p.id}/push`, { token: p.mac, body });

  function mockApns(res = () => new Response(null, { status: 200 })) {
    return vi.spyOn(globalThis, "fetch").mockImplementation(async () => res());
  }
  function sent(spy: ReturnType<typeof mockApns>, i = 0) {
    const [url, init] = spy.mock.calls[i] as [string, RequestInit];
    return { url, headers: init.headers as Record<string, string>, body: JSON.parse(init.body as string) };
  }
  async function readJwt(authorization: string) {
    const jwt = authorization.replace(/^bearer /, "");
    const [h, c, s] = jwt.split(".");
    const part = (x: string) => JSON.parse(new TextDecoder().decode(unb64(x)));
    const valid = await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, publicKey, unb64(s), new TextEncoder().encode(`${h}.${c}`));
    return { jwt, header: part(h), claims: part(c), valid };
  }

  it("answers 501 without APNs secrets", async () => {
    const p = await withTokens();
    expect((await push(p, { event: "update", message: "m", urgent: false })).status).toBe(501);
  });

  describe("with APNs secrets", () => {
    beforeAll(async () => {
      const keys = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
      publicKey = keys.publicKey;
      const der = new Uint8Array((await crypto.subtle.exportKey("pkcs8", keys.privateKey)) as ArrayBuffer);
      const pem = `-----BEGIN PRIVATE KEY-----\n${der.toBase64().match(/.{1,64}/g)!.join("\n")}\n-----END PRIVATE KEY-----\n`;
      Object.assign(env, { APNS_KEY_P8: pem, APNS_KEY_ID: "KEY1234567", APNS_TEAM_ID: "TEAM123456", APNS_TOPIC: "app.agentpulse.ios" });
    });
    afterAll(() => {
      Object.assign(env, { APNS_KEY_P8: undefined, APNS_KEY_ID: undefined, APNS_TEAM_ID: undefined, APNS_TOPIC: undefined });
    });

    it("validates push tokens", async () => {
      const p = await pair();
      const path = `/v1/pairings/${p.id}/push-tokens`;
      for (const body of [{ activity }, { activity, environment: "staging" }, { activity: "zz", environment: "production" }, { start: 5, environment: "production" }, { activity: "a".repeat(63), environment: "production" }, { start: "a".repeat(201), environment: "production" }]) {
        expect((await req("PUT", path, { token: p.phone, body })).status, JSON.stringify(body)).toBe(400);
      }
      expect((await req("PUT", path, { token: p.mac, body: { activity, environment: "production" } })).status).toBe(401);
      expect((await req("PUT", path, { token: p.phone, body: { activity: "a".repeat(64), start: "F".repeat(200), environment: "production" } })).status).toBe(204);
    });

    it("lets a Mac write down and push in the same second", async () => {
      mockApns();
      const p = await withTokens("production");
      expect((await req("PUT", `/v1/pairings/${p.id}/down`, { token: p.mac, body: { seq: 1, message: "a" } })).status).toBe(204);
      expect((await req("POST", `/v1/pairings/${p.id}/push`, { token: p.mac, body: { event: "update", message: "m", urgent: false }, tick: false })).status).toBe(200);
    });

    it("allows 60 regular and 12 urgent pushes an hour, counted apart", async () => {
      const apns = mockApns();
      const p = await withTokens("production");
      for (let i = 0; i < 12; i++) expect((await push(p, { event: "update", message: "m", urgent: true })).status).toBe(200);
      expect((await push(p, { event: "update", message: "m", urgent: true })).status).toBe(429);
      for (let i = 0; i < 60; i++) expect((await push(p, { event: "update", message: "m", urgent: false })).status).toBe(200);
      expect((await push(p, { event: "update", message: "m", urgent: false })).status).toBe(429);
      expect(apns).toHaveBeenCalledTimes(72);
      clock += 60 * MIN;
      expect((await push(p, { event: "update", message: "m", urgent: true })).status).toBe(200);
    });

    it("doesn't bring a pairing deleted during a push back, nor drop a token replaced meanwhile", async () => {
      // APNs answers from inside the Durable Object: promises and I/O objects can't cross
      // between it and the test, so the test releases the answer with runInDurableObject.
      let gone!: () => void;
      const apns = vi.spyOn(globalThis, "fetch").mockImplementation(async () => {
        await new Promise<void>((r) => (gone = r));
        return Response.json({ reason: "Unregistered" }, { status: 410 });
      });

      const deleted = await withTokens("production");
      let pending = push(deleted, { event: "update", message: "m", urgent: false });
      await vi.waitFor(() => expect(apns).toHaveBeenCalledTimes(1));
      expect((await req("DELETE", `/v1/pairings/${deleted.id}`, { token: deleted.phone })).status).toBe(204);
      await runInDurableObject(stub(deleted.id), () => gone());
      expect((await pending).status).toBe(410);
      expect((await stored(deleted.id)).size).toBe(0);

      const replaced = await withTokens("production");
      pending = push(replaced, { event: "update", message: "m", urgent: false });
      await vi.waitFor(() => expect(apns).toHaveBeenCalledTimes(2));
      const fresh = "ef".repeat(40);
      expect((await req("PUT", `/v1/pairings/${replaced.id}/push-tokens`, { token: replaced.phone, body: { activity: fresh, environment: "production" } })).status).toBe(204);
      await runInDurableObject(stub(replaced.id), () => gone());
      expect((await pending).status).toBe(410);
      expect(((await stored(replaced.id)).get("meta") as { push: { activity: string } }).push.activity).toBe(fresh);
    });

    it("signs a new JWT after APNs calls the cached one expired or invalid", async () => {
      for (const reason of ["ExpiredProviderToken", "InvalidProviderToken"]) {
        const apns = mockApns();
        const p = await withTokens("production");
        apns.mockImplementationOnce(async () => Response.json({ reason }, { status: 403 }));
        const r = await push(p, { event: "update", message: "m", urgent: false });
        expect(r.status).toBe(403);
        expect(await r.json()).toEqual({ reason });
        await push(p, { event: "update", message: "m", urgent: false });
        await push(p, { event: "update", message: "m", urgent: false });
        const [a, b, c] = [0, 1, 2].map((i) => sent(apns, i).headers.authorization);
        expect(b).not.toBe(a);
        expect(c).toBe(b);
        apns.mockRestore();
      }
    });

    it("answers 500 when the P8 key won't import", async () => {
      const good = env.APNS_KEY_P8;
      const p = await withTokens("production");
      clock += 51 * MIN; // past the cached JWT
      Object.assign(env, { APNS_KEY_P8: "-----BEGIN PRIVATE KEY-----\nbm90IGEga2V5\n-----END PRIVATE KEY-----" });
      try {
        const r = await push(p, { event: "update", message: "m", urgent: false });
        expect(r.status).toBe(500);
        expect(await r.json()).toEqual({ error: "push not configured" });
      } finally {
        Object.assign(env, { APNS_KEY_P8: good });
      }
    });

    it("validates the push and needs the matching token", async () => {
      const apns = mockApns();
      const p = await withTokens("development", { activity });
      for (const body of [{ event: "pause", message: "m", urgent: false }, { event: "update", message: "m" }, { event: "update", message: "m", urgent: "yes" }, { event: "update", message: "a/b", urgent: false }]) {
        expect((await push(p, body)).status, JSON.stringify(body)).toBe(400);
      }
      expect((await push(p, { event: "update", message: "x".repeat(4097), urgent: false })).status).toBe(413);
      expect((await push(p, { event: "start", message: "m", urgent: false })).status).toBe(409);
      expect((await req("POST", `/v1/pairings/${p.id}/push`, { token: p.phone, body: { event: "update", message: "m", urgent: false } })).status).toBe(401);
      const noTokens = await pair();
      expect((await push(noTokens, { event: "update", message: "m", urgent: false })).status).toBe(409);
      expect(apns).not.toHaveBeenCalled();
    });

    it("sends an update to the sandbox with a signed JWT", async () => {
      const apns = mockApns();
      const p = await withTokens("development");
      expect((await push(p, { event: "update", message: "c2VhbGVk", urgent: false })).status).toBe(200);
      const { url, headers, body } = sent(apns);
      expect(url).toBe(`https://api.sandbox.push.apple.com/3/device/${activity}`);
      expect(headers).toMatchObject({
        "apns-push-type": "liveactivity",
        "apns-topic": "app.agentpulse.ios.push-type.liveactivity",
        "apns-priority": "5",
      });
      expect(body).toEqual({ aps: { timestamp: Math.floor(clock / 1000), event: "update", "content-state": { m: "c2VhbGVk" } } });
      const jwt = await readJwt(headers.authorization);
      expect(jwt.valid).toBe(true);
      expect(jwt.header).toEqual({ alg: "ES256", kid: "KEY1234567" });
      expect(jwt.claims).toEqual({ iss: "TEAM123456", iat: expect.any(Number) });
    });

    it("sends start with the start token to production, urgent at priority 10", async () => {
      const apns = mockApns();
      const p = await withTokens("production");
      expect((await push(p, { event: "start", message: "bQ", urgent: true })).status).toBe(200);
      const { url, headers, body } = sent(apns);
      expect(url).toBe(`https://api.push.apple.com/3/device/${start}`);
      expect(headers["apns-priority"]).toBe("10");
      expect(body.aps).toMatchObject({
        event: "start",
        "content-state": { m: "bQ" },
        "attributes-type": "AgentPulseActivity",
        attributes: { pairing: p.id },
        alert: { title: expect.any(String), body: expect.any(String) },
      });
    });

    it("sends end with a dismissal date", async () => {
      const apns = mockApns();
      const p = await withTokens("production");
      expect((await push(p, { event: "end", message: "bQ", urgent: false })).status).toBe(200);
      const { url, body } = sent(apns);
      expect(url).toBe(`https://api.push.apple.com/3/device/${activity}`);
      expect(body.aps).toEqual({ timestamp: Math.floor(clock / 1000), event: "end", "content-state": { m: "bQ" }, "dismissal-date": Math.floor(clock / 1000) });
    });

    it("passes APNs errors back and drops a token APNs says is gone", async () => {
      mockApns(() => Response.json({ reason: "Unregistered" }, { status: 410 }));
      const p = await withTokens("production");
      const r = await push(p, { event: "update", message: "m", urgent: false });
      expect(r.status).toBe(410);
      expect(await r.json()).toEqual({ reason: "Unregistered" });
      expect((await push(p, { event: "update", message: "m", urgent: false })).status).toBe(409);
    });

    it("answers 502 when APNs can't be reached", async () => {
      vi.spyOn(globalThis, "fetch").mockRejectedValue(new TypeError("network"));
      const p = await withTokens("production");
      expect((await push(p, { event: "update", message: "m", urgent: false })).status).toBe(502);
    });

    it("reuses the JWT for 50 minutes", async () => {
      const apns = mockApns();
      const p = await withTokens("production");
      await push(p, { event: "update", message: "m", urgent: false });
      await push(p, { event: "update", message: "m", urgent: false });
      clock += 51 * MIN;
      await push(p, { event: "update", message: "m", urgent: false });
      const [a, b, c] = await Promise.all([0, 1, 2].map((i) => readJwt(sent(apns, i).headers.authorization)));
      expect(b.jwt).toBe(a.jwt);
      expect(c.jwt).not.toBe(a.jwt);
      expect(c.valid).toBe(true);
      expect(c.claims.iat).toBe(Math.floor(clock / 1000));
    });
  });
});

describe("clientKey", () => {
  it("keeps IPv4 and folds IPv6 to its /64", async () => {
    const { clientKey } = await import("../src/client-key");
    expect(clientKey(null)).toBe("unknown");
    expect(clientKey("203.0.113.7")).toBe("203.0.113.7");
    expect(clientKey("2001:db8:85a3:1:8a2e:370:7334:1")).toBe("2001:db8:85a3:1::/64");
    expect(clientKey("2001:db8:85a3:1::42")).toBe("2001:db8:85a3:1::/64");
    expect(clientKey("2001:DB8::1")).toBe("2001:db8:0:0::/64");
    expect(clientKey("::1")).toBe("0:0:0:0::/64");
  });
});
