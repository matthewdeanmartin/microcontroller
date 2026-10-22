//! The outbound worker (`spec/04-channels.md#outbound-queue`): one step at a
//! time, network outside the state lock, results committed after. Sign-in
//! steps go first (a person is waiting), then channel checks, a delivery,
//! the bot's poll and publication.
use crate::api::App;
use crate::messages::{BrevoSenders, Mention, Mentions, RemoteAccount, StatusPost, StoredJob};
use crate::outbound::{BOT, MAIL};
use miniframework::fetch::{Fetch, Outgoing};
use miniframework::wire::{self, Format};
use miniframework::{wall_ms, ApiError};

const BREVO: &str = "https://api.brevo.com/v3";
const LIMIT: usize = 64 * 1024;

fn wall() -> Option<u64> {
    wall_ms().map(|ms| ms / 1000)
}

fn request(
    fetch: &dyn Fetch,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let reply = fetch
        .send(&Outgoing {
            method,
            url,
            headers,
            body,
            limit: LIMIT,
        })
        .map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        let detail: String = String::from_utf8_lossy(&reply.body)
            .chars()
            .take(160)
            .collect();
        Err(format!("HTTP {}: {detail}", reply.status))
    }
}

/// The two HTTPS clients: household CA and public CAs.
pub struct Fetchers<'a> {
    pub household: &'a dyn Fetch,
    pub public: &'a dyn Fetch,
}

impl Fetchers<'_> {
    fn for_trust(&self, trust: &str) -> &dyn Fetch {
        if trust == "household" {
            self.household
        } else {
            self.public
        }
    }
}

impl App {
    /// Records a worker result (commit failures are logged; the next step retries).
    fn record(&self, what: &str, f: impl FnOnce(&mut crate::state::State)) {
        let result = self.mutate(|inner| {
            f(&mut inner.state);
            Ok::<(), ApiError>(())
        });
        if let Err(e) = result {
            log::warn!("worker {what}: {}", e.message);
        }
    }

    /// One unit of outbound work. Returns whether there was any (the caller
    /// sleeps a little when there wasn't). Blocks for HTTP: call it from a
    /// worker thread, never the serving loop.
    pub fn work(&self, f: &Fetchers<'_>) -> bool {
        if self.oauth.as_ref().is_some_and(|o| o.run_once()) {
            return true;
        }
        // The rest uses wall-clock times (retries, polls, publication).
        let Some(now) = wall() else { return false };
        self.check_mail(f, now)
            || self.check_bot(f, now)
            || self.deliver_one(f, now)
            || self.poll_bot(f, now)
            || self.publish_once(f, now)
    }

    /// A new Brevo key: find its sender (and so prove it works).
    fn check_mail(&self, f: &Fetchers<'_>, _now: u64) -> bool {
        let Some(m) = self
            .lock()
            .state
            .mail()
            .filter(|m| m.state == "checking")
            .cloned()
        else {
            return false;
        };
        let outcome = request(
            f.public,
            "GET",
            &format!("{BREVO}/senders"),
            &[("api-key", &m.api_key), ("Accept", "application/json")],
            &[],
        )
        .map_err(|e| {
            if e.starts_with("HTTP 401") {
                "Brevo doesn't know that key".to_string()
            } else {
                e
            }
        })
        .and_then(|body| {
            wire::decode::<BrevoSenders>(Format::Json, &body).map_err(|e| e.to_string())
        })
        .and_then(|list| {
            let want = m.sender_email.clone();
            list.senders
                .into_iter()
                .filter(|s| s.active)
                .find(|s| want.is_empty() || s.email.to_lowercase() == want)
                .map(|s| s.email)
                .ok_or_else(|| {
                    if want.is_empty() {
                        "The Brevo account has no verified sender yet (Brevo → Senders)".to_string()
                    } else {
                        format!("{want} isn't a verified sender on that Brevo account")
                    }
                })
        });
        self.record("mail check", |s| {
            if let Some(mm) = s
                .channel_mut(MAIL)
                .mail
                .as_mut()
                .filter(|mm| mm.api_key == m.api_key)
            {
                match outcome {
                    Ok(sender) => {
                        mm.sender_email = sender;
                        mm.state = "ready".into();
                        mm.error.clear();
                    }
                    Err(e) => {
                        mm.state = "failed".into();
                        mm.error = e;
                    }
                }
            }
        });
        true
    }

    /// A new bot token: whose is it?
    fn check_bot(&self, f: &Fetchers<'_>, _now: u64) -> bool {
        let Some(b) = self
            .lock()
            .state
            .bot()
            .filter(|b| b.state == "checking")
            .cloned()
        else {
            return false;
        };
        let auth = format!("Bearer {}", b.token);
        let outcome = request(
            f.for_trust(&b.trust),
            "GET",
            &format!("{}/api/v1/accounts/verify_credentials", b.instance),
            &[("Authorization", &auth), ("Accept", "application/json")],
            &[],
        )
        .and_then(|body| {
            wire::decode::<RemoteAccount>(Format::Json, &body).map_err(|e| e.to_string())
        });
        self.record("bot check", |s| {
            if let Some(bb) = s
                .channel_mut(BOT)
                .bot
                .as_mut()
                .filter(|bb| bb.token == b.token)
            {
                match outcome {
                    Ok(me) => {
                        bb.handle = format!(
                            "@{}@{}",
                            me.username.to_lowercase(),
                            crate::identity::host_of(&bb.instance)
                        );
                        bb.account_id = me.id;
                        bb.state = "ready".into();
                        bb.error.clear();
                    }
                    Err(e) => {
                        bb.state = "failed".into();
                        bb.error = if e.starts_with("HTTP 401") {
                            "The instance didn't accept that token".into()
                        } else {
                            e
                        };
                    }
                }
            }
        });
        true
    }

    /// Sends the next due email or DM.
    fn deliver_one(&self, f: &Fetchers<'_>, now: u64) -> bool {
        let (job, mail, bot) = {
            let inner = self.lock();
            let s = &inner.state;
            let Some(job) = s.next_job(now) else {
                return false;
            };
            if job.kind == "mail" && !s.mail_budget_left(now) {
                return false;
            }
            (job, s.mail().cloned(), s.bot().cloned())
        };
        let result = match job.kind.as_str() {
            "mail" => {
                let m = mail.expect("next_job checks it");
                let body = crate::mail::brevo_body(&m.sender_name, &m.sender_email, &job);
                request(
                    f.public,
                    "POST",
                    &format!("{BREVO}/smtp/email"),
                    &[
                        ("api-key", &m.api_key),
                        ("Content-Type", "application/json"),
                        ("Accept", "application/json"),
                    ],
                    &body,
                )
                .map(drop)
            }
            _ => {
                let b = bot.expect("next_job checks it");
                post_dm(f.for_trust(&b.trust), &b.instance, &b.token, &job)
            }
        };
        if let Err(e) = &result {
            log::warn!("delivery {} ({}): {e}", job.id, job.kind);
        }
        self.record("delivery", |s| s.job_done(job.id, result, now));
        true
    }

    /// The bot's mentions since its cursor, run as commands.
    fn poll_bot(&self, f: &Fetchers<'_>, now: u64) -> bool {
        let Some(b) = self
            .lock()
            .state
            .bot()
            .filter(|b| {
                b.state == "ready"
                    && b.commands
                    && now >= b.last_poll + u64::from(b.poll_secs.max(30))
            })
            .cloned()
        else {
            return false;
        };
        // The first poll only finds where "now" is; later ones page forward.
        let query = if b.cursor.is_empty() {
            "limit=1".to_string()
        } else {
            format!("limit=20&min_id={}", b.cursor)
        };
        let auth = format!("Bearer {}", b.token);
        let outcome = request(
            f.for_trust(&b.trust),
            "GET",
            &format!(
                "{}/api/v1/notifications?types%5B%5D=mention&{query}",
                b.instance
            ),
            &[("Authorization", &auth), ("Accept", "application/json")],
            &[],
        )
        .and_then(|bytes| {
            let mut wrapped = b"{\"items\":".to_vec();
            wrapped.extend_from_slice(&bytes);
            wrapped.push(b'}');
            wire::decode::<Mentions>(Format::Json, &wrapped)
                .map(|m| m.items)
                .map_err(|e| e.to_string())
        });
        match outcome {
            Ok(batch) => {
                let batch: Vec<Mention> = batch;
                self.record("bot poll", |s| {
                    if s.bot().is_some_and(|bb| bb.token == b.token) {
                        s.take_mentions(batch, now);
                    }
                });
            }
            Err(e) => self.record("bot poll", |s| {
                if let Some(bb) = s.channel_mut(BOT).bot.as_mut() {
                    bb.last_poll = now;
                    bb.error = e;
                }
            }),
        }
        true
    }

    /// Publication, when it's due and something changed.
    fn publish_once(&self, f: &Fetchers<'_>, now: u64) -> bool {
        let due = self.lock().state.publication_due(now);
        let Some((setup, files, changed, removed)) = due else {
            // Nothing to push; note the check so it isn't redone every step.
            let checked = self.lock().state.publish().is_some_and(|p| {
                !p.token.is_empty()
                    && now >= p.last_attempt + u64::from(p.every_minutes.max(10)) * 60
            });
            if checked {
                self.record("publish check", |s| s.publication_checked(now));
            }
            return false;
        };
        let result = crate::publish::push(f.public, &setup, &files, &changed, &removed);
        if let Err(e) = &result {
            log::warn!("publication: {e}");
        }
        self.record("publication", |s| s.publication_done(&files, result, now));
        true
    }
}

/// Posts a direct status (`@handle text`). The Idempotency-Key makes a
/// retry after a timeout a no-op on the instance.
fn post_dm(fetch: &dyn Fetch, instance: &str, token: &str, job: &StoredJob) -> Result<(), String> {
    let body = wire::to_vec(
        Format::Json,
        &StatusPost {
            status: job.text.chars().take(480).collect(),
            visibility: "direct".into(),
        },
    )
    .map_err(|e| e.to_string())?;
    let auth = format!("Bearer {token}");
    let key = format!("party-{}-{}", job.id, job.created);
    request(
        fetch,
        "POST",
        &format!("{instance}/api/v1/statuses"),
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("Idempotency-Key", &key),
        ],
        &body,
    )
    .map(drop)
}
