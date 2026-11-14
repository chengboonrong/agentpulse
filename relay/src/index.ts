// AgentPulse relay and push gateway. Spec: docs/mobile.md ("Relay API", "Push gateway").
// It stores ciphertext, SHA-256 hashes of tokens and Apple push tokens, nothing else.
import { DurableObject } from "cloudflare:workers";
import { clientKey } from "./client-key";

declare global {
  namespace Cloudflare {
    interface Env {
      PAIRINGS: DurableObjectNamespace<Pairing>;
      APNS_KEY_P8?: string;
      APNS_KEY_ID?: string;
      APNS_TEAM_ID?: string;
      APNS_TOPIC?: string;
      RELAY_KEY?: string;
      CREATE_LIMITER?: RateLimit;
    }
    interface GlobalProps {
      mainModule: typeof import("./index");
    }
  }
}

const VERSION = "0.1.0";
const HELLO_TTL_MS = 10 * 60_000;
const IDLE_MS = 30 * 86_400_000;
const MAX_BODY = 20_000;
const MAX_DOWN = 16_384;
const MAX_SMALL = 4_096;
const MAX_QUEUE = 32;
const JWT_TTL_S = 50 * 60;
const HOUR_MS = 3_600_000;
const MAX_PUSHES = 60; // regular, an hour: one update a minute
const MAX_URGENT = 12; // urgent, an hour, counted apart

type Side = "mac" | "phone";
type Json = Record<string, unknown>;
interface Call {
  id: string;
  action: string;
  method: string;
  token: string | null;
  etag: string | null;
  body: Json;
}
interface Reply {
  status: number;
  body?: object;
  headers?: Record<string, string>;
}
interface PushTokens {
  activity?: string;
  start?: string;
  environment: "production" | "development";
}
interface Meta {
  id: string;
  macTokenHash: string;
  phoneTokenHash?: string;
  secretHash?: string;
  createdAt: number;
  lastActive: number;
  push?: PushTokens;
}
interface Push {
  at: number;
  urgent: boolean;
}
interface Down {
  seq: number;
  message: string;
}

// Methods allowed on /v1/pairings/{id}[/{action}].
const ROUTES = new Map([
  ["", ["DELETE"]],
  ["hello", ["POST"]],
  ["down", ["PUT", "GET"]],
  ["up", ["POST", "GET"]],
  ["push-tokens", ["PUT"]],
  ["push", ["POST"]],
]);
// The token each token-authenticated route takes (DELETE takes either).
const SIDE = new Map<string, Side>([
  ["PUT down", "mac"],
  ["GET down", "phone"],
  ["POST up", "phone"],
  ["GET up", "mac"],
  ["PUT push-tokens", "phone"],
  ["POST push", "mac"],
]);

const isB64 = (v: unknown, len?: number): v is string =>
  typeof v === "string" && /^[A-Za-z0-9_-]+$/.test(v) && (len === undefined || v.length === len);
const isKey = (v: unknown): v is string => isB64(v, 43); // 32 bytes
const isHex = (v: unknown): v is string => typeof v === "string" && /^[0-9a-fA-F]{64,200}$/.test(v);
const isSeq = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const err = (status: number, error: string): Reply => ({ status, body: { error } });
const NO_CONTENT: Reply = { status: 204 };

// The message itself, or the error to answer with.
function message(v: unknown, max: number): string | Reply {
  if (!isB64(v)) return err(400, "message must be base64url");
  return v.length > max ? err(413, "message too large") : v;
}

const fromB64 = (s: string, alphabet: "base64" | "base64url" = "base64url") => Uint8Array.fromBase64(s, { alphabet });
const toB64 = (b: Uint8Array) => b.toBase64({ alphabet: "base64url", omitPadding: true });
const sha256 = async (b: Uint8Array) => new Uint8Array(await crypto.subtle.digest("SHA-256", b));
const same = (a: Uint8Array, hash: string) => crypto.subtle.timingSafeEqual(a, fromB64(hash));

// The body as text, or null once it passes MAX_BODY bytes (Content-Length or not).
async function readBody(req: Request): Promise<string | null> {
  if (!req.body) return "";
  const reader = req.body.getReader();
  const decoder = new TextDecoder();
  let text = "";
  let size = 0;
  for (let r = await reader.read(); !r.done; r = await reader.read()) {
    size += r.value.byteLength;
    if (size > MAX_BODY) {
      await reader.cancel();
      return null;
    }
    text += decoder.decode(r.value, { stream: true });
  }
  return text + decoder.decode();
}

function respond(r: Reply): Response {
  const init = { status: r.status, headers: r.headers };
  return r.body === undefined ? new Response(null, init) : Response.json(r.body, init);
}

export default {
  async fetch(req, env): Promise<Response> {
    const { pathname } = new URL(req.url);
    if (pathname === "/v1/health") {
      return respond(req.method === "GET" ? { status: 200, body: { version: VERSION } } : notAllowed(["GET"]));
    }
    const m = /^\/v1\/pairings(?:\/([^/]+)(?:\/([^/]+))?)?$/.exec(pathname);
    const pathId = m?.[1];
    const action = m?.[2] ?? "";
    const methods = !m ? undefined : pathId === undefined ? ["POST"] : ROUTES.get(action);
    if (!methods) return respond(err(404, "not found"));
    if (!methods.includes(req.method)) return respond(notAllowed(methods));

    const create = pathId === undefined;
    if ((create || action === "hello") && env.CREATE_LIMITER) {
      const { success } = await env.CREATE_LIMITER.limit({ key: clientKey(req.headers.get("cf-connecting-ip")) });
      if (!success) return respond(err(429, "too many pairings from this address"));
    }
    if (create && env.RELAY_KEY) {
      const given = /^Bearer (.+)$/.exec(req.headers.get("authorization") ?? "")?.[1] ?? "";
      const utf8 = (v: string) => sha256(new TextEncoder().encode(v));
      if (!crypto.subtle.timingSafeEqual(await utf8(given), await utf8(env.RELAY_KEY))) return respond(err(401, "unauthorized"));
    }

    let body: Json = {};
    if (req.method === "POST" || req.method === "PUT") {
      const text = await readBody(req);
      if (text === null) return respond(err(413, "body too large"));
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        return respond(err(400, "body must be JSON"));
      }
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        return respond(err(400, "body must be a JSON object"));
      }
      body = parsed as Json;
    }

    const id = pathId ?? body.id;
    if (!isB64(id, 22)) return respond(err(400, "id must be 16 bytes of base64url"));
    const auth = /^Bearer ([A-Za-z0-9_-]{43})$/.exec(req.headers.get("authorization") ?? "");
    const stub = env.PAIRINGS.get(env.PAIRINGS.idFromName(id));
    return respond(
      await stub.handle({
        id,
        action: create ? "create" : action,
        method: req.method,
        token: auth?.[1] ?? null,
        etag: req.headers.get("if-none-match"),
        body,
      }),
    );
  },
} satisfies ExportedHandler<Cloudflare.Env>;

function notAllowed(methods: string[]): Reply {
  return { ...err(405, "method not allowed"), headers: { allow: methods.join(", ") } };
}

export class Pairing extends DurableObject {
  // Last request per side and route. In memory: an evicted object has been idle far longer
  // than the 1 s window.
  private lastRequest = new Map<string, number>();

  // Between reading meta and writing it back there are only storage awaits, which the input
  // gate keeps atomic; anything else that awaits (hashing, signing, APNs) runs before the read
  // or re-reads afterwards, so a DELETE or a second hello can't interleave and be undone.
  async handle(c: Call): Promise<Reply> {
    const s = this.ctx.storage;
    const b = c.body;
    const tokenHash = c.token ? await sha256(fromB64(c.token)) : null;
    const secretHash = c.action === "hello" && isKey(b.secret) ? await sha256(fromB64(b.secret)) : null;
    const meta = await s.get<Meta>("meta");
    const now = Date.now();

    if (c.action === "create") {
      if (!isKey(b.macTokenHash) || !isKey(b.secretHash)) return err(400, "macTokenHash and secretHash must be 32 bytes of base64url");
      if (meta) return err(409, "pairing exists");
      await s.put<Meta>("meta", { id: c.id, macTokenHash: b.macTokenHash, secretHash: b.secretHash, createdAt: now, lastActive: now });
      await s.setAlarm(now + HELLO_TTL_MS);
      return { status: 201 };
    }

    if (c.action === "hello") {
      if (!secretHash || !isKey(b.phoneTokenHash)) return err(400, "secret and phoneTokenHash must be 32 bytes of base64url");
      const msg = message(b.message, MAX_SMALL);
      if (typeof msg !== "string") return msg;
      if (meta?.phoneTokenHash) return err(409, "pairing already has its hello");
      if (!meta?.secretHash || now >= meta.createdAt + HELLO_TTL_MS || !same(secretHash, meta.secretHash)) {
        return err(401, "unauthorized");
      }
      delete meta.secretHash;
      await s.put({ meta: { ...meta, phoneTokenHash: b.phoneTokenHash, lastActive: now }, up: [msg] });
      await s.setAlarm(now + IDLE_MS);
      return NO_CONTENT;
    }

    const route = `${c.method} ${c.action}`.trimEnd();
    const side = meta && tokenHash ? whoIs(meta, tokenHash) : null;
    if (!meta || !side || (route !== "DELETE" && SIDE.get(route) !== side)) return err(401, "unauthorized");

    if (route === "DELETE") {
      await s.deleteAlarm();
      await s.deleteAll();
      return NO_CONTENT;
    }
    const key = `${side} ${route}`;
    // Reads get 4 a second: the app and its widgets share the phone's token.
    const gap = c.method === "GET" ? 250 : 1000;
    const wait = (this.lastRequest.get(key) ?? 0) + gap - now;
    if (wait > 0) return { ...err(429, "too many requests"), headers: { "retry-after": String(Math.ceil(wait / 1000)) } };
    this.lastRequest.set(key, now);
    // Hourly is precise enough for a 30-day expiry, and keeps reads from writing.
    if (now - meta.lastActive > HOUR_MS) {
      meta.lastActive = now;
      await s.put("meta", meta);
    }

    switch (route) {
      case "PUT down": {
        if (!isSeq(b.seq)) return err(400, "seq must be a non-negative integer");
        const msg = message(b.message, MAX_DOWN);
        if (typeof msg !== "string") return msg;
        const down = await s.get<Down>("down");
        if (down && b.seq <= down.seq) return err(409, "seq not newer than the stored one");
        await s.put<Down>("down", { seq: b.seq, message: msg });
        return NO_CONTENT;
      }
      case "GET down": {
        const down = await s.get<Down>("down");
        if (!down) return NO_CONTENT;
        const headers = { etag: `"${down.seq}"` };
        if (c.etag?.replace(/^W\/|"/g, "") === String(down.seq)) return { status: 304, headers };
        return { status: 200, body: down, headers };
      }
      case "POST up": {
        const msg = message(b.message, MAX_SMALL);
        if (typeof msg !== "string") return msg;
        const up = (await s.get<string[]>("up")) ?? [];
        if (up.length >= MAX_QUEUE) return err(429, "queue full");
        up.push(msg);
        await s.put("up", up);
        return NO_CONTENT;
      }
      case "GET up": {
        const messages = (await s.get<string[]>("up")) ?? [];
        if (messages.length) await s.delete("up");
        return { status: 200, body: { messages } };
      }
      case "PUT push-tokens": {
        const { activity, start, environment } = b;
        if (environment !== "production" && environment !== "development") return err(400, "environment must be production or development");
        if ((activity !== undefined && !isHex(activity)) || (start !== undefined && !isHex(start))) return err(400, "push tokens must be 64 to 200 hex characters");
        meta.push = { activity, start, environment };
        await s.put("meta", meta);
        return NO_CONTENT;
      }
      default:
        return this.push(meta, b);
    }
  }

  private async push(meta: Meta, b: Json): Promise<Reply> {
    const { APNS_KEY_P8, APNS_KEY_ID, APNS_TEAM_ID, APNS_TOPIC } = this.env;
    if (!APNS_KEY_P8 || !APNS_KEY_ID || !APNS_TEAM_ID || !APNS_TOPIC) return err(501, "push is not configured on this relay");
    const { event, urgent } = b;
    if (event !== "update" && event !== "start" && event !== "end") return err(400, "event must be update, start or end");
    if (typeof urgent !== "boolean") return err(400, "urgent must be a boolean");
    const msg = message(b.message, MAX_SMALL);
    if (typeof msg !== "string") return msg;
    const kind = event === "start" ? "start" : "activity";
    const token = meta.push?.[kind];
    if (!meta.push || !token) return err(409, `no ${kind} push token`);

    const s = this.ctx.storage;
    const pushes = ((await s.get<Push[]>("pushes")) ?? []).filter((p) => p.at > Date.now() - HOUR_MS);
    if (!urgent && pushes.filter((p) => !p.urgent).length >= MAX_PUSHES) return err(429, `at most ${MAX_PUSHES} pushes an hour`);
    if (urgent && pushes.filter((p) => p.urgent).length >= MAX_URGENT) return err(429, `at most ${MAX_URGENT} urgent pushes an hour`);
    await s.put("pushes", [...pushes, { at: Date.now(), urgent }]);

    const now = Math.floor(Date.now() / 1000);
    const aps: Json = { timestamp: now, event, "content-state": { m: msg } };
    if (event === "start") {
      aps["attributes-type"] = "AgentPulseActivity";
      aps.attributes = { pairing: meta.id };
      // Push-to-start payloads carry an alert in Apple's documented format:
      // https://developer.apple.com/documentation/activitykit/starting-and-updating-live-activities-with-activitykit-push-notifications
      // Fixed text only; the activity's own content stays encrypted in `m`.
      aps.alert = { title: "AgentPulse", body: "An agent is working" };
    }
    if (event === "end") aps["dismissal-date"] = now;

    const host = meta.push.environment === "production" ? "api.push.apple.com" : "api.sandbox.push.apple.com";
    let jwt: string;
    try {
      jwt = await apnsJwt(APNS_KEY_P8, APNS_KEY_ID, APNS_TEAM_ID);
    } catch {
      return err(500, "push not configured");
    }
    let res: Response;
    try {
      res = await fetch(`https://${host}/3/device/${token}`, {
        method: "POST",
        headers: {
          authorization: `bearer ${jwt}`,
          "apns-push-type": "liveactivity",
          "apns-topic": `${APNS_TOPIC}.push-type.liveactivity`,
          "apns-priority": urgent ? "10" : "5",
          "content-type": "application/json",
        },
        body: JSON.stringify({ aps }),
      });
    } catch {
      return err(502, "APNs unreachable");
    }
    const body = await res.json<{ reason?: string }>().catch(() => undefined);
    if (res.status === 403 && (body?.reason === "ExpiredProviderToken" || body?.reason === "InvalidProviderToken")) jwtCache = undefined;
    if (res.status === 410) {
      // Re-read: the pairing may have been deleted, or the token replaced, while APNs answered.
      const current = await s.get<Meta>("meta");
      if (current?.push?.[kind] === token) {
        delete current.push[kind];
        await s.put("meta", current);
      }
    }
    // APNs' own status, with its {reason} body when it sends one.
    return { status: res.status, body };
  }

  async alarm() {
    const meta = await this.ctx.storage.get<Meta>("meta");
    if (!meta) return;
    const due = meta.phoneTokenHash ? meta.lastActive + IDLE_MS : meta.createdAt + HELLO_TTL_MS;
    if (Date.now() < due) return this.ctx.storage.setAlarm(due);
    await this.ctx.storage.deleteAll();
  }
}

function whoIs(meta: Meta, tokenHash: Uint8Array): Side | null {
  if (same(tokenHash, meta.macTokenHash)) return "mac";
  if (meta.phoneTokenHash && same(tokenHash, meta.phoneTokenHash)) return "phone";
  return null;
}

// APNs wants a token no older than an hour and refreshed at most every 20 minutes.
let jwtCache: { token: string; iat: number } | undefined;

async function apnsJwt(p8: string, keyId: string, teamId: string): Promise<string> {
  const iat = Math.floor(Date.now() / 1000);
  if (jwtCache && iat - jwtCache.iat < JWT_TTL_S) return jwtCache.token;
  const der = fromB64(p8.replace(/-----[^-]+-----|\s/g, ""), "base64");
  const key = await crypto.subtle.importKey("pkcs8", der, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);
  const part = (o: object) => toB64(new TextEncoder().encode(JSON.stringify(o)));
  const input = `${part({ alg: "ES256", kid: keyId })}.${part({ iss: teamId, iat })}`;
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(input));
  jwtCache = { token: `${input}.${toB64(new Uint8Array(sig))}`, iat };
  return jwtCache.token;
}

