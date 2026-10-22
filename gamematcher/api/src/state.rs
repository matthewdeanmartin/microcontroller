//! The domain state and its rules, free of HTTP and storage so it can be
//! tested directly. `api.rs` locks it, calls these, and commits.
use crate::messages::{
    AdminUserUpdate, StoredAccount, StoredGame, StoredMeta, StoredPlan, StoredProvider,
    StoredSession, StoredState,
};
use crate::pin;
use crate::table::Table;
use miniframework::session::{Record, SessionConfig, Sessions};
use miniframework::ApiError;

/// Record schema this firmware writes (and the newest it can read).
/// Schema 1 was the two-slot snapshot, read only to migrate.
pub const SCHEMA: u16 = 2;

/// Bounds that differ between a board and a desktop
/// (`spec/05-storage-limits.md`).
#[derive(Clone, Debug)]
pub struct Limits {
    pub accounts: usize,
    pub sessions: usize,
    pub sessions_per_account: usize,
    /// PBKDF2 rounds for new PIN hashes.
    pub pin_rounds: u32,
    /// Catalog size, archived games included.
    pub games: usize,
    /// Plans that are open or locked in.
    pub active_plans: usize,
    /// Done and cancelled plans kept (oldest dropped first).
    pub past_plans: usize,
    /// Notifications kept per person (oldest dropped first).
    pub notifications: usize,
    pub comments_per_plan: usize,
    /// Comments on the whole board (flash: each thread is its own blob).
    pub comments_total: usize,
}

impl Limits {
    pub fn board() -> Self {
        Self {
            accounts: 32,
            sessions: 64,
            sessions_per_account: 4,
            // ~100 ms per check on the C6 (2,000 rounds measured 197 ms,
            // October 17, 2026).
            pin_rounds: 1_000,
            games: 256,
            active_plans: 16,
            past_plans: 128,
            notifications: 32,
            comments_per_plan: 64,
            comments_total: 1_024,
        }
    }
}

/// Rows and sessions as loaded from flash.
#[derive(Default)]
pub struct Parts {
    pub accounts: Vec<StoredAccount>,
    pub games: Vec<StoredGame>,
    pub plans: Vec<StoredPlan>,
    pub providers: Vec<StoredProvider>,
    pub sessions: Vec<StoredSession>,
    pub commits: u64,
}

pub struct State {
    pub accounts: Table<StoredAccount>,
    pub sessions: Sessions,
    /// Sessions may differ from flash although `sessions.dirty()` says
    /// otherwise (a rollback rebuilt them).
    pub sessions_unsure: bool,
    pub next_account: u32,
    pub signup_closed: bool,
    pub commits: u64,
    pub games: Table<StoredGame>,
    pub next_game: u32,
    pub plans: Table<StoredPlan>,
    pub next_plan: u32,
    pub next_notification: u32,
    /// SHA-256 (hex) of the household calendar token; empty = off.
    pub house_feed_hash: String,
    /// Mastodon instances people may sign in with.
    pub providers: Table<StoredProvider>,
    pub next_provider: u32,
    /// Comment threads to delete from the Kv (their plans left the ring);
    /// `api.rs` drains this after each commit.
    pub dropped_threads: Vec<u32>,
    /// Recent answers and lock-ins for the screen (RAM only).
    pub feed: std::collections::VecDeque<crate::screen::deck::FeedEvent>,
    /// Notifications for outbound email / direct messages (RAM only); `api.rs` drains this after
    /// each commit, so a rolled-back change mails nothing.
    pub mail: Vec<crate::mail::Pending>,
    pub limits: Limits,
}

/// Where [`State::rollback`] returns to. Rows cost only what changed since;
/// sessions (≤ 64 small records) and the screen feed are copied whole.
pub struct Savepoint {
    tables: [usize; 4],
    meta: StoredMeta,
    commits: u64,
    sessions: Vec<Record>,
    sessions_dirty: bool,
    feed: std::collections::VecDeque<crate::screen::deck::FeedEvent>,
    mail: usize,
    dropped_threads: usize,
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
        per_subject: limits.sessions_per_account,
        capacity: limits.sessions,
        ..SessionConfig::new("gm_session")
    }
}

pub fn role(a: &StoredAccount) -> &'static str {
    if a.admin {
        "admin"
    } else {
        "member"
    }
}

/// Lower-cased, trimmed login name that looks like `x@y`.
pub fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_lowercase();
    let ok = email.len() <= 64
        && !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(user, host)| {
            !user.is_empty() && !host.is_empty() && !host.contains('@')
        });
    if ok {
        Ok(email)
    } else {
        Err(ApiError::bad_request(
            "Enter an email address (it's only your sign-in name; nothing is sent to it)",
        ))
    }
}

pub fn normalize_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    let len = name.chars().count();
    if (1..=32).contains(&len) && !name.chars().any(char::is_control) {
        Ok(name.to_string())
    } else {
        Err(ApiError::bad_request("Display name: 1 to 32 characters"))
    }
}

pub fn check_pin(pin: &str) -> Result<(), ApiError> {
    if pin::valid(pin) {
        Ok(())
    } else {
        Err(ApiError::bad_request("PIN: 4 to 8 digits"))
    }
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
        Self {
            next_account: next(meta.next_account, &mut parts.accounts.iter().map(|a| a.id)),
            next_game: next(meta.next_game, &mut parts.games.iter().map(|g| g.id)),
            next_plan: next(meta.next_plan, &mut parts.plans.iter().map(|p| p.id)),
            next_provider: next(
                meta.next_provider,
                &mut parts.providers.iter().map(|p| p.id),
            ),
            accounts: Table::from_rows(parts.accounts),
            sessions: Sessions::restore(session_config(&limits), records),
            sessions_unsure: false,
            signup_closed: meta.signup_closed,
            commits: parts.commits,
            games: Table::from_rows(parts.games),
            plans: Table::from_rows(parts.plans),
            next_notification: meta.next_notification.max(1),
            house_feed_hash: meta.house_feed_hash,
            providers: Table::from_rows(parts.providers),
            dropped_threads: Vec::new(),
            feed: Default::default(),
            mail: Vec::new(),
            limits,
        }
    }

    /// Schema 1's snapshot (migration only).
    pub fn from_stored(stored: StoredState, limits: Limits) -> Self {
        let meta = StoredMeta {
            schema: SCHEMA as u32,
            next_account: stored.next_account,
            signup_closed: stored.signup_closed,
            next_game: stored.next_game,
            next_plan: stored.next_plan,
            next_notification: stored.next_notification,
            house_feed_hash: stored.house_feed_hash,
            next_provider: stored.next_provider,
        };
        let parts = Parts {
            accounts: stored.accounts,
            games: stored.games,
            plans: stored.plans,
            providers: stored.providers,
            sessions: stored.sessions,
            commits: stored.commits,
        };
        Self::from_parts(meta, parts, limits)
    }

    /// Everything that isn't a row.
    pub fn meta(&self) -> StoredMeta {
        StoredMeta {
            schema: SCHEMA as u32,
            next_account: self.next_account,
            signup_closed: self.signup_closed,
            next_game: self.next_game,
            next_plan: self.next_plan,
            next_notification: self.next_notification,
            house_feed_hash: self.house_feed_hash.clone(),
            next_provider: self.next_provider,
        }
    }

    fn set_meta(&mut self, m: StoredMeta) {
        self.next_account = m.next_account;
        self.signup_closed = m.signup_closed;
        self.next_game = m.next_game;
        self.next_plan = m.next_plan;
        self.next_notification = m.next_notification;
        self.house_feed_hash = m.house_feed_hash;
        self.next_provider = m.next_provider;
    }

    /// Sessions in their stored form, grouped by account (ascending).
    pub fn stored_sessions(&self) -> std::collections::BTreeMap<u32, Vec<StoredSession>> {
        let mut by: std::collections::BTreeMap<u32, Vec<StoredSession>> = Default::default();
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
                self.accounts.savepoint(),
                self.games.savepoint(),
                self.plans.savepoint(),
                self.providers.savepoint(),
            ],
            meta: self.meta(),
            commits: self.commits,
            sessions: self.sessions.records().to_vec(),
            sessions_dirty: self.sessions.dirty() || self.sessions_unsure,
            feed: self.feed.clone(),
            mail: self.mail.len(),
            dropped_threads: self.dropped_threads.len(),
        }
    }

    /// Undoes everything since `sp`, in RAM. Flash is untouched.
    pub fn rollback(&mut self, sp: Savepoint) {
        let [a, g, p, v] = sp.tables;
        self.accounts.rollback(a);
        self.games.rollback(g);
        self.plans.rollback(p);
        self.providers.rollback(v);
        self.set_meta(sp.meta);
        self.commits = sp.commits;
        if self.sessions.records() != sp.sessions.as_slice() {
            self.sessions = Sessions::restore(session_config(&self.limits), sp.sessions);
            self.sessions_unsure |= sp.sessions_dirty;
        }
        self.feed = sp.feed;
        self.mail.truncate(sp.mail);
        self.dropped_threads.truncate(sp.dropped_threads);
    }

    /// The records just written match RAM.
    pub fn saved(&mut self) {
        self.accounts.saved();
        self.games.saved();
        self.plans.saved();
        self.providers.saved();
        self.sessions.saved();
        self.sessions_unsure = false;
    }

    /// Marks every row dirty (migration writes them all).
    pub fn mark_all(&mut self) {
        self.accounts.mark_all();
        self.games.mark_all();
        self.plans.mark_all();
        self.providers.mark_all();
        self.sessions_unsure = true;
    }

    /// The name to show for an account (or a placeholder for a gone one).
    pub fn name(&self, id: u32) -> String {
        self.account(id)
            .map_or_else(|| "someone".to_string(), |a| a.display_name.clone())
    }

    pub fn account(&self, id: u32) -> Option<&StoredAccount> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn account_mut(&mut self, id: u32) -> Result<&mut StoredAccount, ApiError> {
        self.accounts
            .get_mut(id)
            .ok_or_else(|| ApiError::not_found("No such account"))
    }

    pub fn by_email(&self, email: &str) -> Option<&StoredAccount> {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            return None;
        }
        self.accounts.iter().find(|a| a.email == email)
    }

    fn active_admins(&self) -> usize {
        self.accounts
            .iter()
            .filter(|a| a.admin && !a.disabled)
            .count()
    }

    /// Creates an account. `admin` only for first-run setup.
    pub fn create_account(
        &mut self,
        email: &str,
        display_name: &str,
        pin: &str,
        admin: bool,
        now: u64,
    ) -> Result<u32, ApiError> {
        let email = normalize_email(email)?;
        let display_name = normalize_name(display_name)?;
        check_pin(pin)?;
        if self.accounts.iter().any(|a| a.email == email) {
            return Err(ApiError::new(
                409,
                "email_taken",
                "There is already an account with that email",
            ));
        }
        if self.accounts.len() >= self.limits.accounts {
            return Err(ApiError::new(
                409,
                "full",
                format!("This board holds at most {} accounts", self.limits.accounts),
            ));
        }
        let id = self.next_account;
        self.accounts.push(StoredAccount {
            id,
            email,
            display_name,
            pin: pin_hash(pin, self.limits.pin_rounds)?,
            admin,
            disabled: false,
            created: now,
            last_seen: now,
            interests: Vec::new(),
            notifications: Vec::new(),
            feed_hash: String::new(),
            mastodon: None,
            email_off: false,
            last_house_reply: String::new(),
            last_public_reply: String::new(),
        });
        self.next_account += 1;
        Ok(id)
    }

    /// First-run setup: the first account, an admin. Atomic under the
    /// caller's lock, so a second concurrent claim gets 409.
    pub fn setup(&mut self, email: &str, name: &str, pin: &str, now: u64) -> Result<u32, ApiError> {
        if !self.accounts.is_empty() {
            return Err(ApiError::new(
                409,
                "already_set_up",
                "Game Matcher already has an admin. Sign in or sign up instead",
            ));
        }
        self.create_account(email, name, pin, true, now)
    }

    pub fn signup(
        &mut self,
        email: &str,
        name: &str,
        pin: &str,
        now: u64,
    ) -> Result<u32, ApiError> {
        if self.accounts.is_empty() {
            return Err(ApiError::new(
                409,
                "setup_needed",
                "Set up Game Matcher first: the first account becomes admin",
            ));
        }
        if self.signup_closed {
            return Err(ApiError::new(
                403,
                "signup_closed",
                "Sign-up is closed. Ask the admin to make you an account",
            ));
        }
        self.create_account(email, name, pin, false, now)
    }

    /// Sets a new PIN after an emailed code was checked (`api.rs`).
    pub fn reset_pin(&mut self, id: u32, new_pin: &str) -> Result<(), ApiError> {
        check_pin(new_pin)?;
        let rounds = self.limits.pin_rounds;
        self.account_mut(id)?.pin = pin_hash(new_pin, rounds)?;
        Ok(())
    }

    pub fn change_pin(&mut self, id: u32, current: &str, new_pin: &str) -> Result<(), ApiError> {
        check_pin(new_pin)?;
        let rounds = self.limits.pin_rounds;
        let account = self.account_mut(id)?;
        if !account.pin.is_empty() && !pin::verify(current, &account.pin) {
            return Err(ApiError::new(403, "wrong_pin", "Current PIN is wrong"));
        }
        account.pin = pin_hash(new_pin, rounds)?;
        Ok(())
    }

    /// Applies an admin's change to account `id`, acting as `actor`.
    pub fn admin_update(
        &mut self,
        actor: u32,
        id: u32,
        change: &AdminUserUpdate,
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
        // Change a copy, check the invariants, then store it.
        let mut account = self.account_mut(id)?.clone();
        let was_active_admin = account.admin && !account.disabled;
        if let Some(admin) = make_admin {
            account.admin = admin;
        }
        if let Some(disabled) = change.disabled {
            account.disabled = disabled;
        }
        if let Some(hash) = pin_hash {
            account.pin = hash;
        }
        if let Some(name) = display_name {
            account.display_name = name;
        }
        let still_active_admin = account.admin && !account.disabled;
        if was_active_admin && !still_active_admin && self.active_admins() <= 1 {
            return Err(ApiError::new(
                409,
                "last_admin",
                "Game Matcher needs at least one active admin",
            ));
        }
        if id == actor && account.disabled {
            return Err(ApiError::new(
                409,
                "self_disable",
                "You can't disable yourself",
            ));
        }
        let disabled = account.disabled;
        *self.account_mut(id)? = account;
        if change.sign_out.unwrap_or(false) || disabled {
            self.sessions.revoke_subject(id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            accounts: 3,
            sessions: 8,
            sessions_per_account: 2,
            pin_rounds: 2,
            ..Limits::board()
        }
    }

    #[test]
    fn setup_once_then_signup() {
        let mut s = State::new(limits());
        assert_eq!(
            s.signup("a@b", "A", "1234", 1).unwrap_err().code,
            "setup_needed"
        );
        let admin = s.setup(" Dad@Home ", "Dad", "1234", 1).unwrap();
        assert!(s.account(admin).unwrap().admin);
        assert_eq!(s.account(admin).unwrap().email, "dad@home");
        assert_eq!(
            s.setup("x@y", "X", "1234", 2).unwrap_err().code,
            "already_set_up"
        );
        let kid = s.signup("kid@home", "Sam", "0000", 3).unwrap();
        assert!(!s.account(kid).unwrap().admin);
        assert_eq!(
            s.signup("DAD@home", "D2", "1111", 4).unwrap_err().code,
            "email_taken"
        );
        s.signup("guest@x", "Guest", "9999", 5).unwrap();
        assert_eq!(s.signup("g2@x", "G2", "9999", 5).unwrap_err().code, "full");
        s.signup_closed = true;
        s.accounts.pop();
        assert_eq!(
            s.signup("g3@x", "G3", "9999", 6).unwrap_err().code,
            "signup_closed"
        );
    }

    #[test]
    fn validation() {
        let mut s = State::new(limits());
        assert_eq!(s.setup("nope", "A", "1234", 0).unwrap_err().status, 400);
        assert_eq!(s.setup("a@b", "", "1234", 0).unwrap_err().status, 400);
        assert_eq!(s.setup("a@b", "A", "12", 0).unwrap_err().status, 400);
        assert_eq!(s.setup("a b@c", "A", "1234", 0).unwrap_err().status, 400);
    }

    #[test]
    fn last_admin_is_protected() {
        let mut s = State::new(limits());
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let mom = s.signup("mom@h", "Mom", "1234", 0).unwrap();
        assert_eq!(
            s.admin_update(
                dad,
                dad,
                &AdminUserUpdate {
                    role: Some("member".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            "last_admin"
        );
        s.admin_update(
            dad,
            mom,
            &AdminUserUpdate {
                role: Some("admin".into()),
                ..Default::default()
            },
        )
        .unwrap();
        s.admin_update(
            mom,
            dad,
            &AdminUserUpdate {
                role: Some("member".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            s.admin_update(
                mom,
                mom,
                &AdminUserUpdate {
                    disabled: Some(true),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            "last_admin"
        );
    }

    #[test]
    fn disabling_signs_out() {
        let mut s = State::new(limits());
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let kid = s.signup("kid@h", "Kid", "1234", 0).unwrap();
        s.sessions.create(kid, Some(10)).unwrap();
        assert_eq!(s.sessions.count(kid), 1);
        s.admin_update(
            dad,
            kid,
            &AdminUserUpdate {
                disabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.sessions.count(kid), 0);
        assert_eq!(
            s.admin_update(
                dad,
                dad,
                &AdminUserUpdate {
                    disabled: Some(true),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            "last_admin"
        );
    }

    #[test]
    fn pin_change_and_reset() {
        let mut s = State::new(limits());
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        assert_eq!(
            s.change_pin(dad, "0000", "5555").unwrap_err().code,
            "wrong_pin"
        );
        s.change_pin(dad, "1234", "5555").unwrap();
        assert!(pin::verify("5555", &s.account(dad).unwrap().pin));
        let kid = s.signup("kid@h", "Kid", "1234", 0).unwrap();
        s.admin_update(
            dad,
            kid,
            &AdminUserUpdate {
                pin: Some("2468".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(pin::verify("2468", &s.account(kid).unwrap().pin));
    }

    #[test]
    fn rollback_undoes_rows_counters_and_sessions() {
        let mut s = State::new(limits());
        let dad = s.setup("dad@h", "Dad", "1234", 5).unwrap();
        s.saved();
        let sp = s.savepoint();
        s.signup("kid@h", "Kid", "1234", 6).unwrap();
        s.account_mut(dad).unwrap().display_name = "Papa".into();
        s.sessions.create(dad, Some(6)).unwrap();
        s.signup_closed = true;
        s.rollback(sp);
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.account(dad).unwrap().display_name, "Dad");
        assert_eq!(s.sessions.count(dad), 0);
        assert!(!s.signup_closed);
        assert_eq!(s.next_account, 2);
    }

    #[test]
    fn schema_one_snapshot_converts() {
        let mut s = State::new(limits());
        let dad = s.setup("dad@h", "Dad", "1234", 5).unwrap();
        s.sessions.create(dad, Some(5)).unwrap();
        let old = StoredState {
            schema: 1,
            next_account: s.next_account,
            accounts: s.accounts.to_vec(),
            sessions: s.stored_sessions().into_values().flatten().collect(),
            signup_closed: true,
            commits: 7,
            ..Default::default()
        };
        let again = State::from_stored(old, limits());
        assert_eq!(again.accounts, s.accounts);
        assert_eq!(again.sessions.records(), s.sessions.records());
        assert!(again.signup_closed);
        assert_eq!(again.next_account, 2);
        assert_eq!(again.commits, 7);
    }
}
