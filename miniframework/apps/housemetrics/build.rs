//! Firmware settings come from the environment or, failing that, from a
//! gitignored `.env` file (this directory, then `miniframework/`, then the
//! file named by `MINIFRAMEWORK_ENV`). Values are never printed.
use std::path::{Path, PathBuf};

#[path = "src/deployment.rs"]
mod deployment;

/// Reads `KEY=value` / `KEY = "value"` lines (dotenv and Python config
/// syntax: comments, optional `export`, quotes).
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

/// Names accepted for each setting. Other household apps' names are
/// accepted so one `.env` can serve every board.
fn aliases(key: &str) -> &'static [&'static str] {
    match key {
        "HOUSEMETRICS_WIFI_SSID" => &[
            "HOUSEMETRICS_WIFI_SSID",
            "MINIFRAMEWORK_WIFI_SSID",
            "WIFI_SSID",
            "MASTOMINI_WIFI_SSID",
            "NANACOIN_WIFI_SSID",
        ],
        "HOUSEMETRICS_WIFI_PASSWORD" => &[
            "HOUSEMETRICS_WIFI_PASSWORD",
            "MINIFRAMEWORK_WIFI_PASSWORD",
            "WIFI_PASSWORD",
            "MASTOMINI_WIFI_PASSWORD",
            "NANACOIN_WIFI_PASSWORD",
        ],
        "HOUSEMETRICS_ADMIN_PASSWORD" => &["HOUSEMETRICS_ADMIN_PASSWORD"],
        "HOUSEMETRICS_HOSTNAME" => &["HOUSEMETRICS_HOSTNAME"],
        "HOUSEMETRICS_ORIGINS" => &["HOUSEMETRICS_ORIGINS"],
        "HOUSEMETRICS_BUILD" => &["HOUSEMETRICS_BUILD"],
        "HOUSEMETRICS_STATUS_LED" => &["HOUSEMETRICS_STATUS_LED"],
        "HOUSEMETRICS_WIFI_SETUP_CODE" => &["HOUSEMETRICS_WIFI_SETUP_CODE"],
        _ => &[],
    }
}

const KEYS: [&str; 8] = [
    "HOUSEMETRICS_WIFI_SSID",
    "HOUSEMETRICS_WIFI_PASSWORD",
    "HOUSEMETRICS_ADMIN_PASSWORD",
    "HOUSEMETRICS_HOSTNAME",
    "HOUSEMETRICS_ORIGINS",
    "HOUSEMETRICS_BUILD",
    "HOUSEMETRICS_STATUS_LED",
    "HOUSEMETRICS_WIFI_SETUP_CODE",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=src/deployment.rs");
    println!("cargo:rerun-if-env-changed=HOUSEMETRICS_CONFIG");
    let config_path = std::env::var_os("HOUSEMETRICS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("config/scrapes.json"));
    println!("cargo:rerun-if-changed={}", config_path.display());
    let bytes = std::fs::read(&config_path).expect("read HOUSEMETRICS_CONFIG scrape configuration");
    deployment::parse(&bytes).expect("invalid scrape deployment configuration");
    std::fs::write(
        PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("scrapes.json"),
        bytes,
    )
    .expect("embed scrape configuration");

    // The firmware's TLS certificate and board marker follow its hostname,
    // so a second board (housemetrics-v2.local) is a build setting, not an
    // edit: certs/<host>.crt/.key (make certs HOST=...) and the marker
    // boardsafe checks before writing.
    if std::env::var_os("CARGO_FEATURE_ESP32").is_some() {
        println!("cargo:rerun-if-env-changed=HOUSEMETRICS_HOSTNAME");
        println!("cargo:rerun-if-env-changed=HOUSEMETRICS_BOARD");
        let host = std::env::var("HOUSEMETRICS_HOSTNAME").unwrap_or_else(|_| "housemetrics".into());
        let board = std::env::var("HOUSEMETRICS_BOARD").unwrap_or_else(|_| "s2".into());
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        for ext in ["crt", "key"] {
            let source = root.join(format!("certs/{host}.{ext}"));
            println!("cargo:rerun-if-changed={}", source.display());
            std::fs::copy(&source, out.join(format!("server.{ext}"))).unwrap_or_else(|_| {
                panic!("no {}: run `make certs HOST={host}`", source.display())
            });
        }
        println!("cargo:rustc-env=HOUSEMETRICS_MARKER=HOUSEMETRICS-BOARD:{board}:{host}.local;");
    }

    if std::env::var_os("CARGO_FEATURE_BUNDLED_WEB").is_some() {
        let assets = root.join(".embuild/web/assets.rs");
        assert!(
            assets.exists(),
            "bundled-web needs the Angular bundle: run `make web` first"
        );
        println!("cargo:rerun-if-changed={}", assets.display());
        std::fs::copy(
            &assets,
            PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("assets.rs"),
        )
        .unwrap();
    }
    for cert in [
        "certs/housemetrics.crt",
        "certs/housemetrics.key",
        "certs/household-ca.der",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(cert).display());
    }

    #[cfg(feature = "esp32")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }

    // Wi-Fi and admin settings are compiled into firmware only; the desktop
    // server reads its own at run time.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("espidf") {
        return;
    }
    println!("cargo:rerun-if-env-changed=MINIFRAMEWORK_ENV");
    let mut files: Vec<PathBuf> = vec![root.join(".env"), root.join("../../.env")];
    if let Some(extra) = std::env::var_os("MINIFRAMEWORK_ENV") {
        files.push(PathBuf::from(extra));
    }
    let sources: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            println!("cargo:rerun-if-changed={}", path.display());
            Some((path, text))
        })
        .collect();
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
        if let Some(value) = aliases(key).iter().find_map(|k| std::env::var(k).ok()) {
            println!("cargo:rustc-env={key}={value}");
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
        }
    }
}
