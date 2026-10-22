//! "Sign in with Mastodon" (and any OAuth 2 provider shaped like it):
//! authorization code + PKCE (S256), confidential client, state bound to the
//! browser by a cookie. See `spec/OAUTH.md`.
//!
//! The framework knows OAuth and Mastodon's API shape, nothing about any
//! particular instance: an app configures providers by base URL.
//!
//! Every network step runs on a worker (the app calls [`OAuth::run_once`]
//! in a loop on its own thread), never in a request handler: one TLS
//! handshake to an internet service is 1-3 s on a board, and the serving
//! loop must not stall for it. The browser follows along with a ticket:
//!
//! 1. [`OAuth::start`] queues registration (first use) and returns a ticket;
//!    the app sets [`OAuth::cookie`] and the UI polls [`OAuth::poll`].
//! 2. `Poll::Redirect(url)`: the UI sends the browser to the instance.
//! 3. The instance redirects to the callback; [`OAuth::callback`] checks the
//!    state against the cookie and queues the code exchange.
//! 4. `Poll::Done(identity, intent)`, once: the app signs the person in.
//!
//! Access tokens are used once (to read who signed in) and revoked; the
//! board keeps none.
use crate::fetch::{form, percent_encode, Fetch, Outgoing};
use crate::kv::Kv;
use crate::session::{cookie, random_token};
use crate::wire::{self, Format};
use crate::{message, ApiError, Request};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Which certificates vouch for a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Signed by the household CA (a board on the LAN, like mastomini).
    Household,
    /// A public CA (mastodon.social and the rest of the internet).
    Public,
}

#[derive(Clone, Debug)]
pub struct Provider {
    /// The app's id for it (stable; used in URLs).
    pub id: String,
    /// `https://mastodon.social`, no trailing slash.
    pub base: String,
    pub trust: Trust,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub provider: String,
    /// The instance's host, e.g. `mastodon.social`.
    pub host: String,
    /// The account id on that instance: with `host`, the identity.
    pub subject: String,
    /// `@user@host`, for display (usernames can change; ids can't).
    pub handle: String,
    pub display_name: String,
    pub avatar: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Working on it (registering, or exchanging the code).
    Waiting,
    /// Send the browser here.
    Redirect(String),
    /// Signed in; returned once, then the ticket is gone.
    Done(Identity, String),
    Failed(String),
}

pub struct Config {
    /// `client_name` when registering with an instance.
    pub app_name: &'static str,
    /// `website` when registering (shown on the instance's consent page).
    pub website: String,
    /// Path the instance redirects back to.
    pub callback_path: &'static str,
    /// Cookie binding a sign-in to the browser that started it.
    pub cookie: &'static str,
    /// Most sign-ins in flight at once.
    pub capacity: usize,
    pub expiry: Duration,
    /// Scopes beyond reading the account (e.g. `write:statuses`), registered
    /// with every instance but asked for only by [`OAuth::start_keeping_token`].
    pub extra_scopes: &'static str,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_name: "miniframework app",
            website: String::new(),
            callback_path: "/auth/callback",
            cookie: "mf_oauth",
            capacity: 8,
            expiry: Duration::from_secs(600),
            extra_scopes: "",
        }
    }
}

message! {
    /// What the board remembers about an instance it registered with
    /// (one Kv blob per instance).
    pub struct Registration {
        1 client_id: String,
        2 client_secret: String,
        3 authorize_url: String,
        4 token_url: String,
        5 revoke_url: String,
        6 scope: String,
        7 redirect_uris: Vec<String>,
    }
}

message! {
    /// RFC 8414 discovery (Mastodon 4.3+); only the fields used.
    pub struct Discovery {
        1 authorization_endpoint: String,
        2 token_endpoint: String,
        3 revocation_endpoint: String,
        4 scopes_supported: Vec<String>,
    }
}

message! {
    /// `POST /api/v1/apps` reply; only the fields used.
    pub struct AppReply {
        1 client_id: String,
        2 client_secret: String,
    }
}

message! {
    pub struct TokenReply {
        1 access_token: String,
        2 token_type: String,
    }
}

message! {
    /// `verify_credentials`; only the fields used.
    pub struct Account {
        1 id: String,
        2 username: String,
        3 acct: String,
        4 display_name: String,
        5 avatar: String,
    }
}

enum Phase {
    /// Waiting for the worker to register (if needed) and build the URL.
    Starting,
    Redirect(String),
    /// Callback arrived; the worker exchanges `code`.
    Exchanging(String),
    Done(Identity),
    Failed(String),
}

struct Flight {
    ticket: String,
    provider: Provider,
    origin: String,
    intent: String,
    verifier: String,
    /// Ask for the extra scopes and keep the token ([`OAuth::take_token`]).
    keep: bool,
    phase: Phase,
    started: Instant,
}

enum Job {
    Start(String),
    Exchange(String),
}

pub struct OAuth {
    config: Config,
    kv: Arc<dyn Kv>,
    household: Arc<dyn Fetch>,
    public: Arc<dyn Fetch>,
    providers: Mutex<Vec<Provider>>,
    flights: Mutex<Vec<Flight>>,
    jobs: Mutex<VecDeque<Job>>,
    /// Tokens kept for the app: (provider, subject, token), until taken.
    kept: Mutex<Vec<(String, String, String)>>,
}

const LIMIT: usize = 32 * 1024;

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// PKCE S256 challenge for `verifier` (RFC 7636).
pub fn challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

fn host_of(base: &str) -> &str {
    base.split_once("://")
        .map_or(base, |(_, rest)| rest)
        .split(['/', ':'])
        .next()
        .unwrap_or("")
}

/// The Kv key for an instance's registration (Kv keys are ≤ 15 chars).
fn registration_key(base: &str) -> String {
    let digest = Sha256::digest(base.as_bytes());
    let hex: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    format!("oa{hex}")
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == name).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn fail(why: impl Into<String>) -> std::io::Error {
    std::io::Error::other(why.into())
}

impl OAuth {
    pub fn new(
        config: Config,
        kv: Arc<dyn Kv>,
        household: Arc<dyn Fetch>,
        public: Arc<dyn Fetch>,
    ) -> Self {
        Self {
            config,
            kv,
            household,
            public,
            providers: Mutex::new(Vec::new()),
            flights: Mutex::new(Vec::new()),
            jobs: Mutex::new(VecDeque::new()),
            kept: Mutex::new(Vec::new()),
        }
    }

    pub fn set_providers(&self, providers: Vec<Provider>) {
        *self.providers.lock().unwrap() = providers;
    }

    fn fetch(&self, trust: Trust) -> &dyn Fetch {
        match trust {
            Trust::Household => self.household.as_ref(),
            Trust::Public => self.public.as_ref(),
        }
    }

    fn redirect_uri(&self, origin: &str) -> String {
        format!("{origin}{}", self.config.callback_path)
    }

    /// Begins a sign-in with `provider` for a browser on `origin`
    /// (`http://gamematcher.local`, as the browser sees the app). `intent`
    /// comes back with the identity (e.g. "login" or "link:7").
    pub fn start(&self, provider: &str, origin: &str, intent: String) -> Result<String, ApiError> {
        self.begin(provider, origin, intent, false)
    }

    /// Like [`OAuth::start`], but asks for [`Config::extra_scopes`] too and
    /// keeps the access token instead of revoking it: collect it with
    /// [`OAuth::take_token`] once the sign-in is done. Keeping it safe is the
    /// app's job (flash isn't encrypted).
    pub fn start_keeping_token(
        &self,
        provider: &str,
        origin: &str,
        intent: String,
    ) -> Result<String, ApiError> {
        self.begin(provider, origin, intent, true)
    }

    /// The token a [`OAuth::start_keeping_token`] sign-in kept for
    /// `identity`; handed out once.
    pub fn take_token(&self, identity: &Identity) -> Option<String> {
        let mut kept = self.kept.lock().unwrap();
        let i = kept
            .iter()
            .position(|(p, s, _)| *p == identity.provider && *s == identity.subject)?;
        Some(kept.remove(i).2)
    }

    /// Revokes a kept token (unlinking). Blocks for the HTTP request: call
    /// it from a worker.
    pub fn revoke(&self, provider: &str, token: &str) -> std::io::Result<()> {
        let provider = self
            .providers
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.id == provider)
            .cloned()
            .ok_or_else(|| fail(format!("no provider {provider}")))?;
        let r: Registration = self
            .kv
            .get(&registration_key(&provider.base))
            .and_then(|bytes| wire::decode(Format::CborInt, &bytes).ok())
            .ok_or_else(|| fail("not registered with that instance"))?;
        let body = form(&[
            ("client_id", &r.client_id),
            ("client_secret", &r.client_secret),
            ("token", token),
        ]);
        let reply = self.fetch(provider.trust).send(&Outgoing {
            method: "POST",
            url: &r.revoke_url,
            headers: &[("Content-Type", "application/x-www-form-urlencoded")],
            body: body.as_bytes(),
            limit: 4096,
        })?;
        if reply.status != 200 {
            return Err(fail(format!("revoking: HTTP {}", reply.status)));
        }
        Ok(())
    }

    fn begin(
        &self,
        provider: &str,
        origin: &str,
        intent: String,
        keep: bool,
    ) -> Result<String, ApiError> {
        let provider = self
            .providers
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.id == provider)
            .cloned()
            .ok_or_else(|| ApiError::not_found("No such sign-in provider"))?;
        let ticket =
            random_token().map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
        let mut raw = [0u8; 32];
        getrandom::getrandom(&mut raw)
            .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
        let mut flights = self.flights.lock().unwrap();
        let expiry = self.config.expiry;
        flights.retain(|f| f.started.elapsed() < expiry);
        if flights.len() >= self.config.capacity {
            return Err(ApiError::new(
                503,
                "busy",
                "Too many sign-ins at once; try again in a minute",
            ));
        }
        flights.push(Flight {
            ticket: ticket.clone(),
            provider,
            origin: origin.trim_end_matches('/').to_string(),
            intent,
            verifier: base64url(&raw),
            keep,
            phase: Phase::Starting,
            started: Instant::now(),
        });
        drop(flights);
        self.jobs
            .lock()
            .unwrap()
            .push_back(Job::Start(ticket.clone()));
        Ok(ticket)
    }

    /// `Set-Cookie` binding `ticket` to this browser.
    pub fn cookie(&self, ticket: &str, secure: bool) -> String {
        format!(
            "{}={ticket}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
            self.config.cookie,
            self.config.expiry.as_secs(),
            if secure { "; Secure" } else { "" }
        )
    }

    /// Where a sign-in stands. Only the browser holding the ticket's cookie
    /// may ask. `Done` is handed out once.
    pub fn poll(&self, req: &Request<'_>, ticket: &str) -> Poll {
        let held = cookie(req, self.config.cookie).unwrap_or("");
        if !same(held, ticket) {
            return Poll::Failed("This sign-in belongs to another browser".into());
        }
        let mut flights = self.flights.lock().unwrap();
        let Some(i) = flights.iter().position(|f| f.ticket == ticket) else {
            return Poll::Failed("This sign-in expired; start again".into());
        };
        if flights[i].started.elapsed() >= self.config.expiry {
            flights.remove(i);
            return Poll::Failed("This sign-in expired; start again".into());
        }
        match &flights[i].phase {
            Phase::Starting | Phase::Exchanging(_) => Poll::Waiting,
            Phase::Redirect(url) => Poll::Redirect(url.clone()),
            Phase::Done(_) | Phase::Failed(_) => {
                let f = flights.remove(i);
                match f.phase {
                    Phase::Done(identity) => Poll::Done(identity, f.intent),
                    Phase::Failed(why) => Poll::Failed(why),
                    _ => unreachable!(),
                }
            }
        }
    }

    /// The instance's redirect back (`?code=&state=` or `?error=`). Checks
    /// the state against the browser's cookie and queues the exchange.
    /// Returns the ticket for the "finishing" page to poll.
    pub fn callback(&self, req: &Request<'_>) -> Result<String, ApiError> {
        let state = query_param(req.query, "state").unwrap_or_default();
        let held = cookie(req, self.config.cookie).unwrap_or("");
        if state.is_empty() || !same(held, &state) {
            return Err(ApiError::new(
                403,
                "state_mismatch",
                "This sign-in didn't start in this browser; start again",
            ));
        }
        let mut flights = self.flights.lock().unwrap();
        let flight = flights
            .iter_mut()
            .find(|f| f.ticket == state && matches!(f.phase, Phase::Redirect(_)))
            .ok_or_else(|| ApiError::new(410, "expired", "This sign-in expired; start again"))?;
        if let Some(error) = query_param(req.query, "error") {
            let why = query_param(req.query, "error_description").unwrap_or(error);
            flight.phase = Phase::Failed(format!("The instance said no: {why}"));
            return Ok(state);
        }
        let code = query_param(req.query, "code")
            .filter(|c| !c.is_empty())
            .ok_or_else(|| ApiError::bad_request("The instance sent no code"))?;
        flight.phase = Phase::Exchanging(code);
        drop(flights);
        self.jobs
            .lock()
            .unwrap()
            .push_back(Job::Exchange(state.clone()));
        Ok(state)
    }

    /// Does one queued network job, if any. Call it in a loop on a worker
    /// thread (it blocks for the length of the HTTP requests). Returns
    /// whether there was one.
    pub fn run_once(&self) -> bool {
        let Some(job) = self.jobs.lock().unwrap().pop_front() else {
            return false;
        };
        let ticket = match &job {
            Job::Start(t) | Job::Exchange(t) => t.clone(),
        };
        let Some((provider, origin, verifier, code, keep)) = ({
            let flights = self.flights.lock().unwrap();
            flights.iter().find(|f| f.ticket == ticket).map(|f| {
                let code = match &f.phase {
                    Phase::Exchanging(c) => Some(c.clone()),
                    _ => None,
                };
                (
                    f.provider.clone(),
                    f.origin.clone(),
                    f.verifier.clone(),
                    code,
                    f.keep,
                )
            })
        }) else {
            return true;
        };
        let outcome = match job {
            Job::Start(_) => self
                .authorize_url(&provider, &origin, &ticket, &verifier, keep)
                .map(Phase::Redirect),
            Job::Exchange(_) => self
                .exchange(
                    &provider,
                    &origin,
                    &verifier,
                    code.as_deref().unwrap_or(""),
                    keep,
                )
                .map(Phase::Done),
        };
        let phase = outcome.unwrap_or_else(|e| {
            log::warn!("sign-in with {}: {e}", provider.base);
            Phase::Failed(format!("Couldn't reach {}: {e}", host_of(&provider.base)))
        });
        if let Some(f) = self
            .flights
            .lock()
            .unwrap()
            .iter_mut()
            .find(|f| f.ticket == ticket)
        {
            f.phase = phase;
        }
        true
    }

    /// The stored registration with this instance, registering (again) if
    /// there is none or it lacks this origin's redirect URI.
    fn registration(&self, provider: &Provider, origin: &str) -> std::io::Result<Registration> {
        let key = registration_key(&provider.base);
        let redirect = self.redirect_uri(origin);
        let stored: Option<Registration> = self
            .kv
            .get(&key)
            .and_then(|bytes| wire::decode(Format::CborInt, &bytes).ok());
        let extra: Vec<&str> = self.config.extra_scopes.split_whitespace().collect();
        if let Some(r) = &stored {
            let scopes: Vec<&str> = r.scope.split_whitespace().collect();
            if r.redirect_uris.contains(&redirect) && extra.iter().all(|e| scopes.contains(e)) {
                return Ok(r.clone());
            }
        }
        let fetch = self.fetch(provider.trust);
        let discovery_url = format!("{}/.well-known/oauth-authorization-server", provider.base);
        let discovered = fetch
            .get(&discovery_url, "application/json", LIMIT)
            .ok()
            .filter(|r| r.status == 200)
            .and_then(|r| wire::decode::<Discovery>(Format::Json, &r.body).ok());
        let (authorize_url, token_url, revoke_url, base_scope) = match discovered {
            Some(d) => {
                let scope = if d.scopes_supported.iter().any(|s| s == "profile") {
                    "profile"
                } else {
                    "read:accounts"
                };
                (
                    d.authorization_endpoint,
                    d.token_endpoint,
                    d.revocation_endpoint,
                    scope.to_string(),
                )
            }
            // Before Mastodon 4.3: the well-known paths, the older scope.
            None => (
                format!("{}/oauth/authorize", provider.base),
                format!("{}/oauth/token", provider.base),
                format!("{}/oauth/revoke", provider.base),
                "read:accounts".to_string(),
            ),
        };
        let scope = std::iter::once(base_scope.as_str())
            .chain(extra.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        let mut redirect_uris = stored.map(|r| r.redirect_uris).unwrap_or_default();
        if !redirect_uris.contains(&redirect) {
            redirect_uris.push(redirect);
        }
        redirect_uris.truncate(8);
        let body = form(&[
            ("client_name", self.config.app_name),
            ("redirect_uris", &redirect_uris.join("\n")),
            ("scopes", &scope),
            ("website", &self.config.website),
        ]);
        let reply = fetch.send(&Outgoing {
            method: "POST",
            url: &format!("{}/api/v1/apps", provider.base),
            headers: &[
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Accept", "application/json"),
            ],
            body: body.as_bytes(),
            limit: LIMIT,
        })?;
        if reply.status != 200 {
            return Err(fail(format!("registering the app: HTTP {}", reply.status)));
        }
        let app: AppReply = wire::decode(Format::Json, &reply.body)
            .map_err(|e| fail(format!("registering the app: {e}")))?;
        let registration = Registration {
            client_id: app.client_id,
            client_secret: app.client_secret,
            authorize_url,
            token_url,
            revoke_url,
            scope,
            redirect_uris,
        };
        let bytes =
            wire::to_vec(Format::CborInt, &registration).map_err(|e| fail(e.to_string()))?;
        self.kv.set(&key, &bytes)?;
        Ok(registration)
    }

    fn authorize_url(
        &self,
        provider: &Provider,
        origin: &str,
        state: &str,
        verifier: &str,
        keep: bool,
    ) -> std::io::Result<String> {
        let r = self.registration(provider, origin)?;
        // A plain sign-in asks only to read the account.
        let extra: Vec<&str> = self.config.extra_scopes.split_whitespace().collect();
        let scope = if keep {
            r.scope.clone()
        } else {
            r.scope
                .split_whitespace()
                .filter(|s| !extra.contains(s))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let separator = if r.authorize_url.contains('?') {
            '&'
        } else {
            '?'
        };
        Ok(format!(
            "{}{separator}response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            r.authorize_url,
            percent_encode(&r.client_id),
            percent_encode(&self.redirect_uri(origin)),
            percent_encode(&scope),
            percent_encode(state),
            challenge(verifier),
        ))
    }

    fn exchange(
        &self,
        provider: &Provider,
        origin: &str,
        verifier: &str,
        code: &str,
        keep: bool,
    ) -> std::io::Result<Identity> {
        let r = self.registration(provider, origin)?;
        let fetch = self.fetch(provider.trust);
        let redirect = self.redirect_uri(origin);
        let body = form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
            ("client_id", &r.client_id),
            ("client_secret", &r.client_secret),
            ("code_verifier", verifier),
        ]);
        let reply = fetch.send(&Outgoing {
            method: "POST",
            url: &r.token_url,
            headers: &[
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Accept", "application/json"),
            ],
            body: body.as_bytes(),
            limit: LIMIT,
        })?;
        if reply.status != 200 {
            return Err(fail(format!("exchanging the code: HTTP {}", reply.status)));
        }
        let token: TokenReply = wire::decode(Format::Json, &reply.body)
            .map_err(|e| fail(format!("token reply: {e}")))?;
        if token.access_token.is_empty() {
            return Err(fail("the instance sent no token"));
        }
        let bearer = format!("Bearer {}", token.access_token);
        let me = fetch.send(&Outgoing {
            method: "GET",
            url: &format!("{}/api/v1/accounts/verify_credentials", provider.base),
            headers: &[("Authorization", &bearer), ("Accept", "application/json")],
            body: &[],
            limit: LIMIT,
        });
        let me = me?;
        if me.status != 200 {
            return Err(fail(format!("reading the account: HTTP {}", me.status)));
        }
        let account: Account =
            wire::decode(Format::Json, &me.body).map_err(|e| fail(format!("account: {e}")))?;
        if account.id.is_empty() {
            return Err(fail("the instance sent no account id"));
        }
        if keep {
            let mut kept = self.kept.lock().unwrap();
            kept.retain(|(p, s, _)| !(*p == provider.id && *s == account.id));
            if kept.len() >= self.config.capacity {
                kept.remove(0);
            }
            kept.push((
                provider.id.clone(),
                account.id.clone(),
                token.access_token.clone(),
            ));
        } else {
            self.give_back(fetch, &r, &provider.base, &token.access_token);
        }
        let host = host_of(&provider.base).to_string();
        Ok(Identity {
            provider: provider.id.clone(),
            handle: format!("@{}@{host}", account.username),
            host,
            subject: account.id,
            display_name: account.display_name,
            avatar: account.avatar,
        })
    }

    /// The token was only for one question: give it back.
    fn give_back(&self, fetch: &dyn Fetch, r: &Registration, base: &str, token: &str) {
        let revoke = form(&[
            ("client_id", &r.client_id),
            ("client_secret", &r.client_secret),
            ("token", token),
        ]);
        if let Err(e) = fetch.send(&Outgoing {
            method: "POST",
            url: &r.revoke_url,
            headers: &[("Content-Type", "application/x-www-form-urlencoded")],
            body: revoke.as_bytes(),
            limit: 4096,
        }) {
            log::info!("revoking the sign-in token at {base}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::PlainFetch;
    use crate::kv::MemKv;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn pkce_matches_rfc_7636_appendix_b() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn decodes_mastodon_json_ignoring_the_rest() {
        let json = r#"{"id":"109","username":"matt","acct":"matt","display_name":"Matt é","locked":false,
          "created_at":"2022-11-01T00:00:00.000Z","note":"<p>hi</p>","avatar":"https://x/a.png",
          "followers_count":12,"last_status_at":null,"source":{"privacy":"public","fields":[{"name":"a","value":"b"}]},
          "emojis":[],"fields":[{"name":"x","value":"y","verified_at":null}],"role":{"id":"-99","permissions":"0"}}"#;
        let a: Account = wire::decode(Format::Json, json.as_bytes()).unwrap();
        assert_eq!((a.id.as_str(), a.username.as_str()), ("109", "matt"));
        assert_eq!(a.display_name, "Matt é");
    }

    #[test]
    fn query_helpers() {
        assert_eq!(
            query_param("a=1&code=x%2By&state=s", "code").as_deref(),
            Some("x+y")
        );
        assert_eq!(percent_decode("a+b%20c"), "a b c");
        assert_eq!(host_of("https://mastodon.social"), "mastodon.social");
        assert_eq!(host_of("http://127.0.0.1:8099"), "127.0.0.1");
        assert!(registration_key("https://mastodon.social").len() <= 15);
    }

    /// A tiny Mastodon: discovery, app registration, token, account,
    /// revoke. Records every request it serves.
    fn mock_instance() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let me = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut s = stream.unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                // Read the head, then the body by Content-Length.
                loop {
                    let n = s.read(&mut buf).unwrap();
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + length || n == 0 {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&raw).to_string();
                let line = text.lines().next().unwrap_or("").to_string();
                log.lock().unwrap().push(text.clone());
                let body = if line.starts_with("GET /.well-known/oauth-authorization-server") {
                    format!(
                        r#"{{"issuer":"{me}/","authorization_endpoint":"{me}/oauth/authorize","token_endpoint":"{me}/oauth/token","revocation_endpoint":"{me}/oauth/revoke","scopes_supported":["read","profile"],"response_types_supported":["code"]}}"#
                    )
                } else if line.starts_with("POST /api/v1/apps") {
                    r#"{"id":"1","name":"t","client_id":"cid","client_secret":"secret","vapid_key":"v"}"#.into()
                } else if line.starts_with("POST /oauth/token") {
                    if text.contains("code=good") && text.contains("code_verifier=") {
                        r#"{"access_token":"tok","token_type":"Bearer","scope":"profile","created_at":1}"#.into()
                    } else {
                        let reply = "HTTP/1.1 400 Bad Request\r\nContent-Length: 2\r\n\r\n{}";
                        s.write_all(reply.as_bytes()).unwrap();
                        continue;
                    }
                } else if line.starts_with("GET /api/v1/accounts/verify_credentials") {
                    assert!(text.contains("Authorization: Bearer tok"));
                    r#"{"id":"42","username":"sam","acct":"sam","display_name":"Sam","avatar":"https://x/a.png","emojis":[],"fields":[]}"#.into()
                } else {
                    "{}".to_string()
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(reply.as_bytes()).unwrap();
            }
        });
        (base, seen)
    }

    fn oauth(base: &str) -> OAuth {
        let fetch: Arc<dyn Fetch> = Arc::new(PlainFetch::default());
        let o = OAuth::new(
            Config {
                app_name: "Test",
                website: "http://app.local".into(),
                ..Config::default()
            },
            Arc::new(MemKv::default()),
            fetch.clone(),
            fetch,
        );
        o.set_providers(vec![Provider {
            id: "home".into(),
            base: base.into(),
            trust: Trust::Household,
        }]);
        o
    }

    fn with_cookie(ticket: &str) -> Vec<(String, String)> {
        vec![("Cookie".into(), format!("mf_oauth={ticket}"))]
    }

    #[test]
    fn full_flow_against_a_mock_instance() {
        let (base, seen) = mock_instance();
        let o = oauth(&base);
        let ticket = o.start("home", "http://app.local", "login".into()).unwrap();
        let h = with_cookie(&ticket);
        let req = Request::new("GET", "/", &h, b"", false);
        assert_eq!(o.poll(&req, &ticket), Poll::Waiting);
        assert!(o.run_once());
        let Poll::Redirect(url) = o.poll(&req, &ticket) else {
            panic!("no redirect")
        };
        assert!(
            url.starts_with(&format!(
                "{base}/oauth/authorize?response_type=code&client_id=cid"
            )),
            "{url}"
        );
        assert!(url.contains("redirect_uri=http%3A%2F%2Fapp.local%2Fauth%2Fcallback"));
        assert!(url.contains("scope=profile") && url.contains("code_challenge_method=S256"));

        // Another browser can't use the ticket.
        let other = Request::new("GET", "/", &[], b"", false);
        assert!(matches!(o.poll(&other, &ticket), Poll::Failed(_)));

        let uri = format!("/auth/callback?code=good&state={ticket}");
        let back = Request::new("GET", &uri, &h, b"", false);
        assert_eq!(o.callback(&back).unwrap(), ticket);
        assert!(o.run_once());
        match o.poll(&req, &ticket) {
            Poll::Done(identity, intent) => {
                assert_eq!(intent, "login");
                assert_eq!(identity.subject, "42");
                assert_eq!(identity.handle, "@sam@127.0.0.1");
                assert_eq!(identity.display_name, "Sam");
            }
            other => panic!("{other:?}"),
        }
        // Done is handed out once.
        assert!(matches!(o.poll(&req, &ticket), Poll::Failed(_)));
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .any(|r| r.starts_with("POST /oauth/revoke") && r.contains("token=tok")),
            "token revoked"
        );
        assert!(seen
            .iter()
            .any(|r| r.contains("redirect_uris=http%3A%2F%2Fapp.local%2Fauth%2Fcallback")));
    }

    #[test]
    fn keeping_a_token_asks_for_more_and_skips_the_revoke() {
        let (base, seen) = mock_instance();
        let fetch: Arc<dyn Fetch> = Arc::new(PlainFetch::default());
        let o = OAuth::new(
            Config {
                app_name: "Test",
                extra_scopes: "write:statuses",
                ..Config::default()
            },
            Arc::new(MemKv::default()),
            fetch.clone(),
            fetch,
        );
        o.set_providers(vec![Provider {
            id: "home".into(),
            base: base.clone(),
            trust: Trust::Household,
        }]);
        // A plain sign-in still asks only to read.
        let t = o.start("home", "http://app.local", "login".into()).unwrap();
        o.run_once();
        let h = with_cookie(&t);
        let Poll::Redirect(url) = o.poll(&Request::new("GET", "/", &h, b"", false), &t) else {
            panic!("no redirect")
        };
        assert!(url.contains("scope=profile&"), "{url}");
        // Keeping the token asks for the extra scope too.
        let t = o
            .start_keeping_token("home", "http://app.local", "link:1".into())
            .unwrap();
        o.run_once();
        let h = with_cookie(&t);
        let req = Request::new("GET", "/", &h, b"", false);
        let Poll::Redirect(url) = o.poll(&req, &t) else {
            panic!("no redirect")
        };
        assert!(url.contains("scope=profile%20write%3Astatuses"), "{url}");
        let uri = format!("/auth/callback?code=good&state={t}");
        o.callback(&Request::new("GET", &uri, &h, b"", false))
            .unwrap();
        o.run_once();
        let Poll::Done(identity, _) = o.poll(&req, &t) else {
            panic!("not done")
        };
        assert_eq!(o.take_token(&identity).as_deref(), Some("tok"));
        assert_eq!(o.take_token(&identity), None, "handed out once");
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|r| r.starts_with("POST /api/v1/apps")
            && r.contains("scopes=profile+write%3Astatuses")
            || r.starts_with("POST /api/v1/apps")
                && r.contains("scopes=profile%20write%3Astatuses")));
        assert!(
            !seen.iter().any(|r| r.starts_with("POST /oauth/revoke")),
            "a kept token isn't revoked"
        );
        drop(seen);
        o.revoke("home", "tok").unwrap();
    }

    #[test]
    fn registration_is_reused_and_extended_per_origin() {
        let (base, seen) = mock_instance();
        let o = oauth(&base);
        for origin in ["http://app.local", "http://app.local", "https://app.local"] {
            let t = o.start("home", origin, "login".into()).unwrap();
            o.run_once();
            let h = with_cookie(&t);
            assert!(matches!(
                o.poll(&Request::new("GET", "/", &h, b"", false), &t),
                Poll::Redirect(_)
            ));
        }
        let apps: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST /api/v1/apps"))
            .cloned()
            .collect();
        assert_eq!(apps.len(), 2, "once for http, again when https appears");
        assert!(
            apps[1].contains("http%3A%2F%2Fapp.local%2Fauth%2Fcallback%0Ahttps%3A%2F%2Fapp.local"),
            "{}",
            apps[1]
        );
    }

    #[test]
    fn refusals() {
        let (base, _) = mock_instance();
        let o = oauth(&base);
        assert_eq!(
            o.start("nope", "http://a", String::new())
                .unwrap_err()
                .status,
            404
        );
        let t = o.start("home", "http://app.local", "login".into()).unwrap();
        o.run_once();
        // Callback without the cookie: refused.
        let uri = format!("/auth/callback?code=good&state={t}");
        assert_eq!(
            o.callback(&Request::new("GET", &uri, &[], b"", false))
                .unwrap_err()
                .status,
            403
        );
        // The person said no at the instance.
        let h = with_cookie(&t);
        let denied = format!(
            "/auth/callback?error=access_denied&error_description=The+user+denied&state={t}"
        );
        o.callback(&Request::new("GET", &denied, &h, b"", false))
            .unwrap();
        match o.poll(&Request::new("GET", "/", &h, b"", false), &t) {
            Poll::Failed(why) => assert!(why.contains("The user denied"), "{why}"),
            other => panic!("{other:?}"),
        }
        // A bad code fails cleanly.
        let t = o.start("home", "http://app.local", "login".into()).unwrap();
        o.run_once();
        let h = with_cookie(&t);
        let uri = format!("/auth/callback?code=bad&state={t}");
        o.callback(&Request::new("GET", &uri, &h, b"", false))
            .unwrap();
        o.run_once();
        assert!(matches!(
            o.poll(&Request::new("GET", "/", &h, b"", false), &t),
            Poll::Failed(_)
        ));
    }
}
