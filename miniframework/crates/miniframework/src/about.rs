//! `GET /.well-known/miniframework`: a board describing itself, so network
//! tools (homelab, other boards) can say "Game Matcher on a Waveshare C6
//! with a 320x172 screen" instead of "a device on port 80". The framework
//! fills in what the board knows (chip, memory, transports, uptime); the
//! app adds what only it knows ([`AboutConfig`]). Any wire format; JSON by
//! default. The mDNS `_http`/`_https` services point here with an `about`
//! TXT record.
use crate::message;

/// The schema id in every answer; bumped only for incompatible changes.
pub const SCHEMA: &str = "miniframework.about/1";

/// What the app says about itself and its board (`Config::about`).
#[derive(Clone, Debug, Default)]
pub struct AboutConfig {
    /// Human name ("Game Matcher"); the app name when empty.
    pub title: &'static str,
    pub description: &'static str,
    /// Board as sold ("Waveshare ESP32-C6-LCD-1.47").
    pub board: &'static str,
    /// Hardware beyond the chip, as `kind` or `kind:detail` strings:
    /// `screen:320x172`, `button:boot`, `led:rgb`, `sd-slot`, `sensor:temperature`.
    pub hardware: &'static [&'static str],
    /// Useful pages: (relation, path), e.g. ("admin", "/admin").
    pub links: &'static [(&'static str, &'static str)],
}

message! {
    pub struct AboutLink {
        /// `home`, `admin`, `metrics`, `sys`, `log`, `schema`, `trust`, `ca`, or app-defined.
        1 rel: String,
        2 href: String,
    }
}

message! {
    pub struct AboutBoard {
        /// As sold, if the app says ("Waveshare ESP32-C6-LCD-1.47").
        1 model: String,
        /// From the silicon ("ESP32-C6", "desktop").
        2 chip: String,
        3 cores: u32,
        4 cpu_mhz: u32,
        5 flash_bytes: u32,
        6 psram_bytes: u32,
        7 mac: String,
        8 hostname: String,
        /// `kind` or `kind:detail` (see [`AboutConfig::hardware`]).
        9 hardware: Vec<String>,
    }
}

message! {
    pub struct About {
        /// Always [`SCHEMA`].
        1 schema: String,
        /// Machine name ("gamematcher").
        2 app: String,
        3 title: String,
        4 description: String,
        5 version: String,
        6 build: String,
        7 board: AboutBoard,
        /// `http`, `https`, `h2`.
        8 transports: Vec<String>,
        9 links: Vec<AboutLink>,
        10 uptime_ms: u64,
        /// The startup/health word from `/api/v1/sys` ("healthy" when fine).
        11 status: String,
        12 framework: String,
    }
}

#[cfg(all(test, not(target_os = "espidf")))]
mod tests {
    use crate::desktop::DesktopPlatform;
    use crate::site::{Config, Scratch};
    use crate::{http, Reply, Request, Service, Site};

    struct Nothing;
    impl Service for Nothing {
        fn handle(&self, _: &Request<'_>, _: &mut Reply<'_>) {}
    }

    #[test]
    fn describes_the_board_with_the_apps_details() {
        let mut config = Config::new("gamematcher", "gamematcher.local");
        config.about = super::AboutConfig {
            title: "Game Matcher",
            board: "Waveshare ESP32-C6-LCD-1.47",
            hardware: &["screen:320x172", "button:boot"],
            links: &[("admin", "/admin"), ("home", "/home")],
            ..Default::default()
        };
        // A bundled single-page app answers unknown paths with index.html;
        // it must not shadow the description (it did on the first C6 build).
        static ASSETS: &[crate::web::Asset] = &[crate::web::Asset {
            path: "/index.html",
            mime: "text/html; charset=utf-8",
            raw: b"<!doctype html>",
            gzip: b"",
            etag: "x",
            immutable: false,
        }];
        config.assets = ASSETS;
        let site = Site::new(config, Nothing, DesktopPlatform);
        let about = site.about();
        assert_eq!(about.schema, super::SCHEMA);
        assert_eq!(
            (about.app.as_str(), about.title.as_str()),
            ("gamematcher", "Game Matcher")
        );
        assert_eq!(about.board.model, "Waveshare ESP32-C6-LCD-1.47");
        assert_eq!(about.board.hardware, ["screen:320x172", "button:boot"]);
        assert!(about.transports.contains(&"http".to_string()));
        let rels: Vec<(&str, &str)> = about
            .links
            .iter()
            .map(|l| (l.rel.as_str(), l.href.as_str()))
            .collect();
        assert!(rels.contains(&("admin", "/admin")) && rels.contains(&("metrics", "/metrics")));
        assert!(
            rels.contains(&("home", "/home")) && !rels.contains(&("home", "/")),
            "the app's link wins"
        );

        let raw = b"GET /.well-known/miniframework HTTP/1.1\r\nHost: x\r\n\r\n";
        let request = http::parse(raw, 1024).unwrap().unwrap();
        let (status, headers, _, _) = site
            .respond(&request, false, &mut Scratch::new(64 * 1024))
            .into_parts();
        assert_eq!(status, 200);
        assert!(headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("json")));
    }
}
