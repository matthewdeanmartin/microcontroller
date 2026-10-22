//! Firmware settings come from the environment or, failing that, from a
//! gitignored `.env` (this directory, then `party/`, then the file named by
//! `MINIFRAMEWORK_ENV`, then the household one in `C:\github`). Values are
//! never printed.
use std::path::{Path, PathBuf};

/// Reads `KEY=value` / `KEY = "value"` lines (comments, optional `export`,
/// quotes).
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

/// Names accepted for each setting, so one household `.env` can serve
/// every board.
fn aliases(key: &str) -> &'static [&'static str] {
    match key {
        "PARTY_WIFI_SSID" => &["PARTY_WIFI_SSID", "MINIFRAMEWORK_WIFI_SSID", "WIFI_SSID"],
        "PARTY_WIFI_PASSWORD" => &[
            "PARTY_WIFI_PASSWORD",
            "MINIFRAMEWORK_WIFI_PASSWORD",
            "WIFI_PASSWORD",
        ],
        "PARTY_HOSTNAME" => &["PARTY_HOSTNAME"],
        "PARTY_BUILD" => &["PARTY_BUILD"],
        "PARTY_SETUP_CODE" => &["PARTY_SETUP_CODE"],
        _ => &[],
    }
}

const KEYS: [&str; 5] = [
    "PARTY_WIFI_SSID",
    "PARTY_WIFI_PASSWORD",
    "PARTY_HOSTNAME",
    "PARTY_BUILD",
    "PARTY_SETUP_CODE",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // The TLS certificate follows the hostname: certs/<host>.crt/.key
    // (`make certs`). Plain-HTTP builds still compile the paths, so copy
    // empty placeholders when HTTPS is off and the files are absent.
    if std::env::var_os("CARGO_FEATURE_ESP32").is_some() {
        println!("cargo:rerun-if-env-changed=PARTY_HOSTNAME");
        let host = std::env::var("PARTY_HOSTNAME").unwrap_or_else(|_| "party".into());
        let https = std::env::var_os("CARGO_FEATURE_HTTPS").is_some();
        for ext in ["crt", "key"] {
            let source = root.join(format!("certs/{host}.{ext}"));
            println!("cargo:rerun-if-changed={}", source.display());
            if source.exists() {
                std::fs::copy(&source, out.join(format!("server.{ext}"))).unwrap();
            } else if https {
                panic!("no {}: run `make certs`", source.display());
            } else {
                std::fs::write(out.join(format!("server.{ext}")), "").unwrap();
            }
        }
    }

    if std::env::var_os("CARGO_FEATURE_BUNDLED_WEB").is_some() {
        let assets = root.join(".embuild/web/assets.rs");
        assert!(
            assets.exists(),
            "bundled-web needs the Angular bundle: run `make web` first"
        );
        println!("cargo:rerun-if-changed={}", assets.display());
        std::fs::copy(&assets, out.join("assets.rs")).unwrap();
    }
    for cert in ["certs/household-ca.crt", "certs/household-ca.der"] {
        println!("cargo:rerun-if-changed={}", root.join(cert).display());
    }

    #[cfg(feature = "esp32")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }

    // Wi-Fi and friends are compiled into firmware only; the desktop server
    // reads its own settings at run time.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("espidf") {
        return;
    }
    println!("cargo:rerun-if-env-changed=MINIFRAMEWORK_ENV");
    let mut files: Vec<PathBuf> = vec![root.join(".env"), root.join("../.env")];
    if let Some(extra) = std::env::var_os("MINIFRAMEWORK_ENV") {
        files.push(PathBuf::from(extra));
    }
    files.push(root.join("../../../.env"));
    let sources: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            println!("cargo:rerun-if-changed={}", path.display());
            Some((path, text))
        })
        .collect();
    let mut found_keys = Vec::new();
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
        for alias in aliases(key) {
            println!("cargo:rerun-if-env-changed={alias}");
        }
        if let Some(value) = aliases(key).iter().find_map(|k| std::env::var(k).ok()) {
            println!("cargo:rustc-env={key}={value}");
            found_keys.push(key);
            continue;
        }
        let found = sources.iter().find_map(|(path, text)| {
            aliases(key)
                .iter()
                .find_map(|name| parse(text, name))
                .map(|value| (path.as_path(), value))
        });
        if let Some((path, value)) = found {
            let shown: &Path = path.strip_prefix(&root).unwrap_or(path);
            println!("cargo:warning={key} taken from {}", shown.display());
            println!("cargo:rustc-env={key}={value}");
            found_keys.push(key);
        }
    }
    // Without Wi-Fi the firmware can only fail at boot, and the compiler then
    // removes the whole app as dead code (an image that looks fine but isn't).
    for key in ["PARTY_WIFI_SSID", "PARTY_WIFI_PASSWORD"] {
        assert!(
            found_keys.contains(&key),
            "{key} is not set: put it (or WIFI_SSID / WIFI_PASSWORD) in party/.env or the environment"
        );
    }
}
