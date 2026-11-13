//! Estimated USD cost of one API response, from a price table that is data
//! (`pricing.toml`), not logic.

use std::collections::HashMap;
use std::path::Path;

use agentpulse_core::UsageEvent;
use anyhow::Context;
use serde::Deserialize;

const BUILTIN: &str = include_str!("../pricing.toml");

/// USD per million tokens.
#[derive(Clone, Copy, Debug, Deserialize)]
struct Rates {
    input: f64,
    output: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
    cache_read: f64,
}

#[derive(Debug, Deserialize)]
struct LongContext {
    threshold: u64,
    #[serde(flatten)]
    rates: Rates,
}

#[derive(Debug, Deserialize)]
struct Model {
    id: String,
    #[serde(flatten)]
    rates: Rates,
    long_context: Option<LongContext>,
}

#[derive(Debug, Deserialize)]
struct File {
    fast_multiplier: f64,
    us_inference_multiplier: f64,
    model: Vec<Model>,
}

#[derive(Debug)]
pub struct PriceBook {
    fast_multiplier: f64,
    us_inference_multiplier: f64,
    models: HashMap<String, Model>,
}

impl PriceBook {
    pub fn builtin() -> Self {
        Self::from_toml(BUILTIN).expect("embedded pricing.toml is valid")
    }

    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        let file: File = toml::from_str(text).context("invalid price table")?;
        Ok(Self {
            fast_multiplier: file.fast_multiplier,
            us_inference_multiplier: file.us_inference_multiplier,
            models: file.model.into_iter().map(|m| (m.id.clone(), m)).collect(),
        })
    }

    /// `path` exists → that file replaces the built-in table; otherwise built-in.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::builtin());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_toml(&text).with_context(|| format!("in {}", path.display()))
    }

    /// Exact id, or an id followed by `-` or `@` and an 8-digit date snapshot.
    /// Never a prefix match: `claude-opus-4-9` is not `claude-opus-4`.
    fn lookup(&self, model: &str) -> Option<&Model> {
        if let Some(m) = self.models.get(model) {
            return Some(m);
        }
        let (base, date) = model.split_at_checked(model.len().checked_sub(8)?)?;
        let base = base.strip_suffix(['-', '@'])?;
        if !date.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        self.models.get(base)
    }

    /// Estimated USD for one response, or None when the model or its service
    /// tier has no price.
    pub fn cost(&self, event: &UsageEvent) -> Option<f64> {
        if event.unknown_tier {
            return None;
        }
        let model = self.lookup(&event.model)?;
        let u = &event.usage;
        let r = match &model.long_context {
            Some(lc) if u.prompt() > lc.threshold => &lc.rates,
            _ => &model.rates,
        };
        let mut usd = (u.input as f64 * r.input
            + u.output as f64 * r.output
            + u.cache_write_5m as f64 * r.cache_write_5m
            + u.cache_write_1h as f64 * r.cache_write_1h
            + u.cache_read as f64 * r.cache_read)
            / 1e6;
        if event.fast {
            usd *= self.fast_multiplier;
        }
        if event.us_inference {
            usd *= self.us_inference_multiplier;
        }
        Some(usd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpulse_core::TokenUsage;

    fn ev(model: &str, usage: TokenUsage) -> UsageEvent {
        UsageEvent {
            event_id: "e".into(),
            session_id: "s".into(),
            model: model.into(),
            ts_ms: 0,
            usage,
            fast: false,
            us_inference: false,
            unknown_tier: false,
        }
    }

    fn input(n: u64) -> TokenUsage {
        TokenUsage {
            input: n,
            ..Default::default()
        }
    }

    fn close(a: Option<f64>, b: f64) {
        let a = a.expect("priced");
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn reproduces_claude_code_recorded_costs() {
        let book = PriceBook::builtin();
        let usage = TokenUsage {
            input: 16,
            output: 1802,
            cache_read: 589_366,
            cache_write_1h: 109_774,
            cache_write_5m: 0,
        };
        close(book.cost(&ev("claude-sonnet-5-5", usage)), 0.5160846);
        let usage = TokenUsage {
            input: 1177,
            output: 16,
            ..Default::default()
        };
        close(book.cost(&ev("claude-haiku-5-5", usage)), 0.0001257);
    }

    #[test]
    fn long_context_switches_exactly_above_threshold() {
        let book = PriceBook::builtin();
        close(
            book.cost(&ev("claude-haiku-5-5", input(100_000))),
            100_000.0 * 0.10 / 1e6,
        );
        close(
            book.cost(&ev("claude-haiku-5-5", input(100_001))),
            100_001.0 * 0.50 / 1e6,
        );
        // Output tokens are not part of the prompt.
        let usage = TokenUsage {
            input: 100_000,
            output: 10,
            ..Default::default()
        };
        close(
            book.cost(&ev("claude-haiku-5-5", usage)),
            (100_000.0 * 0.10 + 10.0 * 0.50) / 1e6,
        );
        // Models without a tier never switch.
        close(
            book.cost(&ev("claude-sonnet-5-5", input(500_000))),
            500_000.0 * 2.0 / 1e6,
        );
    }

    #[test]
    fn fast_and_us_multipliers() {
        let book = PriceBook::builtin();
        let base = 1_000_000.0 * 2.0 / 1e6;
        let mut e = ev("claude-sonnet-5-5", input(1_000_000));
        close(book.cost(&e), base);
        e.fast = true;
        close(book.cost(&e), base * 2.0);
        e.fast = false;
        e.us_inference = true;
        close(book.cost(&e), base * 1.1);
        e.fast = true;
        close(book.cost(&e), base * 2.0 * 1.1);
    }

    /// A Codex response as the adapter maps it: `input` excludes cached
    /// input, so the prompt is input + cache_read, the request's input_tokens.
    fn codex(model: &str, input_tokens: u64, cached: u64, output: u64) -> UsageEvent {
        ev(
            model,
            TokenUsage {
                input: input_tokens - cached,
                cache_read: cached,
                output,
                ..Default::default()
            },
        )
    }

    #[test]
    fn codex_long_context_starts_above_272k_prompt_tokens() {
        let book = PriceBook::builtin();
        // 272,000 prompt tokens, 200,000 of them cached: standard rates.
        close(
            book.cost(&codex("gpt-6.1-sol", 272_000, 200_000, 1_000)),
            (72_000.0 * 2.0 + 200_000.0 * 0.10 + 1_000.0 * 10.0) / 1e6,
        );
        // One more prompt token, cached or not, switches every bucket.
        close(
            book.cost(&codex("gpt-6.1-sol", 272_001, 200_001, 1_000)),
            (72_000.0 * 4.0 + 200_001.0 * 0.20 + 1_000.0 * 15.0) / 1e6,
        );
        // Output never counts toward the threshold.
        close(
            book.cost(&codex("gpt-6-astra", 272_000, 0, 500_000)),
            (272_000.0 * 10.0 + 500_000.0 * 50.0) / 1e6,
        );
        close(
            book.cost(&codex("gpt-6-luna", 1_000_000, 0, 0)),
            1_000_000.0 * 0.20 / 1e6,
        );
    }

    #[test]
    fn codex_fast_tier_doubles_and_unknown_tier_is_unpriced() {
        let book = PriceBook::builtin();
        let mut e = codex("gpt-6.1-terra", 100_000, 40_000, 2_000);
        let base = (60_000.0 * 1.0 + 40_000.0 * 0.05 + 2_000.0 * 5.0) / 1e6;
        close(book.cost(&e), base);
        e.fast = true;
        close(book.cost(&e), base * 2.0);
        e.fast = false;
        e.unknown_tier = true;
        assert_eq!(book.cost(&e), None);
    }

    #[test]
    fn date_suffix_matches_only_exact_snapshots() {
        let book = PriceBook::builtin();
        close(
            book.cost(&ev("claude-haiku-4-5-20251001", input(1_000_000))),
            1.0,
        );
        close(
            book.cost(&ev("claude-opus-4-5@20251101", input(1_000_000))),
            5.0,
        );
        close(book.cost(&ev("claude-opus-4-5", input(1_000_000))), 5.0);
        assert_eq!(book.cost(&ev("claude-opus-4-9", input(1))), None);
        assert_eq!(book.cost(&ev("claude-opus-4-5-2025110", input(1))), None);
        assert_eq!(book.cost(&ev("claude-opus-4-5-2025110x", input(1))), None);
        assert_eq!(book.cost(&ev("claude-opus-4-5_20251101", input(1))), None);
        assert_eq!(book.cost(&ev("gpt-5", input(1))), None);
        assert_eq!(book.cost(&ev("<synthetic>", input(1))), None);
        assert_eq!(book.cost(&ev("é", input(1))), None);
    }

    #[test]
    fn builtin_contains_every_model() {
        let book = PriceBook::builtin();
        for id in [
            "claude-fable-5-2",
            "claude-mythos-5-2",
            "claude-fable-5-1",
            "claude-mythos-5-1",
            "claude-fable-5",
            "claude-mythos-5",
            "claude-opus-5-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-opus-4-5",
            "claude-opus-4-1",
            "claude-opus-4",
            "claude-sonnet-5-5",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5",
            "claude-sonnet-4",
            "claude-haiku-5-5",
            "claude-haiku-4-5",
            "gpt-6-astra",
            "gpt-6.1-sol",
            "gpt-6.1-terra",
            "gpt-6-luna",
            "gpt-6-sol",
        ] {
            assert!(book.models.contains_key(id), "missing {id}");
        }
        assert_eq!(book.models.len(), 26);
    }

    #[test]
    fn load_missing_path_is_builtin() {
        let dir = std::env::temp_dir().join("agentpulse-cost-missing");
        let book = PriceBook::load(&dir.join("nope.toml")).unwrap();
        assert_eq!(book.models.len(), 26);
    }

    const ONE_MODEL: &str = r#"
fast_multiplier = 3.0
us_inference_multiplier = 1.0
[[model]]
id = "x"
input = 1.0
output = 1.0
cache_write_5m = 1.0
cache_write_1h = 1.0
cache_read = 1.0
"#;

    #[test]
    fn load_file_replaces_table() {
        let dir = std::env::temp_dir().join(format!("agentpulse-cost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.toml");
        std::fs::write(&path, ONE_MODEL).unwrap();
        let book = PriceBook::load(&path).unwrap();
        close(book.cost(&ev("x", input(1_000_000))), 1.0);
        assert_eq!(book.cost(&ev("claude-sonnet-5-5", input(1))), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(PriceBook::from_toml("this is = = not toml").is_err());
        assert!(PriceBook::from_toml("fast_multiplier = 2.0").is_err());
    }
}
