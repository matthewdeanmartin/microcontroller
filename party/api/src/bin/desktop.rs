//! party on a PC: `make run` (API on :8090; `make web-dev` for the Angular
//! dev server), or `make run-bundle` to serve the built UI too.
//!
//! Settings: PARTY_ADDR (default 127.0.0.1:8090), PARTY_DATA (state
//! directory, default .local/state), PARTY_SETUP_CODE.
use miniframework::desktop::{serve, DesktopPlatform};
use miniframework::fetch::PlainFetch;
use miniframework::kv::FileKv;
use miniframework::oauth::{Config as OAuthConfig, OAuth};
use miniframework::{Config, Site};
use party::work::Fetchers;
use party::{App, Limits, Shared};
use std::sync::Arc;

#[cfg(feature = "bundled-web")]
mod bundle {
    use miniframework::web::Asset;
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
    pub fn assets() -> &'static [Asset] {
        ASSETS
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    miniframework::desktop::init_logging();
    let address = std::env::var("PARTY_ADDR").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let data = std::env::var("PARTY_DATA").unwrap_or_else(|_| ".local/state".into());
    let kv = Arc::new(FileKv::open(&data)?);
    let mut config = Config::new("party", address.clone());
    config.proto_package = "party";
    // Calendar feeds (token URLs) and the sign-in callback are answered by
    // the app, not static files.
    config.app_paths = &["/cal", "/auth"];
    config.about = miniframework::about::AboutConfig {
        title: "party",
        description: "Clubs and parties (desktop development server)",
        links: &[("admin", "/admin")],
        ..Default::default()
    };
    #[cfg(feature = "bundled-web")]
    {
        config.assets = bundle::assets();
    }
    if let Ok(ca) = std::fs::read("certs/household-ca.der") {
        config.ca_der = Some(Box::leak(ca.into_boxed_slice()));
    }
    let mut app = App::open(kv.clone(), Limits::board())?;
    app.dev_origins = config.origins.clone();
    app.setup_code = std::env::var("PARTY_SETUP_CODE")
        .ok()
        .map(|c| &*Box::leak(c.into_boxed_str()));
    // HTTPS to instances, Brevo and GitHub (public roots, plus the household
    // CA when certs/household-ca.der is here), all on one worker thread.
    let fetch = Arc::new(PlainFetch {
        timeout: std::time::Duration::from_secs(15),
        roots: std::fs::read("certs/household-ca.der")
            .into_iter()
            .collect(),
    });
    let oauth = Arc::new(OAuth::new(
        OAuthConfig {
            app_name: "party",
            website: format!("http://{address}"),
            ..OAuthConfig::default()
        },
        kv,
        fetch.clone(),
        fetch.clone(),
    ));
    app.attach_oauth(oauth);
    app.set_origin(&format!("http://{address}"));
    let app = Arc::new(app);
    let worker = app.clone();
    std::thread::spawn(move || loop {
        let f = Fetchers {
            household: fetch.as_ref(),
            public: fetch.as_ref(),
        };
        if !worker.work(&f) {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
    log::info!("party on http://{address}/ (state in {data})");
    let site = Site::new(config, Shared(app), DesktopPlatform);
    Ok(serve(&site, &address, || site.service.0.tick())?)
}
