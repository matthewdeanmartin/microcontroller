//! Browser sessions: a random cookie, stored only as its SHA-256.
//!
//! The table is plain data with no lock or persistence of its own: an app
//! keeps it inside its own state, persists [`Sessions::records`] alongside
//! everything else, and calls [`Sessions::restore`] at boot. Times are Unix
//! seconds supplied by the caller, `None` while the board's clock is unset
//! (nothing expires and nothing is touched until it is).
//!
//! Cookies are `HttpOnly; SameSite=Lax`, plus `Secure` when issued over
//! HTTPS. Plain HTTP is supported on purpose: household boards serve guests
//! who have not installed the household CA. See `spec/OAUTH.md`.
use crate::{ApiError, Request};
use sha2::{Digest, Sha256};
use std::io;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Cookie name.
    pub cookie: &'static str,
    /// A session unused this long ends.
    pub idle_secs: u64,
    /// A session ends this long after sign-in regardless of use.
    pub absolute_secs: u64,
    /// Most sessions per subject; the oldest is evicted.
    pub per_subject: usize,
    /// Most sessions in total; the least recently used is evicted.
    pub capacity: usize,
    /// `last_seen` is updated at most this often (flash wear).
    pub touch_secs: u64,
}

impl SessionConfig {
    pub fn new(cookie: &'static str) -> Self {
        Self {
            cookie,
            idle_secs: 30 * 86_400,
            absolute_secs: 180 * 86_400,
            per_subject: 4,
            capacity: 64,
            touch_secs: 3_600,
        }
    }
}

/// One stored session. `created`/`last_seen` are 0 when the clock was
/// unset; such sessions are dated on their first use with a set clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub hash: [u8; 32],
    pub subject: u32,
    pub created: u64,
    pub last_seen: u64,
}

pub struct Sessions {
    config: SessionConfig,
    records: Vec<Record>,
    dirty: bool,
}

fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Constant-time comparison.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits as 32 hex characters.
pub fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The value of cookie `name` in the request, if any.
pub fn cookie<'a>(req: &Request<'a>, name: &str) -> Option<&'a str> {
    req.headers()
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .flat_map(|(_, v)| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim())
}

impl Sessions {
    pub fn new(config: SessionConfig) -> Self {
        Self::restore(config, Vec::new())
    }

    /// Rebuilds the table from persisted records (excess ones dropped).
    pub fn restore(config: SessionConfig, mut records: Vec<Record>) -> Self {
        records.truncate(config.capacity);
        Self {
            config,
            records,
            dirty: false,
        }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// True after any change worth persisting; cleared by [`Self::saved`].
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn saved(&mut self) {
        self.dirty = false;
    }

    fn expired(&self, r: &Record, now: Option<u64>) -> bool {
        let Some(now) = now else { return false };
        (r.last_seen != 0 && now.saturating_sub(r.last_seen) > self.config.idle_secs)
            || (r.created != 0 && now.saturating_sub(r.created) > self.config.absolute_secs)
    }

    /// Drops expired sessions.
    pub fn prune(&mut self, now: Option<u64>) {
        let before = self.records.len();
        let records = std::mem::take(&mut self.records);
        self.records = records
            .into_iter()
            .filter(|r| !self.expired(r, now))
            .collect();
        self.dirty |= self.records.len() != before;
    }

    /// Starts a session and returns its token (put it in [`Self::set_cookie`]).
    pub fn create(&mut self, subject: u32, now: Option<u64>) -> io::Result<String> {
        self.prune(now);
        let mine: Vec<usize> = (0..self.records.len())
            .filter(|&i| self.records[i].subject == subject)
            .collect();
        if mine.len() >= self.config.per_subject.max(1) {
            let oldest = *mine
                .iter()
                .min_by_key(|&&i| self.records[i].created)
                .unwrap();
            self.records.remove(oldest);
        }
        if self.records.len() >= self.config.capacity.max(1) {
            let idle = (0..self.records.len())
                .min_by_key(|&i| self.records[i].last_seen)
                .unwrap();
            self.records.remove(idle);
        }
        let token = random_token()?;
        let at = now.unwrap_or(0);
        self.records.push(Record {
            hash: hash(&token),
            subject,
            created: at,
            last_seen: at,
        });
        self.dirty = true;
        Ok(token)
    }

    /// The signed-in subject, or `None`. Touches `last_seen` (at most once
    /// per `touch_secs`) and drops the session if it has expired.
    pub fn check(&mut self, req: &Request<'_>, now: Option<u64>) -> Option<u32> {
        let token = cookie(req, self.config.cookie)?;
        let wanted = hash(token);
        let mut found = None;
        for (i, r) in self.records.iter().enumerate() {
            if same(&r.hash, &wanted) {
                found = Some(i);
            }
        }
        let i = found?;
        if self.expired(&self.records[i], now) {
            self.records.remove(i);
            self.dirty = true;
            return None;
        }
        if let Some(now) = now {
            let r = &mut self.records[i];
            if r.created == 0 {
                r.created = now;
            }
            if now.saturating_sub(r.last_seen) >= self.config.touch_secs {
                r.last_seen = now;
                self.dirty = true;
            }
        }
        Some(self.records[i].subject)
    }

    /// Ends the request's own session. Returns whether one existed.
    pub fn revoke(&mut self, req: &Request<'_>) -> bool {
        let Some(token) = cookie(req, self.config.cookie) else {
            return false;
        };
        let wanted = hash(token);
        let before = self.records.len();
        self.records.retain(|r| !same(&r.hash, &wanted));
        let removed = self.records.len() != before;
        self.dirty |= removed;
        removed
    }

    /// Ends every session of `subject` ("sign out everywhere").
    pub fn revoke_subject(&mut self, subject: u32) -> usize {
        let before = self.records.len();
        self.records.retain(|r| r.subject != subject);
        let removed = before - self.records.len();
        self.dirty |= removed > 0;
        removed
    }

    pub fn count(&self, subject: u32) -> usize {
        self.records.iter().filter(|r| r.subject == subject).count()
    }

    /// `Set-Cookie` value for a new session.
    pub fn set_cookie(&self, token: &str, secure: bool) -> String {
        format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
            self.config.cookie,
            self.config.absolute_secs,
            if secure { "; Secure" } else { "" }
        )
    }

    /// `Set-Cookie` value that removes the cookie.
    pub fn clear_cookie(&self, secure: bool) -> String {
        format!(
            "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
            self.config.cookie,
            if secure { "; Secure" } else { "" }
        )
    }
}

/// CSRF guard for cookie-authenticated APIs. Safe methods pass. Unsafe ones
/// need an `Origin` (or, failing that, `Referer`) whose host is the
/// request's own `Host`, or one of `extra` (exact origins such as the
/// Angular dev server). Browsers always send `Origin` on cross-origin and
/// on non-GET fetches, so a missing header means a non-browser client that
/// must say where it comes from.
pub fn require_same_origin(req: &Request<'_>, extra: &[String]) -> Result<(), ApiError> {
    if matches!(req.method, "GET" | "HEAD" | "OPTIONS") {
        return Ok(());
    }
    let origin = match req.header("Origin").trim() {
        "" | "null" => {
            let referer = req.header("Referer").trim();
            let scheme_end = referer.find("://").map_or(0, |i| i + 3);
            let end = referer[scheme_end..]
                .find('/')
                .map_or(referer.len(), |i| scheme_end + i);
            &referer[..end]
        }
        o => o,
    };
    let refuse = || {
        ApiError::new(
            403,
            "cross_origin",
            "This request must come from the board's own pages",
        )
    };
    if origin.is_empty() {
        return Err(refuse());
    }
    let authority = origin.split_once("://").map_or("", |(_, a)| a);
    let host = req.header("Host").trim();
    if (!host.is_empty() && authority.eq_ignore_ascii_case(host))
        || extra.iter().any(|o| o.eq_ignore_ascii_case(origin))
    {
        Ok(())
    } else {
        Err(refuse())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn config() -> SessionConfig {
        SessionConfig {
            per_subject: 2,
            capacity: 3,
            ..SessionConfig::new("s")
        }
    }

    #[test]
    fn create_check_revoke() {
        let mut s = Sessions::new(config());
        let token = s.create(7, Some(1_000)).unwrap();
        assert_eq!(token.len(), 32);
        assert!(s.dirty());
        let h = headers(&[("Cookie", &format!("a=b; s={token}"))]);
        let req = Request::new("GET", "/", &h, b"", false);
        assert_eq!(s.check(&req, Some(1_001)), Some(7));
        // Only the hash is stored.
        assert!(s.records()[0].hash != [0; 32]);
        assert!(s.revoke(&req));
        assert_eq!(s.check(&req, Some(1_002)), None);
    }

    #[test]
    fn idle_and_absolute_expiry() {
        let mut s = Sessions::new(config());
        let token = s.create(1, Some(10)).unwrap();
        let h = headers(&[("Cookie", &format!("s={token}"))]);
        let req = Request::new("GET", "/", &h, b"", false);
        let day = 86_400;
        // Touches move last_seen forward, so steady use survives idle...
        for d in 1..=170 {
            assert_eq!(s.check(&req, Some(10 + d * day)), Some(1), "day {d}");
        }
        // ...but not the absolute limit.
        assert_eq!(s.check(&req, Some(10 + 181 * day)), None);

        let token = s.create(1, Some(10)).unwrap();
        let h = headers(&[("Cookie", &format!("s={token}"))]);
        let req = Request::new("GET", "/", &h, b"", false);
        assert_eq!(s.check(&req, Some(10 + 31 * day)), None);
    }

    #[test]
    fn unset_clock_never_expires_and_dates_later() {
        let mut s = Sessions::new(config());
        let token = s.create(1, None).unwrap();
        let h = headers(&[("Cookie", &format!("s={token}"))]);
        let req = Request::new("GET", "/", &h, b"", false);
        assert_eq!(s.check(&req, None), Some(1));
        assert_eq!(s.check(&req, Some(5_000_000)), Some(1));
        assert_eq!(s.records()[0].created, 5_000_000);
    }

    #[test]
    fn eviction_per_subject_and_total() {
        let mut s = Sessions::new(config());
        let first = s.create(1, Some(1)).unwrap();
        s.create(1, Some(2)).unwrap();
        s.create(1, Some(3)).unwrap();
        assert_eq!(s.count(1), 2);
        let h = headers(&[("Cookie", &format!("s={first}"))]);
        let req = Request::new("GET", "/", &h, b"", false);
        assert_eq!(s.check(&req, Some(4)), None, "oldest of subject 1 evicted");
        s.create(2, Some(5)).unwrap();
        s.create(3, Some(6)).unwrap();
        assert_eq!(s.records().len(), 3);
        assert_eq!(s.revoke_subject(1), 1);
    }

    #[test]
    fn restore_round_trip() {
        let mut s = Sessions::new(config());
        let token = s.create(9, Some(100)).unwrap();
        let mut again = Sessions::restore(config(), s.records().to_vec());
        let h = headers(&[("Cookie", &format!("s={token}"))]);
        let req = Request::new("GET", "/", &h, b"", false);
        assert_eq!(again.check(&req, Some(101)), Some(9));
        assert!(!again.dirty());
    }

    #[test]
    fn cookie_attributes() {
        let s = Sessions::new(config());
        assert_eq!(
            s.set_cookie("t", false),
            "s=t; Path=/; HttpOnly; SameSite=Lax; Max-Age=15552000"
        );
        assert!(s.set_cookie("t", true).ends_with("; Secure"));
        assert!(s.clear_cookie(false).contains("Max-Age=0"));
    }

    #[test]
    fn same_origin_guard() {
        let ok = |pairs: &[(&str, &str)], method: &str| {
            let h = headers(pairs);
            let req = Request::new(method, "/api/v1/x", &h, b"", false);
            require_same_origin(&req, &["http://localhost:4200".into()]).is_ok()
        };
        assert!(ok(&[], "GET"));
        assert!(!ok(&[], "POST"));
        assert!(ok(
            &[
                ("Host", "gamematcher.local"),
                ("Origin", "http://gamematcher.local")
            ],
            "POST"
        ));
        assert!(ok(
            &[("Host", "192.168.1.9"), ("Origin", "https://192.168.1.9")],
            "PUT"
        ));
        assert!(!ok(
            &[
                ("Host", "gamematcher.local"),
                ("Origin", "http://evil.example")
            ],
            "POST"
        ));
        assert!(ok(
            &[
                ("Host", "127.0.0.1:8080"),
                ("Origin", "http://localhost:4200")
            ],
            "DELETE"
        ));
        assert!(ok(
            &[
                ("Host", "gamematcher.local"),
                ("Referer", "http://gamematcher.local/me")
            ],
            "POST"
        ));
    }
}
