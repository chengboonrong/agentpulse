//! The worker against a fake bridge and a fake relay; no network, no Keychain.

use std::collections::{HashMap, HashSet};
use std::fs;

use super::*;
use crate::settings::Phones;
use crate::widget_export::placeholder;

const RELAY: &str = "https://relay.test";
const T0: i64 = 1_800_000_000_000;

/// Stands in for the Swift bridge. "Sealing" is `sealed|<direction>|<plaintext>`, a
/// hello is `hello|<secret>|<phone key>|<phone name>`, the code is always 123456.
#[derive(Clone, Default)]
struct FakeBridge(Arc<Mutex<BridgeState>>);

#[derive(Default)]
struct BridgeState {
    calls: Vec<Value>,
    /// Pairing id to its secret, while its Keychain items exist.
    keys: HashMap<String, String>,
    made: u32,
    relay_key: Option<String>,
    /// Ops that fail, with their error.
    fail: HashMap<&'static str, &'static str>,
}

impl FakeBridge {
    fn ops(&self) -> Vec<String> {
        let s = self.0.lock().unwrap();
        s.calls
            .iter()
            .map(|c| c["op"].as_str().unwrap().to_owned())
            .collect()
    }

    fn calls(&self, op: &str) -> Vec<Value> {
        let s = self.0.lock().unwrap();
        s.calls.iter().filter(|c| c["op"] == op).cloned().collect()
    }

    fn has_keys(&self, id: &str) -> bool {
        self.0.lock().unwrap().keys.contains_key(id)
    }

    fn fail(&self, op: &'static str, error: Option<&'static str>) {
        let mut s = self.0.lock().unwrap();
        match error {
            Some(e) => s.fail.insert(op, e),
            None => s.fail.remove(op),
        };
    }
}

impl Bridge for FakeBridge {
    fn call(&self, r: &Value) -> Result<Value, String> {
        let mut s = self.0.lock().unwrap();
        s.calls.push(r.clone());
        let id = r["id"].as_str().unwrap_or_default().to_owned();
        let field = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
        if let Some(e) = s.fail.get(r["op"].as_str().unwrap()) {
            return Err((*e).into());
        }
        match r["op"].as_str().unwrap() {
            "pair.new" => {
                s.made += 1;
                let (id, secret) = (format!("pairing{}", s.made), format!("secret{}", s.made));
                s.keys.insert(id.clone(), secret.clone());
                Ok(json!({
                    "id": id, "macKey": "mackey",
                    "link": format!("agentpulse://pair?v=1&r={}&id={id}&s={secret}", field("relay")),
                    "secretHash": format!("hash:{secret}"),
                    "macTokenHash": format!("hash:token-{id}"),
                }))
            }
            "pair.token" if s.keys.contains_key(&id) => Ok(json!({"token": format!("token-{id}")})),
            "hello.open" => {
                let m = field("message");
                let parts: Vec<&str> = m.split('|').collect();
                match (parts.as_slice(), s.keys.get(&id)) {
                    (["hello", secret, key, name], Some(own)) if secret == own => {
                        Ok(json!({"phoneKey": key, "phoneName": name, "code": "123456"}))
                    }
                    _ => Err("wrongSecret".into()),
                }
            }
            "pair.code" => Ok(json!({"code": "123456"})),
            "seal" => Ok(json!({
                "message": format!("sealed|{}|{}", field("direction"), field("plaintext")),
            })),
            "open" => {
                match field("message").strip_prefix(&format!("sealed|{}|", field("direction"))) {
                    Some(p) => Ok(json!({"plaintext": p})),
                    None => Err("invalidMessage".into()),
                }
            }
            "pair.delete" => {
                s.keys.remove(&id);
                Ok(json!({}))
            }
            "listener.set" | "listener.send" => Ok(json!({})),
            "relaykey.set" => {
                s.relay_key = r["key"].as_str().map(str::to_owned);
                Ok(json!({}))
            }
            "relaykey.get" => Ok(json!({"key": s.relay_key})),
            // A 2×2 code, dark only at the top left.
            "qr" => {
                let mut rgba = vec![255u8; 2 * 2 * 4];
                rgba[..3].fill(0);
                Ok(json!({"width": 2, "height": 2, "rgba": b64url(&rgba)}))
            }
            op => Err(format!("unexpected op {op}")),
        }
    }
}

/// An in-memory relay with relay/src/index.ts's status codes for the routes the Mac uses.
#[derive(Clone, Default)]
struct FakeRelay(Arc<Mutex<RelayState>>);

#[derive(Default)]
struct RelayState {
    requests: Vec<(String, String)>,
    /// Each request's bearer token, beside `requests`.
    tokens: Vec<Option<String>>,
    /// The deployer's `RELAY_KEY`, required on create when set.
    relay_key: Option<String>,
    pairings: HashMap<String, Mailbox>,
    /// The next request's status and Retry-After.
    forced: Option<(u16, Option<i64>)>,
    online: bool,
}

#[derive(Default)]
struct Mailbox {
    mac_token_hash: String,
    secret_hash: Option<String>,
    down: Option<(u64, String)>,
    up: Vec<String>,
}

impl FakeRelay {
    fn requests(&self) -> Vec<(String, String)> {
        self.0.lock().unwrap().requests.clone()
    }

    fn count(&self, method: &str, action: &str) -> usize {
        self.requests()
            .iter()
            .filter(|(m, p)| m == method && p.ends_with(action))
            .count()
    }

    /// The phone's hello, as the relay checks it.
    fn hello(&self, id: &str, secret: &str, message: &str) -> u16 {
        let mut s = self.0.lock().unwrap();
        let Some(b) = s.pairings.get_mut(id) else {
            return 401;
        };
        if b.secret_hash.as_deref() != Some(&format!("hash:{secret}")) {
            return 401;
        }
        b.secret_hash = None;
        b.up.insert(0, message.into());
        204
    }

    fn post_up(&self, id: &str, msg: Value) {
        let mut s = self.0.lock().unwrap();
        let sealed = format!("sealed|up|{}", b64url(msg.to_string().as_bytes()));
        s.pairings.get_mut(id).unwrap().up.push(sealed);
    }

    /// The latest `down` message, opened.
    fn down(&self, id: &str) -> Option<(u64, Value)> {
        let s = self.0.lock().unwrap();
        let (seq, m) = s.pairings.get(id)?.down.clone()?;
        let plaintext = from_b64url(m.strip_prefix("sealed|down|")?)?;
        Some((seq, serde_json::from_slice(&plaintext).ok()?))
    }

    fn exists(&self, id: &str) -> bool {
        self.0.lock().unwrap().pairings.contains_key(id)
    }

    fn drop_pairing(&self, id: &str) {
        self.0.lock().unwrap().pairings.remove(id);
    }

    fn force(&self, status: u16, retry_after: Option<i64>) {
        self.0.lock().unwrap().forced = Some((status, retry_after));
    }
}

impl Relay for FakeRelay {
    fn call(
        &self,
        method: &str,
        url: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Reply, String> {
        let mut s = self.0.lock().unwrap();
        let path = url
            .strip_prefix(RELAY)
            .expect("the configured relay")
            .to_owned();
        s.requests.push((method.into(), path.clone()));
        s.tokens.push(token.map(str::to_owned));
        if !s.online {
            return Err("offline".into());
        }
        let reply = |status, body| {
            Ok(Reply {
                status,
                body,
                retry_after: None,
            })
        };
        if let Some((status, retry_after)) = s.forced.take() {
            return Ok(Reply {
                status,
                body: Value::Null,
                retry_after,
            });
        }
        let body = body.cloned().unwrap_or_default();
        let parts: Vec<&str> = path.trim_start_matches("/v1/pairings").split('/').collect();
        if parts == [""] && method == "POST" {
            if s.relay_key.is_some() && token.map(str::to_owned) != s.relay_key {
                return reply(401, Value::Null);
            }
            let (Some(id), Some(mac), Some(secret)) = (
                body["id"].as_str(),
                body["macTokenHash"].as_str(),
                body["secretHash"].as_str(),
            ) else {
                return reply(400, Value::Null);
            };
            if s.pairings.contains_key(id) {
                return reply(409, Value::Null);
            }
            let b = Mailbox {
                mac_token_hash: mac.into(),
                secret_hash: Some(secret.into()),
                ..Mailbox::default()
            };
            s.pairings.insert(id.into(), b);
            return reply(201, Value::Null);
        }
        let (id, action) = match parts.as_slice() {
            ["", id] => (*id, ""),
            ["", id, action] => (*id, *action),
            _ => return reply(404, Value::Null),
        };
        let Some(b) = s.pairings.get_mut(id) else {
            return reply(401, Value::Null);
        };
        if token.map(|t| format!("hash:{t}")) != Some(b.mac_token_hash.clone()) {
            return reply(401, Value::Null);
        }
        match (method, action) {
            ("DELETE", "") => {
                s.pairings.remove(id);
                reply(204, Value::Null)
            }
            ("GET", "up") => reply(200, json!({"messages": std::mem::take(&mut b.up)})),
            ("PUT", "down") => {
                let seq = body["seq"].as_u64().unwrap();
                if b.down.as_ref().is_some_and(|(last, _)| seq <= *last) {
                    return reply(409, Value::Null);
                }
                b.down = Some((seq, body["message"].as_str().unwrap().into()));
                reply(204, Value::Null)
            }
            _ => reply(405, Value::Null),
        }
    }
}

struct Rig {
    core: Core,
    bridge: FakeBridge,
    relay: FakeRelay,
    settings: Settings,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).ok();
    }
}

fn rig(name: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("agentpulse-phones-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let settings = Settings::load(&dir);
    settings.set_phones(Phones {
        relay: Some(format!("{RELAY}/")),
        direct: true,
    });
    let (bridge, relay) = (FakeBridge::default(), FakeRelay::default());
    relay.0.lock().unwrap().online = true;
    let (changed, _) = async_channel::bounded(1);
    let shared = Shared {
        view: Arc::default(),
        changed,
        publishing: Arc::default(),
    };
    let core = Core::new(
        Some(Box::new(bridge.clone())),
        Box::new(relay.clone()),
        settings.clone(),
        dir.clone(),
        shared,
    );
    Rig {
        core,
        bridge,
        relay,
        settings,
        dir,
    }
}

fn payload(generated_at_ms: i64, active: usize) -> Payload {
    let mut p = placeholder();
    p.generated_at_ms = generated_at_ms;
    p.agents.active = active;
    p
}

fn view(r: &Rig) -> View {
    r.core.shared.view.lock().unwrap().clone()
}

impl Rig {
    /// Pairs a phone at T0, confirmed and paired by T0 + 2 s.
    fn pair(&mut self) -> String {
        let started = self.core.start_pairing(T0).expect("pairing starts");
        let id = started.id;
        let n = id.trim_start_matches("pairing");
        let hello = format!("hello|secret{n}|phonekey{n}|Phone {n}");
        assert_eq!(self.relay.hello(&id, &format!("secret{n}"), &hello), 204);
        self.core.tick(T0 + POLL_WAITING_MS);
        self.core.handle(
            Msg::Command(Command::Confirm(id.clone())),
            T0 + POLL_WAITING_MS,
        );
        id
    }

    fn publish(&mut self, p: Payload, now: i64) {
        self.core.handle(Msg::Publish(Box::new(p)), now);
        self.core.tick(now);
    }
}

#[test]
fn nothing_happens_without_pairings() {
    let mut r = rig("idle");
    r.settings.set_phones(Phones::default());
    r.core.handle(Msg::Publish(Box::new(payload(1, 1))), T0);
    r.core.handle(Msg::Wake, T0);
    r.core.tick(T0);
    r.core.tick(T0 + 3_600_000);
    assert_eq!(r.core.next_due(T0), None);
    assert!(r.bridge.ops().is_empty());
    assert!(r.relay.requests().is_empty());
    assert!(!r.dir.join(store::FILE_NAME).exists());
    assert!(!r.core.shared.publishing.load(Ordering::Relaxed));
    assert_eq!(view(&r).phones, []);
}

#[test]
fn a_relay_address_alone_only_reads_the_relay_key() {
    let mut r = rig("relay-only");
    for t in [T0, T0 + 1_000, T0 + 3_600_000] {
        r.core.tick(t);
    }
    assert_eq!(r.bridge.ops(), ["relaykey.get"]);
    assert!(r.relay.requests().is_empty());
    assert_eq!(view(&r).relay_key, Some(false));

    // A failed read leaves it unknown, and the next tick tries again.
    let mut r = rig("relay-key-retry");
    r.bridge.fail("relaykey.get", Some("keychain: locked"));
    r.core.tick(T0);
    assert_eq!(view(&r).relay_key, None);
    r.bridge.fail("relaykey.get", None);
    r.core.tick(T0 + 1_000);
    assert_eq!(view(&r).relay_key, Some(false));
}

#[test]
fn pairing_needs_a_relay_and_the_bridge() {
    let mut r = rig("no-relay");
    r.settings.set_phones(Phones::default());
    assert!(r.core.start_pairing(T0).unwrap_err().contains("relay"));
    r.core.bridge = None;
    assert!(r.core.start_pairing(T0).unwrap_err().contains("app bundle"));
    assert!(r.bridge.ops().is_empty() && r.relay.requests().is_empty());
}

#[test]
fn a_full_pairing() {
    let mut r = rig("flow");
    let (reply, result) = async_channel::bounded(1);
    r.core
        .handle(Msg::Command(Command::StartPairing(reply)), T0);
    let started = result.try_recv().unwrap().unwrap();
    let id = started.id.clone();
    assert_eq!(started.expires_at_ms, T0 + EXPIRY_MS);
    let dark = vec![true, false, false, false];
    assert_eq!(started.qr, Qr { size: 2, dark });
    assert!(
        started
            .link
            .starts_with("agentpulse://pair?v=1&r=https://relay.test&id=pairing1")
    );
    assert_eq!(view(&r).pending, Some(started));
    assert!(r.relay.exists(&id));
    assert_eq!(store::load(&r.dir)[0].state, State::Waiting);
    // Not polled until 2 s later, then every 2 s.
    r.core.tick(T0 + 1_000);
    assert_eq!(r.relay.count("GET", "/up"), 0);
    r.core.tick(T0 + 2_000);
    assert_eq!(r.relay.count("GET", "/up"), 1);
    assert_eq!(r.core.next_due(T0 + 2_000), Some(T0 + 4_000));

    let hello = "hello|secret1|phonekey1|Chris's iPhone";
    assert_eq!(r.relay.hello(&id, "secret1", hello), 204);
    r.core.tick(T0 + 4_000);
    let v = view(&r);
    assert_eq!(v.pending, None);
    let phone = &v.phones[0];
    assert_eq!(phone.state, State::Confirming);
    assert_eq!(phone.code.as_deref(), Some("123456"));
    assert_eq!(phone.name.as_deref(), Some("Chris's iPhone"));
    // Nothing is published before the user confirms.
    r.publish(payload(1, 1), T0 + 5_000);
    assert_eq!(r.relay.count("PUT", "/down"), 0);
    assert_eq!(r.core.next_due(T0 + 5_000), None);

    r.core
        .handle(Msg::Command(Command::Confirm(id.clone())), T0 + 6_000);
    assert!(r.core.shared.publishing.load(Ordering::Relaxed));
    let p = payload(2, 1);
    r.publish(p.clone(), T0 + 7_000);
    let (seq, msg) = r.relay.down(&id).unwrap();
    assert_eq!(seq, 0);
    assert_eq!(msg, json!({"seq": 0, "type": "state", "payload": p}));
    // A publish reads `up` too.
    assert_eq!(r.relay.count("GET", "/up"), 3);
    let set = r.bridge.calls("listener.set");
    assert_eq!(
        set,
        [json!({"op": "listener.set", "enabled": true,
            "pairings": [{"id": id, "phoneKey": "phonekey1"}]})]
    );

    let saved = &store::load(&r.dir)[0];
    assert_eq!(saved.state, State::Paired);
    assert_eq!(saved.phone_key.as_deref(), Some("phonekey1"));
    assert_eq!(saved.phone_name.as_deref(), Some("Chris's iPhone"));
    assert_eq!((saved.down_seq, saved.up_seq), (1, 0));
    assert_eq!(saved.relay, RELAY);
    let text = fs::read_to_string(r.dir.join(store::FILE_NAME)).unwrap();
    assert!(
        !text.contains("secret") && !text.contains("token"),
        "{text}"
    );
}

#[test]
fn a_hello_with_the_wrong_secret_ends_the_pairing() {
    let mut r = rig("wrong-secret");
    let id = r.core.start_pairing(T0).unwrap().id;
    // The relay checks the secret it is shown; the sealed one differs.
    assert_eq!(
        r.relay.hello(&id, "secret1", "hello|guess|phonekey|Eve"),
        204
    );
    r.core.tick(T0 + 2_000);
    let v = view(&r);
    assert_eq!(v.phones, []);
    assert_eq!(v.pending, None);
    assert!(v.error.is_some());
    assert!(!r.relay.exists(&id), "deleted at the relay");
    assert!(!r.bridge.has_keys(&id), "keys deleted");
    assert_eq!(store::load(&r.dir), []);
}

#[test]
fn an_unanswered_code_expires() {
    let mut r = rig("expiry");
    let id = r.core.start_pairing(T0).unwrap().id;
    r.core.tick(T0 + EXPIRY_MS - 1);
    assert_eq!(view(&r).phones.len(), 1);
    assert_eq!(
        r.core.next_due(T0 + EXPIRY_MS - 1),
        Some(T0 + EXPIRY_MS + 99)
    );
    r.core.tick(T0 + EXPIRY_MS);
    let v = view(&r);
    assert_eq!((v.phones.len(), v.pending.is_none()), (0, true));
    assert!(v.error.unwrap().contains("expired"));
    assert_eq!(r.relay.count("DELETE", &id), 1);
    assert!(!r.bridge.has_keys(&id));
    assert_eq!(r.core.next_due(T0 + EXPIRY_MS), None);
}

#[test]
fn declining_deletes_the_pairing() {
    let mut r = rig("decline");
    let id = r.core.start_pairing(T0).unwrap().id;
    r.relay
        .hello(&id, "secret1", "hello|secret1|phonekey1|Phone");
    r.core.tick(T0 + 2_000);
    r.core
        .handle(Msg::Command(Command::Decline(id.clone())), T0 + 3_000);
    assert_eq!(view(&r).phones, []);
    assert!(!r.relay.exists(&id));
    assert!(!r.bridge.has_keys(&id));
    // Starting again replaces a code still out.
    let first = r.core.start_pairing(T0).unwrap().id;
    let second = r.core.start_pairing(T0).unwrap().id;
    assert!(!r.relay.exists(&first) && r.relay.exists(&second));
    r.core.handle(Msg::Command(Command::CancelPairing), T0);
    assert!(!r.relay.exists(&second) && view(&r).phones.is_empty());
}

#[test]
fn publishing_follows_changes_with_a_floor_and_a_resend() {
    let mut r = rig("publish");
    let id = r.pair();
    let t = T0 + 60_000;
    let puts = |r: &Rig| r.relay.count("PUT", "/down");
    r.publish(payload(1, 1), t);
    assert_eq!(puts(&r), 1);
    // Only generatedAtMs changed: nothing to send.
    r.publish(payload(2, 1), t + 11_000);
    assert_eq!(puts(&r), 1);
    r.publish(payload(3, 2), t + 12_000);
    assert_eq!(puts(&r), 2);
    // Changed again, but within 10 s of the last.
    r.publish(payload(4, 3), t + 15_000);
    assert_eq!(puts(&r), 2);
    assert_eq!(r.core.next_due(t + 15_000), Some(t + 22_000));
    r.core.tick(t + 22_000);
    assert_eq!(puts(&r), 3);
    assert_eq!(
        r.relay.down(&id).unwrap().1["payload"]["agents"]["active"],
        3
    );
    // Unchanged for 5 minutes: the state again, with the latest generatedAtMs.
    let resend = t + 22_000 + RESEND_MS;
    r.publish(payload(5, 3), resend - 1);
    assert_eq!(puts(&r), 3);
    r.publish(payload(6, 3), resend);
    let (seq, msg) = r.relay.down(&id).unwrap();
    assert_eq!(puts(&r), 4);
    assert_eq!(
        msg,
        json!({"seq": 3, "type": "state", "payload": payload(6, 3)})
    );
    assert_eq!(seq, 3);
    // `up` is read after each publish and every 30 s.
    let gets = r.relay.count("GET", "/up");
    r.core.tick(resend + POLL_PAIRED_MS);
    assert_eq!(r.relay.count("GET", "/up"), gets + 1);
}

#[test]
fn up_messages_follow_the_seq_rule_and_unpair_removes() {
    let mut r = rig("seq");
    let id = r.pair();
    let t = T0 + 60_000;
    r.publish(payload(1, 1), t);
    // The hello was seq 0, so a 0 now is old.
    r.relay
        .post_up(&id, json!({"seq": 0, "type": "seen", "sentAtMs": 1}));
    r.core.tick(t + POLL_PAIRED_MS);
    assert_eq!(view(&r).phones[0].last_seen_ms, None);
    r.relay
        .post_up(&id, json!({"seq": 1, "type": "seen", "sentAtMs": 1}));
    r.core.tick(t + 2 * POLL_PAIRED_MS);
    assert_eq!(
        view(&r).phones[0].last_seen_ms,
        Some(t + 2 * POLL_PAIRED_MS)
    );
    r.relay
        .post_up(&id, json!({"seq": 1, "type": "seen", "sentAtMs": 2}));
    r.core.tick(t + 3 * POLL_PAIRED_MS);
    assert_eq!(
        view(&r).phones[0].last_seen_ms,
        Some(t + 2 * POLL_PAIRED_MS)
    );
    assert_eq!(store::load(&r.dir)[0].up_seq, 1);

    r.relay.post_up(&id, json!({"seq": 2, "type": "unpair"}));
    r.core.tick(t + 4 * POLL_PAIRED_MS);
    assert_eq!(view(&r).phones, []);
    assert!(!r.bridge.has_keys(&id));
    assert_eq!(
        r.relay.count("DELETE", &id),
        0,
        "the phone already deleted it"
    );
    assert_eq!(store::load(&r.dir), []);
    assert!(!r.core.shared.publishing.load(Ordering::Relaxed));
}

#[test]
fn a_pairing_the_relay_forgot_is_gone() {
    let mut r = rig("gone");
    let id = r.pair();
    let t = T0 + 60_000;
    r.relay.drop_pairing(&id);
    r.publish(payload(1, 1), t);
    assert_eq!(view(&r).phones[0].state, State::Gone);
    assert_eq!(store::load(&r.dir)[0].state, State::Gone);
    let before = r.relay.requests().len();
    r.publish(payload(2, 2), t + RESEND_MS);
    assert_eq!(r.relay.requests().len(), before, "no more requests");
    r.core
        .handle(Msg::Command(Command::Remove(id.clone())), t + RESEND_MS);
    assert_eq!(
        r.relay.requests().len(),
        before,
        "nothing to delete at the relay"
    );
    assert!(!r.bridge.has_keys(&id));
    assert_eq!(view(&r).phones, []);
}

#[test]
fn removing_a_paired_phone_deletes_it_at_the_relay() {
    let mut r = rig("remove");
    let id = r.pair();
    r.core
        .handle(Msg::Command(Command::Remove(id.clone())), T0 + 60_000);
    assert!(!r.relay.exists(&id));
    assert!(!r.bridge.has_keys(&id));
    assert_eq!(store::load(&r.dir), []);
}

#[test]
fn too_many_requests_wait_for_retry_after() {
    let mut r = rig("backoff");
    r.pair();
    let t = T0 + 60_000;
    r.relay.force(429, Some(20));
    r.publish(payload(1, 1), t);
    let before = r.relay.requests().len();
    let direct = r.bridge.calls("listener.send").len();
    r.core.tick(t + 10_000);
    r.core.tick(t + 19_999);
    assert_eq!(r.relay.requests().len(), before);
    // The relay's backoff holds back only the relay: the state still goes out directly.
    assert_eq!(r.bridge.calls("listener.send").len(), direct + 1);
    assert_eq!(r.core.next_due(t + 19_999), Some(t + 20_099));
    r.core.tick(t + 20_000);
    assert_eq!(r.relay.count("PUT", "/down"), 2);
    // A network error is retried at the next due time, not at once.
    r.relay.0.lock().unwrap().online = false;
    r.publish(payload(2, 2), t + 30_000);
    let failed = r.relay.requests().len();
    r.core.tick(t + 35_000);
    assert_eq!(r.relay.requests().len(), failed);
}

#[test]
fn the_direct_listener_follows_pairings_and_settings() {
    let mut r = rig("direct");
    let id = r.pair();
    r.core.tick(T0 + 3_000);
    let set = |r: &Rig| r.bridge.calls("listener.set");
    // Not before there is a state to send first.
    assert_eq!(set(&r).len(), 0);
    let t = T0 + 60_000;
    r.publish(payload(1, 1), t);
    assert_eq!(set(&r).len(), 1);
    assert_eq!(set(&r)[0]["enabled"], true);
    let sends = r.bridge.calls("listener.send");
    assert_eq!(sends.len(), 1);
    let state = from_b64url(sends[0]["plaintext"].as_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&state).unwrap()["type"],
        "state"
    );

    let event = |r: &mut Rig, e: Value, now| r.core.handle(Msg::Event(e), now);
    event(
        &mut r,
        json!({"event": "open", "connection": 7, "id": id}),
        t,
    );
    assert!(view(&r).phones[0].direct);
    let seen = b64url(
        json!({"seq": 1, "type": "seen", "sentAtMs": 1})
            .to_string()
            .as_bytes(),
    );
    event(
        &mut r,
        json!({"event": "message", "connection": 7, "id": id, "plaintext": seen}),
        t + 1,
    );
    assert_eq!(view(&r).phones[0].last_seen_ms, Some(t + 1));
    // A heartbeat every 30 s while connected, with the state's seq and none of its own.
    r.core.tick(t + DIRECT_HEARTBEAT_MS);
    let sends = r.bridge.calls("listener.send");
    assert_eq!(sends.len(), 2);
    let beat = from_b64url(sends[1]["plaintext"].as_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&beat).unwrap(),
        json!({"seq": 0, "type": "heartbeat", "sentAtMs": t + DIRECT_HEARTBEAT_MS})
    );
    assert_eq!(store::load(&r.dir)[0].down_seq, 1);
    event(
        &mut r,
        json!({"event": "close", "connection": 7, "id": id}),
        t,
    );
    assert!(!view(&r).phones[0].direct);

    r.settings.set_phones(Phones {
        relay: Some(RELAY.into()),
        direct: false,
    });
    r.core.handle(Msg::Wake, t + 40_000);
    r.core.tick(t + 40_000);
    assert_eq!(set(&r).len(), 2);
    assert_eq!(
        set(&r)[1],
        json!({"op": "listener.set", "pairings": [], "enabled": false})
    );

    r.settings.set_phones(Phones {
        relay: Some(RELAY.into()),
        direct: true,
    });
    r.core.tick(t + 41_000);
    assert_eq!(set(&r).len(), 3);
    // Primed with the latest state for the first frame.
    let sends = r.bridge.calls("listener.send");
    assert_eq!(sends.last().unwrap()["plaintext"], b64url(&state));

    r.core.handle(Msg::Command(Command::Remove(id)), t + 42_000);
    r.core.tick(t + 42_000);
    assert_eq!(set(&r).len(), 4);
    assert_eq!(set(&r)[3]["enabled"], false);
    let ops: HashSet<String> = r.bridge.ops().into_iter().collect();
    assert!(ops.contains("pair.delete"));
}

#[test]
fn phones_json_round_trips_and_a_corrupt_one_is_kept() {
    let r = rig("store");
    let pairings = vec![
        Pairing {
            id: "a".into(),
            relay: RELAY.into(),
            phone_name: Some("Phone".into()),
            phone_key: Some("key".into()),
            state: State::Paired,
            created_at_ms: 1,
            last_seen_ms: Some(2),
            down_seq: 3,
            up_seq: 4,
        },
        Pairing {
            id: "b".into(),
            relay: RELAY.into(),
            phone_name: None,
            phone_key: None,
            state: State::Waiting,
            created_at_ms: 5,
            last_seen_ms: None,
            down_seq: 0,
            up_seq: 0,
        },
    ];
    store::save(&r.dir, &pairings).unwrap();
    assert_eq!(store::load(&r.dir), pairings);
    let text = fs::read_to_string(r.dir.join(store::FILE_NAME)).unwrap();
    assert!(
        text.contains("\"phoneName\"") && text.contains("\"downSeq\": 3"),
        "{text}"
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(r.dir.join(store::FILE_NAME))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    fs::write(r.dir.join(store::FILE_NAME), "{\"pairings\": [").unwrap();
    assert_eq!(store::load(&r.dir), []);
    assert!(!r.dir.join(store::FILE_NAME).exists());
    let kept = fs::read_dir(&r.dir)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy().starts_with("phones.json.bad-")
        })
        .count();
    assert_eq!(kept, 1);
}

impl Rig {
    /// A new worker over the same Keychain, relay and data directory.
    fn restart(&mut self) {
        let (changed, _) = async_channel::bounded(1);
        self.core = Core::new(
            Some(Box::new(self.bridge.clone())),
            Box::new(self.relay.clone()),
            self.settings.clone(),
            self.dir.clone(),
            Shared {
                view: Arc::default(),
                changed,
                publishing: Arc::default(),
            },
        );
    }
}

#[test]
fn pairings_survive_a_restart() {
    let mut r = rig("restart");
    let id = r.pair();
    r.publish(payload(1, 1), T0 + 60_000);
    r.restart();
    assert!(r.core.shared.publishing.load(Ordering::Relaxed));
    r.publish(payload(2, 2), T0 + 61_000);
    // The seq carried on, so the relay took it.
    assert_eq!(r.relay.down(&id).unwrap().0, 1);

    // A code awaiting confirmation is worked out again.
    let id = r.core.start_pairing(T0).unwrap().id;
    r.relay
        .hello(&id, "secret2", "hello|secret2|phonekey2|Phone 2");
    r.core.tick(T0 + 2_000);
    r.restart();
    r.core.tick(T0 + 3_000);
    let v = view(&r);
    let phone = v.phones.iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        (phone.state, phone.code.as_deref()),
        (State::Confirming, Some("123456"))
    );

    // A code no longer on screen is forgotten: here, at the relay and its keys.
    let id = r.core.start_pairing(T0).unwrap().id;
    r.restart();
    assert!(view(&r).phones.iter().all(|p| p.id != id));
    assert!(store::load(&r.dir).iter().all(|p| p.id != id));
    assert!(!r.relay.exists(&id) && !r.bridge.has_keys(&id));
}

#[test]
fn base64url_round_trips() {
    assert_eq!(b64url(b""), "");
    assert_eq!(b64url(b"hello"), "aGVsbG8");
    assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    for n in 0..40u8 {
        let bytes: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
        assert_eq!(from_b64url(&b64url(&bytes)), Some(bytes));
    }
    assert_eq!(from_b64url("a"), None);
    assert_eq!(from_b64url("aGVs+G8"), None);
    assert_eq!(from_b64url("aGVsbG8="), None);
}

#[test]
fn the_relay_key_goes_only_on_create() {
    let mut r = rig("relay-key");
    r.relay.0.lock().unwrap().relay_key = Some("deployer-key".into());
    r.core.tick(T0);
    assert_eq!(view(&r).relay_key, Some(false));
    assert!(r.core.start_pairing(T0).unwrap_err().contains("401"));
    r.core.handle(
        Msg::Command(Command::SetRelayKey(Some("deployer-key".into()))),
        T0,
    );
    assert_eq!(view(&r).relay_key, Some(true));
    let id = r.pair();
    r.publish(payload(1, 1), T0 + 60_000);
    r.core
        .handle(Msg::Command(Command::Remove(id)), T0 + 61_000);
    let s = r.relay.0.lock().unwrap();
    let key = Some("deployer-key".to_owned());
    // The first create went before the key was set.
    assert_eq!(s.tokens[0], None);
    for ((method, path), token) in s.requests.iter().zip(&s.tokens).skip(1) {
        let create = method == "POST" && path == "/v1/pairings";
        assert_eq!(token == &key, create, "{method} {path}");
    }
    assert!(s.requests.len() > 4);
    drop(s);
    let saved = fs::read_to_string(r.dir.join(store::FILE_NAME)).unwrap();
    assert!(!saved.contains("deployer-key"));
    r.core.handle(Msg::Command(Command::SetRelayKey(None)), T0);
    assert_eq!(r.bridge.0.lock().unwrap().relay_key, None);
    assert_eq!(view(&r).relay_key, Some(false));
}

#[test]
fn a_refused_create_forgets_the_saved_record() {
    let mut r = rig("create-fails");
    r.relay.force(500, None);
    assert!(r.core.start_pairing(T0).unwrap_err().contains("500"));
    // Saved before the request, gone after it failed, keys too.
    let made = r.bridge.calls("pair.new").len();
    assert_eq!(made, 1);
    assert!(!r.bridge.has_keys("pairing1"));
    assert_eq!(store::load(&r.dir), []);
    assert_eq!(view(&r).phones, []);
}

#[test]
fn removing_keeps_the_phone_when_its_keys_stay() {
    let mut r = rig("remove-fails");
    let id = r.pair();
    r.bridge
        .fail("pair.delete", Some("keychain: cannot delete (-25244)"));
    r.core
        .handle(Msg::Command(Command::Remove(id.clone())), T0 + 60_000);
    let v = view(&r);
    assert_eq!(v.phones[0].state, State::Paired);
    assert!(v.error.unwrap().contains("Try again"));
    assert!(r.relay.exists(&id) && r.bridge.has_keys(&id));
    assert_eq!(store::load(&r.dir).len(), 1);
    r.bridge.fail("pair.delete", None);
    r.core
        .handle(Msg::Command(Command::Remove(id.clone())), T0 + 61_000);
    assert!(!r.relay.exists(&id) && !r.bridge.has_keys(&id));
    assert_eq!(view(&r).phones, []);
}

#[test]
fn a_seq_that_cannot_be_saved_is_not_sent() {
    let mut r = rig("save-fails");
    let id = r.pair();
    let file = r.dir.join(store::FILE_NAME);
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    r.publish(payload(1, 1), T0 + 60_000);
    assert_eq!(r.relay.count("PUT", "/down"), 0);
    assert!(r.bridge.calls("listener.send").is_empty());
    // Tried again on the usual 10 s, not at once.
    assert_eq!(r.core.next_due(T0 + 60_000), Some(T0 + 70_000));
    fs::remove_dir(&file).unwrap();
    r.publish(payload(1, 1), T0 + 70_000);
    // The unsaved seq 0 was never sent and is not reused.
    assert_eq!(r.relay.down(&id).unwrap().0, 1);
    assert_eq!(store::load(&r.dir)[0].down_seq, 2);
}

#[test]
fn a_keychain_error_on_the_hello_tries_again() {
    let mut r = rig("hello-keychain");
    let id = r.core.start_pairing(T0).unwrap().id;
    r.relay
        .hello(&id, "secret1", "hello|secret1|phonekey1|Phone");
    // Only the batch's first message is taken as the hello.
    let up = "hello|secret1|phonekey9|Eve".to_owned();
    r.relay
        .0
        .lock()
        .unwrap()
        .pairings
        .get_mut(&id)
        .unwrap()
        .up
        .push(up);
    r.bridge.fail("hello.open", Some("keychain: -25308"));
    r.core.tick(T0 + 2_000);
    assert_eq!(r.bridge.calls("hello.open").len(), 1);
    assert_eq!(view(&r).phones[0].state, State::Waiting);
    assert!(r.relay.exists(&id) && r.bridge.has_keys(&id));
    r.bridge.fail("hello.open", None);
    r.core.tick(T0 + 4_000);
    // The hello the relay handed out once is opened from memory.
    let v = view(&r);
    assert_eq!(v.phones[0].state, State::Confirming);
    assert_eq!(v.phones[0].code.as_deref(), Some("123456"));
    assert_eq!(
        store::load(&r.dir)[0].phone_key.as_deref(),
        Some("phonekey1")
    );
    assert_eq!(r.relay.count("GET", "/up"), 1);
    // The code came with the hello.
    assert!(r.bridge.calls("pair.code").is_empty());
}

#[test]
fn a_failed_listener_set_is_tried_again_every_30_s() {
    let mut r = rig("listener-fails");
    let t = T0 + 60_000;
    let a = r.pair();
    r.publish(payload(1, 1), t);
    let set = |r: &Rig| r.bridge.calls("listener.set");
    assert_eq!(set(&r).len(), 1);
    let b = r.pair();
    // B's first state wants the listener for A and B; that fails and stops it.
    r.bridge.fail("listener.set", Some("keychain: -25308"));
    r.publish(payload(2, 2), t + 10_000);
    assert_eq!(set(&r).len(), 2);
    assert!(view(&r).error.is_some());
    assert!(r.core.next_due(t + 39_000) <= Some(t + 40_000));
    // B goes; the listener is off now, so A's must be set again, but not before 30 s.
    r.core.handle(Msg::Command(Command::Remove(b)), t + 11_000);
    r.core.tick(t + 20_000);
    assert_eq!(set(&r).len(), 2);
    r.bridge.fail("listener.set", None);
    r.core.tick(t + 40_000);
    assert_eq!(set(&r).len(), 3);
    assert_eq!(
        set(&r)[2],
        json!({"op": "listener.set", "enabled": true,
            "pairings": [{"id": a, "phoneKey": "phonekey1"}]})
    );
    // Primed with A's latest state.
    let sends = r.bridge.calls("listener.send");
    let last = from_b64url(sends.last().unwrap()["plaintext"].as_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&last).unwrap()["type"],
        "state"
    );
    r.core.tick(t + 41_000);
    assert_eq!(set(&r).len(), 3);

    // The listener failing after it started also turns it off, and it is set again later.
    r.core.handle(
        Msg::Event(json!({"event": "listener-failed", "error": "closed"})),
        t + 50_000,
    );
    r.core.tick(t + 50_000);
    assert_eq!(set(&r).len(), 3);
    r.core.tick(t + 80_000);
    assert_eq!(set(&r).len(), 4);
}
