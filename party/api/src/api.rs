//! The party HTTP API (`spec/06-api.md`). Every route answers in the
//! format the client negotiates; the UI uses JSON.
//!
//! Auth is the `party_session` cookie. Every non-GET request must come from
//! the board's own pages (`require_same_origin`). Every change is committed
//! to flash before the response is sent, and a commit writes only the
//! records the change touched (`records.rs`); if it fails, the change is
//! rolled back and the client gets 500.
use crate::messages::*;
use crate::pin::{self, Guard, GuardLimits};
use crate::records::RecordStore;
use crate::state::{self, Limits, State, ACTIVE};
use miniframework::kv::Kv;
use miniframework::ops::{Check, Status as CheckStatus};
use miniframework::session::require_same_origin;
use miniframework::wire::Schema;
use miniframework::{uptime_ms, wall_ms, ApiError, Message, Reply, Request, Service};
use std::sync::{Arc, Mutex, MutexGuard};

/// A hash that never matches, verified for unknown logins so a miss costs
/// the same time as a wrong PIN.
const DUMMY_PIN: &str = "pbkdf2-sha256$1000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

/// Page size of lists.
const PAGE: usize = 50;

/// Unix seconds, or `None` while the board's clock is unset.
fn wall() -> Option<u64> {
    wall_ms().map(|ms| ms / 1000)
}

pub(crate) struct Inner {
    pub(crate) state: State,
    pub(crate) records: RecordStore,
    pub(crate) guard: Guard,
    /// Records that failed to load at boot (left on flash).
    pub(crate) quarantined: Vec<String>,
    /// Monotonic ms of the last flush of touch-only changes.
    last_flush: u64,
}

pub struct App {
    pub(crate) inner: Mutex<Inner>,
    /// "Sign in with Mastodon" (absent until [`App::attach_oauth`]).
    pub(crate) oauth: Option<Arc<miniframework::oauth::OAuth>>,
    /// Outstanding "forgot PIN" codes (RAM only).
    pub(crate) resets: Mutex<Vec<crate::api_more::Reset>>,
    /// Origins besides the board's own allowed to send unsafe requests
    /// (the Angular dev server).
    pub dev_origins: Vec<String>,
    pub setup_code: Option<&'static str>,
    pub build: &'static str,
    #[cfg(feature = "gate")]
    pub gate: Arc<crate::gate::Gate>,
}

/// Who is calling.
#[derive(Clone, Copy)]
pub(crate) struct Caller {
    pub(crate) id: u32,
    pub(crate) admin: bool,
}

impl App {
    /// Loads the state's records from `kv`. Refuses, rather than starting
    /// empty, when `meta` or an index is unreadable.
    pub fn open(kv: Arc<dyn Kv>, limits: Limits) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_with(kv, limits, GuardLimits::default())
    }

    pub fn open_with(
        kv: Arc<dyn Kv>,
        limits: Limits,
        guard: GuardLimits,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let opened = RecordStore::open(kv, limits)?;
        Ok(Self {
            inner: Mutex::new(Inner {
                state: opened.state,
                records: opened.store,
                guard: Guard::new(guard),
                quarantined: opened.quarantined,
                last_flush: 0,
            }),
            dev_origins: Vec::new(),
            oauth: None,
            resets: Mutex::new(Vec::new()),
            setup_code: None,
            build: "dev",
            #[cfg(feature = "gate")]
            gate: Arc::new(crate::gate::Gate::default()),
        })
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the records that changed since the last commit.
    pub(crate) fn commit(inner: &mut Inner) -> Result<(), ApiError> {
        let failed = |e: std::io::Error| {
            log::error!("state commit failed: {e}");
            ApiError::new(500, "storage_failed", "Could not save; nothing changed")
        };
        let changes = inner.records.changes(&inner.state).map_err(failed)?;
        if changes.writes.is_empty() {
            return Ok(());
        }
        inner.records.write(changes).map_err(failed)?;
        inner.state.commits += 1;
        inner.state.saved();
        Ok(())
    }

    /// Runs `f` on the state and commits; any error leaves the state as it was.
    pub(crate) fn mutate<R>(
        &self,
        f: impl FnOnce(&mut Inner) -> Result<R, ApiError>,
    ) -> Result<R, ApiError> {
        let mut inner = self.lock();
        let sp = inner.state.savepoint();
        let result = f(&mut inner).and_then(|r| Self::commit(&mut inner).map(|_| r));
        if result.is_err() {
            inner.state.rollback(sp);
        }
        result
    }

    /// Housekeeping, called from the main loop: persists session touches and
    /// the commit count at most once a minute (flash wear).
    pub fn tick(&self) {
        let now = uptime_ms();
        let mut inner = self.lock();
        if now.saturating_sub(inner.last_flush) < 60_000 {
            return;
        }
        inner.last_flush = now;
        let commits = inner.state.commits;
        inner.records.save_commits(commits);
        // Clock-driven changes need a set clock.
        let moved = wall().is_some_and(|w| {
            let s = &mut inner.state;
            let done = s.advance_events(w);
            let reminded = s.send_reminders(w);
            let made = s.materialize_all(w);
            done || reminded || made
        });
        if !moved && !inner.state.sessions.dirty() {
            return;
        }
        inner.state.sessions.prune(wall());
        if let Err(e) = Self::commit(&mut inner) {
            log::warn!("housekeeping commit: {}", e.message);
        }
    }

    /// The personal calendar feed at `/cal/<token>.ics` (no cookie: calendar
    /// apps can't sign in, so the token is the credential).
    fn feed_route(&self, rest: &str, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        let token = rest.strip_suffix(".ics").unwrap_or(rest);
        let hash = token_hash(token);
        let inner = self.lock();
        let person = inner
            .state
            .people
            .iter()
            .find(|p| !p.feed_hash.is_empty() && p.feed_hash == hash && p.status == ACTIVE)
            .map(|p| p.id)
            .ok_or_else(|| ApiError::not_found("No such calendar"))?;
        let body = crate::ics::feed(&inner.state, person, wall().unwrap_or(0));
        reply.bytes(200, "text/calendar; charset=utf-8", body.as_bytes());
        reply.header("Cache-Control", "no-store");
        Ok(())
    }

    /// The signed-in caller, if any (a disabled person is signed out).
    pub(crate) fn viewer(&self, req: &Request<'_>) -> Option<Caller> {
        let mut inner = self.lock();
        let id = inner.state.sessions.check(req, wall())?;
        match inner.state.person(id) {
            Some(p) if p.status == ACTIVE => Some(Caller { id, admin: p.admin }),
            _ => {
                inner.state.sessions.revoke(req);
                None
            }
        }
    }

    pub(crate) fn caller(&self, req: &Request<'_>) -> Result<Caller, ApiError> {
        self.viewer(req)
            .ok_or_else(|| ApiError::unauthorized("Sign in first"))
    }

    pub(crate) fn admin(&self, req: &Request<'_>) -> Result<Caller, ApiError> {
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

    pub(crate) fn me(state: &State, id: u32) -> Result<Me, ApiError> {
        let p = state
            .person(id)
            .ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
        Ok(Me {
            id: p.id,
            display_name: p.display_name.clone(),
            email: p.email.clone(),
            role: state::role(p).into(),
            bio: p.bio.clone(),
            household: p.household,
            household_name: state.household_name(p.household),
            discoverable: p.discoverable.clone(),
            sessions: state.sessions.count(id) as u32,
            created: p.created,
            has_pin: !p.pin.is_empty(),
            identities: p.identities.iter().map(|i| i.handle.clone()).collect(),
            email_notify: !p.email.is_empty() && !p.email_off,
            dm_notify: !p.identities.is_empty() && !p.dm_off,
            personal_link: match state.publish().map(|x| x.site_url.as_str()) {
                Some(site) if !site.is_empty() && !p.personal_key.is_empty() => format!(
                    "{}/#/k/{}.{}",
                    site.trim_end_matches('/'),
                    p.id,
                    p.personal_key
                ),
                _ => String::new(),
            },
        })
    }

    fn person_view(state: &State, p: &StoredPerson) -> PersonView {
        PersonView {
            id: p.id,
            display_name: p.display_name.clone(),
            bio: p.bio.clone(),
            household: p.household,
            household_name: state.household_name(p.household),
        }
    }

    /// Starts a session for `id` and sets its cookie.
    pub(crate) fn sign_in(
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
        if let (Some(now), Ok(p)) = (now, inner.state.person_mut(id)) {
            p.last_seen = now;
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
            app: "party".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            build: self.build.into(),
            setup_needed: inner.state.people.is_empty(),
            signup_open: !inner.state.signup_closed,
            clock_synced: wall().is_some(),
            setup_code_required: self.setup_code.is_some() && inner.state.people.is_empty(),
            secure: req.secure,
            instance: "party".into(),
            providers: if self.oauth.is_some() {
                inner
                    .state
                    .providers()
                    .iter()
                    .map(|p| ProviderButton {
                        id: p.id,
                        name: p.name.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            mail: inner.state.mail_ready(),
            bot: inner
                .state
                .bot()
                .filter(|b| b.state == "ready")
                .map(|b| b.handle.clone())
                .unwrap_or_default(),
        }
    }

    fn pin_sign_in(
        &self,
        body: &PinSignIn,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
    ) -> Result<Me, ApiError> {
        let mono = uptime_ms();
        let wrong = || ApiError::new(401, "wrong_pin", "Wrong name, email or PIN");
        let mut inner = self.lock();
        let found = inner
            .state
            .by_login(&body.login)
            .map(|p| (p.id, p.pin.clone(), p.status.clone()));
        let wait = inner.guard.wait(found.as_ref().map(|f| f.0), mono);
        if wait > 0 {
            let secs = wait.div_ceil(1000);
            reply.header("Retry-After", secs.to_string());
            return Err(ApiError::new(
                429,
                "locked",
                format!("Too many wrong PINs. Try again in {secs} s"),
            ));
        }
        let Some((id, stored, status)) = found else {
            pin::verify(&body.pin, DUMMY_PIN);
            inner.guard.failed(None, mono);
            return Err(wrong());
        };
        if stored.is_empty() || !pin::verify(&body.pin, &stored) {
            inner.guard.failed(Some(id), mono);
            return Err(wrong());
        }
        if status != ACTIVE {
            return Err(ApiError::new(403, "disabled", "This account is disabled"));
        }
        inner.guard.succeeded(id);
        drop(inner);
        self.mutate(|inner| {
            Self::sign_in(inner, id, req, reply)?;
            Self::me(&inner.state, id)
        })
    }

    fn group_id(&self, raw: &str) -> Result<u32, ApiError> {
        self.lock()
            .state
            .group_ref(raw)
            .ok_or_else(|| ApiError::not_found("No such group"))
    }

    fn route(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        let path = req.path;
        if let Some(rest) = path.strip_prefix("/cal/") {
            if req.method == "GET" {
                self.feed_route(rest, reply)?;
            }
            return Ok(());
        }
        if path == "/auth/callback" && req.method == "GET" {
            return self.oauth_callback(req, reply);
        }
        if !path.starts_with("/api/v1/") {
            return Ok(());
        }
        require_same_origin(req, &self.dev_origins)?;
        let now = wall().unwrap_or(0);
        let page = |req: &Request<'_>| -> Result<(u32, usize), ApiError> {
            let after = req.param_num::<u32>("after")?.unwrap_or(0);
            let limit = req
                .param_num::<usize>("limit")?
                .unwrap_or(PAGE)
                .clamp(1, PAGE);
            Ok((after, limit))
        };
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
                        .setup(&new.display_name, &new.email, &new.pin, now)?;
                    Self::sign_in(inner, id, req, reply)?;
                    Self::me(&inner.state, id)
                })?;
                log::info!("first-run setup: admin {} created", me.id);
                reply.wire_status(req, 201, &me);
            }

            ("POST", "/api/v1/auth/signup") => {
                let new: NewAccount = req.decode()?;
                let me = self.mutate(|inner| {
                    let s = &mut inner.state;
                    let id = if new.invite.is_empty() {
                        s.signup(&new.display_name, &new.email, &new.pin, now)?
                    } else {
                        s.signup_invited(&new.display_name, &new.email, &new.pin, &new.invite, now)?
                    };
                    Self::sign_in(inner, id, req, reply)?;
                    Self::me(&inner.state, id)
                })?;
                reply.wire_status(req, 201, &me);
            }

            ("POST", "/api/v1/auth/pin") => {
                let body: PinSignIn = req.decode()?;
                let me = self.pin_sign_in(&body, req, reply)?;
                reply.wire(req, &me);
            }

            ("POST", "/api/v1/auth/logout") => {
                let all = req.param("all").is_some_and(|v| v != "0");
                let caller = self.viewer(req);
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

            ("GET", "/api/v1/auth/me") => {
                let c = self.caller(req)?;
                let me = Self::me(&self.lock().state, c.id)?;
                reply.wire(req, &me);
            }

            ("PATCH", "/api/v1/me") => {
                let c = self.caller(req)?;
                let change: MeUpdate = req.decode()?;
                let me = self.mutate(|inner| {
                    inner.state.update_me(c.id, &change)?;
                    Self::me(&inner.state, c.id)
                })?;
                reply.wire(req, &me);
            }

            ("PUT", "/api/v1/me/pin") => {
                let c = self.caller(req)?;
                let change: PinChange = req.decode()?;
                self.mutate(|inner| inner.state.change_pin(c.id, &change.current, &change.pin))?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", "/api/v1/people") => {
                let c = self.caller(req)?;
                let q = state::fold(&req.param("q").unwrap_or_default());
                let (after, limit) = page(req)?;
                let inner = self.lock();
                let s = &inner.state;
                let mut people = Vec::new();
                let mut next = 0;
                for p in s.people.iter().filter(|p| p.id > after) {
                    if !s.can_see_person(Some(c.id), p) {
                        continue;
                    }
                    if !q.is_empty() && !state::fold(&p.display_name).contains(&q) {
                        continue;
                    }
                    if people.len() == limit {
                        next = people.last().map_or(0, |v: &PersonView| v.id);
                        break;
                    }
                    people.push(Self::person_view(s, p));
                }
                reply.wire(req, &PersonList { people, next });
            }

            ("GET", "/api/v1/households") => {
                let c = self.caller(req)?;
                let inner = self.lock();
                let s = &inner.state;
                let households = s
                    .households
                    .iter()
                    .map(|h| HouseholdView {
                        id: h.id,
                        name: h.name.clone(),
                        members: s
                            .household_members(h.id)
                            .filter(|p| s.can_see_person(Some(c.id), p))
                            .map(|p| Self::person_view(s, p))
                            .collect(),
                    })
                    .collect();
                reply.wire(req, &HouseholdList { households });
            }

            ("POST", "/api/v1/households") => {
                let c = self.admin(req)?;
                let input: HouseholdInput = req.decode()?;
                let id =
                    self.mutate(|inner| inner.state.create_household(c.id, &input.name, now))?;
                reply.wire_status(
                    req,
                    201,
                    &HouseholdView {
                        id,
                        name: input.name.trim().into(),
                        members: Vec::new(),
                    },
                );
            }

            ("GET", "/api/v1/groups") => {
                let viewer = self.viewer(req).map(|c| c.id);
                let q = req.param("q").unwrap_or_default();
                let mine = req.param("mine").is_some_and(|v| v != "0");
                let (after, limit) = page(req)?;
                let (groups, next) = self
                    .lock()
                    .state
                    .list_groups(viewer, &q, mine, after, limit);
                reply.wire(req, &GroupList { groups, next });
            }

            ("POST", "/api/v1/groups") => {
                let c = self.caller(req)?;
                let input: GroupInput = req.decode()?;
                let view = self.mutate(|inner| {
                    let id = inner.state.create_group(c.id, &input, now)?;
                    inner.state.view_group(Some(c.id), id)
                })?;
                reply.wire_status(req, 201, &view);
            }

            ("GET", "/api/v1/admin/people") => {
                self.admin(req)?;
                let (after, limit) = page(req)?;
                let mono = uptime_ms();
                let mut inner = self.lock();
                let Inner { state, guard, .. } = &mut *inner;
                let mut people = Vec::new();
                let mut next = 0;
                for p in state.people.iter().filter(|p| p.id > after) {
                    if people.len() == limit {
                        next = people.last().map_or(0, |v: &AdminPerson| v.id);
                        break;
                    }
                    let session_seen = state
                        .sessions
                        .records()
                        .iter()
                        .filter(|r| r.subject == p.id)
                        .map(|r| r.last_seen)
                        .max()
                        .unwrap_or(0);
                    people.push(AdminPerson {
                        id: p.id,
                        display_name: p.display_name.clone(),
                        email: p.email.clone(),
                        role: state::role(p).into(),
                        status: p.status.clone(),
                        household: p.household,
                        has_pin: !p.pin.is_empty(),
                        sessions: state.sessions.count(p.id) as u32,
                        created: p.created,
                        last_seen: p.last_seen.max(session_seen),
                        locked_secs: guard.wait(Some(p.id), mono).div_ceil(1000) as u32,
                    });
                }
                reply.wire(req, &AdminPersonList { people, next });
            }

            ("GET", "/api/v1/admin/settings") => {
                self.admin(req)?;
                let inner = self.lock();
                reply.wire(
                    req,
                    &Settings {
                        signup_open: !inner.state.signup_closed,
                        groups_admin_only: inner.state.groups_admin_only,
                    },
                );
            }

            ("PATCH", "/api/v1/admin/settings") => {
                let c = self.admin(req)?;
                let update: SettingsUpdate = req.decode()?;
                let settings = self.mutate(|inner| {
                    let s = &mut inner.state;
                    let before = (s.signup_closed, s.groups_admin_only);
                    if let Some(open) = update.signup_open {
                        s.signup_closed = !open;
                    }
                    if let Some(only) = update.groups_admin_only {
                        s.groups_admin_only = only;
                    }
                    if before != (s.signup_closed, s.groups_admin_only) {
                        s.log(
                            c.id,
                            "settings",
                            format!(
                                "signup_open={} groups_admin_only={}",
                                !s.signup_closed, s.groups_admin_only
                            ),
                            now,
                        );
                    }
                    Ok(Settings {
                        signup_open: !s.signup_closed,
                        groups_admin_only: s.groups_admin_only,
                    })
                })?;
                reply.wire(req, &settings);
            }

            ("GET", "/api/v1/admin/storage") => {
                self.admin(req)?;
                let inner = self.lock();
                let s = &inner.state;
                reply.wire(
                    req,
                    &StorageInfo {
                        records: inner.records.records(),
                        last_commit_bytes: inner.records.last_commit_bytes,
                        max_commit_bytes: inner.records.max_commit_bytes,
                        commits: s.commits,
                        people: s.people.len() as u32,
                        max_people: s.limits.people as u32,
                        sessions: s.sessions.records().len() as u32,
                        max_sessions: s.limits.sessions as u32,
                        groups: s.groups.len() as u32,
                        max_groups: s.limits.groups as u32,
                        households: s.households.len() as u32,
                        max_households: s.limits.households as u32,
                        quarantined: inner.quarantined.clone(),
                    },
                );
            }

            ("GET", "/api/v1/admin/audit") => {
                self.admin(req)?;
                let limit = req
                    .param_num::<usize>("limit")?
                    .unwrap_or(100)
                    .clamp(1, 500);
                let entries = self.lock().state.audit_entries(limit);
                reply.wire(req, &AuditList { entries });
            }

            ("GET", "/api/v1/admin/zone") => {
                self.admin(req)?;
                reply.wire(req, &self.zone_setting());
            }

            ("PATCH", "/api/v1/admin/zone") => {
                let c = self.admin(req)?;
                let z: ZoneSetting = req.decode()?;
                let zone = crate::tz::Zone::parse(&z.zone)?;
                self.mutate(|inner| {
                    let s = &mut inner.state;
                    let spec = z.zone.trim().to_string();
                    if spec != s.zone_spec {
                        s.zone_spec = spec;
                        s.zone = zone;
                        s.log(c.id, "settings", format!("zone={}", s.zone_spec), now);
                    }
                    Ok(())
                })?;
                reply.wire(req, &self.zone_setting());
            }

            ("GET", "/api/v1/events") => {
                let viewer = self.viewer(req).map(|c| c.id);
                let group = match req.param("group") {
                    Some(g) if !g.is_empty() => Some(self.group_id(&g)?),
                    _ => None,
                };
                let mine = req.param("mine").is_some_and(|v| v != "0");
                let past = req.param("past").is_some_and(|v| v != "0");
                let (after, limit) = page(req)?;
                let (events, next) = self
                    .lock()
                    .state
                    .list_events(viewer, group, mine, past, now, after, limit);
                reply.wire(req, &EventList { events, next });
            }

            ("GET", "/api/v1/notifications") => {
                let c = self.caller(req)?;
                let notices = self.lock().state.notices(c.id);
                reply.wire(req, &NoticeList { notices });
            }

            ("POST", "/api/v1/notifications/read") => {
                let c = self.caller(req)?;
                let mark: MarkRead = req.decode()?;
                self.mutate(|inner| {
                    inner.state.mark_read(c.id, mark.up_to);
                    Ok(())
                })?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", "/api/v1/me/summary") => {
                let c = self.caller(req)?;
                let summary = self.lock().state.summary(c.id, now);
                reply.wire(req, &summary);
            }

            ("POST", "/api/v1/me/calendar") => {
                let c = self.caller(req)?;
                let token = miniframework::session::random_token()
                    .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
                let hash = token_hash(&token);
                self.mutate(|inner| {
                    inner.state.person_mut(c.id)?.feed_hash = hash;
                    Ok(())
                })?;
                reply.wire(
                    req,
                    &FeedUrl {
                        path: format!("/cal/{token}.ics"),
                    },
                );
            }

            #[cfg(feature = "gate")]
            ("GET", "/api/v1/gate") => reply.wire(req, &self.gate.report()),
            #[cfg(feature = "gate")]
            ("POST", "/api/v1/gate/storage") => {
                let records = req
                    .param_num::<u32>("records")?
                    .unwrap_or(20_000)
                    .min(60_000);
                let bytes = req
                    .param_num::<u32>("bytes")?
                    .unwrap_or(200)
                    .clamp(1, 4_000);
                self.gate_job(crate::gate::Job::Storage { records, bytes })?;
                reply.wire(req, &self.gate.report());
            }
            #[cfg(feature = "gate")]
            ("POST", "/api/v1/gate/crypto") => {
                let bytes = req
                    .param_num::<u32>("bytes")?
                    .unwrap_or(256 * 1024)
                    .clamp(1, 1 << 20);
                self.gate_job(crate::gate::Job::Crypto { bytes })?;
                reply.wire(req, &self.gate.report());
            }
            #[cfg(feature = "gate")]
            ("POST", "/api/v1/gate/outbound") => {
                let secs = req.param_num::<u64>("secs")?.unwrap_or(300).min(3600);
                self.gate.start_outbound(secs);
                reply.wire(req, &self.gate.report());
            }

            _ => return self.route_items(req, reply, now),
        }
        Ok(())
    }

    fn zone_setting(&self) -> ZoneSetting {
        let inner = self.lock();
        let s = &inner.state;
        ZoneSetting {
            zone: if s.zone_spec.is_empty() {
                crate::tz::DEFAULT.into()
            } else {
                s.zone_spec.clone()
            },
            offset_secs: s.zone.offset(wall().unwrap_or(0)) as i32,
        }
    }

    #[cfg(feature = "gate")]
    fn gate_job(&self, job: crate::gate::Job) -> Result<(), ApiError> {
        if self.gate.queue(job) {
            Ok(())
        } else {
            Err(ApiError::new(
                409,
                "busy",
                "A measurement is already running",
            ))
        }
    }

    /// Routes with an id in the path.
    fn route_items(
        &self,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        now: u64,
    ) -> Result<(), ApiError> {
        let rest = req.path.trim_start_matches("/api/v1/");
        let parts: Vec<&str> = rest.split('/').collect();
        match (req.method, parts.as_slice()) {
            ("GET", ["people", id]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such person")?;
                let inner = self.lock();
                let s = &inner.state;
                let p = s
                    .person(id)
                    .filter(|p| s.can_see_person(Some(c.id), p))
                    .ok_or_else(|| ApiError::not_found("No such person"))?;
                reply.wire(req, &Self::person_view(s, p));
            }

            ("PATCH", ["admin", "people", id]) => {
                let c = self.admin(req)?;
                let id = parse_id(id, "No such person")?;
                let change: AdminPersonUpdate = req.decode()?;
                self.mutate(|inner| inner.state.admin_update(c.id, id, &change, now))?;
                if change.pin.is_some() {
                    self.lock().guard.clear(id);
                }
                reply.wire(req, &Done { ok: true });
            }

            ("PATCH", ["households", id]) => {
                let c = self.admin(req)?;
                let id = parse_id(id, "No such household")?;
                let input: HouseholdInput = req.decode()?;
                self.mutate(|inner| inner.state.rename_household(c.id, id, &input.name, now))?;
                reply.wire(req, &Done { ok: true });
            }

            ("DELETE", ["households", id]) => {
                let c = self.admin(req)?;
                let id = parse_id(id, "No such household")?;
                self.mutate(|inner| inner.state.delete_household(c.id, id, now))?;
                reply.wire(req, &Done { ok: true });
            }

            ("PUT", ["households", id, "members", person]) => {
                let c = self.admin(req)?;
                let id = parse_id(id, "No such household")?;
                let person = parse_id(person, "No such person")?;
                self.mutate(|inner| inner.state.set_household(c.id, person, id, now))?;
                reply.wire(req, &Done { ok: true });
            }

            ("DELETE", ["households", id, "members", person]) => {
                let c = self.admin(req)?;
                let id = parse_id(id, "No such household")?;
                let person = parse_id(person, "No such person")?;
                self.mutate(|inner| {
                    let here = inner
                        .state
                        .person(person)
                        .is_some_and(|p| p.household == id);
                    if here {
                        inner.state.set_household(c.id, person, 0, now)?;
                    }
                    Ok(())
                })?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", ["groups", g]) => {
                let viewer = self.viewer(req).map(|c| c.id);
                let id = self.group_id(g)?;
                let view = self.lock().state.view_group(viewer, id)?;
                reply.wire(req, &view);
            }

            ("PATCH", ["groups", g]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                let input: GroupInput = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.update_group(c.id, id, &input, now)?;
                    inner.state.view_group(Some(c.id), id)
                })?;
                reply.wire(req, &view);
            }

            ("DELETE", ["groups", g]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                self.mutate(|inner| inner.state.delete_group(c.id, id, now))?;
                reply.wire(req, &Done { ok: true });
            }

            ("GET", ["groups", g, "members"]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                let members = self.lock().state.group_members(c.id, id)?;
                reply.wire(req, &MemberList { members });
            }

            ("POST", ["groups", g, "join"]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                let view = self.mutate(|inner| {
                    inner.state.join_group(c.id, id, now)?;
                    inner.state.view_group(Some(c.id), id)
                })?;
                reply.wire(req, &view);
            }

            ("POST", ["groups", g, "leave"]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                self.mutate(|inner| inner.state.leave_group(c.id, id))?;
                let view = self.lock().state.view_group(Some(c.id), id).ok();
                match view {
                    Some(v) => reply.wire(req, &v),
                    None => reply.wire(req, &Done { ok: true }),
                }
            }

            ("PUT", ["groups", g, "members", person]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                let person = parse_id(person, "No such person")?;
                let update: MemberUpdate = req.decode()?;
                let members = self.mutate(|inner| {
                    inner.state.set_member(
                        c.id,
                        id,
                        person,
                        update.role.as_deref(),
                        update.status.as_deref(),
                        now,
                    )?;
                    inner.state.group_members(c.id, id)
                })?;
                reply.wire(req, &MemberList { members });
            }

            ("POST", ["groups", g, "transfer"]) => {
                let c = self.caller(req)?;
                let id = self.group_id(g)?;
                let t: Transfer = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.transfer_group(c.id, id, t.to, now)?;
                    inner.state.view_group(Some(c.id), id)
                })?;
                reply.wire(req, &view);
            }

            ("POST", ["groups", g, "events"]) => {
                let c = self.caller(req)?;
                let group = self.group_id(g)?;
                let input: EventInput = req.decode()?;
                let view = self.mutate(|inner| {
                    let id = inner.state.create_event(c.id, group, &input, now)?;
                    inner.state.event_view(Some(c.id), id, now)
                })?;
                reply.wire_status(req, 201, &view);
            }

            ("POST", ["groups", g, "series"]) => {
                let c = self.caller(req)?;
                let group = self.group_id(g)?;
                let input: SeriesInput = req.decode()?;
                let view = self.mutate(|inner| {
                    let id = inner.state.create_series(c.id, group, &input, now)?;
                    inner.state.series_view(c.id, id, now)
                })?;
                reply.wire_status(req, 201, &view);
            }

            ("GET", ["events", id]) if !id.ends_with(".ics") => {
                let viewer = self.viewer(req).map(|c| c.id);
                let id = parse_id(id, "No such event")?;
                let view = self.lock().state.event_view(viewer, id, now)?;
                reply.wire(req, &view);
            }

            ("PATCH", ["events", id]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                let input: EventInput = req.decode()?;
                let following = req.param("scope").is_some_and(|s| s == "following");
                let view = self.mutate(|inner| {
                    if following {
                        inner.state.update_following(c.id, id, &input, now)?;
                    } else {
                        inner.state.update_event(c.id, id, &input, now)?;
                    }
                    inner.state.event_view(Some(c.id), id, now)
                })?;
                reply.wire(req, &view);
            }

            ("DELETE", ["events", id]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                self.mutate(|inner| inner.state.delete_event(c.id, id, now))?;
                reply.wire(req, &Done { ok: true });
            }

            ("POST", ["events", id, "cancel"]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                let view = self.mutate(|inner| {
                    inner.state.cancel_event(c.id, id, now)?;
                    inner.state.event_view(Some(c.id), id, now)
                })?;
                reply.wire(req, &view);
            }

            ("POST", ["events", id, "lock"]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                let lock: LockIn = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.lock_in(c.id, id, lock.option, now)?;
                    inner.state.event_view(Some(c.id), id, now)
                })?;
                reply.wire(req, &view);
            }

            ("GET", ["events", id, "rsvps"]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                let rsvps = self.lock().state.rsvp_list(c.id, id)?;
                reply.wire(req, &RsvpList { rsvps });
            }

            ("PUT", ["events", id, "rsvps", who]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                let person = if *who == "me" {
                    c.id
                } else {
                    parse_id(who, "No such person")?
                };
                let input: RsvpInput = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.answer(c.id, id, person, &input, now)?;
                    inner.state.event_view(Some(c.id), id, now)
                })?;
                reply.wire(req, &view);
            }

            ("GET", ["events", id, "rides"]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such event")?;
                let board = self.lock().state.ride_board(c.id, id)?;
                reply.wire(req, &board);
            }

            ("GET", ["events", file]) if file.ends_with(".ics") => {
                let viewer = self.viewer(req).map(|c| c.id);
                let id = parse_id(file.trim_end_matches(".ics"), "No such event")?;
                let inner = self.lock();
                let (e, _) = inner.state.visible_event(viewer, id)?;
                let body = crate::ics::event_file(&inner.state, e);
                reply.bytes(200, "text/calendar; charset=utf-8", body.as_bytes());
                reply.header(
                    "Content-Disposition",
                    format!("attachment; filename=\"party-{id}.ics\""),
                );
            }

            ("GET", ["series", id]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such series")?;
                let view = self.lock().state.series_view(c.id, id, now)?;
                reply.wire(req, &view);
            }

            ("PATCH", ["series", id]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such series")?;
                let input: SeriesInput = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.update_series(c.id, id, &input, now)?;
                    inner.state.series_view(c.id, id, now)
                })?;
                reply.wire(req, &view);
            }

            ("DELETE", ["series", id]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such series")?;
                let cancel = req.param("cancel").is_some_and(|v| v != "0");
                self.mutate(|inner| inner.state.end_series(c.id, id, cancel, now))?;
                reply.wire(req, &Done { ok: true });
            }

            ("PUT", ["series", id, "regular"]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such series")?;
                let r: Regular = req.decode()?;
                let view = self.mutate(|inner| {
                    inner.state.set_regular(c.id, id, r.on, now)?;
                    inner.state.series_view(c.id, id, now)
                })?;
                reply.wire(req, &view);
            }

            _ => return self.route_more(req, reply, now),
        }
        Ok(())
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
            PersonView::SCHEMA,
            PersonList::SCHEMA,
            AdminPersonList::SCHEMA,
            AdminPersonUpdate::SCHEMA,
            Settings::SCHEMA,
            SettingsUpdate::SCHEMA,
            StorageInfo::SCHEMA,
            AuditList::SCHEMA,
            Done::SCHEMA,
            HouseholdList::SCHEMA,
            HouseholdView::SCHEMA,
            HouseholdInput::SCHEMA,
            GroupInput::SCHEMA,
            GroupView::SCHEMA,
            GroupList::SCHEMA,
            MemberList::SCHEMA,
            MemberUpdate::SCHEMA,
            Transfer::SCHEMA,
            EventInput::SCHEMA,
            EventView::SCHEMA,
            EventList::SCHEMA,
            RsvpInput::SCHEMA,
            RsvpList::SCHEMA,
            RideBoard::SCHEMA,
            LockIn::SCHEMA,
            SeriesInput::SCHEMA,
            SeriesView::SCHEMA,
            Regular::SCHEMA,
            NoticeList::SCHEMA,
            MarkRead::SCHEMA,
            Summary::SCHEMA,
            FeedUrl::SCHEMA,
            ZoneSetting::SCHEMA,
        ];
        #[cfg(feature = "gate")]
        let schemas = [schemas, vec![crate::gate::GateReport::SCHEMA]].concat();
        schemas
    }

    fn metrics(&self) -> Vec<(&'static str, f64)> {
        let inner = self.lock();
        let s = &inner.state;
        vec![
            ("party_people", s.people.len() as f64),
            ("party_groups", s.groups.len() as f64),
            ("party_households", s.households.len() as f64),
            ("party_sessions", s.sessions.records().len() as f64),
            ("party_commits", s.commits as f64),
            (
                "party_last_commit_bytes",
                inner.records.last_commit_bytes as f64,
            ),
            (
                "party_max_commit_bytes",
                inner.records.max_commit_bytes as f64,
            ),
        ]
    }

    fn checks(&self) -> Vec<Check> {
        let inner = self.lock();
        let s = &inner.state;
        let records = if inner.quarantined.is_empty() {
            Check::new("store.records", CheckStatus::Ok, "Every record loaded")
        } else {
            Check::new(
                "store.records",
                CheckStatus::Warn,
                format!(
                    "{} record(s) failed to load: {}",
                    inner.quarantined.len(),
                    inner.quarantined.join("; ")
                ),
            )
        };
        vec![
            records.runbook("storage"),
            Check::usage(
                "store.people",
                "People",
                s.people.len() as f64,
                s.limits.people as f64,
                80.0,
                95.0,
            ),
            Check::usage(
                "store.groups",
                "Groups",
                s.groups.len() as f64,
                s.limits.groups as f64,
                80.0,
                95.0,
            ),
        ]
    }
}

/// SHA-256 (hex) of a calendar token: only the hash is stored.
fn token_hash(token: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn parse_id(raw: &str, missing: &str) -> Result<u32, ApiError> {
    raw.parse()
        .map_err(|_| ApiError::not_found(missing.to_string()))
}

/// The app behind an `Arc`, so the outbound worker thread can share it with
/// the server (`Site` owns its service).
pub struct Shared(pub Arc<App>);

impl Service for Shared {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        self.0.handle(req, reply);
    }
    fn schemas(&self) -> Vec<&'static Schema> {
        self.0.schemas()
    }
    fn metrics(&self) -> Vec<(&'static str, f64)> {
        self.0.metrics()
    }
    fn checks(&self) -> Vec<Check> {
        self.0.checks()
    }
}
