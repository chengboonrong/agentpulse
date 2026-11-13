//! Asks GitHub whether a newer AgentPulse release exists. One GET of the public
//! releases list with a User-Agent of `AgentPulse/<version>`; nothing else is sent.

use serde::Deserialize;

/// Not `/releases/latest`: it skips pre-releases, and every release so far is one.
pub const URL: &str = "https://api.github.com/repos/chengboonrong/agentpulse/releases?per_page=10";
/// A release's page must be under this, or the release is ignored.
const RELEASES: &str = "https://github.com/chengboonrong/agentpulse/releases/";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const TIMEOUT_S: f64 = 20.;
const NOTES_MAX: usize = 4_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    UpToDate,
    Available(Release),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// The release notes as plain text, capped at [`NOTES_MAX`] characters.
    pub notes: String,
    /// `2026-11-13`, when GitHub gives a date.
    pub published: Option<String>,
}

#[derive(Deserialize)]
struct Raw {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    published_at: Option<String>,
}

/// Blocking: run it on a background thread.
pub fn check() -> Result<Check, String> {
    let body = agentpulse_platform_macos::fetch(URL, &format!("AgentPulse/{CURRENT}"), TIMEOUT_S)?;
    parse(&body, CURRENT)
}

/// Major, minor, patch, and whether it is a plain release, which ranks above
/// any `-suffix` of the same numbers.
type Version = (u64, u64, u64, bool);

/// `v1.2.3`, `1.2.3` or `1.2.3-beta.1`; anything else is None.
fn version(tag: &str) -> Option<Version> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let (core, suffix) = match tag.split_once('-') {
        Some((core, suffix)) if !suffix.is_empty() => (core, Some(suffix)),
        Some(_) => return None,
        None => (tag, None),
    };
    let mut parts = core.split('.').map(|p| {
        // No signs or spaces, which u64's parser would accept.
        p.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| p.parse().ok())
            .flatten()
    });
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts
        .next()
        .is_none()
        .then_some((v.0, v.1, v.2, suffix.is_none()))
}

/// The newest usable release in GitHub's list, against `current`.
pub fn parse(body: &[u8], current: &str) -> Result<Check, String> {
    let list: Vec<Raw> =
        serde_json::from_slice(body).map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    let current = version(current).ok_or("this build's version is not a release")?;
    let mut newest: Option<(Version, Raw)> = None;
    for r in list {
        let Some(v) = version(&r.tag_name) else {
            continue;
        };
        if r.draft || !trusted_page(&r.html_url) {
            continue;
        }
        // Strictly newer: of equal versions the first listed, the newest, stays.
        if newest.as_ref().is_none_or(|(best, _)| v > *best) {
            newest = Some((v, r));
        }
    }
    Ok(match newest {
        Some((v, r)) if v > current => Check::Available(Release {
            version: r
                .tag_name
                .strip_prefix('v')
                .unwrap_or(&r.tag_name)
                .to_string(),
            url: r.html_url,
            notes: notes(r.body.as_deref().unwrap_or("")),
            published: r.published_at.and_then(|d| d.get(..10).map(str::to_owned)),
        }),
        _ => Check::UpToDate,
    })
}

/// Under AgentPulse's releases on GitHub, with no `..` that could climb out of it.
fn trusted_page(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    url.starts_with(RELEASES) && !lower.contains("..") && !lower.contains("%2e")
}

fn notes(body: &str) -> String {
    let body = body.replace("\r\n", "\n");
    let body = body.trim();
    match body.char_indices().nth(NOTES_MAX) {
        Some((cut, _)) => format!("{}…", body[..cut].trim_end()),
        None => body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, draft: bool, url: &str) -> String {
        format!(
            r#"{{"tag_name": "{tag}", "html_url": "{url}", "draft": {draft},
                "body": "  Notes for {tag}\n", "published_at": "2026-11-01T09:30:00Z"}}"#
        )
    }

    fn page(tag: &str) -> String {
        format!("{RELEASES}tag/{tag}")
    }

    fn list(items: &[String]) -> Vec<u8> {
        format!("[{}]", items.join(",")).into_bytes()
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(version("0.0.5"), Some((0, 0, 5, true)));
        assert_eq!(version("v1.2.3"), Some((1, 2, 3, true)));
        assert_eq!(version("v0.1.0-beta.2"), Some((0, 1, 0, false)));
        for bad in [
            "", "v", "1.2", "1.2.3.4", "1.2.x", "1.2.3-", "latest", "v-1.2.3", "1.+2.3", " 1.2.3",
        ] {
            assert_eq!(version(bad), None, "{bad:?}");
        }
        assert!(version("0.0.5") > version("0.0.5-rc.1"));
        assert!(version("0.0.5-rc.1") > version("0.0.4"));
        assert!(version("0.1.0") > version("0.0.10"));
        assert!(version("0.0.10") > version("0.0.9"));
    }

    #[test]
    fn the_newest_usable_release_is_picked() {
        let body = list(&[
            release("v0.0.9", true, &page("v0.0.9")),
            release("nightly", false, &page("nightly")),
            release(
                "v0.0.6",
                false,
                "https://example.com/agentpulse/releases/v0.0.6",
            ),
            release("v0.0.5", false, &page("v0.0.5")),
            release("v0.0.5-rc.1", false, &page("v0.0.5-rc.1")),
            release("v0.0.4", false, &page("v0.0.4")),
        ]);
        let Ok(Check::Available(r)) = parse(&body, "0.0.4") else {
            panic!("expected an update");
        };
        assert_eq!(r.version, "0.0.5");
        assert_eq!(r.url, page("v0.0.5"));
        assert_eq!(r.notes, "Notes for v0.0.5");
        assert_eq!(r.published.as_deref(), Some("2026-11-01"));
    }

    #[test]
    fn up_to_date_when_nothing_is_newer() {
        let body = list(&[
            release("v0.0.4", false, &page("v0.0.4")),
            release("v0.0.3", false, &page("v0.0.3")),
        ]);
        assert_eq!(parse(&body, "0.0.4"), Ok(Check::UpToDate));
        assert_eq!(parse(&body, "0.0.5"), Ok(Check::UpToDate));
        assert_eq!(parse(b"[]", "0.0.4"), Ok(Check::UpToDate));
        // A newer release whose page is elsewhere is not offered.
        let foreign = list(&[release(
            "v9.0.0",
            false,
            "https://github.com/someone/agentpulse/releases/tag/v9.0.0",
        )]);
        assert_eq!(parse(&foreign, "0.0.4"), Ok(Check::UpToDate));
        for climbing in [
            "../../../evil/releases/tag/v9.0.0",
            "%2E%2e/x/v9.0.0",
            "tag/v9%2E0.0",
        ] {
            let body = list(&[release("v9.0.0", false, &format!("{RELEASES}{climbing}"))]);
            assert_eq!(parse(&body, "0.0.4"), Ok(Check::UpToDate), "{climbing}");
        }
        let draft = list(&[release("v9.0.0", true, &page("v9.0.0"))]);
        assert_eq!(parse(&draft, "0.0.4"), Ok(Check::UpToDate));
    }

    #[test]
    fn bad_replies_are_errors() {
        assert!(parse(b"{\"message\": \"rate limited\"}", "0.0.4").is_err());
        assert!(parse(b"<html>", "0.0.4").is_err());
    }

    #[test]
    fn long_notes_are_cut_at_a_character() {
        let long = "é".repeat(NOTES_MAX + 10);
        let n = notes(&long);
        assert_eq!(n.chars().count(), NOTES_MAX + 1);
        assert!(n.ends_with("é…"));
        assert_eq!(notes(" short\r\nlines\r\n"), "short\nlines");
    }
}
