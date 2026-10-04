//! Static files compiled into the firmware, and the certificate trust page.
//!
//! `tools/bundle-web.mjs` turns an Angular `dist/` folder into an
//! `assets.rs` that an app `include!`s and passes to
//! [`Config::assets`](crate::site::Config). Lookups are exact matches
//! against that table: no filesystem, no path traversal.

/// One bundled file. `raw` may be empty for gzip-only bundles (4 MiB flash).
pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub raw: &'static [u8],
    pub gzip: &'static [u8],
    pub etag: &'static str,
    /// Content-hashed file name (Angular's `main-ABCD1234.js`): cache forever.
    pub immutable: bool,
}

pub struct StaticReply {
    pub status: u16,
    pub bytes: &'static [u8],
    pub headers: Vec<(&'static str, &'static str)>,
}

/// The q-value a client gives `wanted` in an `Accept-Encoding` header, or
/// `None` when it names neither `wanted` nor `*`.
fn quality(header: &str, wanted: &str) -> Option<f32> {
    let mut wildcard = None;
    for part in header.split(',') {
        let mut fields = part.trim().split(';');
        let name = fields.next().unwrap_or("").trim();
        let mut q = 1.0;
        for field in fields {
            if let Some(value) = field.trim().strip_prefix("q=") {
                q = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| (0.0..=1.0).contains(v))
                    .unwrap_or(0.0);
            }
        }
        if name.eq_ignore_ascii_case(wanted) {
            return Some(q);
        }
        if name == "*" {
            wildcard = Some(q);
        }
    }
    wildcard
}

/// True when the client accepts a gzip response body.
pub fn accepts_gzip(accept_encoding: &str) -> bool {
    quality(accept_encoding, "gzip").is_some_and(|q| q > 0.0)
}

/// `If-None-Match` against an entity tag, with weak comparison (RFC 9110
/// 13.1.2): lists, `W/` prefixes and the `*` wildcard. A comma inside a
/// quoted tag is part of the tag, not a list separator.
pub fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    if if_none_match.trim().is_empty() {
        return false;
    }
    if if_none_match.trim() == "*" {
        return true;
    }
    fn strip(tag: &str) -> &str {
        tag.strip_prefix("W/").unwrap_or(tag)
    }
    let wanted = strip(etag);
    let mut quoted = false;
    if_none_match
        .split(|ch| {
            if ch == '"' {
                quoted = !quoted;
            }
            ch == ',' && !quoted
        })
        .any(|candidate| strip(candidate.trim()) == wanted)
}

/// Which paths are Angular routes (served `/index.html`).
#[derive(Clone, Copy, Debug)]
pub enum Spa {
    /// Any path whose last segment has no `.`: simple, but every unknown
    /// path is a 200.
    Extensionless,
    /// Only these paths (trailing `/` ignored, `""` is the root); anything
    /// else without a bundled file is a 404.
    Routes(&'static [&'static str]),
}

impl Spa {
    fn matches(self, path: &str) -> bool {
        match self {
            Spa::Extensionless => !path.rsplit('/').next().unwrap_or("").contains('.'),
            Spa::Routes(routes) => routes.contains(&path.trim_end_matches('/')),
        }
    }
}

/// Serves a bundled file. Angular routes (see [`Spa`]) get `/index.html`
/// so the router handles deep links; a missing file stays a 404.
pub fn route(
    assets: &'static [Asset],
    spa: Spa,
    method: &str,
    path: &str,
    accept_encoding: &str,
    if_none_match: &str,
) -> Option<StaticReply> {
    if assets.is_empty() {
        return None;
    }
    let mut reply = StaticReply {
        status: 404,
        bytes: b"Not found",
        headers: vec![
            ("X-Content-Type-Options", "nosniff"),
            ("Vary", "Accept-Encoding"),
        ],
    };
    if method != "GET" && method != "HEAD" {
        reply.status = 405;
        reply.bytes = b"Method not allowed";
        reply.headers.push(("Allow", "GET, HEAD"));
        return Some(reply);
    }
    let path = if spa.matches(path) {
        "/index.html"
    } else {
        path
    };
    let Some(asset) = assets.iter().find(|a| a.path == path) else {
        reply
            .headers
            .push(("Content-Type", "text/plain; charset=utf-8"));
        reply.headers.push(("Cache-Control", "no-store"));
        return Some(reply);
    };
    // A gzip-only bundle has no identity copy. A missing Accept-Encoding
    // accepts any coding (RFC 9110 12.5.3), so serve gzip then too.
    // Identity is acceptable unless excluded, directly or by `*;q=0`.
    let gzip_only = asset.raw.is_empty();
    let gz = if gzip_only && accept_encoding.trim().is_empty() {
        1.0
    } else {
        quality(accept_encoding, "gzip").unwrap_or(0.0)
    };
    let identity = if gzip_only {
        0.0
    } else {
        quality(accept_encoding, "identity").unwrap_or(1.0)
    };
    if gz == 0.0 && identity == 0.0 {
        reply.status = 406;
        reply.bytes = if gzip_only {
            b"This board stores files gzip-compressed only"
        } else {
            b"No acceptable representation"
        };
        reply
            .headers
            .push(("Content-Type", "text/plain; charset=utf-8"));
        reply.headers.push(("Cache-Control", "no-store"));
        return Some(reply);
    }
    reply.headers.push(("Content-Type", asset.mime));
    reply.headers.push(("ETag", asset.etag));
    reply.headers.push((
        "Cache-Control",
        if asset.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        },
    ));
    if etag_matches(if_none_match, asset.etag) {
        reply.status = 304;
        reply.bytes = b"";
        return Some(reply);
    }
    if gz > 0.0 && gz >= identity {
        reply.headers.push(("Content-Encoding", "gzip"));
        reply.bytes = asset.gzip;
    } else {
        reply.bytes = asset.raw;
    }
    reply.status = 200;
    Some(reply)
}

/// The SHA-256 fingerprint as `AB:CD:...`.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The page served at `/trust`: how to install the household CA.
pub fn trust_page(app: &str, host: &str, ca_der: &[u8]) -> String {
    TRUST_TEMPLATE
        .replace("@APP@", app)
        .replace("@HOST@", host)
        .replace("@FINGERPRINT@", &fingerprint(ca_der))
}

const TRUST_TEMPLATE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Trust this board</title>
<style>
:root{color-scheme:light dark;--bg:#fbfaf7;--fg:#1d1d1b;--muted:#5d5b55;--card:#fff;--line:#dedad2;--accent:#1f6f5b}
@media (prefers-color-scheme:dark){:root{--bg:#141413;--fg:#ecebe6;--muted:#a5a39b;--card:#1d1d1b;--line:#33322e;--accent:#6cc4a8}}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,sans-serif}
main{max-width:40rem;margin:0 auto;padding:2rem 1rem}
h1{font-size:1.5rem;margin:0 0 .5rem}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:1rem 1.25rem;margin:1rem 0}
code{font:13px/1.4 ui-monospace,monospace;word-break:break-all}
a.button{display:inline-block;background:var(--accent);color:var(--bg);padding:.5rem 1rem;border-radius:8px;text-decoration:none;font-weight:600}
li{margin:.35rem 0}.muted{color:var(--muted)}
</style></head><body><main>
<h1>@APP@ on @HOST@</h1>
<p class="muted">This board serves HTTPS with a certificate from your household's own certificate authority (the same one as your other boards). Install it once per device and every board signed by it is trusted.</p>
<div class="card">
<p><a class="button" href="/ca">Download the household CA</a></p>
<p>Before trusting it, check its SHA-256 fingerprint matches the one you were given:</p>
<p><code>@FINGERPRINT@</code></p>
</div>
<div class="card"><ol>
<li><b>Windows:</b> open the file, <i>Install Certificate</i>, <i>Current User</i>, <i>Place in: Trusted Root Certification Authorities</i>.</li>
<li><b>macOS:</b> open it in Keychain Access, then set <i>When using this certificate</i> to <i>Always Trust</i>.</li>
<li><b>iPhone / iPad:</b> install the profile, then enable it in <i>Settings, General, About, Certificate Trust Settings</i>.</li>
<li><b>Android:</b> <i>Settings, Security, Encryption &amp; credentials, Install a certificate, CA certificate</i>.</li>
<li><b>Firefox</b> keeps its own list: <i>Settings, Privacy &amp; Security, Certificates, View Certificates, Authorities, Import</i>.</li>
</ol></div>
<p><a href="https://@HOST@/">Continue to https://@HOST@/</a></p>
</main></body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    static ASSETS: &[Asset] = &[
        Asset {
            path: "/index.html",
            mime: "text/html; charset=utf-8",
            raw: b"<html>",
            gzip: b"gz-html",
            etag: "W/\"1\"",
            immutable: false,
        },
        Asset {
            path: "/main-ABCDEFGH.js",
            mime: "text/javascript; charset=utf-8",
            raw: b"",
            gzip: b"gz-js",
            etag: "W/\"2\"",
            immutable: true,
        },
    ];

    #[test]
    fn spa_fallback_and_negotiation() {
        let r = route(
            ASSETS,
            Spa::Extensionless,
            "GET",
            "/dashboard/3",
            "gzip, br",
            "",
        )
        .unwrap();
        assert_eq!((r.status, r.bytes), (200, &b"gz-html"[..]));
        let r = route(ASSETS, Spa::Extensionless, "GET", "/", "identity", "").unwrap();
        assert_eq!(r.bytes, b"<html>");
        let r = route(ASSETS, Spa::Extensionless, "GET", "/missing.js", "gzip", "").unwrap();
        assert_eq!(r.status, 404);
        let r = route(
            ASSETS,
            Spa::Extensionless,
            "GET",
            "/main-ABCDEFGH.js",
            "",
            "",
        )
        .unwrap();
        assert_eq!(r.bytes, b"gz-js");
        let r = route(
            ASSETS,
            Spa::Extensionless,
            "GET",
            "/main-ABCDEFGH.js",
            "identity",
            "",
        )
        .unwrap();
        assert_eq!(r.status, 406);
        let r = route(
            ASSETS,
            Spa::Extensionless,
            "GET",
            "/main-ABCDEFGH.js",
            "gzip",
            "W/\"2\"",
        )
        .unwrap();
        assert_eq!(r.status, 304);
        assert_eq!(
            route(ASSETS, Spa::Extensionless, "POST", "/", "", "")
                .unwrap()
                .status,
            405
        );
    }

    #[test]
    fn listed_routes_only_and_traversal_is_a_404() {
        let spa = Spa::Routes(&["", "/market", "/nana"]);
        let get = |path| route(ASSETS, spa, "GET", path, "gzip", "").unwrap();
        for path in ["/", "/market", "/market/", "/nana"] {
            assert_eq!(get(path).bytes, b"gz-html", "{path}");
        }
        for path in [
            "/secrets",
            "/apiary",
            "/../index.html",
            "/%2e%2e/index.html",
        ] {
            assert_eq!(get(path).status, 404, "{path}");
        }
        assert_eq!(get("/index.html").status, 200);
        assert!(route(&[], spa, "GET", "/", "", "").is_none());
    }

    #[test]
    fn identity_and_gzip_negotiation_follow_q_values() {
        let get =
            |accept| route(ASSETS, Spa::Extensionless, "GET", "/index.html", accept, "").unwrap();
        assert_eq!(get("gzip, br").bytes, b"gz-html");
        assert_eq!(get("gzip;q=0").bytes, b"<html>");
        assert_eq!(get("gzip;q=0.5, identity;q=0.1").bytes, b"gz-html");
        assert_eq!(get("gzip;q=0.1, identity;q=0.5").bytes, b"<html>");
        assert_eq!(get("gzip;q=7").bytes, b"<html>", "out-of-range q is 0");
        assert_eq!(get("*;q=0").status, 406);
        assert_eq!(get("").bytes, b"<html>");
        let r = get("gzip");
        assert!(r.headers.contains(&("Content-Encoding", "gzip")));
        assert!(r.headers.contains(&("Cache-Control", "no-cache")));
        // The gzip-only file is never answered with an empty identity body.
        for accept in ["", "gzip, deflate, br", "identity;q=1, gzip;q=0.1"] {
            let r = route(
                ASSETS,
                Spa::Extensionless,
                "GET",
                "/main-ABCDEFGH.js",
                accept,
                "",
            )
            .unwrap();
            assert_eq!(r.bytes, b"gz-js", "{accept}");
        }
        for accept in ["identity", "gzip;q=0", "*;q=0"] {
            let r = route(
                ASSETS,
                Spa::Extensionless,
                "GET",
                "/main-ABCDEFGH.js",
                accept,
                "",
            )
            .unwrap();
            assert_eq!(r.status, 406, "{accept}");
        }
    }

    #[test]
    fn head_is_get_without_a_body_upstream() {
        let r = route(ASSETS, Spa::Extensionless, "HEAD", "/", "gzip", "").unwrap();
        assert_eq!(r.status, 200);
    }

    #[test]
    fn etags_compare_weakly_in_lists() {
        assert!(etag_matches("W/\"2\"", "W/\"2\""));
        assert!(etag_matches("\"2\"", "W/\"2\""));
        assert!(etag_matches("\"other\", W/\"2\"", "\"2\""));
        assert!(etag_matches("*", "\"anything\""));
        assert!(!etag_matches("", "\"2\""));
        assert!(!etag_matches("\"3\"", "\"2\""));
        // A comma inside quotes belongs to the tag.
        assert!(!etag_matches("\"a,b\"", "\"b\""));
        assert!(etag_matches("\"a,b\"", "W/\"a,b\""));
    }
}
