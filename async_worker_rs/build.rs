use std::path::PathBuf;

fn parse(text: &str, want: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != want {
            continue;
        }
        // Strip a trailing comment only when the value is unquoted; a quoted
        // password may legitimately contain '#'.
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
        let value = match unquoted {
            Some(v) => v.to_string(),
            None => value.split('#').next().unwrap_or("").trim().to_string(),
        };
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WORKER_NTP_ADDR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("espidf") {
        return;
    }
    #[cfg(feature = "esp32")]
    embuild::espidf::sysenv::output();
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let paths = [
        ".env",
        "config.py",
        "../archive/nanacoin_rs/.env",
        "../archive/nanacoin_web/config.py",
        "../hello_wifi_py/config.py",
        "../secret_messages/config.py",
        "../hello_wifi_s3_py/config.py",
        "../../mastomini/mastomini_rs/.env",
    ];
    let sources: Vec<_> = paths
        .iter()
        .filter_map(|p| {
            let path = root.join(p);
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::read_to_string(&path).ok().map(|text| (path, text))
        })
        .collect();
    for (key, aliases) in [
        (
            "WORKER_WIFI_SSID",
            [
                "WORKER_WIFI_SSID",
                "WIFI_SSID",
                "NANACOIN_WIFI_SSID",
                "MASTOMINI_WIFI_SSID",
            ],
        ),
        (
            "WORKER_WIFI_PASSWORD",
            [
                "WORKER_WIFI_PASSWORD",
                "WIFI_PASSWORD",
                "NANACOIN_WIFI_PASSWORD",
                "MASTOMINI_WIFI_PASSWORD",
            ],
        ),
    ] {
        for alias in aliases {
            println!("cargo:rerun-if-env-changed={alias}");
        }
        let found = aliases
            .iter()
            .find_map(|a| std::env::var(a).ok().map(|v| ("environment".to_owned(), v)))
            .or_else(|| {
                sources.iter().find_map(|(path, text)| {
                    aliases
                        .iter()
                        .find_map(|a| parse(text, a))
                        .map(|v| (path.display().to_string(), v))
                })
            });
        let (source, value) = found
            .unwrap_or_else(|| panic!("Set {key} or WIFI_SSID/WIFI_PASSWORD in .env or config.py"));
        assert!(
            !value
                .chars()
                .any(|c| c == char::from(13) || c == char::from(10)),
            "Wi-Fi configuration cannot contain newlines"
        );
        println!("cargo:warning={key} taken from {source}");
        println!("cargo:rustc-env={key}={value}");
    }
}
