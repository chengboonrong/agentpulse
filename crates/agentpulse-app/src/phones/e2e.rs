//! `--phone-e2e <relay>`, scripts/e2e-phones.sh's Mac side: the real bridge and worker
//! against a running relay and the phone simulator (`link/Sources/phone-sim`). Headless.
//! Prints `LINK <link>` and `CODE <code>` for the script, then `OK`, or `FAIL <why>`
//! and exits 1.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use super::{PhoneLink, State, View, now_ms};
use crate::settings::{Phones, Settings};
use crate::widget_export::placeholder;

const TIMEOUT: Duration = Duration::from_secs(90);

pub fn run(dir: &Path, relay: &str) -> ! {
    // Only ever the test's own data, keychain and bridge.
    let vars = [
        "AGENTPULSE_DATA_DIR",
        "AGENTPULSE_KEYCHAIN",
        "AGENTPULSE_LINK_BRIDGE",
    ];
    if let Some(v) = vars
        .into_iter()
        .find(|v| std::env::var_os(v).is_none_or(|x| x.is_empty()))
    {
        say(&format!("FAIL {v} is not set"));
        std::process::exit(1)
    }
    match drive(dir, relay) {
        Ok(()) => {
            say("OK");
            std::process::exit(0)
        }
        Err(e) => {
            say(&format!("FAIL {e}"));
            std::process::exit(1)
        }
    }
}

fn say(line: &str) {
    let mut out = std::io::stdout();
    writeln!(out, "{line}").and_then(|_| out.flush()).ok();
}

fn drive(dir: &Path, relay: &str) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    let settings = Settings::load(dir);
    settings.set_phones(Phones {
        relay: Some(relay.into()),
        direct: true,
    });
    let link = PhoneLink::spawn(dir, settings).map_err(|e| e.to_string())?;
    if let Some(key) = std::env::var("AGENTPULSE_E2E_RELAY_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        link.set_relay_key(Some(key));
        wait(&link, deadline, "the relay key", |v| {
            (v.relay_key == Some(true)).then_some(())
        })?;
    }
    let started = link
        .start_pairing()
        .recv_blocking()
        .map_err(|e| e.to_string())??;
    say(&format!("LINK {}", started.link));

    let phone = wait(&link, deadline, "the phone's hello", |v| {
        v.phones
            .iter()
            .find(|p| p.state == State::Confirming && p.code.is_some())
            .cloned()
    })?;
    say(&format!(
        "CODE {}",
        phone.code.as_deref().unwrap_or_default()
    ));
    link.confirm(&phone.id);
    let id = phone.id;
    wait(&link, deadline, "the confirmed pairing", |v| {
        v.phones
            .iter()
            .any(|p| p.id == id && p.state == State::Paired)
            .then_some(())
    })?;

    // Two states: the second goes out 10 s after the first.
    let mut payload = placeholder();
    payload.generated_at_ms = now_ms();
    link.publish(payload.clone());
    payload.agents.active += 1;
    link.publish(payload);

    wait(&link, deadline, "a seen phone connected directly", |v| {
        v.phones
            .iter()
            .any(|p| p.id == id && p.last_seen_ms.is_some() && p.direct)
            .then_some(())
    })?;
    wait(&link, deadline, "the phone's unpair", |v| {
        v.phones.is_empty().then_some(())
    })
}

/// Polls the view until `done` gives a value, or fails at the deadline.
fn wait<T>(
    link: &PhoneLink,
    deadline: Instant,
    what: &str,
    done: impl Fn(&View) -> Option<T>,
) -> Result<T, String> {
    loop {
        let v = link.view();
        if let Some(t) = done(&v) {
            return Ok(t);
        }
        if Instant::now() >= deadline {
            let error = v.error.map(|e| format!(" ({e})")).unwrap_or_default();
            return Err(format!("timed out waiting for {what}{error}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
