//! housemetrics on a PC: `make run` (API on :8080; `make web-dev` for the
//! Angular dev server), or `make run-bundle` to serve the built UI too.
use housemetrics::{build, configure_scrapes, record_self, Profile};
use miniframework::desktop::{serve, DesktopPlatform};
use miniframework::fetch::PlainFetch;
use miniframework::kv::FileKv;
use miniframework::{Config, Site};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "bundled-web")]
mod bundle {
    use miniframework::web::Asset;
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
    pub fn assets() -> &'static [Asset] {
        ASSETS
    }
}

fn main() -> std::io::Result<()> {
    miniframework::desktop::init_logging();
    let address = std::env::var("HOUSEMETRICS_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let password = std::env::var("HOUSEMETRICS_ADMIN_PASSWORD").unwrap_or_else(|_| {
        log::warn!(
            "HOUSEMETRICS_ADMIN_PASSWORD is not set; the desktop admin password is \"admin\""
        );
        "admin".into()
    });
    let profile = Profile::desktop();
    let data = std::env::var("HOUSEMETRICS_DATA").unwrap_or_else(|_| ".local/kv".into());
    let kv = Arc::new(FileKv::open(data)?);
    let app = build(kv, &password, true, &profile);
    configure_scrapes(&app.scrapes).map_err(|e| std::io::Error::other(e.message))?;
    let store = app.store.clone();
    let scrapes = app.scrapes.clone();
    std::thread::spawn(move || {
        let fetch = PlainFetch::default();
        loop {
            scrapes.run_due(&fetch, &store);
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    let mut config = Config::new("housemetrics", address.clone());
    config.body_limit = profile.body_limit;
    config.response_limit = profile.response_limit;
    config.proto_package = "housemetrics";
    #[cfg(feature = "bundled-web")]
    {
        config.assets = bundle::assets();
    }
    if let Ok(ca) = std::fs::read("certs/household-ca.der") {
        config.ca_der = Some(Box::leak(ca.into_boxed_slice()));
    }
    let site = Site::new(config, app, DesktopPlatform);
    let mut last = Instant::now() - Duration::from_secs(60);
    serve(&site, &address, || {
        if last.elapsed() >= Duration::from_secs(10) {
            last = Instant::now();
            record_self(&site.service.store, &site.sysinfo());
        }
    })
}
