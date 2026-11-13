//! `phones.json` in the data directory: the pairings, without secrets (those are in the
//! Keychain, held by the bridge).
//!
//! ```json
//! {"pairings": [{"id": "…", "relay": "https://…", "phoneName": "…", "phoneKey": "…",
//!   "state": "paired", "createdAtMs": 0, "lastSeenMs": 0, "downSeq": 3, "upSeq": 1}]}
//! ```
//!
//! `downSeq` is the next `down` seq to send, `upSeq` the last `up` seq accepted (the
//! hello is 0). A file that is not this is moved to `phones.json.bad-<unix ms>` and
//! no pairings are used.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "phones.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The QR code is out; waiting for the phone's hello.
    Waiting,
    /// The hello arrived; the user compares codes.
    Confirming,
    Paired,
    /// The relay no longer knows the pairing; the user can remove it.
    Gone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub id: String,
    pub relay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_key: Option<String>,
    pub state: State,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<i64>,
    #[serde(default)]
    pub down_seq: u64,
    #[serde(default)]
    pub up_seq: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    pairings: Vec<Pairing>,
}

/// The pairings in `dir`; none when there is no file, or it cannot be read or parsed.
pub fn load(dir: &Path) -> Vec<Pairing> {
    let path = dir.join(FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read {FILE_NAME}");
            return Vec::new();
        }
    };
    match serde_json::from_slice::<File>(&bytes) {
        Ok(f) => f.pairings,
        Err(e) => {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            let bad = (ms..)
                .map(|ms| path.with_extension(format!("json.bad-{ms}")))
                .find(|p| !p.exists())
                .expect("an unused name");
            tracing::warn!(error = %e, kept = %bad.display(), "corrupt {FILE_NAME} moved aside");
            if let Err(e) = fs::rename(&path, &bad) {
                tracing::warn!(error = %e, "cannot move {FILE_NAME} aside");
            }
            Vec::new()
        }
    }
}

/// Replaces `dir/phones.json` (mode 0600) through a temp file and rename.
pub fn save(dir: &Path, pairings: &[Pairing]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let file = File {
        pairings: pairings.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file).expect("plain values serialize");
    let tmp = dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all((text + "\n").as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, dir.join(FILE_NAME))
}
