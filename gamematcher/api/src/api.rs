//! The Game Matcher HTTP API (`spec/04-api.md`). Every route answers in the
//! format the client negotiates; the UI uses JSON.
//!
//! Auth is the `gm_session` cookie. Every non-GET request must come from
//! the board's own pages (`require_same_origin`). Every change is committed
//! to flash before the response is sent; if the commit fails the change is
//! rolled back and the client gets 500.
use crate::announce::{self, News};
use crate::comments;
use crate::ics;
use crate::identity;
use crate::mail;
use crate::messages::*;
use crate::outbound::{self, MailSetup as MailConfig, Outbox};
use crate::pin::{self, Guard, GuardLimits};
use crate::screen::{self, cards::Context, BmpStream, Frame};
use crate::state::{self, Limits, State, SCHEMA};
use crate::store::SlotStore;
use crate::views;
use miniframework::kv::Kv;
use miniframework::oauth::{OAuth, Poll};
use miniframework::ops::{Check, Status as CheckStatus};
use miniframework::session::require_same_origin;
use miniframework::wire::{self, Format, Schema};
use miniframework::{uptime_ms, wall_ms, ApiError, Message, Reply, Request, Service};
use std::sync::{Arc, Mutex, MutexGuard};

/// A hash that never matches, verified for unknown emails so a miss costs
/// the same time as a wrong PIN.
const DUMMY_PIN: &str = "pbkdf2-sha256$1000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

/// Unix seconds, or `None` while the board's clock is unset.
fn wall() -> Option<u64> {
    wall_ms().map(|ms| ms / 1000)
}

struct Inner {
    state: State,
    store: SlotStore,
    guard: Guard,
    loaded_slot: u32,
    fallback: bool,
    snapshot_bytes: u32,
    /// Monotonic ms of the last flush of touch-only changes.
    last_flush: u64,
}

pub struct App {
    inner: Mutex<Inner>,
    /// Origins besides the board's own allowed to send unsafe requests
    /// (the Angular dev server).
    pub dev_origins: Vec<String>,
    pub setup_code: Option<&'static str>,
    pub build: &'static str,
    /// The same store the snapshot lives in; comment threads are blobs here.
    kv: Arc<dyn Kv>,
    /// "Sign in with Mastodon" (absent until [`App::attach_oauth`]).
    oauth: Option<Arc<OAuth>>,
    /// The household's Mastodon bot (absent until [`App::attach_outbox`]).
    outbox: Option<Arc<Outbox>>,
    public_outbox: Option<Arc<Outbox>>,
    /// The board's own address, for links in bot posts
    /// (`http://gamematcher.local`).
    pub origin: String,
    /// The household's UTC offset at an instant (seconds); the board asks
    /// newlib with its TZ string, the desktop uses a fixed offset.
    pub utc_offset: fn(u64) -> i64,
    /// Which card the board's screen shows (for `/api/v1/display/frame.bmp`).
    pub display_page: std::sync::atomic::AtomicUsize,
    /// Outstanding "forgot PIN" codes (RAM only: a reboot just means
    /// asking again).
    resets: Mutex<Vec<Reset>>,
    #[cfg(feature = "gate")]
    pub gate: Arc<crate::gate::Gate>,
}

/// A "forgot PIN" code sent to an account's email.
struct Reset {
    account: u32,
    code: String,
    /// Monotonic ms.
    sent: u64,
    tries: u32,
}

/// Codes work this long, and are resent at most this often.
const RESET_VALID_MS: u64 = 15 * 60_000;
const RESET_RESEND_MS: u64 = 60_000;
const RESET_TRIES: u32 = 5;

/// Who is calling.
struct Caller {
    id: u32,
    admin: bool,
}

impl App {
    /// Loads the newest snapshot from `kv`. Refuses (rather than starting
    /// empty) when both slots are unreadable.
    pub fn open(kv: Arc<dyn Kv>, limits: Limits) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_with(kv, limits, GuardLimits::default())
    }

    pub fn open_with(
        kv: Arc<dyn Kv>,
        limits: Limits,
        guard: GuardLimits,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (store, loaded) = SlotStore::open(kv.clone(), SCHEMA)?;
        let (state, loaded_slot, fallback, bytes) = match loaded {
            None => (State::new(limits), 0, false, 0),
            Some(l) => {
                let stored: StoredState = wire::decode(Format::CborInt, &l.payload)
                    .map_err(|e| format!("state snapshot: {e}"))?;
                (
                    State::from_stored(stored, limits),
                    l.slot as u32,
                    l.fallback,
                    l.payload.len() as u32,
                )
            }
        };
        Ok(Self {
            inner: Mutex::new(Inner {
                state,
                store,
                guard: Guard::new(guard),
                loaded_slot,
                fallback,
                snapshot_bytes: bytes,
                last_flush: 0,
            }),
            dev_origins: Vec::new(),
            kv,
            oauth: None,
            outbox: None,
            public_outbox: None,
            origin: "http://gamematcher.local".into(),
            utc_offset: |_| 0,
            display_page: Default::default(),
            resets: Mutex::new(Vec::new()),
            setup_code: None,
            build: "dev",
            #[cfg(feature = "gate")]
            gate: Arc::new(crate::gate::Gate::default()),
        })
    }

    /// Turns on Mastodon sign-in. The caller runs `oauth.run_once()` in a
    /// loop on a worker thread (outbound HTTPS must not block serving).
    pub fn attach_oauth(&mut self, oauth: Arc<OAuth>) {
        self.oauth = Some(oauth);
        self.sync_providers();
    }

    /// Turns on the household bot. The caller runs `outbox.run_once(now)`
    /// on the same worker thread as OAuth.
    pub fn attach_outbox(&mut self, outbox: Arc<Outbox>) {
        self.outbox = Some(outbox);
    }

    pub fn attach_public_outbox(&mut self, outbox: Arc<Outbox>) {
        self.public_outbox = Some(outbox);
    }

    fn outbox(&self) -> Result<&Outbox, ApiError> {
        self.outbox
            .as_deref()
            .ok_or_else(|| ApiError::new(409, "no_outbox", "This build has no outbound Mastodon"))
    }

    /// Announce only to the networks selected by the host.
    fn announce_plan(&self, id: u32, news: News, now: u64) {
        let inner = self.lock();
        let Ok(p) = inner.state.plan(id) else { return };
        for (outbox, enabled) in [
            (&self.outbox, p.announce),
            (&self.public_outbox, p.announce_public),
        ] {
            if !enabled {
                continue;
            }
            if let Some(o) = outbox {
                let text = announce::plan_post(
                    &inner.state,
                    p,
                    news,
                    &self.origin,
                    now,
                    (self.utc_offset)(now),
                );
                let footer = format!(
                    "
Reply in, maybe or out to this post, or mention me with: in {}
{}/plans/{}",
                    p.id, self.origin, p.id
                );
                let text = format!(
                    "{}{}",
                    announce::fit(&text, 500usize.saturating_sub(footer.chars().count())),
                    footer
                );
                o.enqueue("post", text, None, now);
            }
        }
    }

    /// The morning digest, if it's time and anything is planned.
    fn digest_if_due(&self, state: &State, now: u64) {
        let offset = (self.utc_offset)(now);
        let local = screen::clock::Local::at(now, offset);
        let today = format!("{:04}-{:02}-{:02}", local.year, local.month, local.day);
        let hhmm = format!("{:02}:{:02}", local.hour, local.minute);
        for (bot, public) in [
            (self.outbox.as_deref(), false),
            (self.public_outbox.as_deref(), true),
        ] {
            if let Some(o) = bot {
                if o.digest_due(&today, &hhmm) {
                    if let Some(text) =
                        announce::digest_for(state, &self.origin, now, offset, public)
                    {
                        o.enqueue("digest", text, None, now);
                    }
                }
            }
        }
    }

    fn sync_providers(&self) {
        if let Some(oauth) = &self.oauth {
            let providers = self
                .lock()
                .state
                .providers
                .iter()
                .map(identity::to_provider)
                .collect();
            oauth.set_providers(providers);
        }
    }

    fn oauth(&self) -> Result<&OAuth, ApiError> {
        self.oauth.as_deref().ok_or_else(|| {
            ApiError::new(
                404,
                "no_oauth",
                "Mastodon sign-in isn't available on this build",
            )
        })
    }

    /// The screen's rotation right now.
    pub fn display_deck(&self) -> Vec<DisplayCard> {
        let now = wall().unwrap_or(0);
        screen::deck::build(&self.lock().state, now)
    }

    /// A card ready to draw. `origin` is what QR codes point at.
    pub fn display_frame(
        &self,
        card: DisplayCard,
        page: usize,
        pages: usize,
        online: bool,
        origin: &str,
        name: &str,
    ) -> Frame {
        let now = wall().unwrap_or(0);
        Frame::new(
            card,
            Context {
                now,
                offset: (self.utc_offset)(now),
                page,
                pages,
                online,
                origin: origin.to_string(),
                name: name.to_string(),
                qr: None,
            },
        )
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the state as the next generation.
    fn commit(inner: &mut Inner) -> Result<(), ApiError> {
        inner.state.commits += 1;
        let bytes =
            wire::to_vec(Format::CborInt, &inner.state.to_stored()).map_err(ApiError::from)?;
        if let Err(e) = inner.store.commit(SCHEMA, &bytes) {
            inner.state.commits -= 1;
            log::error!("state commit failed: {e}");
            return Err(ApiError::new(
                500,
                "storage_failed",
                "Could not save; nothing changed",
            ));
        }
        inner.snapshot_bytes = bytes.len() as u32;
        inner.state.sessions.saved();
        Ok(())
    }

    /// Runs `f` on the state and commits; any error leaves the state as it was.
    fn mutate<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R, ApiError>) -> Result<R, ApiError> {
        let mut inner = self.lock();
        let backup = inner.state.to_stored();
        let result = f(&mut inner).and_then(|r| Self::commit(&mut inner).map(|_| r));
        if result.is_err() {
            let limits = inner.state.limits.clone();
            inner.state = State::from_stored(backup, limits);
        }
        self.drop_threads(&mut inner);
        self.flush_mail(&mut inner);
        result
    }

    /// Invalid/unlinked incoming messages must not wear flash with empty commits.
    fn mutate_optional<R>(
        &self,
        f: impl FnOnce(&mut Inner) -> Result<Option<R>, ApiError>,
    ) -> Result<Option<R>, ApiError> {
        let mut inner = self.lock();
        let backup = inner.state.to_stored();
        let result = f(&mut inner).and_then(|r| {
            if r.is_some() {
                Self::commit(&mut inner)?;
            }
            Ok(r)
        });
        if result.is_err() {
            inner.state = State::from_stored(backup, inner.state.limits.clone());
        }
        self.drop_threads(&mut inner);
        self.flush_mail(&mut inner);
        result
    }

    /// Queues email and private Mastodon deliveries from the last change.
    fn flush_mail(&self, inner: &mut Inner) {
        let pending = std::mem::take(&mut inner.state.mail);
        let (Some(o), Some(now)) = (&self.outbox, wall()) else {
            return;
        };
        if pending.is_empty() {
            return;
        }
        let offset = (self.utc_offset)(now);
        for n in &pending {
            let Some(account) = inner.state.account(n.to).filter(|a| !a.disabled) else {
                continue;
            };
            if o.mail_ready() && !account.email_off {
                if let Some(email) = mail::notification(&inner.state, n, &self.origin, now, offset)
                {
                    o.email(email, now);
                }
            }
            if matches!(
                n.kind.as_str(),
                crate::notify::REMINDER | crate::notify::CHANGED
            ) {
                if let Some(m) = account.mastodon.as_ref() {
                    let destination = self.bot_for(&m.host);
                    if let Some(destination) = destination.filter(|_| m.handle.starts_with('@')) {
                        if let Ok(p) = inner.state.plan(n.plan) {
                            let label = if n.kind == crate::notify::REMINDER {
                                "Starting soon"
                            } else {
                                "Plan updated"
                            };
                            let text = format!(
                                "{}
{label}: {}
{}
{}",
                                m.handle,
                                p.title,
                                mail::times(p, now, offset),
                                mail::plan_link(&self.origin, p)
                            );
                            destination.enqueue("dm", text, Some("direct"), now);
                        }
                    }
                }
            }
        }
    }

    fn mailer(&self) -> Result<&Outbox, ApiError> {
        self.outbox
            .as_deref()
            .filter(|o| o.mail_ready())
            .ok_or_else(|| ApiError::new(409, "no_mail", "Email isn't set up on this board"))
    }

    /// Checked addresses from an invite form.
    fn addresses(input: &InviteEmails) -> Result<Vec<String>, ApiError> {
        let list: Vec<String> = input
            .emails
            .iter()
            .flat_map(|e| e.split([',', ';', '\n', ' ']))
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        if list.is_empty() || list.len() > mail::MAX_INVITES {
            return Err(ApiError::bad_request(format!(
                "Up to {} email addresses",
                mail::MAX_INVITES
            )));
        }
        if let Some(bad) = list.iter().find(|e| !mail::valid_address(e)) {
            return Err(ApiError::bad_request(format!(
                "{bad} doesn't look like an email address"
            )));
        }
        Ok(list)
    }

    /// Mails a "forgot PIN" code if `email` is an account's (and says
    /// nothing either way, so the form can't be used to test addresses).
    fn start_reset(&self, email: &str) -> Result<(), ApiError> {
        let o = self.mailer()?;
        let mono = uptime_ms();
        let Some((id, name, address)) = self
            .lock()
            .state
            .by_email(email)
            .filter(|a| !a.disabled)
            .map(|a| (a.id, a.display_name.clone(), a.email.clone()))
        else {
            return Ok(());
        };
        let mut resets = self.resets.lock().unwrap_or_else(|e| e.into_inner());
        resets.retain(|r| mono.saturating_sub(r.sent) < RESET_VALID_MS);
        if resets
            .iter()
            .any(|r| r.account == id && mono.saturating_sub(r.sent) < RESET_RESEND_MS)
        {
            return Ok(());
        }
        let mut bytes = [0u8; 4];
        getrandom::getrandom(&mut bytes)
            .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
        let code = format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000);
        if o.email(
            mail::pin_code(&name, &address, &code, &self.origin),
            wall().unwrap_or(0),
        ) {
            resets.retain(|r| r.account != id);
            resets.push(Reset {
                account: id,
                code,
                sent: mono,
                tries: 0,
            });
        }
        Ok(())
    }

    /// Checks an emailed code and sets the new PIN; signs in on success.
    fn finish_reset(
        &self,
        body: &PinResetConfirm,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
    ) -> Result<Me, ApiError> {
        let wrong = || {
            ApiError::new(
                401,
                "wrong_code",
                "That code is wrong or too old; ask for a new one",
            )
        };
        let id = self
            .lock()
            .state
            .by_email(&body.email)
            .map(|a| a.id)
            .ok_or_else(wrong)?;
        state::check_pin(&body.pin)?;
        {
            let mono = uptime_ms();
            let mut resets = self.resets.lock().unwrap_or_else(|e| e.into_inner());
            let i = resets
                .iter()
                .position(|r| r.account == id && mono.saturating_sub(r.sent) < RESET_VALID_MS)
                .ok_or_else(wrong)?;
            if !same_hex(&resets[i].code, body.code.trim()) {
                resets[i].tries += 1;
                if resets[i].tries >= RESET_TRIES {
                    resets.remove(i);
                }
                return Err(wrong());
            }
            resets.remove(i);
        }
        let me = self.mutate(|inner| {
            inner.state.reset_pin(id, &body.pin)?;
            inner.guard.clear(id);
            Self::sign_in(inner, id, req, reply)?;
            Self::me(&inner.state, id)
        })?;
        log::info!("account {id} set a new PIN with an emailed code");
        Ok(me)
    }

    /// `/api/v1/admin/mail[/test]`
    fn mail_route(
        &self,
        method: &str,
        rest: &str,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        now: u64,
    ) -> Result<(), ApiError> {
        let admin = self.admin(req)?;
        let o = self.outbox()?;
        match (method, rest) {
            ("GET", "") => {}
            ("POST", "") => {
                let s: MailSetup = req.decode()?;
                o.begin_mail_setup(MailConfig {
                    api_key: s.api_key,
                    sender_email: s.sender_email,
                    sender_name: s.sender_name,
                    built_in: false,
                })?;
            }
            ("DELETE", "") => o.remove_mail(),
            ("POST", "/test") => {
                let (name, address) = {
                    let inner = self.lock();
                    let a = inner
                        .state
                        .account(admin.id)
                        .ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
                    (a.display_name.clone(), a.email.clone())
                };
                if address.is_empty() {
                    return Err(ApiError::bad_request(
                        "Your account has no email address to send to",
                    ));
                }
                let email = mail::Email {
                    to: address,
                    to_name: name.clone(),
                    subject: "Game Matcher can send email".into(),
                    text: format!("Hi {name},\n\nThis is the board's test email. Invites, \"forgot PIN\" codes and plan news will come like this.\n\n-- \nGame Matcher at home ({})", self.origin),
                };
                if !self.mailer()?.email(email, now) {
                    return Err(ApiError::new(
                        429,
                        "mail_limit",
                        "Today's email allowance is used up",
                    ));
                }
            }
            _ => return Err(no_route()),
        }
        reply.wire(req, &o.mail_view());
        Ok(())
    }

    /// Deletes the comment blobs of plans that left the past-plans ring.
    fn drop_threads(&self, inner: &mut Inner) {
        for plan in inner.state.dropped_threads.drain(..) {
            if let Err(e) = self.kv.remove(&comments::key(plan)) {
                log::warn!("removing comments of plan {plan}: {e}");
            }
        }
    }

    fn load_thread(&self, plan: u32) -> StoredThread {
        self.kv
            .get(&comments::key(plan))
            .and_then(|bytes| wire::decode(Format::CborInt, &bytes).ok())
            .unwrap_or_default()
    }

    fn save_thread(&self, plan: u32, thread: &StoredThread) -> Result<(), ApiError> {
        let key = comments::key(plan);
        let result = if thread.comments.is_empty() {
            self.kv.remove(&key)
        } else {
            let bytes = wire::to_vec(Format::CborInt, thread).map_err(ApiError::from)?;
            self.kv.set(&key, &bytes)
        };
        result.map_err(|e| {
            log::error!("saving comments of plan {plan}: {e}");
            ApiError::new(500, "storage_failed", "Could not save the comment")
        })
    }

    /// Housekeeping, called from the board's main loop: persists session
    /// touches at most once a minute (flash wear).
    pub fn tick(&self) {
        if let Some(now) = wall() {
            self.receive_replies(now);
        }
        let now = uptime_ms();
        let mut inner = self.lock();
        if now.saturating_sub(inner.last_flush) < 60_000 {
            return;
        }
        inner.last_flush = now;
        if let Some(w) = wall() {
            self.digest_if_due(&inner.state, w);
        }
        // Locked-in plans whose time has passed become done (needs a clock).
        let plans_moved = wall().is_some_and(|w| {
            let done = inner.state.advance_plans(w);
            inner.state.send_reminders(w) || done
        });
        if !plans_moved && !inner.state.sessions.dirty() {
            return;
        }
        inner.state.sessions.prune(wall());
        if let Err(e) = Self::commit(&mut inner) {
            log::warn!("housekeeping commit: {}", e.message);
        }
        self.drop_threads(&mut inner);
        self.flush_mail(&mut inner);
    }

    fn caller(&self, req: &Request<'_>) -> Result<Caller, ApiError> {
        let mut inner = self.lock();
        let id = inner
            .state
            .sessions
            .check(req, wall())
            .ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
        match inner.state.account(id) {
            Some(a) if !a.disabled => Ok(Caller { id, admin: a.admin }),
            _ => {
                inner.state.sessions.revoke(req);
                Err(ApiError::unauthorized("Sign in first"))
            }
        }
    }

    fn admin(&self, req: &Request<'_>) -> Result<Caller, ApiError> {
        let caller = self.caller(req)?;
        if caller.admin {
            Ok(caller)
        } else {
            Err(ApiError::new(
                403,
                "admin_only",
                "Only an admin can do that",
            ))
        }
    }

    fn me(state: &State, id: u32) -> Result<Me, ApiError> {
        let a = state
            .account(id)
            .ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
        Ok(Me {
            id: a.id,
            email: a.email.clone(),
            display_name: a.display_name.clone(),
            role: state::role(a).into(),
            has_pin: !a.pin.is_empty(),
            sessions: state.sessions.count(id) as u32,
            created: a.created,
            email_notify: !a.email.is_empty() && !a.email_off,
            mastodon: a
                .mastodon
                .as_ref()
                .map(|m| m.handle.clone())
                .unwrap_or_default(),
        })
    }

    /// Starts a session for `id` and sets its cookie.
    fn sign_in(
        inner: &mut Inner,
        id: u32,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
    ) -> Result<(), ApiError> {
        let now = wall();
        let token = inner
            .state
            .sessions
            .create(id, now)
            .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
        if let (Some(now), Ok(a)) = (now, inner.state.account_mut(id)) {
            a.last_seen = now;
        }
        reply.header(
            "Set-Cookie",
            inner.state.sessions.set_cookie(&token, req.secure),
        );
        Ok(())
    }

    fn status(&self, req: &Request<'_>) -> Status {
        let inner = self.lock();
        Status {
            app: "gamematcher".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            build: self.build.into(),
            setup_needed: inner.state.accounts.is_empty(),
            signup_open: !inner.state.signup_closed,
            clock_synced: wall().is_some(),
            setup_code_required: self.setup_code.is_some(),
            secure: req.secure,
            providers: if inner.state.accounts.is_empty() || self.oauth.is_none() {
                Vec::new()
            } else {
                inner
                    .state
                    .providers
                    .iter()
                    .map(|p| ProviderButton {
                        id: p.id,
                        name: p.name.clone(),
                    })
                    .collect()
            },
            bot: self
                .outbox
                .as_ref()
                .and_then(|o| o.handle())
                .unwrap_or_default(),
            mail: self.outbox.as_ref().is_some_and(|o| o.mail_ready()),
            public_bot: self
                .public_outbox
                .as_ref()
                .and_then(|o| o.handle())
                .unwrap_or_default(),
        }
    }

    fn route(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        let path = req.path;
        if path == "/auth/callback" && req.method == "GET" {
            // The instance sends the browser back here; continue on the
            // "finishing" page, which polls the ticket.
            let location = match self.oauth().and_then(|o| o.callback(req)) {
                Ok(ticket) => format!("/signin/finishing?t={ticket}"),
                Err(e) => format!(
                    "/signin?error={}",
                    miniframework::fetch::percent_encode(&e.message)
                ),
            };
            reply.bytes(302, "text/plain; charset=utf-8", b"");
            reply.header("Location", location);
            reply.header("Cache-Control", "no-store");
            return Ok(());
        }
        if let Some(rest) = path.strip_prefix("/cal/") {
            if req.method == "GET" {
                self.feed_route(rest, reply)?;
            }
            return Ok(());
        }
        if !path.starts_with("/api/v1/") {
            return Ok(());
        }
        require_same_origin(req, &self.dev_origins)?;
        let now = wall().unwrap_or(0);
        match (req.method, path) {
            ("GET", "/api/v1/status") => reply.wire(req, &self.status(req)),

            ("POST", "/api/v1/setup") => {
                let new: NewAccount = req.decode()?;
                if let Some(code) = self.setup_code {
                    if new.setup_code != code {
                        return Err(ApiError::new(403, "wrong_setup_code", "Wrong setup code"));
                    }
                }
                let me = self.mutate(|inner| {
                    let id = inner
                        .state
                        .setup(&new.email, &new.display_name, &new.pin, now)?;
                    Self::sign_in(inner, id, req, reply)?;
                    Self::me(&inner.state, id)
                })?;
                log::info!("first-run setup: admin account {} created", me.id);
                reply.wire_status(req, 201, &me);
            }

            ("POST", "/api/v1/auth/signup") => {
                let new: NewAccount = req.decode()?;
                let me = self.mutate(|inner| {
                    let id = inner
                        .state
                        .signup(&new.email, &new.display_name, &new.pin, now)?;
                    Self::sign_in(inner, id, req, reply)?;
                    Self::me(&inner.state, id)
                })?;
                reply.wire_status(req, 201, &me);
            }

            ("POST", "/api/v1/auth/pin-reset") => {
                let body: PinResetStart = req.decode()?;
                self.start_reset(&body.email)?;
                reply.wire(req, &Done { ok: true });
            }

            ("POST", "/api/v1/auth/pin-reset/confirm") => {
                let body: PinResetConfirm = req.decode()?;
                let me = self.finish_reset(&body, req, reply)?;
                reply.wire(req, &me);
            }

            ("POST", "/api/v1/invite") => {
                let c = self.caller(req)?;
                let to = Self::addresses(&req.decode()?)?;
                let o = self.mailer()?;
                let emails: Vec<mail::Email> = {
                    let inner = self.lock();
                    if inner.state.signup_closed && !c.admin {
                        return Err(ApiError::new(
                            403,
                            "signup_closed",
                            "Sign-up is closed; ask an admin to add them",
                        ));
                    }
                    to.iter()
                        .map(|a| mail::site_invite(&inner.state, c.id, a, &self.origin))
                        .collect()
                };
                let sent = emails
                    .into_iter()
                    .filter(|e| o.email(e.clone(), now))
                    .count() as u32;
                reply.wire(req, &Sent { sent });
            }

            ("POST", "/api/v1/auth/pin") => {
                let body: PinSignIn = req.decode()?;
                let me = self.pin_sign_in(&body, req, reply)?;
                reply.wire(req, &me);
            }

            ("POST", "/api/v1/auth/logout") => {
                let all = req.param("all").is_some_and(|v| v != "0");
                let caller = self.caller(req).ok();
                self.mutate(|inner| {
                    match (all, caller) {
                        (true, Some(c)) => {
                            inner.state.sessions.revoke_subject(c.id);
                        }
                        _ => {
                            inner.state.sessions.revoke(req);
                        }
                    }
                    Ok(())
                })?;
                reply.header(
                    "Set-Cookie",
                    self.lock().state.sessions.clear_cookie(req.secure),
                );
                reply.wire(req, &Done { ok: true });
            }

            ("POST", "/api/v1/auth/oauth/start") => {
                let start: OAuthStart = req.decode()?;
                let intent = if start.link {
                    format!("link:{}", self.caller(req)?.id)
                } else {
                    format!("login:{}", start.provider)
                };
                self.lock().state.provider(start.provider)?;
                let host = req.header("Host").trim();
                let origin = format!("{}://{host}", if req.secure { "https" } else { "http" });
                let oauth = self.oauth()?;
                let ticket = oauth.start(&start.provider.to_string(), &origin, intent)?;
                reply.header("Set-Cookie", oauth.cookie(&ticket, req.secure));
                reply.wire(req, &OAuthTicket { ticket });
            }

            ("DELETE", "/api/v1/me/mastodon") => {
                let c = self.caller(req)?;
                self.mutate(|inner| inner.state.unlink_identity(c.id))?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", "/api/v1/admin/providers") => {
                self.admin(req)?;
                let list = views::providers(&self.lock().state);
                reply.wire(req, &list);
            }

            ("POST", "/api/v1/admin/providers") => {
                self.admin(req)?;
                let input: ProviderInput = req.decode()?;
                self.mutate(|inner| inner.state.add_provider(&input))?;
                self.sync_providers();
                let list = views::providers(&self.lock().state);
                reply.wire_status(req, 201, &list);
            }

            ("GET", "/api/v1/display") => {
                // For remote displays: the cards, first names only.
                let deck = DisplayDeck {
                    cards: self.display_deck(),
                    generated: wall().unwrap_or(0),
                };
                reply.wire(req, &deck);
            }

            ("GET", "/api/v1/display/frame.bmp") => {
                // Exactly what the board's screen draws, rendered a strip at a
                // time as it's sent: `?card=N`, or the card showing now.
                let cards = self.display_deck();
                let shown = self.display_page.load(std::sync::atomic::Ordering::Relaxed);
                let page = req.param_num::<usize>("card")?.unwrap_or(shown) % cards.len();
                let host = req.header("Host").trim();
                let origin = format!("{}://{host}", if req.secure { "https" } else { "http" });
                let pages = cards.len();
                let card = cards.into_iter().nth(page).expect("page is in range");
                let frame = self.display_frame(card, page, pages, true, &origin, host);
                reply.stream(
                    200,
                    "image/bmp",
                    screen::BMP_LEN,
                    Box::new(BmpStream::new(frame)),
                );
                reply.header("Cache-Control", "no-store");
            }

            ("GET", "/api/v1/me/summary") => {
                let c = self.caller(req)?;
                let inner = self.lock();
                let st = &inner.state;
                let awaiting = st
                    .plans
                    .iter()
                    .filter(|p| {
                        p.status == crate::plans::OPEN
                            && !p.responses.iter().any(|r| r.account == c.id)
                    })
                    .count() as u32;
                let latest = st
                    .account(c.id)
                    .and_then(|a| a.notifications.first())
                    .map_or(0, |n| n.id);
                reply.wire(
                    req,
                    &Summary {
                        unread: st.unread(c.id),
                        awaiting,
                        latest,
                    },
                );
            }

            ("GET", "/api/v1/notifications") => {
                let c = self.caller(req)?;
                let list = views::notifications(&self.lock().state, c.id);
                reply.wire(req, &list);
            }

            ("POST", "/api/v1/notifications/read") => {
                let c = self.caller(req)?;
                let mark: MarkRead = req.decode()?;
                let changed = self.lock().state.mark_read(c.id, mark.up_to);
                if changed {
                    // Read flags aren't worth a flash write of their own.
                    self.mutate(|_| Ok(()))?;
                }
                reply.wire(req, &Done { ok: true });
            }

            ("GET", "/api/v1/me/feed") => {
                let c = self.caller(req)?;
                let enabled = self
                    .lock()
                    .state
                    .account(c.id)
                    .is_some_and(|a| !a.feed_hash.is_empty());
                reply.wire(
                    req,
                    &FeedUrl {
                        enabled,
                        ..Default::default()
                    },
                );
            }

            ("POST" | "DELETE", "/api/v1/me/feed") => {
                let c = self.caller(req)?;
                let token = (req.method == "POST").then(new_token).transpose()?;
                self.mutate(|inner| {
                    inner.state.account_mut(c.id)?.feed_hash =
                        token.as_deref().map(hash_hex).unwrap_or_default();
                    Ok(())
                })?;
                reply.wire(
                    req,
                    &feed_url(req, token.as_deref().map(|t| format!("/cal/{t}.ics"))),
                );
            }

            ("GET", "/api/v1/admin/house-feed") => {
                self.admin(req)?;
                let enabled = !self.lock().state.house_feed_hash.is_empty();
                reply.wire(
                    req,
                    &FeedUrl {
                        enabled,
                        ..Default::default()
                    },
                );
            }

            ("POST" | "DELETE", "/api/v1/admin/house-feed") => {
                self.admin(req)?;
                let token = (req.method == "POST").then(new_token).transpose()?;
                self.mutate(|inner| {
                    inner.state.house_feed_hash =
                        token.as_deref().map(hash_hex).unwrap_or_default();
                    Ok(())
                })?;
                reply.wire(
                    req,
                    &feed_url(req, token.as_deref().map(|t| format!("/cal/house/{t}.ics"))),
                );
            }

            ("GET", "/api/v1/me") => {
                let c = self.caller(req)?;
                let me = Self::me(&self.lock().state, c.id)?;
                reply.wire(req, &me);
            }

            ("PATCH", "/api/v1/me") => {
                let c = self.caller(req)?;
                let update: MeUpdate = req.decode()?;
                let me = self.mutate(|inner| {
                    if let Some(name) = &update.display_name {
                        let name = state::normalize_name(name)?;
                        inner.state.account_mut(c.id)?.display_name = name;
                    }
                    if let Some(on) = update.email_notify {
                        inner.state.account_mut(c.id)?.email_off = !on;
                    }
                    Self::me(&inner.state, c.id)
                })?;
                reply.wire(req, &me);
            }

            ("PUT", "/api/v1/me/pin") => {
                let c = self.caller(req)?;
                let change: PinChange = req.decode()?;
                self.mutate(|inner| {
                    inner
                        .state
                        .change_pin(c.id, &change.current, &change.new_pin)
                })?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", "/api/v1/users") => {
                self.caller(req)?;
                let inner = self.lock();
                let users = inner
                    .state
                    .accounts
                    .iter()
                    .filter(|a| !a.disabled)
                    .map(|a| UserSummary {
                        id: a.id,
                        display_name: a.display_name.clone(),
                        role: state::role(a).into(),
                    })
                    .collect();
                reply.wire(req, &UserList { users });
            }

            ("GET", "/api/v1/admin/users") => {
                self.admin(req)?;
                let mono = uptime_ms();
                let mut inner = self.lock();
                let Inner { state, guard, .. } = &mut *inner;
                let users = state
                    .accounts
                    .iter()
                    .map(|a| {
                        let session_seen = state
                            .sessions
                            .records()
                            .iter()
                            .filter(|r| r.subject == a.id)
                            .map(|r| r.last_seen)
                            .max()
                            .unwrap_or(0);
                        AdminUser {
                            id: a.id,
                            email: a.email.clone(),
                            display_name: a.display_name.clone(),
                            role: state::role(a).into(),
                            disabled: a.disabled,
                            has_pin: !a.pin.is_empty(),
                            sessions: state.sessions.count(a.id) as u32,
                            created: a.created,
                            last_seen: a.last_seen.max(session_seen),
                            locked_secs: guard.wait(Some(a.id), mono).div_ceil(1000) as u32,
                            mastodon: a
                                .mastodon
                                .as_ref()
                                .map(|m| m.handle.clone())
                                .unwrap_or_default(),
                        }
                    })
                    .collect();
                reply.wire(req, &AdminUserList { users });
            }

            (method, _) if path.starts_with("/api/v1/admin/mail") => {
                let rest = &path["/api/v1/admin/mail".len()..];
                self.mail_route(method, rest, req, reply, now)?;
            }

            ("GET", "/api/v1/admin/delivery") => {
                self.admin(req)?;
                reply.wire(
                    req,
                    &DeliveryStatus {
                        household: self.outbox.as_ref().map(|o| o.view()).unwrap_or_default(),
                        public: self
                            .public_outbox
                            .as_ref()
                            .map(|o| o.view())
                            .unwrap_or_default(),
                        mail: self
                            .outbox
                            .as_ref()
                            .map(|o| o.mail_view())
                            .unwrap_or_default(),
                    },
                );
            }

            (method, _) if path.starts_with("/api/v1/admin/public-bot") => {
                let rest = &path["/api/v1/admin/public-bot".len()..];
                self.bot_route(method, rest, req, reply, now, true)?;
            }

            (method, _) if path.starts_with("/api/v1/admin/bot") => {
                let rest = &path["/api/v1/admin/bot".len()..];
                self.bot_route(method, rest, req, reply, now, false)?;
            }

            ("GET", "/api/v1/admin/settings") => {
                self.admin(req)?;
                let open = !self.lock().state.signup_closed;
                reply.wire(req, &Settings { signup_open: open });
            }

            ("PATCH", "/api/v1/admin/settings") => {
                self.admin(req)?;
                let update: SettingsUpdate = req.decode()?;
                let open = self.mutate(|inner| {
                    if let Some(open) = update.signup_open {
                        inner.state.signup_closed = !open;
                    }
                    Ok(!inner.state.signup_closed)
                })?;
                reply.wire(req, &Settings { signup_open: open });
            }

            ("GET", "/api/v1/admin/storage") => {
                self.admin(req)?;
                let inner = self.lock();
                reply.wire(
                    req,
                    &StorageInfo {
                        generation: inner.store.generation,
                        snapshot_bytes: inner.snapshot_bytes,
                        commits: inner.state.commits,
                        accounts: inner.state.accounts.len() as u32,
                        max_accounts: inner.state.limits.accounts as u32,
                        sessions: inner.state.sessions.records().len() as u32,
                        max_sessions: inner.state.limits.sessions as u32,
                        loaded_slot: inner.loaded_slot,
                        fallback_used: inner.fallback,
                    },
                );
            }

            #[cfg(feature = "gate")]
            ("GET", "/api/v1/gate") => reply.wire(req, &self.gate.stats()),
            #[cfg(feature = "gate")]
            ("POST", "/api/v1/gate") => {
                let secs = req.param_num::<u64>("secs")?.unwrap_or(300).min(3600);
                self.gate.start(secs);
                reply.wire_status(req, 202, &self.gate.stats());
            }

            (method, _) if path.starts_with("/api/v1/admin/users/") => {
                let id: u32 = path["/api/v1/admin/users/".len()..]
                    .parse()
                    .map_err(|_| ApiError::not_found("No such account"))?;
                if method != "PATCH" {
                    return Err(ApiError::new(405, "method", "Use PATCH"));
                }
                let actor = self.admin(req)?;
                let u: AdminUserUpdate = req.decode()?;
                self.mutate(|inner| {
                    inner.state.admin_update(actor.id, id, &u)?;
                    if u.pin.is_some() {
                        inner.guard.clear(id);
                    }
                    Ok(())
                })?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", "/api/v1/games") => {
                let c = self.caller(req)?;
                let archived = req.param("archived").is_some_and(|v| v != "0");
                let list = views::games(&self.lock().state, c.id, archived);
                reply.wire(req, &list);
            }

            ("POST", "/api/v1/games") => {
                let c = self.caller(req)?;
                let input: GameInput = req.decode()?;
                let added = self.mutate(|inner| {
                    let id = inner.state.add_game(c.id, &input, now)?;
                    let game = inner.state.game(id).expect("just added");
                    Ok(views::game(&inner.state, game, c.id))
                });
                match added {
                    Ok(view) => reply.wire_status(req, 201, &view),
                    Err(e) => {
                        // Lets the UI jump to the game that already exists.
                        let existing = input
                            .title
                            .as_deref()
                            .and_then(|t| self.lock().state.game_titled(t, 0).map(|g| g.id));
                        if let (Some(id), "duplicate_title") = (existing, e.code) {
                            reply.header("X-Existing-Id", id.to_string());
                        }
                        return Err(e);
                    }
                }
            }

            ("GET", "/api/v1/plans") => {
                let c = self.caller(req)?;
                let status = req
                    .param("status")
                    .map_or_else(|| "open,on".to_string(), |s| s.into_owned());
                if !views::valid_statuses(&status) {
                    return Err(ApiError::bad_request(
                        "status: open, on, done, cancelled (comma separated)",
                    ));
                }
                let list = views::plans(&self.lock().state, c.id, &status);
                reply.wire(req, &list);
            }

            ("POST", "/api/v1/plans") => {
                let c = self.caller(req)?;
                let input: PlanInput = req.decode()?;
                let view = self.mutate(|inner| {
                    let id = inner
                        .state
                        .create_series(c.id, &input, now, self.utc_offset)?;
                    Ok(views::plan(&inner.state, inner.state.plan(id)?, c.id))
                })?;
                let ids = if view.series.is_empty() {
                    vec![view.id]
                } else {
                    view.series.clone()
                };
                for id in ids {
                    self.announce_plan(id, News::New, now);
                }
                reply.wire_status(req, 201, &view);
            }

            (method, _) if path.starts_with("/api/v1/games/") => {
                self.game_route(method, &path["/api/v1/games/".len()..], req, reply, now)?;
            }

            (method, _) if path.starts_with("/api/v1/plans/") => {
                self.plan_route(method, &path["/api/v1/plans/".len()..], req, reply, now)?;
            }

            ("GET", _) if path.starts_with("/api/v1/auth/oauth/") => {
                let ticket = &path["/api/v1/auth/oauth/".len()..];
                let status = self.oauth_status(req, reply, ticket)?;
                reply.wire(req, &status);
            }

            (method, _) if path.starts_with("/api/v1/admin/providers/") => {
                let id = parse_id(
                    &path["/api/v1/admin/providers/".len()..],
                    "No such instance",
                )?;
                self.admin(req)?;
                match method {
                    "PATCH" => {
                        let input: ProviderInput = req.decode()?;
                        self.mutate(|inner| inner.state.edit_provider(id, &input))?;
                    }
                    "DELETE" => self.mutate(|inner| inner.state.delete_provider(id))?,
                    _ => return Err(no_route()),
                }
                self.sync_providers();
                let list = views::providers(&self.lock().state);
                reply.wire(req, &list);
            }

            (method, _) if path.starts_with("/api/v1/admin/games/") => {
                let parts: Vec<&str> = path["/api/v1/admin/games/".len()..].split('/').collect();
                let id = parse_id(parts[0], "No such game")?;
                self.admin(req)?;
                match (method, &parts[1..]) {
                    ("POST", ["merge"]) => {
                        let merge: Merge = req.decode()?;
                        self.mutate(|inner| inner.state.merge_games(id, merge.into))?;
                        reply.wire(req, &Done { ok: true });
                    }
                    ("DELETE", []) => {
                        let deleted = self.mutate(|inner| inner.state.delete_game(id))?;
                        // ok=false: a plan uses it, so it was archived instead.
                        reply.wire(req, &Done { ok: deleted });
                    }
                    _ => return Err(no_route()),
                }
            }

            _ => {}
        }
        Ok(())
    }

    /// Where a Mastodon sign-in stands. When the framework hands over the
    /// identity, this is where it becomes a session (or a link).
    fn oauth_status(
        &self,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        ticket: &str,
    ) -> Result<OAuthStatus, ApiError> {
        let status = |state: &str| OAuthStatus {
            state: state.into(),
            ..Default::default()
        };
        Ok(match self.oauth()?.poll(req, ticket) {
            Poll::Waiting => status("waiting"),
            Poll::Redirect(location) => OAuthStatus {
                location,
                ..status("redirect")
            },
            Poll::Failed(message) => OAuthStatus {
                message,
                ..status("failed")
            },
            Poll::Done(identity, intent) => {
                let now = wall().unwrap_or(0);
                let outcome = if let Some(account) = intent.strip_prefix("link:") {
                    let account: u32 = account.parse().unwrap_or(0);
                    self.mutate(|inner| inner.state.link_identity(account, &identity))
                        .map(|_| "linked")
                } else {
                    let provider: u32 = intent
                        .strip_prefix("login:")
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(0);
                    self.mutate(|inner| {
                        let id = inner.state.sign_in_with(provider, &identity, now)?;
                        Self::sign_in(inner, id, req, reply)
                    })
                    .map(|_| "signed_in")
                };
                match outcome {
                    Ok(result) => {
                        log::info!("Mastodon sign-in: {} ({result})", identity.handle);
                        OAuthStatus {
                            result: result.into(),
                            ..status("done")
                        }
                    }
                    Err(e) => OAuthStatus {
                        message: e.message,
                        ..status("failed")
                    },
                }
            }
        })
    }

    /// `/cal/<token>.ics` (a person's feed) and `/cal/house/<token>.ics`.
    /// The token is the credential: calendar apps can't sign in.
    fn feed_route(&self, rest: &str, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        let (house, token) = match rest.strip_prefix("house/") {
            Some(t) => (true, t),
            None => (false, rest),
        };
        let token = token
            .strip_suffix(".ics")
            .ok_or_else(|| ApiError::not_found("No such calendar"))?;
        let hash = hash_hex(token);
        let now = wall().unwrap_or(0);
        let inner = self.lock();
        let st = &inner.state;
        let body = if house {
            if st.house_feed_hash.is_empty() || !same_hex(&st.house_feed_hash, &hash) {
                return Err(ApiError::not_found("No such calendar"));
            }
            ics::feed(st, None, now)
        } else {
            let account = st
                .accounts
                .iter()
                .find(|a| !a.disabled && !a.feed_hash.is_empty() && same_hex(&a.feed_hash, &hash))
                .ok_or_else(|| ApiError::not_found("No such calendar"))?;
            ics::feed(st, Some(account.id), now)
        };
        reply.bytes(200, "text/calendar; charset=utf-8", body.as_bytes());
        reply.header("Cache-Control", "no-cache");
        Ok(())
    }

    /// `/api/v1/games/{id}[/interest]`
    fn game_route(
        &self,
        method: &str,
        rest: &str,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        _now: u64,
    ) -> Result<(), ApiError> {
        let parts: Vec<&str> = rest.split('/').collect();
        let id = parse_id(parts[0], "No such game")?;
        let c = self.caller(req)?;
        match (method, &parts[1..]) {
            ("GET", []) => {
                let inner = self.lock();
                let game = inner
                    .state
                    .game(id)
                    .ok_or_else(|| ApiError::not_found("No such game"))?;
                reply.wire(req, &views::game_detail(&inner.state, game, c.id));
            }
            ("PATCH", []) => {
                let input: GameInput = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.edit_game(c.id, id, &input)?;
                    Ok(views::game(
                        &inner.state,
                        inner.state.game(id).expect("edited"),
                        c.id,
                    ))
                })?;
                reply.wire(req, &view);
            }
            ("PUT" | "DELETE", ["interest"]) => {
                let view = self.mutate(|inner| {
                    inner.state.set_interest(c.id, id, method == "PUT")?;
                    Ok(views::game(
                        &inner.state,
                        inner.state.game(id).expect("exists"),
                        c.id,
                    ))
                })?;
                reply.wire(req, &view);
            }
            _ => return Err(no_route()),
        }
        Ok(())
    }

    /// `/api/v1/plans/{id}[/response|lock|reopen|cancel|host]`
    fn plan_route(
        &self,
        method: &str,
        rest: &str,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        now: u64,
    ) -> Result<(), ApiError> {
        let parts: Vec<&str> = rest.split('/').collect();
        let id = parse_id(parts[0], "No such plan")?;
        let c = self.caller(req)?;
        match (method, &parts[1..]) {
            ("GET", []) => {
                let inner = self.lock();
                reply.wire(req, &views::plan(&inner.state, inner.state.plan(id)?, c.id));
                return Ok(());
            }
            ("GET", ["calendar.ics"]) => {
                let inner = self.lock();
                let plan = inner.state.plan(id)?;
                let body = ics::plan_file(&inner.state, plan);
                reply.bytes(200, "text/calendar; charset=utf-8", body.as_bytes());
                reply.header(
                    "Content-Disposition",
                    format!("attachment; filename=\"game-plan-{id}.ics\""),
                );
                return Ok(());
            }
            ("GET", ["comments"]) => {
                let after = req.param_num::<u32>("after")?.unwrap_or(0);
                let thread = self.load_thread(id);
                let inner = self.lock();
                inner.state.plan(id)?;
                let list = views::comments(&inner.state, id, &thread, c.id, after, now);
                reply.wire(req, &list);
                return Ok(());
            }
            ("POST", ["comments"]) => {
                let input: CommentInput = req.decode()?;
                let dm = self.mutate(|inner| {
                    let mut thread = self.load_thread(id);
                    inner
                        .state
                        .add_comment(&mut thread, c.id, id, &input.text, now)?;
                    self.save_thread(id, &thread)?;
                    Ok(if input.dm {
                        self.comment_dms(&inner.state, id, c.id, &input.text)
                    } else {
                        vec![]
                    })
                })?;
                let sent_dms = dm
                    .into_iter()
                    .map(|(public, text, count)| {
                        let bot = if public {
                            self.public_outbox.as_deref()
                        } else {
                            self.outbox.as_deref()
                        };
                        if bot.is_some_and(|o| o.enqueue("dm", text, Some("direct"), now)) {
                            count
                        } else {
                            0
                        }
                    })
                    .sum();
                let thread = self.load_thread(id);
                // Answer with the newest page, which ends with the new comment.
                let skip = thread.comments.len().saturating_sub(views::COMMENT_PAGE);
                let after = if skip == 0 {
                    0
                } else {
                    thread.comments[skip - 1].id
                };
                let mut list = views::comments(&self.lock().state, id, &thread, c.id, after, now);
                list.sent_dms = sent_dms;
                reply.wire_status(req, 201, &list);
                return Ok(());
            }
            ("POST", ["invite"]) => {
                let to = Self::addresses(&req.decode()?)?;
                let o = self.mailer()?;
                let emails: Vec<mail::Email> = {
                    let inner = self.lock();
                    let p = inner.state.plan(id)?;
                    if !matches!(p.status.as_str(), "open" | "on") {
                        return Err(ApiError::new(409, "closed", "This plan is over"));
                    }
                    let offset = (self.utc_offset)(now);
                    to.iter()
                        .map(|a| {
                            mail::plan_invite(&inner.state, p, c.id, a, &self.origin, now, offset)
                        })
                        .collect()
                };
                let sent = emails
                    .into_iter()
                    .filter(|e| o.email(e.clone(), now))
                    .count() as u32;
                reply.wire(req, &Sent { sent });
                return Ok(());
            }
            (_, ["comments", cid]) => {
                let cid = parse_id(cid, "No such comment")?;
                self.mutate(|inner| {
                    let mut thread = self.load_thread(id);
                    match method {
                        "PATCH" => {
                            let input: CommentInput = req.decode()?;
                            inner.state.edit_comment(
                                &mut thread,
                                c.id,
                                id,
                                cid,
                                &input.text,
                                now,
                            )?
                        }
                        "DELETE" => inner.state.delete_comment(&mut thread, c.id, id, cid)?,
                        _ => return Err(no_route()),
                    }
                    self.save_thread(id, &thread)
                })?;
                reply.wire(req, &Done { ok: true });
                return Ok(());
            }
            _ => {}
        }
        let (view, news) = self.mutate(|inner| {
            let s = &mut inner.state;
            let _was_announced = s.plan(id)?.announce;
            let news = match (method, &parts[1..]) {
                ("PATCH", []) => {
                    s.edit_plan(c.id, id, &req.decode()?, now)?;
                    // Ticking "post to Mastodon" later posts it then.
                    Some(News::Updated)
                }
                ("PUT", ["response"]) => {
                    s.respond(c.id, id, &req.decode()?, now)?;
                    None
                }
                ("POST", ["lock"]) => {
                    s.lock_plan(c.id, id, &req.decode()?, now)?;
                    Some(News::Locked)
                }
                ("POST", ["reopen"]) => {
                    s.reopen_plan(c.id, id, now)?;
                    None
                }
                ("POST", ["cancel"]) => {
                    s.cancel_plan(c.id, id, now)?;
                    Some(News::Cancelled)
                }
                ("PUT", ["report"]) => {
                    s.report_plan(c.id, id, &req.decode()?, now)?;
                    None
                }
                ("POST", ["host"]) => {
                    let to: HandOver = req.decode()?;
                    s.hand_over(c.id, id, to.user, now)?;
                    None
                }
                _ => return Err(no_route()),
            };
            Ok((views::plan(s, s.plan(id)?, c.id), news))
        })?;
        if let Some(news) = news {
            self.announce_plan(id, news, now);
        }
        reply.wire(req, &view);
        Ok(())
    }

    /// Keep each network's recipients in its own direct-message group.
    fn comment_dms(
        &self,
        state: &State,
        id: u32,
        author: u32,
        text: &str,
    ) -> Vec<(bool, String, u32)> {
        let Ok(p) = state.plan(id) else { return vec![] };
        let mut result = Vec::new();
        for (bot, public) in [
            (self.outbox.as_deref(), false),
            (self.public_outbox.as_deref(), true),
        ] {
            let Some(bot) = bot else { continue };
            let mentions: Vec<String> = state
                .engaged(id)
                .into_iter()
                .filter(|id| *id != author)
                .filter_map(|id| state.account(id).filter(|a| !a.disabled)?.mastodon.as_ref())
                .filter(|m| {
                    self.bot_for(&m.host)
                        .is_some_and(|destination| std::ptr::eq(destination, bot))
                })
                .map(|m| m.handle.clone())
                .collect();
            if !mentions.is_empty() {
                result.push((
                    public,
                    announce::comment_dm(state, p, author, text, &mentions, &self.origin),
                    mentions.len() as u32,
                ));
            }
        }
        result
    }

    /// `/api/v1/admin/bot[/digest]`
    fn bot_route(
        &self,
        method: &str,
        rest: &str,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        now: u64,
        public: bool,
    ) -> Result<(), ApiError> {
        self.admin(req)?;
        let o = if public {
            self.public_outbox
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("Public bot unavailable in this build"))?
        } else {
            self.outbox()?
        };
        match (method, rest) {
            ("GET", "") => {}
            ("POST", "") => o.begin_setup(outbound::setup_from(&req.decode::<BotSetup>()?)?)?,
            ("PATCH", "") => {
                let s: BotSettings = req.decode()?;
                o.settings(s.visibility.as_deref(), s.digest, s.digest_time.as_deref())?;
                if let Some(enabled) = s.replies {
                    o.reply_settings(enabled)?;
                }
            }
            ("DELETE", "") => o.remove(),
            ("POST", action) if action.starts_with("/retry/") => {
                o.retry(parse_id(&action[7..], "No such delivery")?, now)?;
            }
            ("POST", "/digest") => {
                if o.handle().is_none() {
                    return Err(ApiError::new(409, "no_bot", "Set up the bot first"));
                }
                let offset = (self.utc_offset)(now);
                let text =
                    announce::digest_for(&self.lock().state, &self.origin, now, offset, public)
                        .ok_or_else(|| {
                            ApiError::new(
                                409,
                                "nothing_planned",
                                "Nothing is planned, so there's nothing to post",
                            )
                        })?;
                o.enqueue("digest", text, None, now);
            }
            _ => return Err(no_route()),
        }
        reply.wire(req, &o.view());
        Ok(())
    }

    fn pin_sign_in(
        &self,
        body: &PinSignIn,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
    ) -> Result<Me, ApiError> {
        let mono = uptime_ms();
        let wrong = || ApiError::new(401, "wrong_pin", "Wrong email or PIN");
        let mut inner = self.lock();
        let account = inner
            .state
            .by_email(&body.email)
            .map(|a| (a.id, a.pin.clone(), a.disabled));
        let wait = inner.guard.wait(account.as_ref().map(|a| a.0), mono);
        if wait > 0 {
            let secs = wait.div_ceil(1000);
            reply.header("Retry-After", secs.to_string());
            return Err(ApiError::new(
                429,
                "locked",
                format!("Too many wrong PINs. Try again in {secs} s"),
            ));
        }
        let Some((id, stored, disabled)) = account else {
            pin::verify(&body.pin, DUMMY_PIN);
            inner.guard.failed(None, mono);
            return Err(wrong());
        };
        if stored.is_empty() || !pin::verify(&body.pin, &stored) {
            inner.guard.failed(Some(id), mono);
            return Err(wrong());
        }
        if disabled {
            return Err(ApiError::new(403, "disabled", "This account is disabled"));
        }
        inner.guard.succeeded(id);
        drop(inner);
        self.mutate(|inner| {
            Self::sign_in(inner, id, req, reply)?;
            Self::me(&inner.state, id)
        })
    }
}

impl Service for App {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        if let Err(e) = self.route(req, reply) {
            reply.fail(req, &e);
        }
    }

    fn schemas(&self) -> Vec<&'static Schema> {
        let schemas = vec![
            Status::SCHEMA,
            NewAccount::SCHEMA,
            PinSignIn::SCHEMA,
            Me::SCHEMA,
            MeUpdate::SCHEMA,
            PinChange::SCHEMA,
            UserList::SCHEMA,
            AdminUserList::SCHEMA,
            AdminUserUpdate::SCHEMA,
            Settings::SCHEMA,
            SettingsUpdate::SCHEMA,
            StorageInfo::SCHEMA,
            Done::SCHEMA,
            GameList::SCHEMA,
            GameDetail::SCHEMA,
            GameView::SCHEMA,
            GameInput::SCHEMA,
            Merge::SCHEMA,
            PlanList::SCHEMA,
            PlanView::SCHEMA,
            PlanInput::SCHEMA,
            PostGameReport::SCHEMA,
            DeliveryStatus::SCHEMA,
            ResponseInput::SCHEMA,
            LockIn::SCHEMA,
            HandOver::SCHEMA,
            CommentList::SCHEMA,
            CommentInput::SCHEMA,
            NotificationList::SCHEMA,
            MarkRead::SCHEMA,
            Summary::SCHEMA,
            FeedUrl::SCHEMA,
            ProviderList::SCHEMA,
            ProviderInput::SCHEMA,
            OAuthStart::SCHEMA,
            OAuthTicket::SCHEMA,
            OAuthStatus::SCHEMA,
            DisplayDeck::SCHEMA,
            BotView::SCHEMA,
            BotSetup::SCHEMA,
            BotSettings::SCHEMA,
            MailView::SCHEMA,
            MailSetup::SCHEMA,
            InviteEmails::SCHEMA,
            Sent::SCHEMA,
            PinResetStart::SCHEMA,
            PinResetConfirm::SCHEMA,
        ];
        #[cfg(feature = "gate")]
        let schemas = [schemas, vec![crate::gate::GateStats::SCHEMA]].concat();
        schemas
    }

    fn metrics(&self) -> Vec<(&'static str, f64)> {
        let inner = self.lock();
        vec![
            ("gm_accounts", inner.state.accounts.len() as f64),
            ("gm_sessions", inner.state.sessions.records().len() as f64),
            ("gm_commits", inner.state.commits as f64),
            ("gm_snapshot_bytes", inner.snapshot_bytes as f64),
            ("gm_games", inner.state.games.len() as f64),
            ("gm_plans", inner.state.plans.len() as f64),
            ("gm_comments", inner.state.comment_total() as f64),
        ]
    }

    fn checks(&self) -> Vec<Check> {
        let inner = self.lock();
        let limits = &inner.state.limits;
        let slot = if inner.fallback {
            Check::new(
                "store.slot",
                CheckStatus::Warn,
                format!(
                    "Booted from slot {}: the other snapshot was invalid",
                    inner.loaded_slot
                ),
            )
        } else {
            Check::new("store.slot", CheckStatus::Ok, "Newest snapshot loaded")
        };
        vec![
            slot.runbook("storage"),
            Check::usage(
                "store.accounts",
                "Accounts",
                inner.state.accounts.len() as f64,
                limits.accounts as f64,
                80.0,
                95.0,
            ),
            Check::usage(
                "store.games",
                "Games",
                inner.state.games.len() as f64,
                limits.games as f64,
                80.0,
                95.0,
            ),
        ]
    }
}

fn parse_id(raw: &str, missing: &str) -> Result<u32, ApiError> {
    raw.parse()
        .map_err(|_| ApiError::not_found(missing.to_string()))
}

fn no_route() -> ApiError {
    ApiError::new(405, "no_route", "No such action here")
}

/// A fresh 128-bit calendar token.
fn new_token() -> Result<String, ApiError> {
    miniframework::session::random_token()
        .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))
}

fn hash_hex(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Constant-time comparison of two hex digests.
fn same_hex(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// A feed URL on the origin the caller used (`gamematcher.local` or the
/// board's IP, http or https), or "off" when `path` is None.
fn feed_url(req: &Request<'_>, path: Option<String>) -> FeedUrl {
    let Some(path) = path else {
        return FeedUrl::default();
    };
    let host = req.header("Host").trim();
    let scheme = if req.secure { "https" } else { "http" };
    FeedUrl {
        url: format!("{scheme}://{host}{path}"),
        webcal: format!("webcal://{host}{path}"),
        enabled: true,
    }
}

#[cfg(test)]
mod reminder_delivery_tests {
    use super::*;
    use crate::outbound::{StoredBot, StoredMail};
    use miniframework::fetch::{Fetch, Fetched};
    use miniframework::kv::MemKv;

    struct Offline;
    impl Fetch for Offline {
        fn get(&self, _: &str, _: &str, _: usize) -> std::io::Result<Fetched> {
            panic!("delivery should only queue work");
        }
    }

    #[test]
    fn reminders_queue_individual_direct_messages_and_email_independently() {
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        kv.set(
            "bot",
            &wire::to_vec(
                Format::CborInt,
                &StoredBot {
                    instance: "https://home.local".into(),
                    token: "test".into(),
                    handle: "@games@home.local".into(),
                    anchor: "household-ca".into(),
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        kv.set(
            "mail",
            &wire::to_vec(
                Format::CborInt,
                &StoredMail {
                    api_key: "test".into(),
                    sender_email: "games@example.com".into(),
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        let fetch = Arc::new(Offline);
        let outbox = Arc::new(Outbox::open(kv.clone(), fetch.clone(), fetch));
        let mut app = App::open(
            kv.clone(),
            Limits {
                pin_rounds: 2,
                ..Limits::board()
            },
        )
        .unwrap();
        app.attach_outbox(outbox.clone());
        let now = wall().unwrap();
        let mut inner = app.lock();
        let s = &mut inner.state;
        let host = s.setup("host@example.com", "Host", "1234", now).unwrap();
        let guest = s.signup("guest@example.com", "Guest", "1234", now).unwrap();
        let maybe = s.signup("maybe@example.com", "Maybe", "1234", now).unwrap();
        for id in [host, guest, maybe] {
            s.account_mut(id).unwrap().mastodon = Some(StoredIdentity {
                host: "home.local".into(),
                handle: format!("@person{id}@home.local"),
                ..Default::default()
            });
        }
        s.account_mut(guest).unwrap().email_off = true;
        let game = s
            .add_game(
                host,
                &GameInput {
                    title: Some("Catan".into()),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        let plan = s
            .create_plan(
                host,
                &PlanInput {
                    games: Some(vec![game]),
                    times: Some(vec![TimeInput {
                        id: 0,
                        start: now + 900,
                        end: 0,
                    }]),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        for (id, overall) in [(host, "in"), (guest, "in"), (maybe, "maybe")] {
            s.respond(
                id,
                plan,
                &ResponseInput {
                    overall: overall.into(),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        }
        let time = s.plan(plan).unwrap().times[0].id;
        s.lock_plan(
            host,
            plan,
            &LockIn {
                time,
                games: vec![],
            },
            now,
        )
        .unwrap();
        s.mail.clear();
        assert!(s.send_reminders(now));
        app.flush_mail(&mut inner);
        let jobs = outbox.view().jobs;
        assert_eq!(jobs.iter().filter(|j| j.kind == "email").count(), 1);
        let dms: Vec<_> = jobs.iter().filter(|j| j.kind == "dm").collect();
        assert_eq!(dms.len(), 2, "host once, attendee once, no maybe");
        assert!(dms
            .iter()
            .any(|j| j.text.starts_with(&format!("@person{guest}@home.local"))));
        drop(inner);
        let stored: crate::outbound::StoredOutbox =
            wire::decode(Format::CborInt, &kv.get("outbox").unwrap()).unwrap();
        assert!(stored
            .jobs
            .iter()
            .filter(|j| j.kind == "dm")
            .all(|j| j.visibility == "direct"));
    }
}

impl App {
    fn bot_for(&self, host: &str) -> Option<&Outbox> {
        let bots = [self.outbox.as_deref(), self.public_outbox.as_deref()];
        bots.into_iter()
            .flatten()
            .find(|o| o.host().is_some_and(|h| h.eq_ignore_ascii_case(host)))
            .or_else(|| {
                if host.ends_with(".local") {
                    None
                } else {
                    bots.into_iter()
                        .flatten()
                        .find(|o| o.federated() && o.handle().is_some())
                }
            })
    }

    /// Drain the worker's durable inbox without doing network I/O on the main task.
    fn receive_replies(&self, now: u64) {
        for (bot, public) in [
            (self.outbox.as_deref(), false),
            (self.public_outbox.as_deref(), true),
        ] {
            let Some(bot) = bot else { continue };
            let Some(host) = bot.host() else { continue };
            for mention in bot.pending_replies() {
                let result = self.mutate_optional(|inner| {
                    let s = &mut inner.state;
                    // A remote instance's numeric account id is not its home id.
                    // Only accept identities native to this bot's instance.
                    if mention.account.acct.contains('@') {
                        return Ok(None);
                    }
                    let Some(account) = s
                        .accounts
                        .iter()
                        .find(|a| {
                            !a.disabled
                                && a.mastodon.as_ref().is_some_and(|m| {
                                    m.host.eq_ignore_ascii_case(&host)
                                        && m.subject == mention.account.id
                                })
                        })
                        .cloned()
                    else {
                        return Ok(None);
                    };
                    let previous = if public {
                        &account.last_public_reply
                    } else {
                        &account.last_house_reply
                    };
                    let source = format!("{}|{}|", host, bot.handle().unwrap_or_default());
                    let previous = previous.strip_prefix(&source).unwrap_or("");
                    if !crate::replies::newer(&mention.id, previous) {
                        return Ok(None);
                    }
                    let Some(status) = &mention.status else {
                        return Ok(None);
                    };
                    let parent = status
                        .in_reply_to_id
                        .as_deref()
                        .and_then(|id| bot.reply_plan(id));
                    let Some((overall, explicit)) = crate::replies::command(&status.content) else {
                        return Ok(None);
                    };
                    if parent.is_some() && explicit.is_some() && parent != explicit {
                        return Ok(None);
                    }
                    let Some(plan) = explicit.or(parent) else {
                        return Ok(None);
                    };
                    let Ok(p) = s.plan(plan) else { return Ok(None) };
                    if public
                        && !p.announce_public
                        && p.host != account.id
                        && !p.responses.iter().any(|r| r.account == account.id)
                    {
                        return Ok(None);
                    }
                    if !crate::plans::active(p) {
                        return Ok(None);
                    }
                    let existing = p.responses.iter().find(|r| r.account == account.id);
                    let mut response = ResponseInput {
                        overall: overall.clone(),
                        times: existing.map(|r| r.times.clone()).unwrap_or_default(),
                        games: existing.map(|r| r.games.clone()).unwrap_or_default(),
                        note: existing.map(|r| r.note.clone()).unwrap_or_default(),
                    };
                    if let Some(time) = p.chosen_time {
                        response.times.retain(|a| a.time != time);
                        response.times.push(TimeAnswer {
                            time,
                            answer: match overall.as_str() {
                                "in" => "yes",
                                "out" => "no",
                                _ => "if_needed",
                            }
                            .into(),
                        });
                    }
                    s.respond(account.id, plan, &response, now)?;
                    let a = s.account_mut(account.id)?;
                    let processed = format!("{source}{}", mention.id);
                    if public {
                        a.last_public_reply = processed;
                    } else {
                        a.last_house_reply = processed;
                    }
                    Ok(Some((account.mastodon.unwrap().handle, overall, plan)))
                });
                match result {
                    Ok(confirmation) => {
                        // State commits before advancing the inbox. Replay after a crash
                        // is harmless because the account's last processed id is durable.
                        bot.acknowledge_reply(&mention.id);
                        if let Some((handle, answer, plan)) = confirmation {
                            bot.enqueue(
                                "dm",
                                format!("{handle} Saved: {answer} for plan #{plan}."),
                                Some("direct"),
                                now,
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("incoming RSVP: {}", e.message);
                        break; // Leave pending for a storage retry.
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "feature_tests.rs"]
mod feature_tests;
