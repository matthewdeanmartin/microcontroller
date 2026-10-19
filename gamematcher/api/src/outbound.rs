//! Outbound Mastodon and email: the household bot and Brevo mail
//! (`spec/08-mastodon.md`).
//!
//! Everything that talks to an instance runs on the app's outbound worker
//! ([`Outbox::run_once`], the same thread as OAuth), never in a request
//! handler. Posts wait in a small queue persisted in the Kv, so a reboot or
//! an internet outage doesn't lose a lock-in announcement; each carries an
//! `Idempotency-Key`, so a retry never double-posts.
//!
//! The bot's token lives in the Kv (`bot`); the mastomini admin password
//! used to create a bot exists only in RAM for that one setup. The Brevo
//! key lives in the Kv (`mail`) or comes from the firmware's build settings.
use crate::mail::{self, Email};
use crate::messages::{BotView, MailView, OutboxEntry, StatusPost};
use miniframework::fetch::{Fetch, Outgoing};
use miniframework::kv::Kv;
use miniframework::wire::{self, Format};
use miniframework::{message, ApiError};
use std::sync::{Arc, Mutex};

const QUEUE: usize = 32;
/// The API key's name on mastomini (its enrollment keys must be `mmb-*`).
const KEY_NAME: &str = "mmb-gamematcher";
const BREVO: &str = "https://api.brevo.com/v3";
/// Marks a setup error as "couldn't reach Brevo" (worth retrying).
const NETWORK: &str = "can't reach Brevo: ";
const LIMIT: usize = 32 * 1024;
/// Give up on a post this long after it was queued.
const GIVE_UP: u64 = 24 * 3600;

message! {
    /// The bot's settings and secret, persisted in the Kv.
    pub struct StoredBot {
        /// `https://mastomini.local`
        1 instance: String,
        2 token: String,
        /// `@gamematcher@mastomini.local`
        3 handle: String,
        /// `household-ca` or `public`.
        4 anchor: String,
        /// `public`, `unlisted` or `private`.
        5 visibility: String,
        6 digest: bool,
        /// Local "HH:MM".
        7 digest_time: String,
        /// Local "YYYY-MM-DD" of the last digest.
        8 last_digest: String,
        9 posts: u32,
        10 last_post: u64,
        11 last_error: String,
        12 replies: bool,
    }
}

message! {
    pub struct StoredJob {
        1 id: u32,
        /// `post` or `dm`.
        2 kind: String,
        3 text: String,
        4 visibility: String,
        5 created: u64,
        6 attempts: u32,
        7 next_at: u64,
        /// `queued`, `sent` or `failed`.
        8 state: String,
        9 error: String,
        10 url: String,
        /// Email only: the address, name and subject (`text` is the body).
        11 to: String,
        12 to_name: String,
        13 subject: String,
        14 retried: u64,
    }
}

message! {
    /// Brevo settings, persisted in the Kv.
    pub struct StoredMail {
        1 api_key: String,
        2 sender_email: String,
        3 sender_name: String,
        4 sent: u32,
        5 last_error: String,
        /// Days since 1970 (UTC) of `sent_today`.
        6 day: u64,
        7 sent_today: u32,
    }
}

message! {
    pub struct BrevoSender {
        1 name: String,
        2 email: String,
        3 active: bool,
    }
}

message! {
    pub struct BrevoSenders {
        1 senders: Vec<BrevoSender>,
    }
}

/// A Brevo setup waiting for the worker.
#[derive(Clone, Default)]
pub struct MailSetup {
    pub api_key: String,
    pub sender_email: String,
    pub sender_name: String,
    pub built_in: bool,
}

message! {
    pub struct StoredOutbox {
        1 jobs: Vec<StoredJob>,
        2 next_id: u32,
        3 links: Vec<crate::replies::PostLink>,
    }
}

message! {
    /// `verify_credentials` / status replies: only what we read.
    pub struct Posted {
        1 id: String,
        2 url: String,
        3 username: String,
        4 acct: String,
    }
}

/// A setup waiting for the worker (credentials in RAM only).
#[derive(Clone)]
pub enum Setup {
    /// Create a bot account on a mastomini (its admin API).
    Mastomini {
        instance: String,
        admin: String,
        password: String,
        username: String,
        display_name: String,
    },
    /// Use an existing account's access token (any Mastodon).
    Token {
        instance: String,
        token: String,
        anchor: String,
    },
}

#[derive(Default)]
struct Inner {
    bot: Option<StoredBot>,
    outbox: StoredOutbox,
    setup: Option<Setup>,
    /// What the last setup said ("" while none; "working" while running).
    setup_state: String,
    mail: Option<StoredMail>,
    mail_setup: Option<MailSetup>,
    mail_state: String,
    /// The key came from the build settings (shown on the admin page).
    mail_built_in: bool,
    /// A setup that couldn't reach Brevo waits until then (wall seconds).
    mail_retry_at: u64,
    inbox: crate::replies::Inbox,
    last_poll: u64,
    reply_error: String,
    generation: u64,
    active_job: Option<u32>,
}

pub struct Outbox {
    kv: Arc<dyn Kv>,
    household: Arc<dyn Fetch>,
    public: Arc<dyn Fetch>,
    inner: Mutex<Inner>,
}

fn host_of(base: &str) -> &str {
    base.split_once("://")
        .map_or(base, |(_, r)| r)
        .split(['/', ':'])
        .next()
        .unwrap_or("")
}

fn clean_instance(raw: &str) -> Result<String, ApiError> {
    let base = raw.trim().trim_end_matches('/').to_string();
    if !(base.starts_with("https://") || base.starts_with("http://")) || host_of(&base).is_empty() {
        return Err(ApiError::bad_request(
            "The instance address starts with https://",
        ));
    }
    Ok(base)
}

fn json_body<M: miniframework::Message>(m: &M) -> Vec<u8> {
    wire::to_vec(Format::Json, m).unwrap_or_default()
}

impl Outbox {
    pub fn open(kv: Arc<dyn Kv>, household: Arc<dyn Fetch>, public: Arc<dyn Fetch>) -> Self {
        let bot = kv
            .get("bot")
            .and_then(|b| wire::decode::<StoredBot>(Format::CborInt, &b).ok())
            .filter(|b| !b.token.is_empty());
        let outbox = kv
            .get("outbox")
            .and_then(|b| wire::decode(Format::CborInt, &b).ok())
            .unwrap_or_default();
        let mail = kv
            .get("mail")
            .and_then(|b| wire::decode::<StoredMail>(Format::CborInt, &b).ok())
            .filter(|m| !m.api_key.is_empty());
        Self {
            kv: kv.clone(),
            household,
            public,
            inner: Mutex::new(Inner {
                inbox: kv
                    .get("inbox")
                    .and_then(|b| wire::decode(Format::CborInt, &b).ok())
                    .unwrap_or_default(),
                bot,
                outbox,
                mail,
                ..Default::default()
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, inner: &Inner) {
        let result = match &inner.bot {
            Some(bot) => self.kv.set(
                "bot",
                &wire::to_vec(Format::CborInt, bot).unwrap_or_default(),
            ),
            None => self.kv.remove("bot"),
        }
        .and_then(|_| {
            self.kv.set(
                "outbox",
                &wire::to_vec(Format::CborInt, &inner.outbox).unwrap_or_default(),
            )
        })
        .and_then(|_| match &inner.mail {
            Some(m) => self.kv.set(
                "mail",
                &wire::to_vec(Format::CborInt, m).unwrap_or_default(),
            ),
            None => self.kv.remove("mail"),
        });
        if let Err(e) = result {
            log::warn!("saving the bot/outbox: {e}");
        }
    }

    /// `@gamematcher@host`, if a bot is set up.
    pub fn handle(&self) -> Option<String> {
        self.lock().bot.as_ref().map(|b| b.handle.clone())
    }

    /// The bot's instance host (DMs reach only accounts on it).
    pub fn host(&self) -> Option<String> {
        self.lock()
            .bot
            .as_ref()
            .map(|b| host_of(&b.instance).to_string())
    }

    /// Whether the bot is on a public instance, which federates: it can DM
    /// accounts elsewhere (a `.local` household instance can't).
    pub fn federated(&self) -> bool {
        self.lock()
            .bot
            .as_ref()
            .is_some_and(|b| b.anchor != "household-ca")
    }

    pub fn view(&self) -> BotView {
        let inner = self.lock();
        let jobs = inner
            .outbox
            .jobs
            .iter()
            .rev()
            .map(|j| OutboxEntry {
                id: j.id,
                kind: j.kind.clone(),
                text: if j.kind == "email" {
                    format!("{} → {}", j.subject, j.to)
                        .chars()
                        .take(80)
                        .collect()
                } else {
                    j.text.chars().take(80).collect()
                },
                state: j.state.clone(),
                attempts: j.attempts,
                error: j.error.clone(),
                created: j.created,
                url: j.url.clone(),
                next_at: j.next_at,
            })
            .collect();
        match &inner.bot {
            Some(b) => BotView {
                configured: true,
                handle: b.handle.clone(),
                instance: b.instance.clone(),
                visibility: b.visibility.clone(),
                digest: b.digest,
                digest_time: b.digest_time.clone(),
                posts: b.posts,
                last_post: b.last_post,
                last_error: b.last_error.clone(),
                setup: inner.setup_state.clone(),
                jobs,
                replies: b.replies,
                last_poll: inner.last_poll,
                reply_error: inner.reply_error.clone(),
            },
            None => BotView {
                setup: inner.setup_state.clone(),
                jobs,
                ..Default::default()
            },
        }
    }

    /// Queues a setup for the worker. Credentials never touch flash.
    pub fn begin_setup(&self, setup: Setup) -> Result<(), ApiError> {
        let mut inner = self.lock();
        if inner.setup.is_some() || inner.setup_state == "working" {
            return Err(ApiError::new(409, "busy", "A setup is already running"));
        }
        inner.generation += 1;
        inner.setup = Some(setup);
        inner.setup_state = "working".into();
        Ok(())
    }

    pub fn settings(
        &self,
        visibility: Option<&str>,
        digest: Option<bool>,
        digest_time: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut inner = self.lock();
        let bot = inner
            .bot
            .as_mut()
            .ok_or_else(|| ApiError::new(409, "no_bot", "Set up the bot first"))?;
        if let Some(v) = visibility {
            if !matches!(v, "public" | "unlisted" | "private") {
                return Err(ApiError::bad_request(
                    "visibility: public, unlisted or private",
                ));
            }
            bot.visibility = v.into();
        }
        if let Some(d) = digest {
            bot.digest = d;
        }
        if let Some(t) = digest_time {
            let ok = t.len() == 5
                && t.as_bytes()[2] == b':'
                && t[..2].parse::<u32>().is_ok_and(|h| h < 24)
                && t[3..].parse::<u32>().is_ok_and(|m| m < 60);
            if !ok {
                return Err(ApiError::bad_request("Digest time: HH:MM"));
            }
            bot.digest_time = t.into();
        }
        self.save(&inner);
        Ok(())
    }

    pub fn remove(&self) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.setup = None;
        inner.bot = None;
        inner.inbox = Default::default();
        inner.outbox.links.clear();
        inner.reply_error.clear();
        let _ = self.kv.remove("inbox");
        inner
            .outbox
            .jobs
            .retain(|j| j.state != "queued" || j.kind == "email");
        inner.setup_state.clear();
        self.save(&inner);
    }

    /// Queues a status. `visibility` None = the bot's default.
    pub fn enqueue(&self, kind: &str, text: String, visibility: Option<&str>, now: u64) -> bool {
        let mut inner = self.lock();
        let Some(bot) = &inner.bot else { return false };
        let visibility = visibility.unwrap_or(&bot.visibility).to_string();
        let job = StoredJob {
            kind: kind.into(),
            text,
            visibility,
            ..Default::default()
        };
        self.push(&mut inner, job, now)
    }

    /// Queues an email; false when mail isn't set up, today's allowance is
    /// spent, or the queue is full of unsent work.
    pub fn email(&self, email: Email, now: u64) -> bool {
        let mut inner = self.lock();
        let Some(m) = inner.mail.as_mut() else {
            return false;
        };
        let day = now / 86_400;
        if m.day != day {
            m.day = day;
            m.sent_today = 0;
        }
        if m.sent_today >= mail::DAILY_LIMIT {
            log::warn!(
                "email allowance for today is spent; not sending \"{}\"",
                email.subject
            );
            return false;
        }
        m.sent_today += 1;
        let job = StoredJob {
            kind: "email".into(),
            text: email.text,
            to: email.to,
            to_name: email.to_name,
            subject: email.subject,
            ..Default::default()
        };
        if self.push(&mut inner, job, now) {
            true
        } else {
            if let Some(m) = inner.mail.as_mut() {
                m.sent_today -= 1;
                m.last_error = "Delivery queue full".into();
            }
            self.save(&inner);
            false
        }
    }

    fn push(&self, inner: &mut Inner, job: StoredJob, now: u64) -> bool {
        let id = inner.outbox.next_id.max(1);
        inner.outbox.next_id = id + 1;
        // Drop the oldest finished entries first; never a queued one.
        while inner.outbox.jobs.len() >= QUEUE {
            match inner.outbox.jobs.iter().position(|j| j.state != "queued") {
                Some(i) => {
                    inner.outbox.jobs.remove(i);
                }
                None => return false,
            }
        }
        inner.outbox.jobs.push(StoredJob {
            id,
            created: now,
            next_at: 0,
            state: "queued".into(),
            ..job
        });
        self.save(inner);
        true
    }

    pub fn mail_ready(&self) -> bool {
        self.lock().mail.is_some()
    }

    pub fn mail_view(&self) -> MailView {
        let inner = self.lock();
        let m = inner.mail.clone().unwrap_or_default();
        MailView {
            configured: inner.mail.is_some(),
            sender: if m.sender_email.is_empty() {
                String::new()
            } else {
                format!("{} <{}>", m.sender_name, m.sender_email)
            },
            sent: m.sent,
            sent_today: m.sent_today,
            daily_limit: mail::DAILY_LIMIT,
            last_error: m.last_error,
            setup: inner.mail_state.clone(),
            built_in: inner.mail_built_in,
        }
    }

    /// Queues a Brevo setup for the worker (it checks the key and finds
    /// the sender). The build's key is used only while none is stored.
    pub fn begin_mail_setup(&self, setup: MailSetup) -> Result<(), ApiError> {
        let key = setup.api_key.trim();
        if key.starts_with("xsmtpsib-") {
            return Err(ApiError::bad_request(
                "That's a Brevo SMTP key; the board needs an API key (xkeysib-...): Brevo → SMTP & API → API keys",
            ));
        }
        if key.len() < 20 || key.contains(char::is_whitespace) {
            return Err(ApiError::bad_request(
                "Paste the whole Brevo API key (xkeysib-...)",
            ));
        }
        let mut inner = self.lock();
        if setup.built_in && inner.mail.is_some() {
            return Ok(());
        }
        if inner.mail_setup.is_some() || inner.mail_state == "working" {
            return Err(ApiError::new(409, "busy", "A setup is already running"));
        }
        inner.mail_built_in = setup.built_in;
        inner.mail_setup = Some(setup);
        inner.mail_state = "working".into();
        Ok(())
    }

    pub fn remove_mail(&self) {
        let mut inner = self.lock();
        inner.mail = None;
        inner.mail_built_in = false;
        inner.mail_state.clear();
        inner
            .outbox
            .jobs
            .retain(|j| j.state != "queued" || j.kind != "email");
        self.save(&inner);
    }

    fn run_mail_setup(&self, setup: &MailSetup) -> Result<StoredMail, String> {
        let reply = self
            .public
            .send(&Outgoing {
                method: "GET",
                url: &format!("{BREVO}/senders"),
                headers: &[
                    ("api-key", setup.api_key.trim()),
                    ("Accept", "application/json"),
                ],
                body: &[],
                limit: LIMIT,
            })
            .map_err(|e| format!("{NETWORK}{e}"))?;
        if reply.status == 401 {
            return Err("Brevo doesn't know that key".into());
        }
        if reply.status != 200 {
            return Err(format!("Brevo said HTTP {}", reply.status));
        }
        let senders: BrevoSenders =
            wire::decode(Format::Json, &reply.body).map_err(|e| e.to_string())?;
        let want = setup.sender_email.trim().to_lowercase();
        let sender = senders
            .senders
            .iter()
            .filter(|s| s.active)
            .find(|s| want.is_empty() || s.email.to_lowercase() == want)
            .ok_or_else(|| {
                if want.is_empty() {
                    "The Brevo account has no verified sender yet (Brevo → Senders)".to_string()
                } else {
                    format!("{want} isn't a verified sender on that Brevo account")
                }
            })?;
        Ok(StoredMail {
            api_key: setup.api_key.trim().into(),
            sender_email: sender.email.clone(),
            sender_name: if setup.sender_name.trim().is_empty() {
                "Game Matcher".into()
            } else {
                setup.sender_name.trim().into()
            },
            ..Default::default()
        })
    }

    fn send_email(&self, m: &StoredMail, job: &StoredJob) -> Result<String, String> {
        let body = mail::brevo_body(
            &m.sender_name,
            &m.sender_email,
            &job.to,
            &job.to_name,
            &job.subject,
            &job.text,
        );
        let reply = self
            .public
            .send(&Outgoing {
                method: "POST",
                url: &format!("{BREVO}/smtp/email"),
                headers: &[
                    ("api-key", &m.api_key),
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                ],
                body: &body,
                limit: LIMIT,
            })
            .map_err(|e| e.to_string())?;
        if !(200..300).contains(&reply.status) {
            let detail: String = String::from_utf8_lossy(&reply.body)
                .chars()
                .take(160)
                .collect();
            return Err(format!("Brevo HTTP {}: {detail}", reply.status));
        }
        Ok(String::new())
    }

    /// Whether the digest is due at local date `today` and time `hhmm`
    /// ("HH:MM"); marks it done when it is (the caller posts it).
    pub fn digest_due(&self, today: &str, hhmm: &str) -> bool {
        let mut inner = self.lock();
        let Some(bot) = inner.bot.as_mut() else {
            return false;
        };
        if !bot.digest || bot.last_digest == today || hhmm < bot.digest_time.as_str() {
            return false;
        }
        bot.last_digest = today.into();
        self.save(&inner);
        true
    }

    /// The password for bots this board creates on mastomini: made once and
    /// kept in the Kv, so a retried setup finds its bot again.
    fn bot_secret(&self) -> Result<String, String> {
        if let Some(saved) = self
            .kv
            .get("bot_secret")
            .and_then(|b| String::from_utf8(b).ok())
        {
            if !saved.is_empty() {
                return Ok(saved);
            }
        }
        let secret = miniframework::session::random_token().map_err(|e| e.to_string())?;
        self.kv
            .set("bot_secret", secret.as_bytes())
            .map_err(|e| format!("saving the bot's password: {e}"))?;
        Ok(secret)
    }

    fn fetch(&self, anchor: &str) -> &dyn Fetch {
        if anchor == "household-ca" {
            self.household.as_ref()
        } else {
            self.public.as_ref()
        }
    }

    /// Does one network job: a pending setup, else the next due post.
    /// Returns whether it did anything. Runs on the worker thread.
    pub fn run_once(&self, now: u64) -> bool {
        let (setup, generation) = {
            let mut inner = self.lock();
            (inner.setup.take(), inner.generation)
        };
        if let Some(setup) = setup {
            let outcome = self.run_setup(setup);
            let mut inner = self.lock();
            if inner.generation != generation {
                return true;
            }
            match outcome {
                Ok(bot) => {
                    if inner
                        .bot
                        .as_ref()
                        .is_some_and(|old| old.instance != bot.instance || old.handle != bot.handle)
                    {
                        inner.outbox.jobs.retain(|j| j.kind == "email");
                        inner.outbox.links.clear();
                        inner.inbox = Default::default();
                        let _ = self.kv.remove("inbox");
                    }
                    inner.setup_state = format!("ready: {}", bot.handle);
                    log::info!("Mastodon bot ready: {}", bot.handle);
                    inner.bot = Some(bot);
                }
                Err(e) => inner.setup_state = format!("failed: {e}"),
            }
            self.save(&inner);
            return true;
        }
        let mail_setup = {
            let mut inner = self.lock();
            if now >= inner.mail_retry_at {
                inner.mail_setup.take()
            } else {
                None
            }
        };
        if let Some(setup) = mail_setup {
            let outcome = self.run_mail_setup(&setup);
            let mut inner = self.lock();
            match outcome {
                Ok(m) => {
                    inner.mail_state = format!("ready: {} <{}>", m.sender_name, m.sender_email);
                    log::info!("email ready: from {}", m.sender_email);
                    inner.mail = Some(m);
                }
                Err(e) if e.starts_with(NETWORK) => {
                    // No network yet (just booted) or no internet: keep it and
                    // try again; a wrong key fails at once below.
                    log::info!("email setup: {e}; trying again in 30 s");
                    inner.mail_setup = Some(setup);
                    inner.mail_retry_at = now + 30;
                }
                Err(e) => {
                    log::warn!("email setup: {e}");
                    inner.mail_state = format!("failed: {e}");
                }
            }
            self.save(&inner);
            return true;
        }
        let (bot, mail, job) = {
            let mut inner = self.lock();
            let ready = |j: &&StoredJob| {
                j.state == "queued"
                    && j.next_at <= now
                    && if j.kind == "email" {
                        inner.mail.is_some()
                    } else {
                        inner.bot.is_some()
                    }
            };
            let Some(job) = inner.outbox.jobs.iter().find(ready).cloned() else {
                drop(inner);
                return self.poll_replies(now);
            };
            inner.active_job = Some(job.id);
            (inner.bot.clone(), inner.mail.clone(), job)
        };
        let result = match (&job.kind[..], &bot, &mail) {
            ("email", _, Some(m)) => self.send_email(m, &job).map(|url| (url, String::new())),
            (_, Some(bot), _) => self.post(bot, &job),
            _ => return false,
        };
        let mut inner = self.lock();
        inner.active_job = None;
        if let Some(j) = inner.outbox.jobs.iter_mut().find(|j| j.id == job.id) {
            j.attempts += 1;
            match &result {
                Ok((url, _)) => {
                    j.state = "sent".into();
                    j.url = url.clone();
                    j.error.clear();
                }
                Err(e) => {
                    j.error = e.clone();
                    let backoff = 60u64 << j.attempts.min(6); // 2 min .. ~1 h
                    j.next_at = now + backoff;
                    if now.saturating_sub(j.retried.max(j.created)) > GIVE_UP || j.attempts >= 12 {
                        j.state = "failed".into();
                    }
                }
            }
        }
        if let Ok((_, remote)) = &result {
            if !remote.is_empty() {
                if let Some(plan) = crate::replies::linked_plan(&job.text) {
                    inner.outbox.links.retain(|l| l.status != *remote);
                    inner.outbox.links.push(crate::replies::PostLink {
                        status: remote.clone(),
                        plan,
                    });
                    if inner.outbox.links.len() > 128 {
                        inner.outbox.links.remove(0);
                    }
                }
            }
        }
        if job.kind == "email" {
            if let Some(m) = inner.mail.as_mut() {
                match &result {
                    Ok(_) => {
                        m.sent += 1;
                        m.last_error.clear();
                    }
                    Err(e) => m.last_error = e.clone(),
                }
            }
        } else if let Some(b) = inner.bot.as_mut() {
            match &result {
                Ok(_) => {
                    b.posts += 1;
                    b.last_post = now;
                    b.last_error.clear();
                }
                Err(e) => b.last_error = e.clone(),
            }
        }
        self.save(&inner);
        true
    }

    fn post(&self, bot: &StoredBot, job: &StoredJob) -> Result<(String, String), String> {
        let body = json_body(&StatusPost {
            status: job.text.clone(),
            visibility: job.visibility.clone(),
        });
        let bearer = format!("Bearer {}", bot.token);
        let key = format!("gamematcher-{}-{}", job.id, job.created);
        let reply = self
            .fetch(&bot.anchor)
            .send(&Outgoing {
                method: "POST",
                url: &format!("{}/api/v1/statuses", bot.instance),
                headers: &[
                    ("Authorization", &bearer),
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                    ("Idempotency-Key", &key),
                ],
                body: &body,
                limit: LIMIT,
            })
            .map_err(|e| e.to_string())?;
        if reply.status != 200 {
            return Err(format!("HTTP {}", reply.status));
        }
        Ok(wire::decode::<Posted>(Format::Json, &reply.body)
            .map(|p| (p.url, p.id))
            .unwrap_or_default())
    }

    fn call(
        &self,
        anchor: &str,
        method: &str,
        url: &str,
        bearer: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, String> {
        let auth = format!("Bearer {bearer}");
        let mut headers = vec![
            ("Accept", "application/json"),
            ("Content-Type", "application/json"),
        ];
        if !bearer.is_empty() {
            headers.push(("Authorization", &auth));
        }
        let reply = self
            .fetch(anchor)
            .send(&Outgoing {
                method,
                url,
                headers: &headers,
                body,
                limit: LIMIT,
            })
            .map_err(|e| e.to_string())?;
        if !(200..300).contains(&reply.status) {
            let detail = String::from_utf8_lossy(&reply.body)
                .chars()
                .take(160)
                .collect::<String>();
            return Err(format!(
                "{} → HTTP {}: {detail}",
                url.rsplit_once("/api/").map_or(url, |(_, p)| p),
                reply.status
            ));
        }
        Ok(reply.body)
    }

    fn verify(&self, instance: &str, anchor: &str, token: &str) -> Result<String, String> {
        let me = self.call(
            anchor,
            "GET",
            &format!("{instance}/api/v1/accounts/verify_credentials"),
            token,
            &[],
        )?;
        let me: Posted = wire::decode(Format::Json, &me).map_err(|e| e.to_string())?;
        Ok(format!("@{}@{}", me.username, host_of(instance)))
    }

    fn run_setup(&self, setup: Setup) -> Result<StoredBot, String> {
        let bot = |instance: String, token: String, handle: String, anchor: String| StoredBot {
            instance,
            token,
            handle,
            anchor,
            visibility: "unlisted".into(),
            digest: false,
            digest_time: "09:00".into(),
            ..Default::default()
        };
        match setup {
            Setup::Token {
                instance,
                token,
                anchor,
            } => {
                let handle = self.verify(&instance, &anchor, &token)?;
                Ok(bot(instance, token, handle, anchor))
            }
            Setup::Mastomini {
                instance,
                admin,
                password,
                username,
                display_name,
            } => {
                // mastomini's admin API, as mastomini-bots' enrollment uses it.
                let anchor = "household-ca";
                let body = json_body(&AdminLogin {
                    username: admin,
                    password,
                });
                let login = self.call(
                    anchor,
                    "POST",
                    &format!("{instance}/api/mastomini/v1/admin/bots/login"),
                    "",
                    &body,
                )?;
                let admin_token = wire::decode::<TokenOnly>(Format::Json, &login)
                    .map_err(|e| e.to_string())?
                    .access_token;
                if admin_token.is_empty() {
                    return Err("mastomini sent no admin token".into());
                }
                // mastomini answers a repeated create (same username and
                // password) with the same bot, so the generated password is
                // saved *before* the bot exists: a setup that fails halfway
                // (or loses its reply) is just run again, as mastomini-bots does.
                let secret = self.bot_secret()?;
                let created = self
                    .call(
                        anchor,
                        "POST",
                        &format!("{instance}/api/mastomini/v1/admin/bots"),
                        &admin_token,
                        &json_body(&NewBot {
                            username: username.clone(),
                            display_name,
                            password: secret.clone(),
                        }),
                    )
                    .map_err(|e| {
                        if e.contains("HTTP 401") {
                            format!(
                                "@{username} already exists on mastomini as a bot this board didn't make \
                                 (or made before it kept its password). Delete it there, or pick another bot name"
                            )
                        } else {
                            e
                        }
                    })?;
                let id = wire::decode::<IdOnly>(Format::Json, &created)
                    .map_err(|e| e.to_string())?
                    .id;
                let key = self.call(
                    anchor,
                    "POST",
                    &format!("{instance}/api/mastomini/v1/admin/bots/{id}/api_keys"),
                    &admin_token,
                    &json_body(&KeyRequest {
                        // mastomini only issues enrollment keys named mmb-*.
                        name: KEY_NAME.into(),
                        password: secret,
                    }),
                )?;
                let token = wire::decode::<KeyOnly>(Format::Json, &key)
                    .map_err(|e| e.to_string())?
                    .key;
                let handle = self.verify(&instance, anchor, &token)?;
                Ok(bot(instance, token, handle, anchor.into()))
            }
        }
    }
}

message! {
    pub struct AdminLogin {
        1 username: String,
        2 password: String,
    }
}
message! {
    pub struct TokenOnly {
        1 access_token: String,
    }
}
message! {
    pub struct NewBot {
        1 username: String,
        2 display_name: String,
        3 password: String,
    }
}
message! {
    pub struct IdOnly {
        1 id: String,
    }
}
message! {
    pub struct KeyRequest {
        1 name: String,
        2 password: String,
    }
}
message! {
    pub struct KeyOnly {
        1 key: String,
    }
}

/// Builds a setup from the admin form (validating before anything is queued).
pub fn setup_from(input: &crate::messages::BotSetup) -> Result<Setup, ApiError> {
    let instance = clean_instance(&input.instance)?;
    if !input.token.trim().is_empty() {
        let anchor = if input.anchor == "household-ca" {
            "household-ca"
        } else {
            "public"
        };
        return Ok(Setup::Token {
            instance,
            token: input.token.trim().into(),
            anchor: anchor.into(),
        });
    }
    let username = input.username.trim().trim_start_matches('@').to_lowercase();
    if username.is_empty()
        || !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        || username.len() > 30
    {
        return Err(ApiError::bad_request("Bot username: letters, digits and _"));
    }
    if input.admin.trim().is_empty() || input.password.is_empty() {
        return Err(ApiError::bad_request("To create a bot, give the mastomini admin's username and password (used once, never stored), or paste an access token"));
    }
    Ok(Setup::Mastomini {
        instance,
        admin: input.admin.trim().into(),
        password: input.password.clone(),
        display_name: if input.display_name.trim().is_empty() {
            "Game Matcher".into()
        } else {
            input.display_name.trim().into()
        },
        username,
    })
}

/// The public bot has independent credentials, queue, and incoming cursor.
struct PublicKv(Arc<dyn Kv>);
impl Kv for PublicKv {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.0.get(&format!("pub_{key}"))
    }
    fn set(&self, key: &str, value: &[u8]) -> std::io::Result<()> {
        self.0.set(&format!("pub_{key}"), value)
    }
    fn remove(&self, key: &str) -> std::io::Result<()> {
        self.0.remove(&format!("pub_{key}"))
    }
}

impl Outbox {
    pub fn open_public(kv: Arc<dyn Kv>, household: Arc<dyn Fetch>, public: Arc<dyn Fetch>) -> Self {
        Self::open(Arc::new(PublicKv(kv)), household, public)
    }

    pub fn retry(&self, id: u32, now: u64) -> Result<(), ApiError> {
        let mut inner = self.lock();
        if inner.active_job == Some(id) {
            return Err(ApiError::new(
                409,
                "busy",
                "This delivery is already being sent",
            ));
        }
        let job = inner
            .outbox
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| ApiError::not_found("No such delivery"))?;
        if job.state == "sent" || (job.state == "queued" && job.attempts == 0) {
            return Err(ApiError::bad_request(
                "Only failed delivery attempts can be retried",
            ));
        }
        job.state = "queued".into();
        job.next_at = now;
        job.retried = now;
        job.attempts = 0;
        job.error.clear();
        self.save(&inner);
        Ok(())
    }

    pub fn reply_settings(&self, enabled: bool) -> Result<(), ApiError> {
        let mut inner = self.lock();
        let bot = inner
            .bot
            .as_mut()
            .ok_or_else(|| ApiError::bad_request("Set up a bot first"))?;
        bot.replies = enabled;
        self.save(&inner);
        Ok(())
    }

    pub fn pending_replies(&self) -> Vec<crate::replies::Mention> {
        let inner = self.lock();
        if inner.bot.as_ref().is_some_and(|b| b.replies) {
            inner.inbox.pending.clone()
        } else {
            vec![]
        }
    }

    pub fn reply_plan(&self, status: &str) -> Option<u32> {
        self.lock()
            .outbox
            .links
            .iter()
            .find(|l| l.status == status)
            .map(|l| l.plan)
    }

    pub fn acknowledge_reply(&self, id: &str) {
        let mut inner = self.lock();
        inner.inbox.pending.retain(|m| m.id != id);
        if let Ok(bytes) = wire::to_vec(Format::CborInt, &inner.inbox) {
            if let Err(e) = self.kv.set("inbox", &bytes) {
                log::warn!("saving replies: {e}");
            }
        }
    }

    fn poll_replies(&self, now: u64) -> bool {
        let (bot, cursor, initialized) = {
            let mut inner = self.lock();
            if now < inner.last_poll + 60 || !inner.inbox.pending.is_empty() {
                return false;
            }
            let Some(bot) = inner.bot.clone().filter(|b| b.replies) else {
                return false;
            };
            inner.last_poll = now;
            (bot, inner.inbox.cursor.clone(), inner.inbox.initialized)
        };
        // Bootstrap at the newest notification, then page forward (not since_id,
        // which can skip a backlog). Only poll on the outbound worker.
        let query = if initialized {
            format!("limit=8&min_id={cursor}")
        } else {
            "limit=1".into()
        };
        let outcome = self
            .call(
                &bot.anchor,
                "GET",
                &format!(
                    "{}/api/v1/notifications?types%5B%5D=mention&{query}",
                    bot.instance
                ),
                &bot.token,
                &[],
            )
            .and_then(|bytes| {
                let mut wrapped = b"{\"items\":".to_vec();
                wrapped.extend_from_slice(&bytes);
                wrapped.push(b'}');
                wire::decode::<crate::replies::Mentions>(Format::Json, &wrapped)
                    .map(|m| m.items)
                    .map_err(|e| e.to_string())
            });
        let mut inner = self.lock();
        if !inner
            .bot
            .as_ref()
            .is_some_and(|b| b.instance == bot.instance && b.token == bot.token && b.replies)
        {
            return true;
        }
        match outcome {
            Ok(mut list) => {
                list.retain(|m| crate::replies::newer(&m.id, &cursor));
                list.sort_by(|a, b| (a.id.len(), &a.id).cmp(&(b.id.len(), &b.id)));
                if list.len() > 8 {
                    inner.reply_error = "Instance ignored the notification page limit".into();
                    return true;
                }
                if initialized && list.is_empty() {
                    inner.reply_error.clear();
                    return true;
                }
                let mut inbox = inner.inbox.clone();
                if let Some(last) = list.last() {
                    inbox.cursor = last.id.clone();
                }
                if inbox.cursor.is_empty() {
                    inbox.cursor = "0".into();
                }
                inbox.initialized = true;
                if initialized {
                    inbox.pending = list;
                }
                match wire::to_vec(Format::CborInt, &inbox)
                    .map_err(|e| e.to_string())
                    .and_then(|b| self.kv.set("inbox", &b).map_err(|e| e.to_string()))
                {
                    Ok(()) => {
                        inner.inbox = inbox;
                        inner.reply_error.clear();
                    }
                    Err(e) => inner.reply_error = e,
                }
            }
            Err(e) => inner.reply_error = e,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniframework::fetch::Fetched;
    use miniframework::kv::MemKv;
    use std::sync::Mutex as StdMutex;

    /// A fake instance: records requests, answers by path.
    #[derive(Default)]
    struct Fake {
        seen: StdMutex<Vec<(String, String, String)>>,
        fail_posts: StdMutex<u32>,
        offline: StdMutex<bool>,
        /// mastomini's bots: (username, password).
        bots: StdMutex<Vec<(String, String)>>,
        /// Fail the next api_keys call (a setup that dies halfway).
        fail_keys: StdMutex<bool>,
    }

    impl Fetch for Fake {
        fn get(&self, url: &str, _: &str, _: usize) -> std::io::Result<Fetched> {
            self.send(&Outgoing {
                method: "GET",
                url,
                headers: &[],
                body: &[],
                limit: 0,
            })
        }
        fn send(&self, r: &Outgoing<'_>) -> std::io::Result<Fetched> {
            let headers = r
                .headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.seen.lock().unwrap().push((
                r.url.to_string(),
                headers,
                String::from_utf8_lossy(r.body).into(),
            ));
            let ok = |body: &str| {
                Ok(Fetched {
                    status: 200,
                    content_type: "application/json".into(),
                    body: body.as_bytes().to_vec(),
                })
            };
            if *self.offline.lock().unwrap() {
                return Err(std::io::Error::other("ESP_ERR_HTTP_CONNECT"));
            }
            if r.url.ends_with("/senders") {
                return ok(
                    r#"{"senders":[{"name":"Dad","email":"dad@example.com","active":true}]}"#,
                );
            }
            if r.url.ends_with("/admin/bots/login") {
                ok(r#"{"access_token":"admintok"}"#)
            } else if r.url.ends_with("/admin/bots") {
                // Like mastomini: same username + password = the same bot.
                let body = String::from_utf8_lossy(r.body).into_owned();
                let field = |k: &str| {
                    let at = body.find(&format!("\"{k}\":\"")).unwrap() + k.len() + 4;
                    body[at..at + body[at..].find('"').unwrap()].to_string()
                };
                let (user, pass) = (field("username"), field("password"));
                let mut bots = self.bots.lock().unwrap();
                match bots.iter().find(|(u, _)| *u == user) {
                    Some((_, p)) if *p != pass => {
                        return Ok(Fetched {
                            status: 401,
                            content_type: String::new(),
                            body: vec![],
                        })
                    }
                    Some(_) => {}
                    None => bots.push((user, pass)),
                }
                ok(r#"{"id":"12","username":"gamematcher"}"#)
            } else if r.url.ends_with("/api_keys") {
                let body = String::from_utf8_lossy(r.body).into_owned();
                if !body.contains(r#""name":"mmb-"#) {
                    return Ok(Fetched {
                        status: 422,
                        content_type: String::new(),
                        body: br#"{"error":"Invalid enrollment key name"}"#.to_vec(),
                    });
                }
                let known = self
                    .bots
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(_, p)| body.contains(p.as_str()));
                if !known || std::mem::take(&mut *self.fail_keys.lock().unwrap()) {
                    return Ok(Fetched {
                        status: 500,
                        content_type: String::new(),
                        body: vec![],
                    });
                }
                ok(r#"{"key":"botkey"}"#)
            } else if r.url.ends_with("/verify_credentials") {
                ok(r#"{"id":"12","username":"gamematcher","acct":"gamematcher"}"#)
            } else if r.url.ends_with("/statuses") {
                let mut fails = self.fail_posts.lock().unwrap();
                if *fails > 0 {
                    *fails -= 1;
                    return Ok(Fetched {
                        status: 503,
                        content_type: String::new(),
                        body: vec![],
                    });
                }
                ok(r#"{"id":"99","url":"https://mastomini.local/@gamematcher/99"}"#)
            } else {
                Ok(Fetched {
                    status: 404,
                    content_type: String::new(),
                    body: vec![],
                })
            }
        }
    }

    fn outbox(fake: Arc<Fake>, kv: Arc<dyn Kv>) -> Outbox {
        Outbox::open(kv, fake.clone(), fake)
    }

    #[test]
    fn creates_a_bot_on_mastomini_without_keeping_the_password() {
        let fake = Arc::new(Fake::default());
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        let o = outbox(fake.clone(), kv.clone());
        let input = crate::messages::BotSetup {
            instance: "https://mastomini.local/".into(),
            admin: "dad".into(),
            password: "hunter2".into(),
            username: "@GameMatcher".into(),
            ..Default::default()
        };
        o.begin_setup(setup_from(&input).unwrap()).unwrap();
        assert_eq!(o.view().setup, "working");
        assert!(o.run_once(100));
        let view = o.view();
        assert!(view.configured, "{}", view.setup);
        assert_eq!(view.handle, "@gamematcher@mastomini.local");
        let seen = fake.seen.lock().unwrap();
        assert!(seen[1].1.contains("Authorization: Bearer admintok"));
        assert!(seen[1].2.contains(r#""username":"gamematcher""#));
        let stored = String::from_utf8_lossy(&kv.get("bot").unwrap()).into_owned();
        assert!(
            stored.contains("botkey") && !stored.contains("hunter2"),
            "only the bot key is kept"
        );
    }

    #[test]
    fn posts_retry_with_the_same_idempotency_key_and_survive_reopen() {
        let fake = Arc::new(Fake::default());
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        let o = outbox(fake.clone(), kv.clone());
        o.begin_setup(Setup::Token {
            instance: "https://mastodon.social".into(),
            token: "t".into(),
            anchor: "public".into(),
        })
        .unwrap();
        o.run_once(0);
        assert!(o.enqueue("post", "New plan: Catan".into(), None, 1000));
        *fake.fail_posts.lock().unwrap() = 1;
        // A reboot between queueing and sending loses nothing.
        let o = outbox(fake.clone(), kv);
        assert!(o.run_once(1000));
        assert_eq!(o.view().jobs[0].state, "queued");
        assert!(!o.run_once(1001), "backing off");
        assert!(o.run_once(1000 + 3600));
        let view = o.view();
        assert_eq!(view.jobs[0].state, "sent");
        assert_eq!(view.posts, 1);
        let seen = fake.seen.lock().unwrap();
        let keys: Vec<&str> = seen
            .iter()
            .filter(|(u, ..)| u.ends_with("/statuses"))
            .map(|(_, h, _)| {
                h.lines()
                    .find(|l| l.starts_with("Idempotency-Key"))
                    .unwrap()
            })
            .collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], keys[1], "a retry can't double-post");
        assert!(seen
            .iter()
            .any(|(_, _, b)| b.contains(r#""visibility":"unlisted""#)));
    }

    #[test]
    fn digest_once_a_day_after_its_time() {
        let fake = Arc::new(Fake::default());
        let o = outbox(fake, Arc::new(MemKv::default()));
        o.begin_setup(Setup::Token {
            instance: "https://m.example".into(),
            token: "t".into(),
            anchor: "public".into(),
        })
        .unwrap();
        o.run_once(0);
        o.settings(None, Some(true), Some("09:00")).unwrap();
        assert!(!o.digest_due("2026-10-18", "08:59"));
        assert!(o.digest_due("2026-10-18", "09:00"));
        assert!(!o.digest_due("2026-10-18", "21:00"), "once a day");
        assert!(o.digest_due("2026-10-19", "09:30"));
        assert_eq!(o.settings(None, None, Some("9am")).unwrap_err().status, 400);
    }

    #[test]
    fn email_setup_waits_for_the_network_after_boot() {
        let fake = Arc::new(Fake::default());
        *fake.offline.lock().unwrap() = true;
        let o = outbox(fake.clone(), Arc::new(MemKv::default()));
        let key = "xkeysib-0123456789abcdefghij";
        o.begin_mail_setup(MailSetup {
            api_key: key.into(),
            built_in: true,
            ..Default::default()
        })
        .unwrap();
        assert!(o.run_once(100));
        assert!(
            !o.mail_ready() && o.mail_view().setup == "working",
            "kept, not failed"
        );
        assert!(!o.run_once(110), "waits 30 s");
        *fake.offline.lock().unwrap() = false;
        assert!(o.run_once(130));
        assert!(o.mail_ready(), "{}", o.mail_view().setup);
        assert_eq!(o.mail_view().sender, "Game Matcher <dad@example.com>");
    }

    #[test]
    fn a_bot_setup_that_fails_halfway_can_be_run_again() {
        let fake = Arc::new(Fake::default());
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        let input = crate::messages::BotSetup {
            instance: "https://mastomini.local".into(),
            admin: "dad".into(),
            password: "hunter2".into(),
            username: "gamematcher".into(),
            ..Default::default()
        };
        *fake.fail_keys.lock().unwrap() = true;
        let o = outbox(fake.clone(), kv.clone());
        o.begin_setup(setup_from(&input).unwrap()).unwrap();
        o.run_once(1);
        assert!(o.view().setup.starts_with("failed"), "{}", o.view().setup);
        // After a reboot, even: the bot's password was saved first.
        let o = outbox(fake.clone(), kv);
        o.begin_setup(setup_from(&input).unwrap()).unwrap();
        o.run_once(2);
        assert!(o.view().configured, "{}", o.view().setup);
        assert_eq!(
            fake.bots.lock().unwrap().len(),
            1,
            "the same bot, not a second one"
        );

        // A bot this board didn't make: a clear message, not "HTTP 401".
        let other = Arc::new(Fake::default());
        other
            .bots
            .lock()
            .unwrap()
            .push(("gamematcher".into(), "someone else's".into()));
        let o = outbox(other, Arc::new(MemKv::default()));
        o.begin_setup(setup_from(&input).unwrap()).unwrap();
        o.run_once(3);
        assert!(
            o.view().setup.contains("already exists on mastomini"),
            "{}",
            o.view().setup
        );
    }
}
