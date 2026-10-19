//! Game Matcher on a PC: `make run` (API on :8088; `make web-dev` for the
//! Angular dev server), or `make run-bundle` to serve the built UI too.
//!
//! Settings: GAMEMATCHER_ADDR (default 127.0.0.1:8088), GAMEMATCHER_DATA
//! (state directory, default .local/state).
use gamematcher::outbound::Outbox;
use gamematcher::{App, Limits};
use miniframework::desktop::{serve, DesktopPlatform};
use miniframework::fetch::PlainFetch;
use miniframework::kv::FileKv;
use miniframework::oauth::{Config as OAuthConfig, OAuth};
use miniframework::{Config, Site};
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
    let address = std::env::var("GAMEMATCHER_ADDR").unwrap_or_else(|_| "127.0.0.1:8088".into());
    let data = std::env::var("GAMEMATCHER_DATA").unwrap_or_else(|_| ".local/state".into());
    let kv = Arc::new(FileKv::open(&data)?);
    let mut config = Config::new("gamematcher", address.clone());
    config.proto_package = "gamematcher";
    // Calendar feeds (token URLs) are answered by the app, not static files.
    config.app_paths = &["/cal", "/auth"];
    config.about = miniframework::about::AboutConfig {
        title: "Game Matcher",
        description: "Household game-night planner (desktop development server)",
        links: &[
            ("admin", "/admin"),
            ("screen", "/api/v1/display/frame.bmp"),
            ("deck", "/api/v1/display"),
        ],
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
    // Mastodon sign-in: HTTPS to instances (public roots, plus the household
    // CA when certs/household-ca.der is here); network steps on a thread.
    let fetch = Arc::new(PlainFetch {
        timeout: std::time::Duration::from_secs(10),
        roots: std::fs::read("certs/household-ca.der")
            .into_iter()
            .collect(),
    });
    let oauth = Arc::new(OAuth::new(
        OAuthConfig {
            app_name: "Game Matcher",
            website: format!("http://{address}"),
            ..OAuthConfig::default()
        },
        kv.clone(),
        fetch.clone(),
        fetch.clone(),
    ));
    // The household Mastodon bot's queue, on the same worker.
    let public_outbox = Arc::new(Outbox::open_public(
        kv.clone(),
        fetch.clone(),
        fetch.clone(),
    ));
    let public_posts = public_outbox.clone();
    app.attach_public_outbox(public_outbox);
    let outbox = Arc::new(Outbox::open(kv, fetch.clone(), fetch));
    let (worker, posts) = (oauth.clone(), outbox.clone());
    std::thread::spawn(move || loop {
        let now = miniframework::wall_ms().map_or(0, |ms| ms / 1000);
        let worked = worker.run_once() | posts.run_once(now) | public_posts.run_once(now);
        if !worked {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    app.attach_oauth(oauth);
    // Email: a Brevo API key in the environment (or set it on the admin page).
    if let Some(key) = [
        "GAMEMATCHER_BREVO_API_KEY",
        "BREVO_API_KEY",
        "BREVO_OTHER_KEY",
    ]
    .iter()
    .find_map(|k| std::env::var(k).ok())
    {
        let setup = gamematcher::outbound::MailSetup {
            api_key: key,
            sender_email: std::env::var("GAMEMATCHER_MAIL_FROM").unwrap_or_default(),
            sender_name: "Game Matcher".into(),
            built_in: true,
        };
        if let Err(e) = outbox.begin_mail_setup(setup) {
            log::warn!("email key from the environment: {}", e.message);
        }
    }
    app.attach_outbox(outbox);
    app.origin = format!("http://{address}");
    app.dev_origins = config.origins.clone();
    app.setup_code = std::env::var("GAMEMATCHER_SETUP_CODE")
        .ok()
        .map(|c| &*Box::leak(c.into_boxed_str()));
    log::info!("Game Matcher on http://{address}/ (state in {data})");
    let site = Site::new(config, app, DesktopPlatform);
    Ok(serve(&site, &address, || site.service.tick())?)
}
