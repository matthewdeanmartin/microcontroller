//! Optional, bounded action gate. Apps must explicitly install callbacks and
//! provision two distinct tokens. The ordinary bot token cannot grant approval.
//! No worker, queue or buffers are allocated by merely enabling `ops`.
use crate::{message, ApiError, Reply, Request};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Checkpoint,
    RestartPlanned,
}
impl Action {
    fn id(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::RestartPlanned => "restart-planned",
        }
    }
    fn disruptive(self) -> bool {
        self == Self::RestartPlanned
    }
}

pub struct Installed {
    pub action: Action,
    /// Must return quickly. Long work belongs to an app's bounded job queue.
    /// Restart callbacks must first quiesce their own background writes.
    pub run: fn() -> Result<(), &'static str>,
    pub cooldown: Duration,
}

message! { pub struct Description {
    1 id: String,
    2 summary: String,
    3 risk: String,
    4 params: String,
    5 cooldown_s: u64,
    6 needs_approval: bool,
} }
message! { pub struct Catalog { 1 schema: String, 2 actions: Vec<Description>, } }
message! { pub struct Outcome { 1 action: String, 2 outcome: String, } }
message! { pub struct Approval { 1 token: String, 2 expires_in_s: u64, } }
message! { pub struct AuditEntry { 1 action: String, 2 outcome: String, 3 actor: String, 4 age_s: u64, } }
message! { pub struct Audit { 1 entries: Vec<AuditEntry>, 2 volatile: bool, } }

pub fn empty_catalog() -> Catalog {
    Catalog {
        schema: "miniframework.actions/1".into(),
        actions: Vec::new(),
    }
}
#[derive(Clone, Copy)]
struct Entry {
    digest: [u8; 32],
    action: Action,
    at: Instant,
    ok: bool,
}
struct Grant {
    digest: [u8; 32],
    token: [u8; 32],
    until: Instant,
}
#[derive(Default)]
struct State {
    entries: [Option<Entry>; 4],
    grant: Option<Grant>,
}

/// Four audit/idempotency slots and one five-minute, boot-local approval.
/// Refuses further execution when full: never evicts replay protection.
/// A reboot drops history and grants; clients must never retry an uncertain
/// action across a reboot without a new human decision.
pub struct Gate {
    safe: [u8; 32],
    admin: [u8; 32],
    installed: &'static [Installed],
    state: Mutex<State>,
}
fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}
fn digest(action: Action, key: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(action.id());
    h.update([0]);
    h.update(key);
    h.finalize().into()
}

impl Gate {
    pub fn new(
        safe_token: &str,
        approval_token: &str,
        installed: &'static [Installed],
    ) -> Result<Self, &'static str> {
        if safe_token.len() < 32
            || approval_token.len() < 32
            || safe_token == approval_token
            || installed.len() > 2
        {
            return Err("Use distinct tokens of at least 32 bytes and at most two actions");
        }
        if installed.len() == 2 && installed[0].action == installed[1].action {
            return Err("Duplicate action");
        }
        Ok(Self {
            safe: hash(safe_token.as_bytes()),
            admin: hash(approval_token.as_bytes()),
            installed,
            state: Mutex::new(State::default()),
        })
    }
    pub fn catalog(&self) -> Catalog {
        Catalog {
            schema: "miniframework.actions/1".into(),
            actions: self
                .installed
                .iter()
                .map(|i| Description {
                    id: i.action.id().into(),
                    summary: match i.action {
                        Action::Checkpoint => "Rotate the journal",
                        Action::RestartPlanned => {
                            "Drain connections and restart; volatile metrics are lost"
                        }
                    }
                    .into(),
                    risk: if i.action.disruptive() {
                        "disruptive"
                    } else {
                        "safe"
                    }
                    .into(),
                    params: "EmptyParams".into(),
                    cooldown_s: i.cooldown.as_secs(),
                    needs_approval: i.action.disruptive(),
                })
                .collect(),
        }
    }
    fn authorized(&self, req: &Request<'_>, admin: bool) -> bool {
        req.secure
            && req
                .header("Authorization")
                .strip_prefix("Bearer ")
                .is_some_and(|t| {
                    same(
                        &hash(t.as_bytes()),
                        if admin { &self.admin } else { &self.safe },
                    )
                })
    }
    /// Returns false for unrelated paths. Every answer on these paths is private.
    pub fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> bool {
        if !req.path.starts_with("/.well-known/ops/") || req.path.ends_with("/health") {
            return false;
        }
        reply.header("Cache-Control", "no-store");
        if let Err(e) = self.route(req, reply) {
            reply.fail(req, &e);
        }
        true
    }
    fn route(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        if req.method == "GET" && req.path == "/.well-known/ops/actions" {
            reply.wire(req, &self.catalog());
            return Ok(());
        }
        let granting = req.path.starts_with("/.well-known/ops/approvals/");
        if !self.authorized(req, granting) {
            return Err(ApiError::unauthorized(
                "An ops token over HTTPS is required",
            ));
        }
        let mut state = self.state.lock().unwrap();
        if req.method == "GET" && req.path == "/.well-known/ops/audit" {
            let entries = state
                .entries
                .iter()
                .flatten()
                .map(|e| AuditEntry {
                    action: e.action.id().into(),
                    outcome: if e.ok {
                        "accepted"
                    } else {
                        "failed-or-uncertain"
                    }
                    .into(),
                    actor: if e.action.disruptive() {
                        "approved-bot"
                    } else {
                        "safe-bot"
                    }
                    .into(),
                    age_s: e.at.elapsed().as_secs(),
                })
                .collect();
            reply.wire(
                req,
                &Audit {
                    entries,
                    volatile: true,
                },
            );
            return Ok(());
        }
        let prefix = if granting {
            "/.well-known/ops/approvals/"
        } else {
            "/.well-known/ops/actions/"
        };
        let id = req
            .path
            .strip_prefix(prefix)
            .ok_or_else(|| ApiError::not_found("No such ops route"))?;
        if req.method != "POST" {
            return Err(ApiError::new(405, "method", "POST required"));
        }
        let item = self
            .installed
            .iter()
            .find(|i| i.action.id() == id)
            .ok_or_else(|| ApiError::not_found("Action is not installed"))?;
        if !req.query.is_empty() && req.query != "dry_run=1" {
            return Err(ApiError::bad_request("Only dry_run=1 is supported"));
        }
        if granting && !req.query.is_empty() {
            return Err(ApiError::bad_request("Approval requests take no query"));
        }
        // Empty schema only; never pass arbitrary network data into a callback.
        if !req.body.is_empty() && req.body != b"{}" {
            return Err(ApiError::bad_request("This action takes an empty body"));
        }
        let key = req.header("Idempotency-Key");
        if !(16..=64).contains(&key.len())
            || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(ApiError::bad_request(
                "Idempotency-Key must be 16–64 ASCII letters, digits or hyphens",
            ));
        }
        let fingerprint = digest(item.action, key);
        let key_hash = hash(key.as_bytes());
        if state
            .entries
            .iter()
            .flatten()
            .any(|e| e.digest == key_hash && e.action != item.action)
        {
            return Err(ApiError::new(
                409,
                "key_conflict",
                "Idempotency-Key belongs to a different action",
            ));
        }
        if granting {
            if !item.action.disruptive() {
                return Err(ApiError::bad_request("This action needs no approval"));
            }
            if state.entries.iter().flatten().any(|e| e.digest == key_hash) {
                return Err(ApiError::new(409, "used", "Action key already used"));
            }
            let mut bytes = [0u8; 16];
            getrandom::getrandom(&mut bytes)
                .map_err(|_| ApiError::new(503, "random", "Approval unavailable"))?;
            let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            state.grant = Some(Grant {
                digest: fingerprint,
                token: hash(token.as_bytes()),
                until: Instant::now() + Duration::from_secs(300),
            });
            reply.wire(
                req,
                &Approval {
                    token,
                    expires_in_s: 300,
                },
            );
            return Ok(());
        }
        if req.param("dry_run").is_some_and(|v| v == "1") {
            reply.wire(
                req,
                &Outcome {
                    action: id.into(),
                    outcome: "dry-run; nothing executed".into(),
                },
            );
            return Ok(());
        }
        if let Some(e) = state
            .entries
            .iter()
            .flatten()
            .find(|e| e.digest == key_hash)
        {
            reply.wire_status(
                req,
                if e.ok { 200 } else { 409 },
                &Outcome {
                    action: id.into(),
                    outcome: if e.ok {
                        "accepted"
                    } else {
                        "failed-or-uncertain; not repeated"
                    }
                    .into(),
                },
            );
            return Ok(());
        }
        if state
            .entries
            .iter()
            .flatten()
            .any(|e| e.action == item.action && e.at.elapsed() < item.cooldown)
        {
            return Err(ApiError::new(429, "cooldown", "Action is cooling down"));
        }
        let slot = state
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or_else(|| {
                ApiError::new(
                    503,
                    "audit_full",
                    "Audit is full; operator intervention required",
                )
            })?;
        if item.action.disruptive() {
            if !state.grant.as_ref().is_some_and(|g| {
                g.until > Instant::now()
                    && g.digest == fingerprint
                    && same(&g.token, &hash(req.header("X-Ops-Approval").as_bytes()))
            }) {
                return Err(ApiError::new(
                    403,
                    "approval",
                    "A fresh approval for this exact action key is required",
                ));
            }
            state.grant = None;
        }
        // Reserve before calling. An error may still have had effects: never
        // retry the callback automatically. Lock also serializes concurrent POSTs.
        state.entries[slot] = Some(Entry {
            digest: key_hash,
            action: item.action,
            at: Instant::now(),
            ok: false,
        });
        (item.run)().map_err(|_| {
            ApiError::new(
                409,
                "uncertain",
                "Action failed or is uncertain; inspect before retrying",
            )
        })?;
        state.entries[slot].as_mut().unwrap().ok = true;
        reply.wire_status(
            req,
            202,
            &Outcome {
                action: id.into(),
                outcome: "accepted".into(),
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static ACTIONS: [Installed; 1] = [Installed {
        action: Action::RestartPlanned,
        run: || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        cooldown: Duration::from_secs(3600),
    }];
    const SAFE: &str = "safe-token-012345678901234567890123";
    const ADMIN: &str = "approval-token-01234567890123456789";
    fn request(
        gate: &Gate,
        path: &str,
        token: &str,
        key: &str,
        approval: &str,
    ) -> (u16, serde_json::Value) {
        let headers = vec![
            ("Authorization".into(), format!("Bearer {token}")),
            ("Idempotency-Key".into(), key.into()),
            ("X-Ops-Approval".into(), approval.into()),
        ];
        let req = Request::new("POST", path, &headers, b"{}", true);
        let (mut body, mut spare) = (Vec::new(), Vec::new());
        let mut reply = Reply::new(&mut body, &mut spare, 4096);
        gate.handle(&req, &mut reply);
        (
            reply.status(),
            serde_json::from_slice(reply.body()).unwrap(),
        )
    }
    #[test]
    fn safe_token_cannot_approve_and_grant_is_scoped_one_use_and_replay_safe() {
        let gate = Gate::new(SAFE, ADMIN, &ACTIONS).unwrap();
        let action = "/.well-known/ops/actions/restart-planned";
        let approve = "/.well-known/ops/approvals/restart-planned";
        let key = "action-key-00000001";
        assert_eq!(request(&gate, action, SAFE, key, "").0, 403);
        assert_eq!(request(&gate, approve, SAFE, key, "").0, 401);
        assert_eq!(
            request(&gate, &format!("{action}?dry_run=1"), SAFE, key, "").0,
            200
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
        let (_, expired) = request(&gate, approve, ADMIN, key, "");
        gate.state.lock().unwrap().grant.as_mut().unwrap().until =
            Instant::now() - Duration::from_secs(1);
        assert_eq!(
            request(&gate, action, SAFE, key, expired["token"].as_str().unwrap()).0,
            403
        );
        let (_, granted) = request(&gate, approve, ADMIN, key, "");
        let grant = granted["token"].as_str().unwrap();
        assert_eq!(
            request(&gate, action, SAFE, "action-key-00000002", grant).0,
            403
        );
        assert_eq!(request(&gate, action, SAFE, key, grant).0, 202);
        assert_eq!(request(&gate, action, SAFE, key, grant).0, 200);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        let rebooted = Gate::new(SAFE, ADMIN, &ACTIONS).unwrap();
        assert_eq!(request(&rebooted, action, SAFE, key, grant).0, 403);
        assert_eq!(request(&gate, approve, ADMIN, key, "").0, 409);
        assert_eq!(
            request(&gate, action, SAFE, "action-key-00000002", grant).0,
            429
        );
    }

    #[test]
    fn bounded_audit_does_not_evict_replay_protection_and_bad_queries_fail_closed() {
        static SAFE_ACTION: [Installed; 1] = [Installed {
            action: Action::Checkpoint,
            run: || Ok(()),
            cooldown: Duration::ZERO,
        }];
        let gate = Gate::new(SAFE, ADMIN, &SAFE_ACTION).unwrap();
        let action = "/.well-known/ops/actions/checkpoint";
        assert_eq!(
            request(
                &gate,
                &format!("{action}?dry_run=maybe"),
                SAFE,
                "action-key-00000000",
                ""
            )
            .0,
            400
        );
        for i in 0..4 {
            assert_eq!(
                request(&gate, action, SAFE, &format!("action-key-0000000{i}"), "").0,
                202
            );
        }
        assert_eq!(
            request(&gate, action, SAFE, "action-key-00000004", "").0,
            503
        );
        assert_eq!(
            request(&gate, action, SAFE, "action-key-00000000", "").0,
            200
        );
        assert!(std::mem::size_of::<Gate>() <= 512);
        eprintln!(
            "Ops Gate: {} bytes on this host; allocated only when provisioned",
            std::mem::size_of::<Gate>()
        );
        let headers = vec![
            ("Authorization".into(), format!("Bearer {SAFE}")),
            ("Idempotency-Key".into(), "action-key-00000000".into()),
        ];
        let req = Request::new("POST", action, &headers, b"{}", false);
        let (mut body, mut spare) = (Vec::new(), Vec::new());
        let mut reply = Reply::new(&mut body, &mut spare, 4096);
        gate.handle(&req, &mut reply);
        assert_eq!(reply.status(), 401);
    }
}
