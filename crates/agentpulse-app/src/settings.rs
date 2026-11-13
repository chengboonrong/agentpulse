//! `settings.json` in the data directory: each agent's cat coat and eye colour.
//!
//! ```json
//! {"cats": {"claude-code": {"coat": "calico", "eye": "blue"}, "codex": {"coat": "tuxedo", "eye": null}}}
//! ```
//!
//! `eye: null` keeps the coat's own eyes. A missing file means the defaults; an
//! unreadable or corrupt file, or an unknown coat or eye, falls back to the
//! default for what it affects, with a warning.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use agentpulse_animation::{Look, find_coat, find_eye};
use agentpulse_core::Provider;
use serde::{Deserialize, Serialize};

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

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    cats: BTreeMap<String, Choice>,
}

#[derive(Serialize, Deserialize)]
struct Choice {
    coat: String,
    #[serde(default)]
    eye: Option<String>,
}

/// Reads the file's text; every problem is warned about and replaced by a default.
fn parse(text: &str) -> Looks {
    let mut looks = Looks::default();
    let file: File = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "corrupt {FILE_NAME}, using default cats");
            return looks;
        }
    };
    for (key, choice) in file.cats {
        let Some(p) = PROVIDERS.into_iter().find(|p| p.as_str() == key) else {
            tracing::warn!(agent = %key, "unknown agent in {FILE_NAME}, ignored");
            continue;
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

fn to_json(looks: &Looks) -> String {
    let cats = PROVIDERS
        .into_iter()
        .map(|p| {
            let l = looks.get(p);
            let choice = Choice {
                coat: l.coat.id.into(),
                eye: l.eye.map(|e| e.id.into()),
            };
            (p.as_str().to_string(), choice)
        })
        .collect();
    serde_json::to_string_pretty(&File { cats }).expect("plain strings serialize")
}

/// The looks, shared by the window and the widget exporter, and their file.
#[derive(Clone)]
pub struct CatSettings {
    path: PathBuf,
    looks: Arc<RwLock<Looks>>,
}

impl CatSettings {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let looks = match fs::read_to_string(&path) {
            Ok(text) => parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Looks::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "cannot read settings, using default cats");
                Looks::default()
            }
        };
        CatSettings {
            path,
            looks: Arc::new(RwLock::new(looks)),
        }
    }

    pub fn looks(&self) -> Looks {
        *self.looks.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Applies at once and saves; a failed save is warned about and the choice
    /// still holds until the app quits.
    pub fn set(&self, p: Provider, look: Look) {
        let looks = {
            let mut l = self.looks.write().unwrap_or_else(|e| e.into_inner());
            l.set(p, look);
            *l
        };
        if let Err(e) = self.save(&looks) {
            tracing::warn!(error = %e, path = %self.path.display(), "cannot save settings");
        }
    }

    /// Through a temp file and rename, so a crash never leaves half a file.
    fn save(&self, looks: &Looks) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Per process: two instances sharing the directory never share a temp file.
        let tmp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        let mut f = fs::File::create(&tmp)?;
        f.write_all((to_json(looks) + "\n").as_bytes())?;
        // On disk before the rename makes it the settings.
        f.sync_all()?;
        fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentpulse-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_is_the_defaults_and_choices_round_trip() {
        let dir = temp_dir("settings");
        let s = CatSettings::load(&dir);
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
        let again = CatSettings::load(&dir).looks();
        assert_eq!(again.get(Provider::ClaudeCode), calico);
        assert_eq!(
            again.get(Provider::Codex),
            Look::default_for(Provider::Codex)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bad_input_falls_back_without_failing() {
        for text in ["", "{", "[]", "{\"cats\": 3}", "\u{0}"] {
            assert_eq!(parse(text), Looks::default(), "{text:?}");
        }
        let looks = parse(
            r#"{"cats": {
                "claude-code": {"coat": "no-such-coat", "eye": "blue"},
                "codex": {"coat": "tuxedo", "eye": "no-such-eye"},
                "opencode": {"coat": "van"},
                "someone-else": {"coat": "calico"}
            }, "future": true}"#,
        );
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
        let v: serde_json::Value = serde_json::from_str(&to_json(&Looks::default())).unwrap();
        assert_eq!(v["cats"]["claude-code"]["coat"], "ginger-tabby");
        assert_eq!(v["cats"]["codex"]["coat"], "codex-night");
        assert_eq!(v["cats"]["opencode"]["eye"], serde_json::Value::Null);
        assert_eq!(parse(&to_json(&Looks::default())), Looks::default());
    }
}
