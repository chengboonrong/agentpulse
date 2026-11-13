//! The relay's HTTP API (docs/mobile.md, "Relay API"; `relay/src/index.ts`).

use serde_json::Value;

const TIMEOUT_S: f64 = 20.;

/// A relay answer of any status. `body` is Null when it is empty or not JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    /// `Retry-After` in seconds.
    pub retry_after: Option<i64>,
}

/// One request to `url` with `Authorization: Bearer <token>` when given and a JSON body.
/// Network errors are `Err`.
pub trait Relay: Send {
    fn call(
        &self,
        method: &str,
        url: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Reply, String>;
}

/// The real relay, through NSURLSession.
pub struct Http;

impl Relay for Http {
    fn call(
        &self,
        method: &str,
        url: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Reply, String> {
        let agent = format!("AgentPulse/{}", env!("CARGO_PKG_VERSION"));
        let auth = token.map(|t| format!("Bearer {t}"));
        let mut headers = vec![
            ("User-Agent", agent.as_str()),
            ("Accept", "application/json"),
            ("Accept-Language", "en"),
        ];
        if let Some(auth) = &auth {
            headers.push(("Authorization", auth));
        }
        let body = body.map(|b| b.to_string().into_bytes());
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let r =
            agentpulse_platform_macos::request(method, url, &headers, body.as_deref(), TIMEOUT_S)?;
        Ok(Reply {
            status: r.status,
            body: serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            retry_after: r.retry_after.and_then(|v| v.trim().parse().ok()),
        })
    }
}
