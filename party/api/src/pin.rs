//! PINs: PBKDF2-HMAC-SHA256 hashes, and the rate limits that make a
//! 4-digit PIN reasonable (`spec/02-accounts-auth.md#pins`).
//!
//! Lockout times use a monotonic millisecond clock supplied by the caller
//! (the board's uptime), so they work before SNTP and are testable. They
//! live in RAM: a reboot clears them, which is acceptable here.
use std::collections::HashMap;

pub const MIN_LEN: usize = 4;
pub const MAX_LEN: usize = 8;

/// Digits only, 4–8 long.
pub fn valid(pin: &str) -> bool {
    (MIN_LEN..=MAX_LEN).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn derive(pin: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pin.as_bytes(), salt, rounds, &mut out);
    out
}

/// `pbkdf2-sha256$<rounds>$<salt hex>$<hash hex>`.
pub fn hash(pin: &str, rounds: u32) -> std::io::Result<String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(format!(
        "pbkdf2-sha256${rounds}${}${}",
        hex(&salt),
        hex(&derive(pin, &salt, rounds))
    ))
}

/// Does `pin` match `stored`? Malformed or empty `stored` never matches.
pub fn verify(pin: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some("pbkdf2-sha256"), Some(rounds), Some(salt), Some(expected), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    let (Ok(rounds), Some(salt), Some(expected)) =
        (rounds.parse::<u32>(), unhex(salt), unhex(expected))
    else {
        return false;
    };
    if rounds == 0 || expected.len() != 32 {
        return false;
    }
    let got = derive(pin, &salt, rounds);
    got.iter()
        .zip(&expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// Rounds recorded in a stored hash (to rehash when the board's default rises).
pub fn rounds(stored: &str) -> Option<u32> {
    stored.split('$').nth(1)?.parse().ok()
}

/// Per-account lockout and a board-wide failure budget.
pub struct Guard {
    accounts: HashMap<u32, Failures>,
    /// Failure times (ms) within the board-wide window.
    recent: Vec<u64>,
    pub limits: GuardLimits,
}

#[derive(Clone, Copy, Debug)]
pub struct GuardLimits {
    /// Failures allowed before an account locks.
    pub free_failures: u32,
    pub first_lock_ms: u64,
    pub max_lock_ms: u64,
    /// Board-wide: this many failures in `window_ms` makes every PIN
    /// sign-in wait until the oldest leaves the window.
    pub board_failures: usize,
    pub window_ms: u64,
}

impl Default for GuardLimits {
    fn default() -> Self {
        Self {
            free_failures: 5,
            first_lock_ms: 60_000,
            max_lock_ms: 3_600_000,
            board_failures: 30,
            window_ms: 600_000,
        }
    }
}

#[derive(Default)]
struct Failures {
    count: u32,
    locked_until: u64,
}

impl Guard {
    pub fn new(limits: GuardLimits) -> Self {
        Self {
            accounts: HashMap::new(),
            recent: Vec::new(),
            limits,
        }
    }

    /// Milliseconds to wait before `account` (or anyone, for the board-wide
    /// budget) may try again; 0 = go ahead.
    pub fn wait(&mut self, account: Option<u32>, now: u64) -> u64 {
        let window = self.limits.window_ms;
        self.recent.retain(|&t| now.saturating_sub(t) < window);
        let board = if self.recent.len() >= self.limits.board_failures {
            (self.recent[0] + window).saturating_sub(now)
        } else {
            0
        };
        let own = account
            .and_then(|a| self.accounts.get(&a))
            .map_or(0, |f| f.locked_until.saturating_sub(now));
        board.max(own)
    }

    /// Records a failed attempt (an unknown email counts board-wide only).
    pub fn failed(&mut self, account: Option<u32>, now: u64) {
        self.recent.push(now);
        let Some(account) = account else { return };
        let limits = self.limits;
        let f = self.accounts.entry(account).or_default();
        f.count += 1;
        if f.count >= limits.free_failures {
            let doublings = (f.count - limits.free_failures).min(16);
            let lock = (limits.first_lock_ms << doublings).min(limits.max_lock_ms);
            f.locked_until = now + lock;
        }
    }

    pub fn succeeded(&mut self, account: u32) {
        self.accounts.remove(&account);
    }

    /// Clears an account's lockout (admin PIN reset).
    pub fn clear(&mut self, account: u32) {
        self.accounts.remove(&account);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_rules() {
        assert!(valid("0000") && valid("12345678"));
        assert!(!valid("123") && !valid("123456789") && !valid("12a4") && !valid(" 1234"));
    }

    #[test]
    fn hash_and_verify() {
        let stored = hash("4321", 10).unwrap();
        assert!(stored.starts_with("pbkdf2-sha256$10$"));
        assert!(verify("4321", &stored));
        assert!(!verify("4322", &stored));
        assert!(!verify("4321", ""));
        assert!(!verify("4321", "pbkdf2-sha256$0$00$00"));
        assert_eq!(rounds(&stored), Some(10));
        // Salted: the same PIN hashes differently.
        assert_ne!(stored, hash("4321", 10).unwrap());
    }

    #[test]
    fn rfc_vector() {
        // RFC 7914 §11, PBKDF2-HMAC-SHA256 ("passwd", "salt", c=1).
        let got = derive("passwd", b"salt", 1);
        assert_eq!(hex(&got[..16]), "55ac046e56e3089fec1691c22544b605");
    }

    #[test]
    fn lockout_doubles_and_caps() {
        let mut g = Guard::new(GuardLimits::default());
        for _ in 0..4 {
            g.failed(Some(1), 0);
        }
        assert_eq!(g.wait(Some(1), 0), 0, "four failures are free");
        g.failed(Some(1), 0);
        assert_eq!(g.wait(Some(1), 0), 60_000);
        g.failed(Some(1), 60_000);
        assert_eq!(g.wait(Some(1), 60_000), 120_000);
        for _ in 0..10 {
            g.failed(Some(1), 200_000);
        }
        assert_eq!(g.wait(Some(1), 200_000), 3_600_000);
        assert_eq!(g.wait(Some(2), 200_000), 0, "other accounts unaffected");
        g.succeeded(1);
        assert_eq!(g.wait(Some(1), 200_000), 0);
    }

    #[test]
    fn board_wide_budget() {
        let limits = GuardLimits {
            board_failures: 3,
            window_ms: 1_000,
            ..GuardLimits::default()
        };
        let mut g = Guard::new(limits);
        g.failed(None, 0);
        g.failed(Some(7), 100);
        assert_eq!(g.wait(Some(9), 200), 0);
        g.failed(None, 200);
        assert_eq!(
            g.wait(Some(9), 300),
            700,
            "until the first failure ages out"
        );
        assert_eq!(g.wait(Some(9), 1_000), 0);
    }
}
