//! System language selection and translation catalog checks.

const LOCALES: [&str; 5] = ["en", "zh-Hans", "zh-Hant", "ja", "ms"];

pub fn initialize() {
    let locale = std::env::var("AGENTPULSE_LOCALE")
        .ok()
        .filter(|value| LOCALES.contains(&value.as_str()))
        .unwrap_or_else(agentpulse_platform_macos::preferred_locale);
    rust_i18n::set_locale(&locale);
}

/// Present the platform transport's diagnostic codes in the app's language.
/// OS-provided descriptions remain intact (Foundation already localizes them).
pub fn network_error(error: String) -> String {
    if error == "bad URL" {
        return rust_i18n::t!("common.bad_url").to_string();
    }
    let Some(detail) = error.strip_prefix("network error: ") else {
        return error;
    };
    let detail = match detail {
        "not an HTTP response" => rust_i18n::t!("common.network_not_http"),
        "no response" => rust_i18n::t!("common.network_no_response"),
        "timed out" => rust_i18n::t!("common.network_timeout"),
        other => std::borrow::Cow::Borrowed(other),
    };
    rust_i18n::t!("common.network_error", error = detail).to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    // Each catalog uses grouped dotted keys and JSON-quoted YAML scalar values.
    // Parse this deliberately small YAML subset without another dependency.
    fn catalog(source: &str) -> BTreeMap<String, String> {
        source
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let (key, value) = line.split_once(": ").expect("key: value");
                (
                    key.to_string(),
                    serde_json::from_str(value).expect("quoted YAML scalar"),
                )
            })
            .collect()
    }

    #[test]
    fn catalogs_have_matching_keys_and_placeholders() {
        let catalogs = [
            include_str!("../locales/en.yml"),
            include_str!("../locales/zh-Hans.yml"),
            include_str!("../locales/zh-Hant.yml"),
            include_str!("../locales/ja.yml"),
            include_str!("../locales/ms.yml"),
        ]
        .map(catalog);
        fn arguments(text: &str) -> Vec<&str> {
            let mut args: Vec<_> = text
                .split("%{")
                .skip(1)
                .map(|part| part.split('}').next().unwrap())
                .collect();
            args.sort_unstable();
            args
        }
        for translated in &catalogs[1..] {
            assert_eq!(
                catalogs[0].keys().collect::<Vec<_>>(),
                translated.keys().collect::<Vec<_>>()
            );
            for (key, english) in &catalogs[0] {
                assert!(
                    !translated[key].trim().is_empty(),
                    "empty translation: {key}"
                );
                assert_eq!(
                    arguments(english),
                    arguments(&translated[key]),
                    "arguments: {key}"
                );
            }
        }
    }

    #[test]
    fn translated_formats_and_ui() {
        // A child test process isolates rust-i18n's global locale from all other tests.
        const CHILD: &str = "AGENTPULSE_LOCALE_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "localization::tests::translated_formats_and_ui",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                rust_i18n::set_locale("en");
            }
        }
        let _restore = Restore;
        rust_i18n::set_locale("ja");
        assert_eq!(crate::format::duration(725_000), "12分05秒");
        assert_eq!(crate::menubar::short_window(Some(10_080)), "weekly");
        assert_eq!(crate::menubar::short_window(Some(300)), "5h");
        assert_eq!(
            super::network_error("network error: timed out".into()),
            "ネットワークエラー：タイムアウトしました"
        );
        assert_eq!(rust_i18n::t!("overview.title"), "概要");
        let date = chrono::DateTime::from_timestamp(1_794_535_200, 0).unwrap();
        assert_eq!(
            date.format(&rust_i18n::t!("format.date")).to_string(),
            "11月13日 02:00"
        );
        assert_eq!(
            crate::format::start_time(1_794_535_200_000, 1_794_621_600_000),
            date.with_timezone(&chrono::Local)
                .format("%-m月%-d日 %H:%M")
                .to_string()
        );
        rust_i18n::set_locale("ms");
        assert_eq!(rust_i18n::t!("overview.title"), "Ringkasan");
        rust_i18n::set_locale("unavailable-language");
        assert_eq!(rust_i18n::t!("overview.title"), "Overview");
    }
}
