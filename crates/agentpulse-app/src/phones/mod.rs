//! The Mac's side of the phone link (docs/mobile.md, "Pairing", "Relay API", "Direct
//! connection", "Mac client"): pairing, publishing the widget payload to paired phones
//! through the relay and the direct listener, and reading what the phones send back.
//!
//! One worker thread makes every bridge and network call. It is driven by commands, by
//! [`PhoneLink::publish`] and by the bridge's listener events. Off by default: with no
//! relay address and no pairing it makes no call at all and only waits on its channel.
//! Secrets stay in the Keychain behind the bridge; `phones.json` holds no secret and
//! nothing here logs one. The Mac's relay tokens are cached in memory only.

mod bridge;
#[cfg(feature = "e2e")]
pub mod e2e;
mod relay;
mod store;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use bridge::{Bridge, b64url, from_b64url};
use relay::{Relay, Reply};
use store::Pairing;
pub use store::State;

use crate::settings::{Looks, Settings};
use crate::widget_export::Payload;

const POLL_WAITING_MS: i64 = 2_000;
const EXPIRY_MS: i64 = 10 * 60_000;
const PUBLISH_EVERY_MS: i64 = 10_000;
/// The unchanged state is re-sent this often, with a fresh `generatedAtMs`.
const RESEND_MS: i64 = 5 * 60_000;
const POLL_PAIRED_MS: i64 = 30_000;
const DIRECT_HEARTBEAT_MS: i64 = 30_000;
const LISTENER_RETRY_MS: i64 = 30_000;
/// A floor on the worker's sleep, so a due time in the past never spins it.
const MIN_WAIT_MS: i64 = 100;

pub enum Msg {
    Command(Command),
    Publish(Box<Payload>),
    /// A listener event from the bridge.
    Event(Value),
    /// Settings changed; re-read them.
    Wake,
}

pub enum Command {
    StartPairing(async_channel::Sender<Result<Started, String>>),
    Confirm(String),
    Decline(String),
    Remove(String),
    CancelPairing,
    /// The relay's `RELAY_KEY`, or None to forget it (Keychain only).
    SetRelayKey(Option<String>),
}

/// A pairing whose QR code is out: `link` holds the one-time secret, so it lives in
/// memory only.
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    pub id: String,
    pub link: String,
    pub expires_at_ms: i64,
    pub qr: Qr,
}

/// The link's QR code: `size` × `size` modules, row by row, true for dark, including the
/// 1-module light margin CIQRCodeGenerator leaves around the symbol.
#[derive(Clone, Debug, PartialEq)]
pub struct Qr {
    pub size: usize,
    pub dark: Vec<bool>,
}

impl Qr {
    /// From the bridge's `qr` reply at scale 1: one RGBA pixel per module.
    fn from_bridge(v: &Value) -> Result<Qr, String> {
        let size = v["width"].as_u64().unwrap_or(0) as usize;
        let rgba = v["rgba"].as_str().and_then(from_b64url).unwrap_or_default();
        if size == 0 || v["height"].as_u64() != Some(size as u64) || rgba.len() != size * size * 4 {
            return Err("bad QR image from the bridge".into());
        }
        Ok(Qr {
            size,
            dark: rgba.chunks(4).map(|px| px[0] < 128).collect(),
        })
    }
}

/// What the Settings card shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// False without the app bundle's bridge ("Phones need the app bundle").
    pub available: bool,
    pub phones: Vec<Phone>,
    pub pending: Option<Started>,
    pub error: Option<String>,
    /// The relay's `RELAY_KEY` is in the Keychain; None until known (read once a relay
    /// address is set).
    pub relay_key: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phone {
    pub id: String,
    pub name: Option<String>,
    pub state: State,
    /// The six-digit code while confirming.
    pub code: Option<String>,
    pub last_seen_ms: Option<i64>,
    /// Connected directly on this network.
    pub direct: bool,
}

/// The handle the app keeps: commands in, [`View`] and change notifications out.
#[derive(Clone)]
pub struct PhoneLink {
    tx: mpsc::Sender<Msg>,
    view: Arc<Mutex<View>>,
    changes: async_channel::Receiver<()>,
    publishing: Arc<AtomicBool>,
    settings: Settings,
}

impl PhoneLink {
    /// Starts the worker. The bridge is used only when running from inside
    /// AgentPulse.app; otherwise phones are unavailable, logged once here.
    pub fn spawn(dir: &Path, settings: Settings) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let bridge = bundle_bridge()
            .map(|path| Box::new(bridge::Dylib::new(path, tx.clone())) as Box<dyn Bridge>);
        if bridge.is_none() {
            tracing::info!("phones need the app bundle; the phone link is off");
        }
        let (changed, changes) = async_channel::bounded(1);
        let shared = Shared {
            view: Arc::new(Mutex::new(View::default())),
            changed,
            publishing: Arc::new(AtomicBool::new(false)),
        };
        let link = PhoneLink {
            tx,
            view: shared.view.clone(),
            changes,
            publishing: shared.publishing.clone(),
            settings: settings.clone(),
        };
        let dir = dir.to_path_buf();
        thread::Builder::new()
            .name("agentpulse-phones".into())
            .spawn(move || {
                let core = Core::new(bridge, Box::new(relay::Http), settings, dir, shared);
                run(core, rx);
            })?;
        Ok(link)
    }

    /// Makes a pairing at the relay in Settings; the reply carries the QR link.
    pub fn start_pairing(&self) -> async_channel::Receiver<Result<Started, String>> {
        let (reply, result) = async_channel::bounded(1);
        self.send(Msg::Command(Command::StartPairing(reply)));
        result
    }

    pub fn confirm(&self, id: &str) {
        self.send(Msg::Command(Command::Confirm(id.into())));
    }

    pub fn decline(&self, id: &str) {
        self.send(Msg::Command(Command::Decline(id.into())));
    }

    pub fn remove(&self, id: &str) {
        self.send(Msg::Command(Command::Remove(id.into())));
    }

    pub fn cancel_pairing(&self) {
        self.send(Msg::Command(Command::CancelPairing));
    }

    /// Keeps the relay's `RELAY_KEY` in the Keychain; None or empty forgets it.
    pub fn set_relay_key(&self, key: Option<String>) {
        let key = key.filter(|k| !k.is_empty());
        self.send(Msg::Command(Command::SetRelayKey(key)));
    }

    /// Call after changing the `phones` settings.
    pub fn settings_changed(&self) {
        self.send(Msg::Wake);
    }

    pub fn view(&self) -> View {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Receives `()` after the view changed (coalesced).
    pub fn changes(&self) -> async_channel::Receiver<()> {
        self.changes.clone()
    }

    /// True while a phone is paired, so the collector only builds the payload then.
    pub fn wants_payload(&self) -> bool {
        self.publishing.load(Ordering::Relaxed)
    }

    /// The looks the widget payload uses, so phones get exactly the widget's payload.
    pub fn looks(&self) -> Looks {
        self.settings.looks()
    }

    pub fn publish(&self, payload: Payload) {
        if self.wants_payload() {
            self.send(Msg::Publish(Box::new(payload)));
        }
    }

    fn send(&self, msg: Msg) {
        // Only fails once the worker is gone, and then there is nothing to tell.
        self.tx.send(msg).ok();
    }
}

fn bundle_bridge() -> Option<PathBuf> {
    // The end-to-end test's bridge, built outside a bundle (scripts/e2e-phones.sh).
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os("AGENTPULSE_LINK_BRIDGE") {
        return Some(path.into());
    }
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    if contents.file_name()? != "Contents" || contents.parent()?.extension()? != "app" {
        return None;
    }
    Some(contents.join(bridge::PATH)).filter(|p| p.is_file())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn run(mut core: Core, rx: mpsc::Receiver<Msg>) {
    loop {
        core.tick(now_ms());
        let now = now_ms();
        let msg = match core.next_due(now) {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(due) => rx.recv_timeout(Duration::from_millis((due - now) as u64)),
        };
        match msg {
            Ok(msg) => core.handle(msg, now_ms()),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// What the worker shares with the handles.
struct Shared {
    view: Arc<Mutex<View>>,
    changed: async_channel::Sender<()>,
    publishing: Arc<AtomicBool>,
}

/// Per pairing, in memory only.
#[derive(Default)]
struct Live {
    /// The Mac's relay token, from the Keychain.
    token: Option<String>,
    code: Option<String>,
    /// The next `GET up`.
    next_poll_ms: i64,
    /// No relay request before this (429).
    backoff_until_ms: i64,
    /// The last `PUT down` attempt.
    put_ms: Option<i64>,
    /// The payload the relay last accepted.
    sent: Option<Payload>,
    /// The latest state message and its seq: the listener's first frame, and the seq
    /// its heartbeats repeat.
    state_message: Option<(u64, Vec<u8>)>,
    direct_sent_ms: i64,
    /// A hello that failed to open for a transient (Keychain) reason, tried again at the
    /// next poll: the relay hands it out only once.
    hello: Option<String>,
}

/// The worker's state machine. Every method takes the time, so tests drive the clock.
struct Core {
    bridge: Option<Box<dyn Bridge>>,
    relay: Box<dyn Relay>,
    settings: Settings,
    dir: PathBuf,
    pairings: Vec<Pairing>,
    live: HashMap<String, Live>,
    payload: Option<Payload>,
    pending: Option<Started>,
    error: Option<String>,
    /// The pairings the listener was last set to (id, phone key); None when off.
    listener: Option<Vec<(String, String)>>,
    /// No `listener.set` before this, after one failed.
    listener_retry_ms: i64,
    /// Live direct connections: connection number to pairing id.
    connections: HashMap<i64, String>,
    /// Whether the relay key is in the Keychain; None until read, once a relay is set.
    relay_key: Option<bool>,
    shared: Shared,
}

impl Core {
    fn new(
        bridge: Option<Box<dyn Bridge>>,
        relay: Box<dyn Relay>,
        settings: Settings,
        dir: PathBuf,
        shared: Shared,
    ) -> Self {
        let mut core = Core {
            pairings: if bridge.is_some() {
                store::load(&dir)
            } else {
                Vec::new()
            },
            bridge,
            relay,
            settings,
            dir,
            live: HashMap::new(),
            payload: None,
            pending: None,
            error: None,
            listener: None,
            listener_retry_ms: 0,
            connections: HashMap::new(),
            relay_key: None,
            shared,
        };
        // A code no longer shown (the app restarted) can't be shown again.
        let waiting: Vec<String> = core
            .pairings
            .iter()
            .filter(|p| p.state == State::Waiting)
            .map(|p| p.id.clone())
            .collect();
        for id in waiting {
            core.forget(&id, true, now_ms());
        }
        core.refresh();
        core
    }

    fn handle(&mut self, msg: Msg, now: i64) {
        match msg {
            // The phone contract has no Codex limit: it stays on this Mac's widget.
            Msg::Publish(p) => {
                self.payload = Some(Payload {
                    codex_limit: None,
                    ..*p
                })
            }
            Msg::Event(e) => self.event(&e, now),
            Msg::Wake => {}
            Msg::Command(Command::StartPairing(reply)) => {
                reply.try_send(self.start_pairing(now)).ok();
            }
            Msg::Command(Command::Confirm(id)) => self.confirm(&id),
            Msg::Command(Command::Decline(id)) => self.decline(&id, now),
            Msg::Command(Command::Remove(id)) => self.remove(&id, now),
            Msg::Command(Command::CancelPairing) => self.cancel_pairing(now),
            Msg::Command(Command::SetRelayKey(key)) => {
                match self.bridge(&json!({"op": "relaykey.set", "key": key})) {
                    Ok(_) => self.relay_key = Some(key.is_some()),
                    Err(e) => self.error = Some(format!("Cannot keep the relay key: {e}")),
                }
            }
        }
        self.refresh();
    }

    // --- commands ---

    fn start_pairing(&mut self, now: i64) -> Result<Started, String> {
        self.error = None;
        if self.bridge.is_none() {
            return Err("Phones need the app bundle.".into());
        }
        let relay = self.settings.phones().relay;
        let relay = relay.as_deref().map(|r| r.trim_end_matches('/'));
        let Some(relay) = relay.filter(|r| !r.is_empty()).map(str::to_owned) else {
            return Err("Set the relay address first.".into());
        };
        self.cancel_pairing(now);
        // The relay's `RELAY_KEY`, sent on this one request only.
        let relay_key = self.bridge(&json!({"op": "relaykey.get"}))?["key"]
            .as_str()
            .map(str::to_owned);
        let made = self.bridge(&json!({"op": "pair.new", "relay": relay}))?;
        let field = |k: &str| made[k].as_str().unwrap_or_default().to_owned();
        let id = field("id");
        let body = json!({
            "id": id, "macTokenHash": made["macTokenHash"], "secretHash": made["secretHash"],
        });
        // Saved first, so a crash before the relay answers leaves a record (forgotten,
        // keys and all, at the next start) rather than orphaned keys.
        self.pairings.push(Pairing {
            id: id.clone(),
            relay: relay.clone(),
            phone_name: None,
            phone_key: None,
            state: State::Waiting,
            created_at_ms: now,
            last_seen_ms: None,
            down_seq: 0,
            up_seq: 0,
        });
        if !self.save() {
            self.forget(&id, false, now);
            return Err(format!("Cannot save {}.", store::FILE_NAME));
        }
        let created = self.relay.call(
            "POST",
            &format!("{relay}/v1/pairings"),
            relay_key.as_deref(),
            Some(&body),
        );
        let failure = match created {
            Ok(Reply { status: 201, .. }) => None,
            Ok(r) => Some(format!(
                "The relay refused the pairing (HTTP {}).",
                r.status
            )),
            Err(e) => Some(format!("Cannot reach the relay: {e}")),
        };
        if let Some(failure) = failure {
            self.forget(&id, false, now);
            return Err(failure);
        }
        let link = field("link");
        let qr = self
            .bridge(&json!({"op": "qr", "text": link, "scale": 1}))
            .and_then(|v| Qr::from_bridge(&v));
        let qr = match qr {
            Ok(qr) => qr,
            Err(e) => {
                self.forget(&id, true, now);
                return Err(format!("Cannot make the QR code: {e}"));
            }
        };
        self.live.entry(id.clone()).or_default().next_poll_ms = now + POLL_WAITING_MS;
        let started = Started {
            id,
            link,
            expires_at_ms: now + EXPIRY_MS,
            qr,
        };
        self.pending = Some(started.clone());
        Ok(started)
    }

    fn confirm(&mut self, id: &str) {
        let Some(p) = self.pairing_mut(id) else {
            return;
        };
        if p.state == State::Confirming {
            // `upSeq` is 0 from the hello: later `up` messages need 1 or more.
            p.state = State::Paired;
            self.live.entry(id.into()).or_default().code = None;
            self.save();
        }
    }

    fn decline(&mut self, id: &str, now: i64) {
        if self
            .pairing(id)
            .is_some_and(|p| matches!(p.state, State::Waiting | State::Confirming))
        {
            self.forget(id, true, now);
        }
    }

    fn remove(&mut self, id: &str, now: i64) {
        let Some(p) = self.pairing(id) else {
            return;
        };
        let at_relay = p.state != State::Gone;
        if at_relay {
            // Cached for the relay's DELETE, which comes after the keys are gone.
            self.token(id);
        }
        // Keys first: if they stay, so does the record, and the user can try again.
        if let Err(e) = self.bridge(&json!({"op": "pair.delete", "id": id})) {
            tracing::warn!(error = %e, "cannot delete a pairing's keys");
            self.error = Some(format!("Cannot remove the phone: {e}. Try again."));
            return;
        }
        self.forget(id, at_relay, now);
    }

    fn cancel_pairing(&mut self, now: i64) {
        let waiting: Vec<String> = self
            .pairings
            .iter()
            .filter(|p| p.state == State::Waiting)
            .map(|p| p.id.clone())
            .collect();
        for id in waiting {
            self.forget(&id, true, now);
        }
        self.pending = None;
    }

    // --- the clock ---

    fn tick(&mut self, now: i64) {
        if self.bridge.is_none() {
            return;
        }
        if self.relay_key.is_none() && self.settings.phones().relay.is_some() {
            // A failure leaves it unknown, so a later tick tries again.
            match self.bridge(&json!({"op": "relaykey.get"})) {
                Ok(v) => self.relay_key = Some(v["key"].is_string()),
                Err(e) => tracing::warn!(error = %e, "cannot read the relay key"),
            }
        }
        let ids: Vec<(String, State)> = self
            .pairings
            .iter()
            .map(|p| (p.id.clone(), p.state))
            .collect();
        for (id, state) in ids {
            let live = self.live.entry(id.clone()).or_default();
            let (poll_due, backoff) = (live.next_poll_ms <= now, live.backoff_until_ms > now);
            match state {
                State::Waiting => {
                    let created = self.pairing(&id).map_or(now, |p| p.created_at_ms);
                    if now >= created + EXPIRY_MS {
                        self.forget(&id, true, now);
                        self.error = Some("The pairing code expired.".into());
                    } else if poll_due && !backoff && self.shown(&id) {
                        self.live_mut(&id).next_poll_ms = now + POLL_WAITING_MS;
                        self.poll(&id, now);
                    }
                }
                State::Paired => {
                    let published = self.publish(&id, now);
                    let polling = self.pairing(&id).is_some_and(|p| p.state == State::Paired)
                        && self.live_mut(&id).backoff_until_ms <= now;
                    if polling && (published || poll_due) {
                        self.live_mut(&id).next_poll_ms = now + POLL_PAIRED_MS;
                        self.poll(&id, now);
                    }
                }
                // The code lives in memory only; after a restart, work it out again.
                State::Confirming if self.live.get(&id).is_some_and(|l| l.code.is_none()) => {
                    let key = self.pairing(&id).and_then(|p| p.phone_key.clone());
                    let code = self.bridge(&json!({"op": "pair.code", "id": id, "phoneKey": key}));
                    match code {
                        Ok(c) => self.live_mut(&id).code = c["code"].as_str().map(str::to_owned),
                        Err(e) => tracing::warn!(error = %e, "cannot work out a pairing's code"),
                    }
                }
                State::Confirming | State::Gone => {}
            }
        }
        self.sync_listener(now);
        self.direct_heartbeats(now);
        self.refresh();
    }

    /// When the worker next has something to do; None when it only waits for messages.
    fn next_due(&self, now: i64) -> Option<i64> {
        self.bridge.as_ref()?;
        let direct = self.settings.phones().direct;
        let mut due: Option<i64> = None;
        let mut at = |t: i64| due = Some(due.map_or(t, |d| d.min(t)));
        for p in &self.pairings {
            let live = self.live.get(&p.id);
            let backoff = live.map_or(0, |l| l.backoff_until_ms);
            let poll = live.map_or(0, |l| l.next_poll_ms).max(backoff);
            match p.state {
                State::Waiting => {
                    at(p.created_at_ms + EXPIRY_MS);
                    if self.shown(&p.id) {
                        at(poll);
                    }
                }
                State::Paired => {
                    at(poll);
                    if let Some(t) = live.and_then(|l| self.publish_due(l)) {
                        // Direct state goes out whatever the relay's backoff.
                        at(if direct { t } else { t.max(backoff) });
                    }
                    if self.connected(&p.id) {
                        at(live.map_or(0, |l| l.direct_sent_ms) + DIRECT_HEARTBEAT_MS);
                    }
                }
                State::Confirming | State::Gone => {}
            }
        }
        if self.listener_retry_ms > now {
            at(self.listener_retry_ms);
        }
        due.map(|t| t.max(now + MIN_WAIT_MS))
    }

    /// When this pairing's next state is due: on a change at most every 10 s, else every
    /// 5 minutes (the relay keeps one slot, so a heartbeat there would replace the state).
    fn publish_due(&self, live: &Live) -> Option<i64> {
        let payload = self.payload.as_ref()?;
        let changed = live.sent.as_ref().is_none_or(|s| {
            Payload {
                generated_at_ms: payload.generated_at_ms,
                ..s.clone()
            } != *payload
        });
        let Some(put) = live.put_ms else {
            return Some(i64::MIN);
        };
        Some(put + if changed { PUBLISH_EVERY_MS } else { RESEND_MS })
    }

    // --- relay ---

    /// The state to the direct listener and `PUT down` when due; true when the `PUT`
    /// was attempted. A relay backoff holds back only the `PUT`.
    fn publish(&mut self, id: &str, now: i64) -> bool {
        let Some(due) = self.live.get(id).and_then(|l| self.publish_due(l)) else {
            return false;
        };
        let backoff = self.live_mut(id).backoff_until_ms > now;
        if due > now || (backoff && !self.settings.phones().direct) {
            return false;
        }
        let Some(phone_key) = self.pairing(id).and_then(|p| p.phone_key.clone()) else {
            return false;
        };
        // Set first, so a failed save waits the usual 10 s instead of spinning.
        self.live_mut(id).put_ms = Some(now);
        let Some((seq, plaintext)) = self.state_message(id) else {
            return false;
        };
        self.live_mut(id).state_message = Some((seq, plaintext.clone()));
        if self.listener.is_some() {
            self.send_direct(id, &plaintext, now);
        }
        if backoff {
            return false;
        }
        let sealed = self.bridge(&json!({
            "op": "seal", "id": id, "phoneKey": phone_key, "direction": "down",
            "plaintext": b64url(&plaintext),
        }));
        let message = match sealed {
            Ok(v) => v["message"].clone(),
            Err(e) => {
                tracing::warn!(error = %e, "cannot seal the state for a phone");
                return true;
            }
        };
        let body = json!({"seq": seq, "message": message});
        if let Some(r) = self.request(id, "PUT", "down", Some(&body), now)
            && r.status == 204
        {
            self.live_mut(id).sent = self.payload.clone();
        }
        true
    }

    /// The next `state` message for this pairing, its seq taken and saved. None when
    /// the seq can't be saved: a seq is never sent twice.
    fn state_message(&mut self, id: &str) -> Option<(u64, Vec<u8>)> {
        let p = self.pairing_mut(id)?;
        let seq = p.down_seq;
        p.down_seq += 1;
        if !self.save() {
            return None;
        }
        let msg = json!({"seq": seq, "type": "state", "payload": self.payload});
        Some((seq, msg.to_string().into_bytes()))
    }

    /// `GET up`: the hello while waiting, the phone's messages after that.
    fn poll(&mut self, id: &str, now: i64) {
        if let Some(hello) = self.live_mut(id).hello.take() {
            return self.hello(id, &hello, now);
        }
        let Some(r) = self.request(id, "GET", "up", None, now) else {
            return;
        };
        if r.status != 200 {
            return;
        }
        let messages: Vec<String> = r.body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m.as_str().map(str::to_owned))
            .collect();
        for (i, m) in messages.into_iter().enumerate() {
            match self.pairing(id).map(|p| (p.state, p.phone_key.clone())) {
                // The hello comes first; anything after it while still waiting is dropped.
                Some((State::Waiting, _)) if i == 0 => self.hello(id, &m, now),
                Some((_, Some(key))) => {
                    let opened = self.bridge(&json!({
                        "op": "open", "id": id, "phoneKey": key, "direction": "up", "message": m,
                    }));
                    match opened.map(|v| from_b64url(v["plaintext"].as_str().unwrap_or("!"))) {
                        Ok(Some(plaintext)) => self.take_up(id, &plaintext, now),
                        Ok(None) => tracing::warn!("bad plaintext from the bridge"),
                        Err(e) => {
                            tracing::warn!(error = %e, "dropped a phone message that does not open")
                        }
                    }
                }
                _ => return,
            }
        }
    }

    fn hello(&mut self, id: &str, message: &str, now: i64) {
        let opened = self.bridge(&json!({"op": "hello.open", "id": id, "message": message}));
        let hello = opened.and_then(|h| {
            let key = h["phoneKey"].as_str().ok_or("no phone key")?.to_owned();
            Ok((
                key,
                h["phoneName"].as_str().map(str::to_owned),
                h["code"].clone(),
            ))
        });
        match hello {
            Ok((key, name, code)) => {
                if let Some(p) = self.pairing_mut(id) {
                    p.state = State::Confirming;
                    p.phone_key = Some(key);
                    p.phone_name = name;
                    p.up_seq = 0;
                }
                self.live_mut(id).code = code.as_str().map(str::to_owned);
                if self.pending.as_ref().is_some_and(|s| s.id == id) {
                    self.pending = None;
                }
                self.save();
            }
            // The Keychain may answer next time; the pairing stays Waiting until it expires.
            Err(e) if e.starts_with("keychain") => {
                tracing::warn!(error = %e, "cannot open a phone's hello yet");
                self.live_mut(id).hello = Some(message.into());
            }
            Err(e) => {
                tracing::warn!(error = %e, "a phone's hello did not check out; pairing deleted");
                self.forget(id, true, now);
                self.error = Some("The phone's answer did not check out. Pair again.".into());
            }
        }
    }

    /// One relay request for this pairing with the Mac's token. Handles 429 (backoff)
    /// and a pairing the relay no longer knows; None on a network or token error.
    fn request(
        &mut self,
        id: &str,
        method: &str,
        action: &str,
        body: Option<&Value>,
        now: i64,
    ) -> Option<Reply> {
        let relay = self.pairing(id)?.relay.clone();
        let token = self.token(id)?;
        let url = format!("{relay}/v1/pairings/{id}/{action}");
        let url = url.trim_end_matches('/');
        let r = match self.relay.call(method, url, Some(&token), body) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "relay request failed");
                return None;
            }
        };
        match r.status {
            429 => {
                let wait = r.retry_after.unwrap_or(1).clamp(1, 3_600);
                self.live_mut(id).backoff_until_ms = now + wait * 1000;
            }
            401 | 404 if method != "DELETE" => match self.pairing(id).map(|p| p.state) {
                Some(State::Waiting) => {
                    self.forget(id, false, now);
                    self.error = Some("The relay dropped the pairing. Pair again.".into());
                }
                Some(State::Paired) => {
                    if let Some(p) = self.pairing_mut(id) {
                        p.state = State::Gone;
                    }
                    self.save();
                }
                _ => {}
            },
            s if !(200..300).contains(&s) => {
                tracing::warn!(status = s, method, action, "relay refused a request");
            }
            _ => {}
        }
        Some(r)
    }

    fn token(&mut self, id: &str) -> Option<String> {
        if let Some(t) = self.live.get(id).and_then(|l| l.token.clone()) {
            return Some(t);
        }
        match self.bridge(&json!({"op": "pair.token", "id": id})) {
            Ok(v) => {
                let t = v["token"].as_str()?.to_owned();
                self.live_mut(id).token = Some(t.clone());
                Some(t)
            }
            Err(e) => {
                tracing::warn!(error = %e, "no relay token for a pairing");
                None
            }
        }
    }

    // --- what the phone sends ---

    /// An `up` plaintext, from the relay or a direct frame.
    fn take_up(&mut self, id: &str, plaintext: &[u8], now: i64) {
        let Ok(msg) = serde_json::from_slice::<Value>(plaintext) else {
            return tracing::warn!("a phone message is not JSON");
        };
        let Some(seq) = msg["seq"].as_u64() else {
            return tracing::warn!("a phone message has no seq");
        };
        let Some(p) = self.pairing_mut(id) else {
            return;
        };
        if seq <= p.up_seq {
            return tracing::debug!(seq, "dropped a phone message not newer than the last");
        }
        p.up_seq = seq;
        match msg["type"].as_str() {
            Some("seen") => p.last_seen_ms = Some(now),
            // The phone removed the pairing; the Mac removes it at the relay too.
            Some("unpair") => return self.forget(id, true, now),
            _ => {}
        }
        self.save();
    }

    fn event(&mut self, e: &Value, now: i64) {
        let id = e["id"].as_str().unwrap_or_default();
        let connection = e["connection"].as_i64().unwrap_or(-1);
        match e["event"].as_str() {
            Some("open") => {
                self.connections.insert(connection, id.into());
            }
            Some("close") => {
                self.connections.remove(&connection);
            }
            Some("message") => {
                let plaintext = e["plaintext"].as_str().and_then(from_b64url);
                if let Some(plaintext) = plaintext
                    && self.pairing(id).is_some_and(|p| p.state == State::Paired)
                {
                    self.take_up(id, &plaintext, now);
                }
            }
            Some("listener-failed") => {
                let error = e["error"].as_str().unwrap_or_default();
                tracing::warn!(error, "the direct listener failed");
                self.error = Some(format!("Direct connection unavailable: {error}"));
                self.listener = None;
                self.listener_retry_ms = now + LISTENER_RETRY_MS;
            }
            _ => {}
        }
    }

    // --- direct ---

    /// Runs the listener while "Direct connection" is on and a paired phone has a state
    /// to send, so a phone's first frame is always a state.
    fn sync_listener(&mut self, now: i64) {
        let want: Vec<(String, String)> = if self.settings.phones().direct {
            self.pairings
                .iter()
                .filter(|p| p.state == State::Paired)
                .filter(|p| {
                    self.live
                        .get(&p.id)
                        .is_some_and(|l| l.state_message.is_some())
                })
                .filter_map(|p| Some((p.id.clone(), p.phone_key.clone()?)))
                .collect()
        } else {
            Vec::new()
        };
        let want = (!want.is_empty()).then_some(want);
        if want == self.listener || now < self.listener_retry_ms {
            return;
        }
        let pairings: Vec<Value> = want
            .iter()
            .flatten()
            .map(|(id, key)| json!({"id": id, "phoneKey": key}))
            .collect();
        let set = json!({"op": "listener.set", "pairings": pairings, "enabled": want.is_some()});
        if let Err(e) = self.bridge(&set) {
            // The bridge stopped the listener; try again in 30 s.
            tracing::warn!(error = %e, "cannot set the direct listener");
            self.error = Some(format!("Direct connection unavailable: {e}"));
            self.listener = None;
            self.listener_retry_ms = now + LISTENER_RETRY_MS;
            return;
        }
        let ids: Vec<String> = want.iter().flatten().map(|(id, _)| id.clone()).collect();
        self.connections.retain(|_, id| ids.contains(id));
        self.listener = want;
        // The latest state, for a phone's first frame.
        for id in ids {
            if let Some((_, m)) = self.live.get(&id).and_then(|l| l.state_message.clone()) {
                self.bridge(&json!({"op": "listener.send", "id": id, "plaintext": b64url(&m)}))
                    .ok();
            }
        }
    }

    fn send_direct(&mut self, id: &str, plaintext: &[u8], now: i64) {
        let send = json!({"op": "listener.send", "id": id, "plaintext": b64url(plaintext)});
        if let Err(e) = self.bridge(&send) {
            tracing::warn!(error = %e, "cannot send to a direct connection");
        }
        self.live_mut(id).direct_sent_ms = now;
    }

    /// A heartbeat every 30 s to directly connected phones, which close after 90 s
    /// silent. It repeats the latest state's seq, so it takes none of its own.
    fn direct_heartbeats(&mut self, now: i64) {
        let due: Vec<(String, u64)> = self
            .pairings
            .iter()
            .filter(|p| p.state == State::Paired && self.connected(&p.id))
            .filter_map(|p| {
                let live = self.live.get(&p.id)?;
                let (seq, _) = live.state_message.as_ref()?;
                (now >= live.direct_sent_ms + DIRECT_HEARTBEAT_MS).then(|| (p.id.clone(), *seq))
            })
            .collect();
        for (id, seq) in due {
            let beat = json!({"seq": seq, "type": "heartbeat", "sentAtMs": now});
            self.send_direct(&id, beat.to_string().as_bytes(), now);
        }
    }

    fn connected(&self, id: &str) -> bool {
        self.connections.values().any(|c| c == id)
    }

    /// This pairing's QR code is on screen, so its hello may come.
    fn shown(&self, id: &str) -> bool {
        self.pending.as_ref().is_some_and(|s| s.id == id)
    }

    // --- bookkeeping ---

    /// Deletes the pairing here and its keys, and at the relay when `at_relay`.
    fn forget(&mut self, id: &str, at_relay: bool, now: i64) {
        if at_relay {
            // Best effort: an unreachable relay drops it after 30 idle days.
            self.request(id, "DELETE", "", None, now);
        }
        if let Err(e) = self.bridge(&json!({"op": "pair.delete", "id": id})) {
            tracing::warn!(error = %e, "cannot delete a pairing's keys");
        }
        self.pairings.retain(|p| p.id != id);
        self.live.remove(id);
        self.connections.retain(|_, c| c != id);
        if self.pending.as_ref().is_some_and(|s| s.id == id) {
            self.pending = None;
        }
        self.save();
    }

    fn bridge(&self, request: &Value) -> Result<Value, String> {
        self.bridge
            .as_ref()
            .ok_or("Phones need the app bundle.")?
            .call(request)
    }

    fn pairing(&self, id: &str) -> Option<&Pairing> {
        self.pairings.iter().find(|p| p.id == id)
    }

    fn pairing_mut(&mut self, id: &str) -> Option<&mut Pairing> {
        self.pairings.iter_mut().find(|p| p.id == id)
    }

    fn live_mut(&mut self, id: &str) -> &mut Live {
        self.live.entry(id.into()).or_default()
    }

    /// True when saved; a failure is logged.
    fn save(&self) -> bool {
        let saved = store::save(&self.dir, &self.pairings);
        if let Err(e) = &saved {
            tracing::warn!(error = %e, "cannot save {}", store::FILE_NAME);
        }
        saved.is_ok()
    }

    /// Publishes the view and tells observers when it changed.
    fn refresh(&self) {
        let view = View {
            available: self.bridge.is_some(),
            phones: self
                .pairings
                .iter()
                .map(|p| Phone {
                    id: p.id.clone(),
                    name: p.phone_name.clone(),
                    state: p.state,
                    code: self.live.get(&p.id).and_then(|l| l.code.clone()),
                    last_seen_ms: p.last_seen_ms,
                    direct: self.connected(&p.id),
                })
                .collect(),
            pending: self.pending.clone(),
            error: self.error.clone(),
            relay_key: self.relay_key,
        };
        let paired = self.pairings.iter().any(|p| p.state == State::Paired);
        self.shared.publishing.store(paired, Ordering::Relaxed);
        let mut current = self.shared.view.lock().unwrap_or_else(|e| e.into_inner());
        if *current != view {
            *current = view;
            self.shared.changed.try_send(()).ok();
        }
    }
}
