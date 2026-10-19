# OAuth sign-in and browser sessions

Status: **implemented**, October 17, 2026: `session` and `oauth` features. The
OAuth client runs every network step on an app-provided worker (`OAuth::run_once`)
and the browser follows a ticket (`start` → `poll` → `callback` → `poll`), rather
than the callback request waiting on a worker as sketched below. `Fetch` gained
`send` (any method, headers, body) on the board and the desktop (HTTPS via rustls
under `desktop-tls`). First user:
[Game Matcher](../../gamematcher/spec/02-accounts-auth.md).

## Why

Apps want "Sign in with <something the household already has>" instead of
another password, and they want browser sessions that survive a reboot.
Both are generic, so they belong in the framework, as **optional** features.

## Ground rules (owner decisions, October 17)

1. **Optional.** Two Cargo features, both off by default:
   `session` (cookie sessions, no network) and `oauth` (implies `session`).
   An app without them links none of this code.
2. **Provider-neutral.** The framework knows OAuth 2.0 and has a *Mastodon
   preset*. It does not know mastomini, NanaCoin or any specific instance.
   mastomini is "a Mastodon instance whose certificate the household CA signed";
   mastodon.social is "a Mastodon instance with a public certificate".
3. **The framework doesn't own users.** It hands the app a verified `Identity`
   and the app decides what that means (create an account, link, refuse).
4. **Local network only.** Boards are never internet-facing. The design does
   not try to resist internet-scale abuse. It does stay correct when the
   browser reached the board over plain HTTP.

## Sessions (`session` feature)

*Implemented as `miniframework::session`.* It differs from the sketch below in one
way: `Sessions` is plain data with no store trait. The app keeps it inside its own
state, persists `records()` with everything else, rebuilds it with
`Sessions::restore`, and flushes when `dirty()`. Times are passed in as
`Option<u64>` Unix seconds, `None` while the clock is unset (nothing expires
until it is). The guard is `require_same_origin(req, &extra_origins)`, matched
against the request's own `Host`.

```rust
let sessions = Sessions::new(SessionConfig {
    cookie: "gm_session",
    idle: Duration::from_secs(30 * 86400),
    absolute: Duration::from_secs(180 * 86400),
    per_subject: 4,
    capacity: 64,
}, store);                        // store: impl SessionStore (app-provided persistence)

let sid = sessions.create(subject_u32, req)?;      // → Set-Cookie header value
let who: Option<u32> = sessions.check(req);         // reads cookie, touches last_seen
sessions.revoke(req); sessions.revoke_subject(id);
```

- Token: 128 bits from the hardware RNG (`esp_fill_random`; `getrandom` on desktop), base64url.
  Only the SHA-256 is stored.
- Cookie: `HttpOnly; SameSite=Lax; Path=/`, plus `Secure` iff `req.secure`.
- `last_seen` updates are rate-limited (hourly) so persistence doesn't wear flash.
- `SessionStore` trait: `load_all`, `put`, `delete`. The app persists sessions
  with its own state (Game Matcher puts them in its snapshot). `MemSessionStore`
  for tests and for apps that are fine with "reboot signs everyone out".
- **Origin guard:** `require_same_origin(req, &allowed_hosts)` for unsafe methods.
  Allowed = the board's hostnames (`x.local`), its current IP, and any
  configured ports, on both schemes. A missing `Origin` falls back to `Referer`.
  If both are missing, the request is refused.

## OAuth client (`oauth` feature)

Authorization code + PKCE (S256), confidential client (the board keeps the
client secret), `state` bound to the browser.

```rust
let oauth = OAuth::new(fetch, kv);                   // HTTPS client + kv for registrations
oauth.add_provider(Provider::mastodon("mastodon.social", "https://mastodon.social", Trust::Public));
oauth.add_provider(Provider::mastodon("home", "https://mastomini.local", Trust::HouseholdCa));
oauth.add_provider(Provider::generic(GenericProvider {   // anything OAuth 2 + a userinfo endpoint
    id: "authelia", authorize_url, token_url, userinfo_url,
    subject_field: "sub", name_field: "preferred_username",
    client_id, client_secret, scopes: "openid profile", trust: Trust::Public,
}));

// GET /auth/{provider}
let redirect: Response = oauth.begin(req, "home", AppState::Login)?;
// GET /auth/callback  (path configurable)
match oauth.finish(req)? {
    Finished { identity, app_state, .. } => { /* app maps identity → account, creates session */ }
}
```

### Flow

1. `begin`: generate `state` (128 bit) and `code_verifier` (43–128 chars).
   Keep `{state → verifier, provider, redirect_uri, app_state, created}` in a
   bounded RAM table (8 entries, 10 min expiry, oldest evicted). Set a
   short-lived `mf_oauth` cookie holding a hash of `state`, so the callback must
   come from the same browser. 302 to the authorize URL with
   `code_challenge`, `code_challenge_method=S256`, `scope`, `state`, and
   `redirect_uri` = **the origin the browser used** + callback path
   (`http://x.local/auth/callback` or `https://…`).
2. `finish`: match `state` + cookie, single-use. On `error=` return
   `Denied`. POST the token endpoint (`grant_type=authorization_code`, `code`,
   `redirect_uri`, `code_verifier`, client auth). GET the identity endpoint with
   the bearer token.
3. Return `Identity { provider, subject, handle, display_name, avatar_url }`.
   **The access token is dropped** unless the app asked for it
   (`AppState` flag `keep_token`). Keeping it is the app's responsibility, because flash isn't encrypted.

The RAM-only pending table means a reboot mid-sign-in just fails that one
attempt. That's acceptable.

### Mastodon preset

- **Dynamic registration:** first use of a provider does `POST /api/v1/apps`
  (`client_name`, `redirect_uris` = both HTTP and HTTPS callback URIs,
  newline-separated, `scopes`, `website`). The client id/secret go to `kv`
  under a key derived from the instance host. If the registration later fails with
  `invalid_client`, the board re-registers once.
- **Discovery:** `GET /.well-known/oauth-authorization-server` (Mastodon 4.3+)
  for endpoints and `scopes_supported`. Fall back to `/oauth/authorize` and
  `/oauth/token` if absent.
- **Scope:** `profile` when advertised, else `read:accounts`.
- **Identity:** `GET /api/v1/accounts/verify_credentials` →
  `subject = id`, `handle = acct@host`, `display_name`, `avatar`.

### Trust anchors and the HTTPS client

The existing `Fetch` trait is GET-only and trusts only the household CA. `oauth` needs:

- `Fetch::post_form(url, fields, bearer, limit)` and a `bearer` on GET.
- Per-request **trust anchor**: `HouseholdCa` (the CA compiled in today) or
  `Public` (ESP-IDF `esp_crt_bundle_attach`, which costs flash for the bundle; enable
  `CONFIG_MBEDTLS_CERTIFICATE_BUNDLE` only for apps with a `Public` provider).
- Desktop: a rustls client under `desktop-tls`, with the same two anchors (household CA
  file, `webpki-roots`), so sign-in can be developed against real instances.
- **One outbound TLS connection at a time** on boards without PSRAM, enforced by
  an `OutboundSlot` the app can also use for its own jobs. Sign-in gets priority. A
  busy slot makes `begin`/`finish` return 503 + `Retry-After: 2` rather than block
  the serving loop. The token exchange runs on a worker task, never on the
  connection loop. The callback request waits on it with a 10 s deadline.

### Tests

- A mock provider (in-process, desktop) implementing authorize/token/
  verify_credentials, including the error paths: wrong state, reused code, wrong
  verifier, `invalid_client` re-registration, slow token endpoint (timeout).
- An optional live test against a real instance, behind an env var.
- No mastomini code or fixtures in the framework's tests.

## Later

- **OIDC:** when a provider returns `id_token`, verify it (JWKS fetched and cached,
  ES256/RS256, `iss`/`aud`/`exp`/`nonce`) instead of calling userinfo.
- Token refresh / storage helpers for apps that keep tokens.
