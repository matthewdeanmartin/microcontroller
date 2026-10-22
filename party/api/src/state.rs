//! The domain state and its rules, free of HTTP and storage so it can be
//! tested directly. `api.rs` locks it, calls these, and commits; a commit
//! writes only the rows that changed (`records.rs`).
//!
//! People, households, settings and the audit log live here; groups and
//! their permissions in `groups.rs`.
use crate::messages::{
    AdminPersonUpdate, StoredAuditBlock, StoredAuditEntry, StoredChannel, StoredEvent, StoredGroup,
    StoredHousehold, StoredInbox, StoredInvite, StoredJob, StoredMeta, StoredPerson,
    StoredRsvpBlock, StoredSeries, StoredSession,
};
use crate::pin;
use crate::table::Table;
use miniframework::session::{Record, SessionConfig, Sessions};
use miniframework::ApiError;
use std::collections::BTreeMap;

/// Record schema this firmware writes (and the newest it can read).
pub const SCHEMA: u32 = 1;

/// Audit entries per block record (`l<id>`); the ring holds
/// `Limits::audit` entries in `audit / AUDIT_BLOCK` blocks.
pub const AUDIT_BLOCK: usize = 32;

pub const ACTIVE: &str = "active";
pub const DISABLED: &str = "disabled";

/// Fixed capacities (`spec/07-storage-limits.md#bounds-s3-profile`). A full
/// collection answers 409 `full`; nothing is evicted silently.
#[derive(Clone, Debug)]
pub struct Limits {
    pub people: usize,
    pub households: usize,
    pub groups: usize,
    pub members_per_group: usize,
    pub sessions: usize,
    pub sessions_per_person: usize,
    /// PBKDF2 rounds for new PIN hashes.
    pub pin_rounds: u32,
    /// Audit entries kept (oldest dropped first, in blocks).
    pub audit: usize,
    /// Upcoming and draft events.
    pub upcoming_events: usize,
    /// Done and cancelled events kept (oldest dropped first).
    pub past_events: usize,
    pub rsvps_per_event: usize,
    pub series: usize,
    /// Notifications kept per person (oldest dropped first).
    pub notices: usize,
    pub invites: usize,
    /// Outbound deliveries queued or recently finished.
    pub jobs: usize,
    /// Emails a day (Brevo's free plan allows 300).
    pub mail_per_day: u32,
}

impl Limits {
    /// The ESP32-S3 N16R8 profile.
    pub fn board() -> Self {
        Self {
            people: 2_000,
            households: 256,
            groups: 128,
            members_per_group: 500,
            sessions: 1_024,
            sessions_per_person: 6,
            // Game Matcher's C6 figure (1,000 rounds ≈ 100 ms); the S3 boot
            // log reports the real time. Re-measure in Sprint 0.
            pin_rounds: 1_000,
            audit: 1_024,
            upcoming_events: 512,
            past_events: 2_048,
            rsvps_per_event: 500,
            series: 128,
            notices: 32,
            invites: 1_024,
            jobs: 64,
            mail_per_day: 100,
        }
    }
}

pub struct State {
    pub people: Table<StoredPerson>,
    pub households: Table<StoredHousehold>,
    pub groups: Table<StoredGroup>,
    pub audit: Table<StoredAuditBlock>,
    pub events: Table<StoredEvent>,
    pub rsvps: Table<StoredRsvpBlock>,
    pub series: Table<StoredSeries>,
    pub inboxes: Table<StoredInbox>,
    pub invites: Table<StoredInvite>,
    pub jobs: Table<StoredJob>,
    pub channels: Table<StoredChannel>,
    pub sessions: Sessions,
    /// Sessions may differ from flash although `sessions.dirty()` says
    /// otherwise (a rollback rebuilt them).
    pub sessions_unsure: bool,
    pub next_person: u32,
    pub next_household: u32,
    pub next_group: u32,
    pub next_audit: u64,
    pub next_event: u32,
    pub next_series: u32,
    pub next_notice: u32,
    pub next_queue: u64,
    pub next_invite: u32,
    pub next_job: u32,
    /// How people reach the local app (`http://party.local`), for links in
    /// email and DMs (RAM; set by the app).
    pub origin: String,
    /// The bot's recent commands (RAM only).
    pub inbox_log: std::collections::VecDeque<crate::messages::InboxEntry>,
    /// Each person's last bot-made change, for `undo` (RAM only): the event,
    /// the answer before, and when.
    pub undo: std::collections::HashMap<u32, (u32, Option<crate::messages::StoredRsvp>, u64)>,
    /// The instance zone's POSIX TZ string ("" = `tz::DEFAULT`), and parsed.
    pub zone_spec: String,
    pub zone: crate::tz::Zone,
    pub signup_closed: bool,
    pub groups_admin_only: bool,
    /// Commits made (RAM; housekeeping persists it).
    pub commits: u64,
    pub limits: Limits,
}

/// Rows and sessions as loaded from flash.
#[derive(Default)]
pub struct Parts {
    pub people: Vec<StoredPerson>,
    pub households: Vec<StoredHousehold>,
    pub groups: Vec<StoredGroup>,
    pub audit: Vec<StoredAuditBlock>,
    pub events: Vec<StoredEvent>,
    pub rsvps: Vec<StoredRsvpBlock>,
    pub series: Vec<StoredSeries>,
    pub inboxes: Vec<StoredInbox>,
    pub invites: Vec<StoredInvite>,
    pub jobs: Vec<StoredJob>,
    pub channels: Vec<StoredChannel>,
    pub sessions: Vec<StoredSession>,
    pub commits: u64,
}

/// Where [`State::rollback`] returns to. Rows cost only what changed since;
/// sessions are copied whole (small records, bounded).
pub struct Savepoint {
    tables: [usize; 11],
    meta: StoredMeta,
    commits: u64,
    sessions: Vec<Record>,
    sessions_dirty: bool,
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unhex32(s: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if s.len() != 64 {
        return None;
    }
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn session_config(limits: &Limits) -> SessionConfig {
    SessionConfig {
        per_subject: limits.sessions_per_person,
        capacity: limits.sessions,
        ..SessionConfig::new("party_session")
    }
}

pub fn full(what: &str, bound: usize) -> ApiError {
    ApiError::new(
        409,
        "full",
        format!("This board holds at most {bound} {what}"),
    )
}

pub fn role(p: &StoredPerson) -> &'static str {
    if p.admin {
        "admin"
    } else {
        "member"
    }
}

/// Lower-cased and trimmed, for comparing names and emails.
pub fn fold(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// An optional email: "" or something that looks like `x@y`.
pub fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = fold(raw);
    if email.is_empty() {
        return Ok(email);
    }
    let ok = email.len() <= 64
        && !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(user, host)| {
            !user.is_empty() && !host.is_empty() && !host.contains('@')
        });
    if ok {
        Ok(email)
    } else {
        Err(ApiError::bad_request(
            "That doesn't look like an email address",
        ))
    }
}

/// 1–40 characters, no control characters, and not shaped like an email
/// (display names are also sign-in names).
pub fn normalize_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    let len = name.chars().count();
    if !(1..=40).contains(&len) || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("Display name: 1 to 40 characters"));
    }
    if name.contains('@') {
        return Err(ApiError::bad_request("Display names can't contain @"));
    }
    Ok(name.to_string())
}

pub fn check_pin(pin: &str) -> Result<(), ApiError> {
    if pin::valid(pin) {
        Ok(())
    } else {
        Err(ApiError::bad_request("PIN: 4 to 8 digits"))
    }
}

pub fn check_discoverable(raw: &str) -> Result<String, ApiError> {
    match raw {
        "nobody" | "members" | "public" => Ok(raw.to_string()),
        _ => Err(ApiError::bad_request(
            "discoverable must be nobody, members or public",
        )),
    }
}

fn bio(raw: &str) -> Result<String, ApiError> {
    let bio = raw.trim();
    if bio.chars().count() > 280 {
        return Err(ApiError::bad_request("About you: at most 280 characters"));
    }
    Ok(bio.to_string())
}

fn pin_hash(pin: &str, rounds: u32) -> Result<String, ApiError> {
    pin::hash(pin, rounds).map_err(|_| ApiError::new(500, "no_randomness", "No random source"))
}

impl State {
    pub fn new(limits: Limits) -> Self {
        Self::from_parts(StoredMeta::default(), Parts::default(), limits)
    }

    /// Builds the state from loaded records. Counters are repaired upwards
    /// from the rows, so a row written just before a crash (whose `meta`
    /// never landed) can't have its id handed out again.
    pub fn from_parts(meta: StoredMeta, parts: Parts, limits: Limits) -> Self {
        let next = |stored: u32, ids: &mut dyn Iterator<Item = u32>| {
            stored.max(ids.map(|id| id + 1).max().unwrap_or(1)).max(1)
        };
        let records = parts
            .sessions
            .iter()
            .filter_map(|s| {
                Some(Record {
                    hash: unhex32(&s.hash)?,
                    subject: s.account,
                    created: s.created,
                    last_seen: s.last_seen,
                })
            })
            .collect();
        let next_audit = parts
            .audit
            .iter()
            .flat_map(|b| b.entries.iter().map(|e| e.seq + 1))
            .max()
            .unwrap_or(0)
            .max(meta.next_audit);
        Self {
            next_person: next(meta.next_person, &mut parts.people.iter().map(|p| p.id)),
            next_household: next(
                meta.next_household,
                &mut parts.households.iter().map(|h| h.id),
            ),
            next_group: next(meta.next_group, &mut parts.groups.iter().map(|g| g.id)),
            next_audit,
            next_event: next(meta.next_event, &mut parts.events.iter().map(|e| e.id)),
            next_series: next(meta.next_series, &mut parts.series.iter().map(|s| s.id)),
            next_notice: meta
                .next_notice
                .max(
                    parts
                        .inboxes
                        .iter()
                        .flat_map(|b| b.notices.iter().map(|n| n.id + 1))
                        .max()
                        .unwrap_or(1),
                )
                .max(1),
            next_queue: meta.next_queue.max(
                parts
                    .rsvps
                    .iter()
                    .flat_map(|b| b.rsvps.iter().map(|r| r.queued + 1))
                    .max()
                    .unwrap_or(1),
            ),
            next_invite: next(meta.next_invite, &mut parts.invites.iter().map(|i| i.id)),
            next_job: next(meta.next_job, &mut parts.jobs.iter().map(|j| j.id)),
            origin: "http://party.local".into(),
            inbox_log: Default::default(),
            undo: Default::default(),
            zone: crate::tz::Zone::parse(&meta.zone)
                .unwrap_or_else(|_| crate::tz::Zone::parse(crate::tz::DEFAULT).expect("valid")),
            zone_spec: meta.zone,
            people: Table::from_rows(parts.people),
            households: Table::from_rows(parts.households),
            groups: Table::from_rows(parts.groups),
            audit: Table::from_rows(parts.audit),
            events: Table::from_rows(parts.events),
            rsvps: Table::from_rows(parts.rsvps),
            series: Table::from_rows(parts.series),
            inboxes: Table::from_rows(parts.inboxes),
            invites: Table::from_rows(parts.invites),
            jobs: Table::from_rows(parts.jobs),
            channels: Table::from_rows(parts.channels),
            sessions: Sessions::restore(session_config(&limits), records),
            sessions_unsure: false,
            signup_closed: meta.signup_closed,
            groups_admin_only: meta.groups_admin_only,
            commits: parts.commits,
            limits,
        }
    }

    /// Everything that isn't a row.
    pub fn meta(&self) -> StoredMeta {
        StoredMeta {
            schema: SCHEMA,
            next_person: self.next_person,
            next_household: self.next_household,
            next_group: self.next_group,
            signup_closed: self.signup_closed,
            groups_admin_only: self.groups_admin_only,
            next_audit: self.next_audit,
            next_event: self.next_event,
            next_series: self.next_series,
            next_notice: self.next_notice,
            zone: self.zone_spec.clone(),
            next_queue: self.next_queue,
            next_invite: self.next_invite,
            next_job: self.next_job,
        }
    }

    fn set_meta(&mut self, m: StoredMeta) {
        self.next_person = m.next_person;
        self.next_household = m.next_household;
        self.next_group = m.next_group;
        self.signup_closed = m.signup_closed;
        self.groups_admin_only = m.groups_admin_only;
        self.next_audit = m.next_audit;
        self.next_event = m.next_event;
        self.next_series = m.next_series;
        self.next_notice = m.next_notice;
        self.next_queue = m.next_queue;
        self.next_invite = m.next_invite;
        self.next_job = m.next_job;
        if m.zone != self.zone_spec {
            if let Ok(z) = crate::tz::Zone::parse(&m.zone) {
                self.zone = z;
            }
            self.zone_spec = m.zone;
        }
    }

    /// Sessions in their stored form, grouped by person (ascending).
    pub fn stored_sessions(&self) -> BTreeMap<u32, Vec<StoredSession>> {
        let mut by: BTreeMap<u32, Vec<StoredSession>> = BTreeMap::new();
        for r in self.sessions.records() {
            by.entry(r.subject).or_default().push(StoredSession {
                hash: hex(&r.hash),
                account: r.subject,
                created: r.created,
                last_seen: r.last_seen,
            });
        }
        by
    }

    pub fn savepoint(&mut self) -> Savepoint {
        Savepoint {
            tables: [
                self.people.savepoint(),
                self.households.savepoint(),
                self.groups.savepoint(),
                self.audit.savepoint(),
                self.events.savepoint(),
                self.rsvps.savepoint(),
                self.series.savepoint(),
                self.inboxes.savepoint(),
                self.invites.savepoint(),
                self.jobs.savepoint(),
                self.channels.savepoint(),
            ],
            meta: self.meta(),
            commits: self.commits,
            sessions: self.sessions.records().to_vec(),
            sessions_dirty: self.sessions.dirty() || self.sessions_unsure,
        }
    }

    /// Undoes everything since `sp`, in RAM. Flash is untouched.
    pub fn rollback(&mut self, sp: Savepoint) {
        let [p, h, g, l, e, r, q, n, v, o, c] = sp.tables;
        self.people.rollback(p);
        self.households.rollback(h);
        self.groups.rollback(g);
        self.audit.rollback(l);
        self.events.rollback(e);
        self.rsvps.rollback(r);
        self.series.rollback(q);
        self.inboxes.rollback(n);
        self.invites.rollback(v);
        self.jobs.rollback(o);
        self.channels.rollback(c);
        self.set_meta(sp.meta);
        self.commits = sp.commits;
        if self.sessions.records() != sp.sessions.as_slice() {
            self.sessions = Sessions::restore(session_config(&self.limits), sp.sessions);
            self.sessions_unsure |= sp.sessions_dirty;
        }
    }

    /// The records just written match RAM.
    pub fn saved(&mut self) {
        self.people.saved();
        self.households.saved();
        self.groups.saved();
        self.audit.saved();
        self.events.saved();
        self.rsvps.saved();
        self.series.saved();
        self.inboxes.saved();
        self.invites.saved();
        self.jobs.saved();
        self.channels.saved();
        self.sessions.saved();
        self.sessions_unsure = false;
    }

    // ---- audit --------------------------------------------------------------

    /// Records an admin or organizer action. The ring keeps the newest
    /// `Limits::audit`; each entry rewrites one block record.
    pub fn log(&mut self, actor: u32, action: &str, detail: impl Into<String>, now: u64) {
        let blocks = (self.limits.audit / AUDIT_BLOCK).max(1) as u64;
        let seq = self.next_audit;
        self.next_audit += 1;
        let block = ((seq / AUDIT_BLOCK as u64) % blocks) as u32 + 1;
        let entry = StoredAuditEntry {
            seq,
            at: now,
            actor,
            action: action.to_string(),
            detail: detail.into(),
        };
        match self.audit.get_mut(block) {
            Some(b) => {
                // A block being reused for a newer lap starts empty.
                if b.entries
                    .first()
                    .is_some_and(|e| e.seq / AUDIT_BLOCK as u64 != seq / AUDIT_BLOCK as u64)
                {
                    b.entries.clear();
                }
                b.entries.push(entry);
            }
            None => self.audit.push(StoredAuditBlock {
                id: block,
                entries: vec![entry],
            }),
        }
    }

    /// Newest first.
    pub fn audit_entries(&self, limit: usize) -> Vec<StoredAuditEntry> {
        let mut all: Vec<&StoredAuditEntry> = self.audit.iter().flat_map(|b| &b.entries).collect();
        all.sort_by_key(|e| std::cmp::Reverse(e.seq));
        all.into_iter().take(limit).cloned().collect()
    }

    // ---- people -------------------------------------------------------------

    /// The name to show for a person (or a placeholder for a gone one).
    pub fn name(&self, id: u32) -> String {
        self.person(id)
            .map_or_else(|| "someone".to_string(), |p| p.display_name.clone())
    }

    pub fn person(&self, id: u32) -> Option<&StoredPerson> {
        self.people.get(id)
    }

    pub fn person_mut(&mut self, id: u32) -> Result<&mut StoredPerson, ApiError> {
        self.people
            .get_mut(id)
            .ok_or_else(|| ApiError::not_found("No such person"))
    }

    /// Sign-in lookup: an email, or a display name (both case-insensitive).
    pub fn by_login(&self, login: &str) -> Option<&StoredPerson> {
        let login = fold(login);
        if login.is_empty() {
            return None;
        }
        if login.contains('@') {
            self.people.iter().find(|p| p.email == login)
        } else {
            self.people.iter().find(|p| fold(&p.display_name) == login)
        }
    }

    pub(crate) fn check_unique(
        &self,
        except: u32,
        name: &str,
        email: &str,
    ) -> Result<(), ApiError> {
        let folded = fold(name);
        if self
            .people
            .iter()
            .any(|p| p.id != except && fold(&p.display_name) == folded)
        {
            return Err(ApiError::new(
                409,
                "name_taken",
                "Someone already uses that display name (it's also a sign-in name)",
            ));
        }
        if !email.is_empty()
            && self
                .people
                .iter()
                .any(|p| p.id != except && p.email == email)
        {
            return Err(ApiError::new(
                409,
                "email_taken",
                "There is already an account with that email",
            ));
        }
        Ok(())
    }

    pub fn active_admins(&self) -> usize {
        self.people
            .iter()
            .filter(|p| p.admin && p.status == ACTIVE)
            .count()
    }

    /// Creates an active person. `admin` only for first-run setup.
    pub fn create_person(
        &mut self,
        name: &str,
        email: &str,
        pin: &str,
        admin: bool,
        now: u64,
    ) -> Result<u32, ApiError> {
        let name = normalize_name(name)?;
        let email = normalize_email(email)?;
        check_pin(pin)?;
        self.check_unique(0, &name, &email)?;
        if self.people.len() >= self.limits.people {
            return Err(full("people", self.limits.people));
        }
        let id = self.next_person;
        self.people.push(StoredPerson {
            id,
            display_name: name,
            email,
            pin: pin_hash(pin, self.limits.pin_rounds)?,
            admin,
            status: ACTIVE.into(),
            household: 0,
            discoverable: "members".into(),
            bio: String::new(),
            created: now,
            last_seen: now,
            ..Default::default()
        });
        self.next_person += 1;
        Ok(id)
    }

    /// First-run setup: the first person, an admin. Atomic under the
    /// caller's lock, so a second concurrent claim gets 409.
    pub fn setup(&mut self, name: &str, email: &str, pin: &str, now: u64) -> Result<u32, ApiError> {
        if !self.people.is_empty() {
            return Err(ApiError::new(
                409,
                "already_set_up",
                "party already has an admin. Sign in or sign up instead",
            ));
        }
        let id = self.create_person(name, email, pin, true, now)?;
        self.log(id, "setup", "first admin", now);
        Ok(id)
    }

    pub fn signup(
        &mut self,
        name: &str,
        email: &str,
        pin: &str,
        now: u64,
    ) -> Result<u32, ApiError> {
        if self.people.is_empty() {
            return Err(ApiError::new(
                409,
                "setup_needed",
                "Set party up first: the first account becomes admin",
            ));
        }
        if self.signup_closed {
            return Err(ApiError::new(
                403,
                "signup_closed",
                "Sign-up is closed. Ask an admin to make you an account",
            ));
        }
        self.create_person(name, email, pin, false, now)
    }

    pub fn change_pin(&mut self, id: u32, current: &str, new_pin: &str) -> Result<(), ApiError> {
        check_pin(new_pin)?;
        let rounds = self.limits.pin_rounds;
        let person = self.person_mut(id)?;
        if !person.pin.is_empty() && !pin::verify(current, &person.pin) {
            return Err(ApiError::new(403, "wrong_pin", "Current PIN is wrong"));
        }
        person.pin = pin_hash(new_pin, rounds)?;
        Ok(())
    }

    /// The person's own profile edit.
    pub fn update_me(
        &mut self,
        id: u32,
        change: &crate::messages::MeUpdate,
    ) -> Result<(), ApiError> {
        // Notification switches touch the record only when they change.
        let (email_off, dm_off) = {
            let p = self
                .person(id)
                .ok_or_else(|| ApiError::not_found("No such person"))?;
            (
                change.email_notify.map(|on| !on).unwrap_or(p.email_off),
                change.dm_notify.map(|on| !on).unwrap_or(p.dm_off),
            )
        };
        if self
            .person(id)
            .is_some_and(|p| p.email_off != email_off || p.dm_off != dm_off)
        {
            let p = self.person_mut(id)?;
            p.email_off = email_off;
            p.dm_off = dm_off;
        }
        let current = self
            .person(id)
            .ok_or_else(|| ApiError::not_found("No such person"))?
            .clone();
        let name = match &change.display_name {
            Some(n) => normalize_name(n)?,
            None => current.display_name.clone(),
        };
        let email = match &change.email {
            Some(e) => normalize_email(e)?,
            None => current.email.clone(),
        };
        let bio = change.bio.as_deref().map(bio).transpose()?;
        let discoverable = change
            .discoverable
            .as_deref()
            .map(check_discoverable)
            .transpose()?;
        self.check_unique(id, &name, &email)?;
        let unchanged = name == current.display_name
            && email == current.email
            && bio.as_ref().is_none_or(|b| *b == current.bio)
            && discoverable
                .as_ref()
                .is_none_or(|d| *d == current.discoverable);
        if unchanged {
            return Ok(());
        }
        let p = self.person_mut(id)?;
        p.display_name = name;
        p.email = email;
        if let Some(b) = bio {
            p.bio = b;
        }
        if let Some(d) = discoverable {
            p.discoverable = d;
        }
        Ok(())
    }

    /// Applies an admin's change to person `id`, acting as `actor`.
    pub fn admin_update(
        &mut self,
        actor: u32,
        id: u32,
        change: &AdminPersonUpdate,
        now: u64,
    ) -> Result<(), ApiError> {
        let rounds = self.limits.pin_rounds;
        let make_admin = match change.role.as_deref() {
            None => None,
            Some("admin") => Some(true),
            Some("member") => Some(false),
            Some(_) => return Err(ApiError::bad_request("role must be admin or member")),
        };
        let pin_hash = change
            .pin
            .as_deref()
            .map(|p| check_pin(p).and_then(|_| pin_hash(p, rounds)))
            .transpose()?;
        let display_name = change
            .display_name
            .as_deref()
            .map(normalize_name)
            .transpose()?;
        if let Some(h) = change.household {
            if h != 0 && self.households.get(h).is_none() {
                return Err(ApiError::not_found("No such household"));
            }
        }
        // Change a copy, check the invariants, then store it.
        let mut person = self
            .person(id)
            .ok_or_else(|| ApiError::not_found("No such person"))?
            .clone();
        let was_active_admin = person.admin && person.status == ACTIVE;
        if let Some(admin) = make_admin {
            person.admin = admin;
        }
        if let Some(disabled) = change.disabled {
            person.status = if disabled { DISABLED } else { ACTIVE }.into();
        }
        if let Some(hash) = pin_hash {
            person.pin = hash;
            // An email-only person with a PIN can sign in now.
            if person.status == crate::invites::INVITED {
                person.status = ACTIVE.into();
            }
        }
        if let Some(name) = display_name {
            self.check_unique(id, &name, &person.email)?;
            person.display_name = name;
        }
        if let Some(h) = change.household {
            person.household = h;
        }
        let still_active_admin = person.admin && person.status == ACTIVE;
        if was_active_admin && !still_active_admin && self.active_admins() <= 1 {
            return Err(ApiError::new(
                409,
                "last_admin",
                "party needs at least one active admin",
            ));
        }
        if id == actor && person.status == DISABLED {
            return Err(ApiError::new(
                409,
                "self_disable",
                "You can't disable yourself",
            ));
        }
        let disabled = person.status == DISABLED;
        let before = self.person(id).cloned();
        if before.as_ref() != Some(&person) {
            *self.person_mut(id)? = person;
            self.log(actor, "person.update", format!("person {id}"), now);
        }
        if change.sign_out.unwrap_or(false) || disabled {
            self.sessions.revoke_subject(id);
        }
        Ok(())
    }

    /// May `viewer` find or see `person`? Admins and the person always may;
    /// otherwise by the person's discoverability.
    pub fn can_see_person(&self, viewer: Option<u32>, person: &StoredPerson) -> bool {
        let Some(v) = viewer else {
            return false;
        };
        if v == person.id || self.person(v).is_some_and(|p| p.admin) {
            return true;
        }
        if person.status != ACTIVE {
            return false;
        }
        match person.discoverable.as_str() {
            "public" => true,
            "members" => self.share_group(v, person.id) || self.same_household(v, person.id),
            _ => false,
        }
    }

    fn same_household(&self, a: u32, b: u32) -> bool {
        let h = |id| self.person(id).map_or(0, |p| p.household);
        h(a) != 0 && h(a) == h(b)
    }

    // ---- households ---------------------------------------------------------

    pub fn household_name(&self, id: u32) -> String {
        self.households
            .get(id)
            .map(|h| h.name.clone())
            .unwrap_or_default()
    }

    fn household_title(raw: &str) -> Result<String, ApiError> {
        let name = raw.trim();
        let len = name.chars().count();
        if (1..=60).contains(&len) && !name.chars().any(char::is_control) {
            Ok(name.to_string())
        } else {
            Err(ApiError::bad_request("Household name: 1 to 60 characters"))
        }
    }

    pub fn create_household(&mut self, actor: u32, name: &str, now: u64) -> Result<u32, ApiError> {
        let name = Self::household_title(name)?;
        if self.households.iter().any(|h| fold(&h.name) == fold(&name)) {
            return Err(ApiError::new(
                409,
                "name_taken",
                "There is already a household with that name",
            ));
        }
        if self.households.len() >= self.limits.households {
            return Err(full("households", self.limits.households));
        }
        let id = self.next_household;
        self.next_household += 1;
        self.households.push(StoredHousehold {
            id,
            name,
            created: now,
        });
        self.log(actor, "household.create", format!("household {id}"), now);
        Ok(id)
    }

    pub fn rename_household(
        &mut self,
        actor: u32,
        id: u32,
        name: &str,
        now: u64,
    ) -> Result<(), ApiError> {
        let name = Self::household_title(name)?;
        if self
            .households
            .iter()
            .any(|h| h.id != id && fold(&h.name) == fold(&name))
        {
            return Err(ApiError::new(
                409,
                "name_taken",
                "There is already a household with that name",
            ));
        }
        let h = self
            .households
            .get_mut(id)
            .ok_or_else(|| ApiError::not_found("No such household"))?;
        if h.name != name {
            h.name = name;
            self.log(actor, "household.rename", format!("household {id}"), now);
        }
        Ok(())
    }

    /// Deletes a household; its people become household-less.
    pub fn delete_household(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let i = self
            .households
            .position(id)
            .ok_or_else(|| ApiError::not_found("No such household"))?;
        self.households.remove(i);
        for j in 0..self.people.len() {
            if self.people[j].household == id {
                self.people[j].household = 0;
            }
        }
        self.log(actor, "household.delete", format!("household {id}"), now);
        Ok(())
    }

    /// Puts `person` in household `id` (0 = none). A person is in at most one.
    pub fn set_household(
        &mut self,
        actor: u32,
        person: u32,
        id: u32,
        now: u64,
    ) -> Result<(), ApiError> {
        if id != 0 && self.households.get(id).is_none() {
            return Err(ApiError::not_found("No such household"));
        }
        let p = self.person_mut(person)?;
        if p.household != id {
            p.household = id;
            self.log(
                actor,
                "household.member",
                format!("person {person} -> {id}"),
                now,
            );
        }
        Ok(())
    }

    pub fn household_members(&self, id: u32) -> impl Iterator<Item = &StoredPerson> {
        self.people.iter().filter(move |p| p.household == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn limits() -> Limits {
        Limits {
            people: 3,
            sessions: 8,
            sessions_per_person: 2,
            pin_rounds: 2,
            audit: 64,
            ..Limits::board()
        }
    }

    #[test]
    fn setup_once_then_signup() {
        let mut s = State::new(limits());
        assert_eq!(
            s.signup("A", "", "1234", 1).unwrap_err().code,
            "setup_needed"
        );
        let admin = s.setup(" Dad ", "Dad@Home", "1234", 1).unwrap();
        assert!(s.person(admin).unwrap().admin);
        assert_eq!(s.person(admin).unwrap().email, "dad@home");
        assert_eq!(s.person(admin).unwrap().display_name, "Dad");
        assert_eq!(
            s.setup("X", "", "1234", 2).unwrap_err().code,
            "already_set_up"
        );
        let kid = s.signup("Sam", "", "0000", 3).unwrap();
        assert!(!s.person(kid).unwrap().admin);
        assert_eq!(
            s.signup("sam", "", "1111", 4).unwrap_err().code,
            "name_taken"
        );
        assert_eq!(
            s.signup("D2", "DAD@home", "1111", 4).unwrap_err().code,
            "email_taken"
        );
        s.signup("Grandma", "", "9999", 5).unwrap();
        assert_eq!(s.signup("G2", "", "9999", 5).unwrap_err().code, "full");
        s.signup_closed = true;
        s.people.pop();
        assert_eq!(
            s.signup("G3", "", "9999", 6).unwrap_err().code,
            "signup_closed"
        );
    }

    #[test]
    fn login_by_email_or_name() {
        let mut s = State::new(limits());
        let dad = s.setup("Dad", "dad@home", "1234", 0).unwrap();
        let kid = s.signup("Sam", "", "0000", 0).unwrap();
        assert_eq!(s.by_login(" DAD@home ").unwrap().id, dad);
        assert_eq!(s.by_login("dad").unwrap().id, dad);
        assert_eq!(s.by_login("SAM").unwrap().id, kid);
        assert!(s.by_login("").is_none());
        assert!(s.by_login("nobody@home").is_none());
    }

    #[test]
    fn validation() {
        let mut s = State::new(limits());
        assert_eq!(s.setup("A", "nope@", "1234", 0).unwrap_err().status, 400);
        assert_eq!(s.setup("", "", "1234", 0).unwrap_err().status, 400);
        assert_eq!(s.setup("A", "", "12", 0).unwrap_err().status, 400);
        assert_eq!(s.setup("a@b", "", "1234", 0).unwrap_err().status, 400);
    }

    #[test]
    fn last_admin_is_protected() {
        let mut s = State::new(limits());
        let dad = s.setup("Dad", "", "1234", 0).unwrap();
        let mom = s.signup("Mom", "", "1234", 0).unwrap();
        let demote = AdminPersonUpdate {
            role: Some("member".into()),
            ..Default::default()
        };
        assert_eq!(
            s.admin_update(dad, dad, &demote, 0).unwrap_err().code,
            "last_admin"
        );
        let promote = AdminPersonUpdate {
            role: Some("admin".into()),
            ..Default::default()
        };
        s.admin_update(dad, mom, &promote, 0).unwrap();
        s.admin_update(mom, dad, &demote, 0).unwrap();
        let disable = AdminPersonUpdate {
            disabled: Some(true),
            ..Default::default()
        };
        assert_eq!(
            s.admin_update(mom, mom, &disable, 0).unwrap_err().code,
            "last_admin"
        );
    }

    #[test]
    fn disabling_signs_out() {
        let mut s = State::new(limits());
        let dad = s.setup("Dad", "", "1234", 0).unwrap();
        let kid = s.signup("Kid", "", "1234", 0).unwrap();
        s.sessions.create(kid, Some(10)).unwrap();
        let disable = AdminPersonUpdate {
            disabled: Some(true),
            ..Default::default()
        };
        s.admin_update(dad, kid, &disable, 0).unwrap();
        assert_eq!(s.sessions.count(kid), 0);
        assert_eq!(s.person(kid).unwrap().status, DISABLED);
    }

    #[test]
    fn households() {
        let mut s = State::new(limits());
        let dad = s.setup("Dad", "", "1234", 0).unwrap();
        let kid = s.signup("Kid", "", "1234", 0).unwrap();
        let home = s.create_household(dad, "The Martins", 0).unwrap();
        assert_eq!(
            s.create_household(dad, "the martins", 0).unwrap_err().code,
            "name_taken"
        );
        s.set_household(dad, kid, home, 0).unwrap();
        s.set_household(dad, dad, home, 0).unwrap();
        assert_eq!(s.household_members(home).count(), 2);
        assert_eq!(s.set_household(dad, kid, 99, 0).unwrap_err().status, 404);
        s.delete_household(dad, home, 0).unwrap();
        assert_eq!(s.person(kid).unwrap().household, 0);
    }

    #[test]
    fn audit_ring_keeps_the_newest() {
        let mut s = State::new(limits());
        for i in 0..200u64 {
            s.log(1, "test", format!("{i}"), i);
        }
        assert_eq!(s.audit.len(), 2, "64 entries in blocks of 32");
        // A block is cleared when its turn comes round, so the ring holds
        // between one block less than its size and its size.
        let newest = s.audit_entries(100);
        assert_eq!(newest.len(), 40);
        assert_eq!(newest[0].detail, "199");
        assert_eq!(newest[39].detail, "160");
    }

    #[test]
    fn rollback_undoes_rows_counters_and_sessions() {
        let mut s = State::new(limits());
        let dad = s.setup("Dad", "", "1234", 5).unwrap();
        s.saved();
        let sp = s.savepoint();
        s.signup("Kid", "", "1234", 6).unwrap();
        s.person_mut(dad).unwrap().display_name = "Papa".into();
        s.sessions.create(dad, Some(6)).unwrap();
        s.signup_closed = true;
        s.rollback(sp);
        assert_eq!(s.people.len(), 1);
        assert_eq!(s.person(dad).unwrap().display_name, "Dad");
        assert_eq!(s.sessions.count(dad), 0);
        assert!(!s.signup_closed);
        assert_eq!(s.next_person, 2);
    }
}
