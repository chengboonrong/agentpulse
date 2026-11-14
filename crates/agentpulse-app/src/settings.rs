//! `settings.json` in the data directory: each agent's cat coat and eye colour,
//! the animation, the update check, and the phone link.
//!
//! ```json
//! {"cats": {"claude-code": {"coat": "calico", "eye": "blue"}, "codex": {"coat": "tuxedo", "eye": null}},
//!  "animation": {"enabled": true, "speed": 1.0},
//!  "updates": {"auto": true, "last_checked_ms": 0, "dismissed": "0.0.5"},
//!  "phones": {"relay": "https://relay.example", "direct": true}}
//! ```
//!
//! `eye: null` keeps the coat's own eyes. A missing file, section, entry or
//! field, or a null, means the default. With a warning, the default is also
//! used for: a section that is not an object; a cat entry that is not
//! `{"coat": <string>, "eye": <string or null>}`, names an unknown agent or an
//! unknown coat (that agent's default look); an unknown eye (the coat's own);
//! a field of the wrong type (that field); a speed outside 0.25 to 3 is clamped.
//! A file that is read whole but is not a JSON object is moved to
//! `settings.json.bad-<unix ms>`, never over an earlier one, and the defaults
//! are used. A file that cannot be read is left alone: the defaults are used
//! and nothing is saved over it.
//!
//! Every change first takes up what is on disk (another instance may have
//! written it), then writes back only what it changed: one agent's entry in
//! `cats`, or the whole `animation`, `updates` or `phones` section. Unknown
//! top-level keys and the other entries in `cats`, including agents this version
//! does not know, are kept; unknown fields inside `animation`, `updates` or
//! `phones` are dropped when that section is saved. A blank relay is null.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use agentpulse_animation::{Look, find_coat, find_eye};
use agentpulse_core::Provider;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const FILE_NAME: &str = "settings.json";
const PROVIDERS: [Provider; 3] = [Provider::ClaudeCode, Provider::Codex, Provider::OpenCode];

/// Each agent's look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Looks {
    claude_code: Look,
    codex: Look,
    open_code: Look,
}

impl Default for Looks {
    fn default() -> Self {
        Looks {
            claude_code: Look::default_for(Provider::ClaudeCode),
            codex: Look::default_for(Provider::Codex),
            open_code: Look::default_for(Provider::OpenCode),
        }
    }
}

impl Looks {
    pub fn get(&self, p: Provider) -> Look {
        match p {
            Provider::ClaudeCode => self.claude_code,
            Provider::Codex => self.codex,
            Provider::OpenCode => self.open_code,
        }
    }

    fn set(&mut self, p: Provider, look: Look) {
        match p {
            Provider::ClaudeCode => self.claude_code = look,
            Provider::Codex => self.codex = look,
            Provider::OpenCode => self.open_code = look,
        }
    }
}

/// The animation as chosen in Settings, before environment overrides and Reduce Motion.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Animation {
    pub enabled: bool,
    pub speed: f32,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            enabled: true,
            speed: 1.0,
        }
    }
}

/// The speeds Settings offers; any other stored speed is clamped to [`SPEED_RANGE`].
pub const SPEEDS: [f32; 4] = [0.5, 1.0, 1.5, 2.0];
const SPEED_RANGE: (f32, f32) = (0.25, 3.0);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Updates {
    pub auto: bool,
    /// When the last check finished, successful or not; 0 means never.
    pub last_checked_ms: i64,
    /// The release whose banner was dismissed.
    pub dismissed: Option<String>,
}

impl Default for Updates {
    fn default() -> Self {
        Updates {
            auto: true,
            last_checked_ms: 0,
            dismissed: None,
        }
    }
}

/// The phone link (docs/mobile.md, "Mac client").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Phones {
    /// The relay's base URL; none until the user sets one.
    pub relay: Option<String>,
    /// The direct connection on the same network.
    pub direct: bool,
}

impl Default for Phones {
    fn default() -> Self {
        Phones {
            relay: None,
            direct: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct State {
    looks: Looks,
    animation: Animation,
    updates: Updates,
    phones: Phones,
}

/// What one change touches; only that part of the file is rewritten.
#[derive(Clone, Copy)]
enum Part {
    Cat(Provider),
    Animation,
    Updates,
    Phones,
}

#[derive(Serialize, Deserialize)]
struct Choice {
    coat: String,
    #[serde(default)]
    eye: Option<String>,
}

impl Choice {
    fn of(l: Look) -> Value {
        let c = Choice {
            coat: l.coat.id.into(),
            eye: l.eye.map(|e| e.id.into()),
        };
        serde_json::to_value(c).expect("plain strings serialize")
    }
}

/// The file's object and the settings in it, or None when it is not a JSON
/// object. A bad section, entry or value inside is warned about and replaced
/// by its default.
fn parse(bytes: &[u8]) -> Option<(Map<String, Value>, State)> {
    let map: Map<String, Value> = match serde_json::from_slice(bytes) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "corrupt {FILE_NAME}");
            return None;
        }
    };
    let section = |name: &str| {
        let v = map.get(name).filter(|v| !v.is_null())?;
        if !v.is_object() {
            tracing::warn!(value = %v, "bad {name} in {FILE_NAME}, using the defaults");
        }
        v.as_object()
    };
    let (cats, animation, updates) = (section("cats"), section("animation"), section("updates"));
    let phones = section("phones");
    let (a, u, p) = (Animation::default(), Updates::default(), Phones::default());
    let speed = field(animation, "animation", "speed", Value::as_f64).map_or(a.speed, |v| v as f32);
    let speed = if speed.is_finite() {
        speed.clamp(SPEED_RANGE.0, SPEED_RANGE.1)
    } else {
        a.speed
    };
    let state = State {
        looks: cats.map(parse_cats).unwrap_or_default(),
        animation: Animation {
            enabled: field(animation, "animation", "enabled", Value::as_bool).unwrap_or(a.enabled),
            speed,
        },
        updates: Updates {
            auto: field(updates, "updates", "auto", Value::as_bool).unwrap_or(u.auto),
            last_checked_ms: field(updates, "updates", "last_checked_ms", Value::as_i64)
                .unwrap_or(u.last_checked_ms),
            dismissed: field(updates, "updates", "dismissed", |v| {
                v.as_str().map(str::to_owned)
            }),
        },
        phones: Phones {
            relay: field(phones, "phones", "relay", |v| v.as_str().map(str::to_owned))
                .and_then(blank_is_none),
            direct: field(phones, "phones", "direct", Value::as_bool).unwrap_or(p.direct),
        },
    };
    Some((map, state))
}

fn blank_is_none(s: String) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// `section.name`, if present, null or of the right type; a wrong type is warned about.
fn field<T>(
    section: Option<&Map<String, Value>>,
    s: &str,
    name: &str,
    get: impl Fn(&Value) -> Option<T>,
) -> Option<T> {
    let v = section?.get(name).filter(|v| !v.is_null())?;
    let got = get(v);
    if got.is_none() {
        tracing::warn!(value = %v, "bad {s}.{name} in {FILE_NAME}, using the default");
    }
    got
}

/// Each entry on its own: a bad one is warned about and that agent keeps its default.
fn parse_cats(cats: &Map<String, Value>) -> Looks {
    let mut looks = Looks::default();
    for (key, value) in cats {
        let Some(p) = PROVIDERS.into_iter().find(|p| p.as_str() == key) else {
            tracing::warn!(agent = %key, "unknown agent in {FILE_NAME}, ignored");
            continue;
        };
        let choice: Choice = match serde_json::from_value(value.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, agent = %key, "bad cat in {FILE_NAME}, using the default");
                continue;
            }
        };
        let Some(coat) = find_coat(&choice.coat) else {
            tracing::warn!(coat = %choice.coat, agent = %key, "unknown coat, using the default");
            continue;
        };
        let eye = choice.eye.as_deref().and_then(|id| {
            let eye = find_eye(id);
            if eye.is_none() {
                tracing::warn!(eye = %id, agent = %key, "unknown eye colour, using the coat's own");
            }
            eye
        });
        looks.set(p, Look { coat, eye });
    }
    looks
}

/// Every section as written to the file.
fn sections(state: &State) -> Map<String, Value> {
    let cats = PROVIDERS
        .into_iter()
        .map(|p| (p.as_str().to_string(), Choice::of(state.looks.get(p))))
        .collect();
    Map::from_iter([
        ("cats".into(), Value::Object(cats)),
        (
            "animation".into(),
            serde_json::to_value(state.animation).expect("plain values"),
        ),
        (
            "updates".into(),
            serde_json::to_value(&state.updates).expect("plain values"),
        ),
        (
            "phones".into(),
            serde_json::to_value(&state.phones).expect("plain values"),
        ),
    ])
}

/// The settings, shared by the window, the hub and the widget exporter, and their file.
#[derive(Clone)]
pub struct Settings {
    path: PathBuf,
    state: Arc<RwLock<State>>,
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let state = match on_disk(&path) {
            Ok(found) => found.map(|(_, s)| s).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "cannot read settings, using defaults");
                State::default()
            }
        };
        Settings {
            path,
            state: Arc::new(RwLock::new(state)),
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn looks(&self) -> Looks {
        self.read().looks
    }

    pub fn animation(&self) -> Animation {
        self.read().animation
    }

    pub fn updates(&self) -> Updates {
        self.read().updates.clone()
    }

    pub fn phones(&self) -> Phones {
        self.read().phones.clone()
    }

    pub fn set(&self, p: Provider, look: Look) {
        self.change(Part::Cat(p), |s| s.looks.set(p, look));
    }

    pub fn set_animation(&self, a: Animation) {
        self.change(Part::Animation, |s| s.animation = a);
    }

    pub fn set_updates(&self, u: Updates) {
        self.change(Part::Updates, |s| s.updates = u);
    }

    // The Settings card (step 2b) sets it.
    #[allow(dead_code)]
    pub fn set_phones(&self, p: Phones) {
        let p = Phones {
            relay: p.relay.and_then(blank_is_none),
            ..p
        };
        self.change(Part::Phones, |s| s.phones = p);
    }

    /// Applies at once and saves the part it changed; a failed save is warned
    /// about and the change still holds until the app quits.
    fn change(&self, part: Part, f: impl FnOnce(&mut State)) {
        // Saved under the lock, so saves land in the order of the changes.
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = self.save(part, &mut state, f) {
            tracing::warn!(error = %e, path = %self.path.display(), "cannot save settings");
        }
    }

    /// Takes up what is on disk (another instance may have written it), applies
    /// `f`, and writes back only `part`, keeping every other key. With no usable
    /// file, writes all of this process's sections; a file that cannot be read
    /// is left alone and nothing is written. Through a temp file and rename, so
    /// a crash never leaves half a file.
    fn save(
        &self,
        part: Part,
        state: &mut State,
        f: impl FnOnce(&mut State),
    ) -> std::io::Result<()> {
        let disk = match on_disk(&self.path) {
            Ok(d) => d,
            Err(e) => {
                f(state);
                return Err(e);
            }
        };
        let mut file = match disk {
            Some((map, on_disk)) => {
                *state = on_disk;
                map
            }
            None => Map::new(),
        };
        f(state);
        if file.is_empty() {
            file = sections(state);
        }
        match part {
            Part::Cat(p) => {
                let mut cats = file
                    .get("cats")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                cats.insert(p.as_str().into(), Choice::of(state.looks.get(p)));
                file.insert("cats".into(), Value::Object(cats));
            }
            Part::Animation | Part::Updates | Part::Phones => {
                let name = match part {
                    Part::Animation => "animation",
                    Part::Updates => "updates",
                    _ => "phones",
                };
                file.insert(name.into(), sections(state)[name].clone());
            }
        }
        let text = serde_json::to_string_pretty(&file).expect("plain values serialize");
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Per process: two instances sharing the directory never share a temp file.
        let tmp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        let mut f = fs::File::create(&tmp)?;
        f.write_all((text + "\n").as_bytes())?;
        // On disk before the rename makes it the settings.
        f.sync_all()?;
        fs::rename(&tmp, &self.path)
    }
}

/// The file's object and settings, None when there is no file. A file read
/// whole whose content is not a JSON object is moved aside to
/// `settings.json.bad-<unix ms>` and is None; any other read error is returned
/// and the file left alone.
fn on_disk(path: &Path) -> std::io::Result<Option<(Map<String, Value>, State)>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let parsed = parse(&bytes);
    if parsed.is_none() {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        // Never over an earlier backup.
        let bad = (ms..)
            .map(|ms| path.with_extension(format!("json.bad-{ms}")))
            .find(|p| !p.exists())
            .expect("an unused name");
        match fs::rename(path, &bad) {
            Ok(()) => {
                tracing::warn!(kept = %bad.display(), "corrupt settings moved aside, using defaults")
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot move corrupt settings aside");
                // Never write over a file that could not be kept.
                return Err(e);
            }
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str) -> State {
        parse(text.as_bytes()).expect("a settings object").1
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentpulse-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_is_the_defaults_and_choices_round_trip() {
        let dir = temp_dir("settings");
        let s = Settings::load(&dir);
        assert_eq!(s.looks(), Looks::default());
        let calico = Look {
            coat: find_coat("calico").unwrap(),
            eye: find_eye("blue"),
        };
        s.set(Provider::ClaudeCode, calico);
        assert_eq!(s.looks().get(Provider::ClaudeCode), calico);
        // The temp file was renamed into place, not left behind.
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE_NAME]);
        let again = Settings::load(&dir).looks();
        assert_eq!(again.get(Provider::ClaudeCode), calico);
        assert_eq!(
            again.get(Provider::Codex),
            Look::default_for(Provider::Codex)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bad_input_falls_back_without_failing() {
        for text in ["", "{", "[]", "3", "\u{0}"] {
            assert!(parse(text.as_bytes()).is_none(), "{text:?}");
        }
        assert!(parse(b"\xff\xfe").is_none());
        let looks = state(
            r#"{"cats": {
                "claude-code": {"coat": "no-such-coat", "eye": "blue"},
                "codex": {"coat": "tuxedo", "eye": "no-such-eye"},
                "opencode": {"coat": "van"},
                "someone-else": {"coat": "calico"}
            }, "future": true}"#,
        )
        .looks;
        assert_eq!(
            looks.get(Provider::ClaudeCode),
            Look::default_for(Provider::ClaudeCode)
        );
        let codex = looks.get(Provider::Codex);
        assert_eq!((codex.coat.id, codex.eye), ("tuxedo", None));
        assert_eq!(looks.get(Provider::OpenCode).coat.id, "van");
    }

    #[test]
    fn each_agent_keeps_its_own_look() {
        let mut looks = Looks::default();
        let coats = ["calico", "tuxedo", "van"];
        for (p, id) in PROVIDERS.into_iter().zip(coats) {
            looks.set(
                p,
                Look {
                    coat: find_coat(id).unwrap(),
                    eye: None,
                },
            );
        }
        for (p, id) in PROVIDERS.into_iter().zip(coats) {
            assert_eq!(looks.get(p).coat.id, id, "{p:?}");
        }
        for p in PROVIDERS {
            assert_eq!(Looks::default().get(p), Look::default_for(p), "{p:?}");
        }
    }

    #[test]
    fn written_file_names_every_agent() {
        let v = Value::Object(sections(&State::default()));
        assert_eq!(v["cats"]["claude-code"]["coat"], "ginger-tabby");
        assert_eq!(v["cats"]["codex"]["coat"], "codex-night");
        assert_eq!(v["cats"]["opencode"]["eye"], Value::Null);
        assert_eq!(state(&v.to_string()), State::default());
    }

    #[test]
    fn a_cats_only_file_loads_with_default_animation_and_updates() {
        let s = state(r#"{"cats": {"codex": {"coat": "tuxedo", "eye": null}}}"#);
        assert_eq!(s.looks.get(Provider::Codex).coat.id, "tuxedo");
        assert_eq!(s.animation, Animation::default());
        assert_eq!(s.updates, Updates::default());
        assert_eq!(s.phones, Phones::default());
        assert_eq!(s.phones.relay, None);
        assert!(s.phones.direct);
    }

    #[test]
    fn every_section_round_trips() {
        let dir = temp_dir("settings-sections");
        let s = Settings::load(&dir);
        let animation = Animation {
            enabled: false,
            speed: 1.5,
        };
        let updates = Updates {
            auto: false,
            last_checked_ms: 1_763_000_000_000,
            dismissed: Some("0.0.5".into()),
        };
        let phones = Phones {
            relay: Some("https://relay.example".into()),
            direct: false,
        };
        s.set_animation(animation);
        s.set_updates(updates.clone());
        s.set_phones(phones.clone());
        let again = Settings::load(&dir);
        assert_eq!(again.animation(), animation);
        assert_eq!(again.updates(), updates);
        assert_eq!(again.phones(), phones);
        assert_eq!(again.looks(), Looks::default());
        // A blank relay is stored as null.
        s.set_phones(Phones {
            relay: Some("  ".into()),
            direct: true,
        });
        let v: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(
            v["phones"],
            serde_json::json!({"relay": null, "direct": true})
        );
        assert_eq!(Settings::load(&dir).phones(), Phones::default());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bad_values_fall_back_per_field() {
        let speed = |v: &str| {
            state(&format!(r#"{{"animation": {{"speed": {v}}}}}"#))
                .animation
                .speed
        };
        assert_eq!(speed("1.5"), 1.5);
        assert_eq!(speed("0.75"), 0.75);
        assert_eq!(speed("10"), 3.0);
        assert_eq!(speed("0"), 0.25);
        assert_eq!(speed("-2"), 0.25);
        assert_eq!(speed("\"fast\""), 1.0);
        let s = state(
            r#"{"animation": {"enabled": "no", "speed": 2},
                "updates": {"auto": false, "last_checked_ms": "soon", "dismissed": 5}}"#,
        );
        assert_eq!(
            s.animation,
            Animation {
                enabled: true,
                speed: 2.0
            }
        );
        assert_eq!(
            s.updates,
            Updates {
                auto: false,
                ..Updates::default()
            }
        );
        assert_eq!(
            state(r#"{"cats": 3, "animation": 3, "updates": null, "phones": []}"#),
            State::default()
        );
        let p = state(r#"{"phones": {"relay": 5, "direct": "yes"}}"#).phones;
        assert_eq!(p, Phones::default());
        let p = state(r#"{"phones": {"relay": " https://r.example ", "direct": false}}"#).phones;
        assert_eq!(p.relay.as_deref(), Some("https://r.example"));
        assert!(!p.direct);
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("settings.json.bad-"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_not_lost() {
        let dir = temp_dir("settings-corrupt");
        fs::create_dir_all(&dir).unwrap();
        let corrupt = "{\"cats\": {\"codex\": ";
        fs::write(dir.join(FILE_NAME), corrupt).unwrap();
        fs::write(dir.join("settings.json.bad-1"), "older").unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.looks(), Looks::default());
        assert!(!dir.join(FILE_NAME).exists());
        let kept = backups(&dir);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!(fs::read_to_string(dir.join(&kept[0])).unwrap(), "older");
        assert_eq!(fs::read_to_string(dir.join(&kept[1])).unwrap(), corrupt);
        // Corrupted again while running: the save keeps that one too.
        fs::write(dir.join(FILE_NAME), "[1]").unwrap();
        s.set_animation(Animation::default());
        let kept = backups(&dir);
        assert_eq!(kept.len(), 3, "{kept:?}");
        let texts: Vec<String> = kept
            .iter()
            .map(|n| fs::read_to_string(dir.join(n)).unwrap())
            .collect();
        assert!(texts.contains(&"[1]".to_string()), "{texts:?}");
        assert_eq!(Settings::load(&dir).animation(), Animation::default());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_file_is_left_alone() {
        let dir = temp_dir("settings-unreadable");
        // A directory where the file should be: reading it fails with
        // "Is a directory", not NotFound.
        fs::create_dir_all(dir.join(FILE_NAME)).unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.animation(), Animation::default());
        let off = Animation {
            enabled: false,
            ..Animation::default()
        };
        s.set_animation(off);
        // The change holds in memory; nothing was written or moved.
        assert_eq!(s.animation(), off);
        assert!(dir.join(FILE_NAME).is_dir());
        assert!(backups(&dir).is_empty());
        let names: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(names.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_instances_keep_each_others_cats() {
        let dir = temp_dir("settings-two-cats");
        let (a, b) = (Settings::load(&dir), Settings::load(&dir));
        let look = |id| Look {
            coat: find_coat(id).unwrap(),
            eye: None,
        };
        a.set(Provider::Codex, look("tuxedo"));
        b.set(Provider::ClaudeCode, look("calico"));
        let on_disk = Settings::load(&dir).looks();
        assert_eq!(on_disk.get(Provider::Codex), look("tuxedo"));
        assert_eq!(on_disk.get(Provider::ClaudeCode), look("calico"));
        // `b` took up `a`'s choice when it saved.
        assert_eq!(b.looks().get(Provider::Codex), look("tuxedo"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_cat_entry_spoils_only_itself_and_stays_on_disk() {
        let dir = temp_dir("settings-bad-cat");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(FILE_NAME),
            r#"{"cats": {"codex": {"coat": null}, "opencode": {"eye": "blue"},
                "claude-code": {"coat": "van"}, "future-agent": {"coat": "calico"}}}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        let looks = s.looks();
        assert_eq!(looks.get(Provider::ClaudeCode).coat.id, "van");
        assert_eq!(
            looks.get(Provider::Codex),
            Look::default_for(Provider::Codex)
        );
        assert_eq!(
            looks.get(Provider::OpenCode),
            Look::default_for(Provider::OpenCode)
        );
        s.set(
            Provider::ClaudeCode,
            Look {
                coat: find_coat("calico").unwrap(),
                eye: None,
            },
        );
        let v: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(v["cats"]["claude-code"]["coat"], "calico");
        assert_eq!(v["cats"]["codex"], serde_json::json!({"coat": null}));
        assert_eq!(v["cats"]["opencode"], serde_json::json!({"eye": "blue"}));
        assert_eq!(v["cats"]["future-agent"]["coat"], "calico");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_keeps_other_instances_sections_and_unknown_keys() {
        let dir = temp_dir("settings-shared");
        let (a, b) = (Settings::load(&dir), Settings::load(&dir));
        let calico = Look {
            coat: find_coat("calico").unwrap(),
            eye: None,
        };
        b.set(Provider::Codex, calico);
        let mut text: Map<String, Value> =
            serde_json::from_str(&fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        text.insert("future".into(), serde_json::json!({"x": 1}));
        fs::write(dir.join(FILE_NAME), Value::Object(text).to_string()).unwrap();
        // `a` still holds the default cats in memory.
        a.set_updates(Updates {
            last_checked_ms: 5,
            ..Updates::default()
        });
        let v: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(v["future"]["x"], 1);
        assert_eq!(v["updates"]["last_checked_ms"], 5);
        assert_eq!(Settings::load(&dir).looks().get(Provider::Codex), calico);
        fs::remove_dir_all(&dir).unwrap();
    }
}
