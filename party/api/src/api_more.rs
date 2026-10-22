//! Routes added in Sprints 6-8 (`spec/06-api.md`): invitations, people
//! without a PIN, Mastodon sign-in and linking, "forgot PIN", and the
//! admin pages for mail, the bot, publication and delivery.
use crate::api::{App, Inner};
use crate::identity::{to_provider, Outcome};
use crate::messages::*;
use crate::outbound::{hint, BOT, MAIL, PUBLISH};
use miniframework::oauth::{OAuth, Poll};
use miniframework::{uptime_ms, wall_ms, ApiError, Reply, Request};
use std::sync::Arc;

/// A "forgot PIN" code sent to someone's email (RAM only).
pub struct Reset {
    person: u32,
    code: String,
    /// Monotonic ms.
    sent: u64,
    tries: u32,
}

const RESET_VALID_MS: u64 = 15 * 60_000;
const RESET_RESEND_MS: u64 = 60_000;
const RESET_TRIES: u32 = 5;

fn wall() -> u64 {
    wall_ms().map_or(0, |ms| ms / 1000)
}

fn parse_id(raw: &str, missing: &str) -> Result<u32, ApiError> {
    raw.parse()
        .map_err(|_| ApiError::not_found(missing.to_string()))
}

fn mail_view(state: &crate::state::State, now: u64) -> MailView {
    let today = (now / 86_400) as u32;
    match state.mail() {
        None => MailView {
            state: "off".into(),
            daily_cap: state.limits.mail_per_day,
            ..Default::default()
        },
        Some(m) => MailView {
            state: m.state.clone(),
            sender_email: m.sender_email.clone(),
            sender_name: m.sender_name.clone(),
            key_hint: hint(&m.api_key),
            error: m.error.clone(),
            sent_today: if m.day == today { m.sent } else { 0 },
            daily_cap: state.limits.mail_per_day,
        },
    }
}

fn bot_view(state: &crate::state::State) -> BotView {
    match state.bot() {
        None => BotView {
            state: "off".into(),
            ..Default::default()
        },
        Some(b) => BotView {
            state: b.state.clone(),
            instance: b.instance.clone(),
            handle: b.handle.clone(),
            commands: b.commands,
            poll_secs: b.poll_secs,
            last_poll: b.last_poll,
            error: b.error.clone(),
            token_hint: hint(&b.token),
        },
    }
}

fn publish_view(state: &crate::state::State) -> PublishView {
    match state.publish() {
        None => PublishView {
            state: "off".into(),
            ..Default::default()
        },
        Some(p) => PublishView {
            state: if !p.last_error.is_empty() {
                "failed".into()
            } else if p.last_published > 0 {
                "published".into()
            } else {
                "waiting".into()
            },
            repo: p.repo.clone(),
            branch: p.branch.clone(),
            site_url: p.site_url.clone(),
            every_minutes: p.every_minutes,
            last_published: p.last_published,
            generation: p.generation,
            files: p.files.len() as u32,
            last_error: p.last_error.clone(),
            token_hint: hint(&p.token),
            last_commit: p.last_commit.clone(),
        },
    }
}

impl App {
    /// Turns on Mastodon sign-in. The caller runs `oauth.run_once()` on a
    /// worker thread (outbound HTTPS must not block serving).
    pub fn attach_oauth(&mut self, oauth: Arc<OAuth>) {
        self.oauth = Some(oauth);
        self.sync_providers();
    }

    /// The board's address as the browser used it, for links and callbacks.
    pub fn set_origin(&self, origin: &str) {
        self.lock().state.origin = origin.trim_end_matches('/').to_string();
    }

    fn sync_providers(&self) {
        if let Some(o) = &self.oauth {
            o.set_providers(
                self.lock()
                    .state
                    .providers()
                    .iter()
                    .map(to_provider)
                    .collect(),
            );
        }
    }

    fn oauth_ref(&self) -> Result<&OAuth, ApiError> {
        self.oauth.as_deref().ok_or_else(|| {
            ApiError::new(
                503,
                "no_oauth",
                "Mastodon sign-in isn't set up on this board",
            )
        })
    }

    /// The instance sends the browser back here; continue on the
    /// "finishing" page, which polls the ticket.
    pub(crate) fn oauth_callback(
        &self,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
    ) -> Result<(), ApiError> {
        let location = match self.oauth_ref().and_then(|o| o.callback(req)) {
            Ok(ticket) => format!("/signin/finishing?t={ticket}"),
            Err(e) => format!(
                "/signin?error={}",
                miniframework::fetch::percent_encode(&e.message)
            ),
        };
        reply.bytes(302, "text/plain; charset=utf-8", b"");
        reply.header("Location", location);
        reply.header("Cache-Control", "no-store");
        Ok(())
    }

    fn start_reset(&self, login: &str) -> Result<(), ApiError> {
        let now = wall();
        let mono = uptime_ms();
        let mut inner = self.lock();
        if !inner.state.mail_ready() {
            return Err(ApiError::new(
                503,
                "no_mail",
                "This board can't send email; ask an admin to set a new PIN",
            ));
        }
        let Some((person, email, name)) = inner
            .state
            .by_login(login)
            .filter(|p| !p.email.is_empty() && p.status == crate::state::ACTIVE)
            .map(|p| (p.id, p.email.clone(), p.display_name.clone()))
        else {
            // Same answer either way, so this can't test who's here.
            return Ok(());
        };
        let mut resets = self.resets.lock().unwrap_or_else(|e| e.into_inner());
        resets.retain(|r| mono.saturating_sub(r.sent) < RESET_VALID_MS);
        if resets
            .iter()
            .any(|r| r.person == person && mono.saturating_sub(r.sent) < RESET_RESEND_MS)
        {
            return Ok(());
        }
        let mut n = [0u8; 4];
        getrandom::getrandom(&mut n)
            .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
        let code = format!("{:06}", u32::from_le_bytes(n) % 1_000_000);
        resets.retain(|r| r.person != person);
        resets.push(Reset {
            person,
            code: code.clone(),
            sent: mono,
            tries: 0,
        });
        drop(resets);
        let (subject, text) = crate::mail::reset_email(&code);
        let sp = inner.state.savepoint();
        inner.state.enqueue(
            StoredJob {
                kind: "mail".into(),
                to: email,
                to_name: name,
                subject,
                text,
                priority: 0,
                ..Default::default()
            },
            now,
        );
        if let Err(e) = Self::commit(&mut inner) {
            inner.state.rollback(sp);
            return Err(e);
        }
        Ok(())
    }

    fn finish_reset(
        &self,
        body: &PinResetConfirm,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
    ) -> Result<Me, ApiError> {
        let mono = uptime_ms();
        let wrong = || {
            ApiError::new(
                403,
                "wrong_code",
                "That code is wrong or expired; ask for a new one",
            )
        };
        let person = self
            .lock()
            .state
            .by_login(&body.login)
            .map(|p| p.id)
            .ok_or_else(wrong)?;
        {
            let mut resets = self.resets.lock().unwrap_or_else(|e| e.into_inner());
            let Some(i) = resets
                .iter()
                .position(|r| r.person == person && mono.saturating_sub(r.sent) < RESET_VALID_MS)
            else {
                return Err(wrong());
            };
            resets[i].tries += 1;
            if resets[i].tries > RESET_TRIES {
                resets.remove(i);
                return Err(wrong());
            }
            if resets[i].code != body.code.trim() {
                return Err(wrong());
            }
            resets.remove(i);
        }
        self.lock().guard.clear(person);
        self.mutate(|inner| {
            let rounds = inner.state.limits.pin_rounds;
            crate::state::check_pin(&body.pin)?;
            let hash = crate::pin::hash(&body.pin, rounds)
                .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
            inner.state.person_mut(person)?.pin = hash;
            Self::sign_in(inner, person, req, reply)?;
            Self::me(&inner.state, person)
        })
    }

    /// The OAuth worker's result for a ticket, acted on once.
    fn oauth_poll(
        &self,
        req: &Request<'_>,
        ticket: &str,
        reply: &mut Reply<'_>,
    ) -> Result<OAuthStatus, ApiError> {
        let now = wall();
        Ok(match self.oauth_ref()?.poll(req, ticket) {
            Poll::Waiting => OAuthStatus {
                state: "waiting".into(),
                ..Default::default()
            },
            Poll::Redirect(url) => OAuthStatus {
                state: "redirect".into(),
                location: url,
                ..Default::default()
            },
            Poll::Failed(why) => OAuthStatus {
                state: "failed".into(),
                message: why,
                ..Default::default()
            },
            Poll::Done(identity, intent) => {
                let result = self.mutate(|inner| {
                    let outcome = inner.state.finish_sign_in(&identity, &intent, now)?;
                    if let Outcome::SignedIn(p) = outcome {
                        Self::sign_in(inner, p, req, reply)?;
                    }
                    Ok(outcome)
                });
                match result {
                    Ok(Outcome::SignedIn(_)) => OAuthStatus {
                        state: "done".into(),
                        location: "/".into(),
                        ..Default::default()
                    },
                    Ok(Outcome::Linked(_)) => OAuthStatus {
                        state: "done".into(),
                        location: "/me".into(),
                        message: "Linked".into(),
                    },
                    Err(e) => OAuthStatus {
                        state: "failed".into(),
                        message: e.message,
                        ..Default::default()
                    },
                }
            }
        })
    }

    pub(crate) fn route_more(
        &self,
        req: &Request<'_>,
        reply: &mut Reply<'_>,
        now: u64,
    ) -> Result<(), ApiError> {
        let rest = req.path.trim_start_matches("/api/v1/");
        let parts: Vec<&str> = rest.split('/').collect();
        match (req.method, parts.as_slice()) {
            // ---- sign-in -----------------------------------------------------------
            ("POST", ["auth", "oauth", "start"]) => {
                let start: OAuthStart = req.decode()?;
                let intent = match start.purpose.as_str() {
                    "link" => format!("link:{}", self.caller(req)?.id),
                    "redeem" => {
                        if !self.lock().state.preview_invite(&start.token, now).valid {
                            return Err(ApiError::new(
                                410,
                                "invite_closed",
                                "This invitation can't be used",
                            ));
                        }
                        format!("redeem:{}", start.token.trim())
                    }
                    _ => "signin".into(),
                };
                self.lock().state.provider(start.provider)?;
                let host = req.header("Host").trim();
                let origin = format!("{}://{host}", if req.secure { "https" } else { "http" });
                let oauth = self.oauth_ref()?;
                let ticket = oauth.start(&start.provider.to_string(), &origin, intent)?;
                reply.header("Set-Cookie", oauth.cookie(&ticket, req.secure));
                reply.wire(req, &OAuthTicket { ticket });
            }
            ("GET", ["auth", "oauth", ticket]) => {
                let status = self.oauth_poll(req, ticket, reply)?;
                reply.wire(req, &status);
            }
            ("POST", ["auth", "pin-reset"]) => {
                let body: PinResetStart = req.decode()?;
                self.start_reset(&body.login)?;
                reply.wire(req, &Done { ok: true });
            }
            ("POST", ["auth", "pin-reset", "confirm"]) => {
                let body: PinResetConfirm = req.decode()?;
                let me = self.finish_reset(&body, req, reply)?;
                reply.wire(req, &me);
            }
            ("DELETE", ["me", "identities"]) => {
                let c = self.caller(req)?;
                let handle = req.param("handle").unwrap_or_default().to_lowercase();
                let me = self.mutate(|inner| {
                    inner.state.unlink_identity(c.id, &handle, now)?;
                    Self::me(&inner.state, c.id)
                })?;
                reply.wire(req, &me);
            }
            ("POST", ["me", "link"]) => {
                let c = self.caller(req)?;
                let rotate = req.param("rotate").is_some_and(|v| v != "0");
                let me = self.mutate(|inner| {
                    inner
                        .state
                        .personal_link(c.id, rotate)
                        .map_err(ApiError::bad_request)?;
                    Self::me(&inner.state, c.id)
                })?;
                reply.wire(req, &me);
            }

            // ---- people and invitations --------------------------------------------
            ("POST", ["people"]) => {
                let c = self.caller(req)?;
                let input: NewPerson = req.decode()?;
                let view = self.mutate(|inner| {
                    let id = inner.state.add_person(c.id, &input, now)?;
                    let p = inner.state.person(id).expect("just made");
                    Ok(PersonView {
                        id,
                        display_name: p.display_name.clone(),
                        bio: String::new(),
                        household: p.household,
                        household_name: inner.state.household_name(p.household),
                    })
                })?;
                reply.wire_status(req, 201, &view);
            }
            ("POST", ["invitations"]) => {
                let c = self.caller(req)?;
                let input: InviteInput = req.decode()?;
                let view = self.mutate(|inner| inner.state.create_invite(c.id, &input, now))?;
                reply.wire_status(req, 201, &view);
            }
            ("GET", ["invitations"]) => {
                let c = self.caller(req)?;
                let kind = req.param("kind").unwrap_or_default();
                let target = req.param_num::<u32>("target")?.unwrap_or(0);
                let invites = self.lock().state.list_invites(c.id, &kind, target, now)?;
                reply.wire(req, &InviteList { invites });
            }
            ("GET", ["invitations", token]) => {
                let inner = self.lock();
                let mut preview = inner.state.preview_invite(token, now);
                if self.oauth.is_some() {
                    preview.providers = inner
                        .state
                        .providers()
                        .iter()
                        .map(|p| ProviderButton {
                            id: p.id,
                            name: p.name.clone(),
                        })
                        .collect();
                }
                reply.wire(req, &preview);
            }
            ("POST", ["invitations", token, "redeem"]) => {
                let c = self.caller(req)?;
                let preview =
                    self.mutate(|inner: &mut Inner| inner.state.redeem(c.id, token, now))?;
                reply.wire(req, &preview);
            }
            ("POST", ["invitations", id, "revoke"]) => {
                let c = self.caller(req)?;
                let id = parse_id(id, "No such invitation")?;
                self.mutate(|inner| inner.state.revoke_invite(c.id, id, now))?;
                reply.wire(req, &Done { ok: true });
            }

            // ---- sign-in providers ----------------------------------------------------
            ("GET", ["admin", "providers"]) => {
                self.admin(req)?;
                let providers = self.lock().state.providers().to_vec();
                reply.wire(req, &ProviderList { providers });
            }
            ("POST", ["admin", "providers"]) => {
                let c = self.admin(req)?;
                let input: ProviderInput = req.decode()?;
                self.mutate(|inner| inner.state.add_provider(c.id, &input, now))?;
                self.sync_providers();
                let providers = self.lock().state.providers().to_vec();
                reply.wire_status(req, 201, &ProviderList { providers });
            }
            ("DELETE", ["admin", "providers", id]) => {
                let c = self.admin(req)?;
                let id = parse_id(id, "No such instance")?;
                self.mutate(|inner| inner.state.delete_provider(c.id, id, now))?;
                self.sync_providers();
                reply.wire(req, &Done { ok: true });
            }

            // ---- channels --------------------------------------------------------------
            ("GET", ["admin", "mail"]) => {
                self.admin(req)?;
                reply.wire(req, &mail_view(&self.lock().state, now));
            }
            ("PUT", ["admin", "mail"]) => {
                let c = self.admin(req)?;
                let setup: MailSetup = req.decode()?;
                let key = setup.api_key.trim();
                if key.starts_with("xsmtpsib-") {
                    return Err(ApiError::bad_request(
                        "That's a Brevo SMTP key; party needs an API key (xkeysib-…)",
                    ));
                }
                if key.is_empty() {
                    return Err(ApiError::bad_request("Paste a Brevo API key (xkeysib-…)"));
                }
                let view = self.mutate(|inner| {
                    let s = &mut inner.state;
                    s.channel_mut(MAIL).mail = Some(StoredMail {
                        api_key: key.into(),
                        sender_email: setup.sender_email.trim().to_lowercase(),
                        sender_name: if setup.sender_name.trim().is_empty() {
                            "party".into()
                        } else {
                            setup.sender_name.trim().into()
                        },
                        state: "checking".into(),
                        ..Default::default()
                    });
                    s.log(c.id, "mail.setup", "", now);
                    Ok(mail_view(s, now))
                })?;
                reply.wire(req, &view);
            }
            ("DELETE", ["admin", "mail"]) => {
                let c = self.admin(req)?;
                self.mutate(|inner| {
                    inner.state.channel_mut(MAIL).mail = None;
                    inner.state.log(c.id, "mail.remove", "", now);
                    Ok(())
                })?;
                reply.wire(req, &Done { ok: true });
            }
            ("GET", ["admin", "bot"]) => {
                self.admin(req)?;
                reply.wire(req, &bot_view(&self.lock().state));
            }
            ("PUT", ["admin", "bot"]) => {
                let c = self.admin(req)?;
                let setup: BotSetup = req.decode()?;
                let instance = crate::identity::clean_base(&setup.instance)?;
                let trust = if setup.trust == "household" {
                    "household"
                } else {
                    "public"
                };
                let view = self.mutate(|inner| {
                    let s = &mut inner.state;
                    let old = s.bot().cloned().unwrap_or_default();
                    let token = if setup.token.trim().is_empty() {
                        old.token.clone()
                    } else {
                        setup.token.trim().to_string()
                    };
                    if token.is_empty() {
                        return Err(ApiError::bad_request(
                            "Paste the bot account's access token",
                        ));
                    }
                    let same = old.instance == instance && old.token == token;
                    s.channel_mut(BOT).bot = Some(StoredBot {
                        instance,
                        trust: trust.into(),
                        token,
                        commands: setup.commands,
                        poll_secs: setup.poll_secs.clamp(30, 600),
                        state: if same && old.state == "ready" {
                            "ready".into()
                        } else {
                            "checking".into()
                        },
                        handle: if same { old.handle } else { String::new() },
                        account_id: if same { old.account_id } else { String::new() },
                        // A new account starts afresh: nothing from before it runs.
                        cursor: if same { old.cursor } else { String::new() },
                        processed: if same { old.processed } else { Vec::new() },
                        ..Default::default()
                    });
                    s.log(c.id, "bot.setup", "", now);
                    Ok(bot_view(s))
                })?;
                reply.wire(req, &view);
            }
            ("DELETE", ["admin", "bot"]) => {
                let c = self.admin(req)?;
                self.mutate(|inner| {
                    inner.state.channel_mut(BOT).bot = None;
                    inner.state.log(c.id, "bot.remove", "", now);
                    Ok(())
                })?;
                reply.wire(req, &Done { ok: true });
            }
            ("GET", ["admin", "delivery"]) => {
                self.admin(req)?;
                let inner = self.lock();
                let dropped = inner.state.channels.get(MAIL).map_or(0, |c| c.dropped);
                reply.wire(
                    req,
                    &Delivery {
                        jobs: inner.state.delivery(),
                        dropped,
                    },
                );
            }
            ("POST", ["admin", "delivery", id, "retry"]) => {
                self.admin(req)?;
                let id = parse_id(id, "No such delivery")?;
                let retried = self.mutate(|inner| Ok(inner.state.retry_job(id, now)))?;
                if !retried {
                    return Err(ApiError::new(
                        409,
                        "not_failed",
                        "Only failed deliveries can be retried",
                    ));
                }
                reply.wire(req, &Done { ok: true });
            }
            ("GET", ["admin", "inbox"]) => {
                self.admin(req)?;
                let entries = self.lock().state.inbox_log.iter().cloned().collect();
                reply.wire(req, &InboxLog { entries });
            }
            ("GET", ["admin", "publish"]) => {
                self.admin(req)?;
                reply.wire(req, &publish_view(&self.lock().state));
            }
            ("PUT", ["admin", "publish"]) => {
                let c = self.admin(req)?;
                let setup: PublishSetup = req.decode()?;
                let repo = setup.repo.trim().trim_matches('/').to_string();
                if repo.split('/').count() != 2 || repo.contains(' ') {
                    return Err(ApiError::bad_request("The data repo is owner/name"));
                }
                let site = setup.site_url.trim().to_string();
                if !site.is_empty() && !site.starts_with("https://") {
                    return Err(ApiError::bad_request(
                        "The site's address starts with https://",
                    ));
                }
                let view = self.mutate(|inner| {
                    let s = &mut inner.state;
                    let old = s.publish().cloned().unwrap_or_default();
                    let token = if setup.token.trim().is_empty() {
                        old.token.clone()
                    } else {
                        setup.token.trim().to_string()
                    };
                    s.channel_mut(PUBLISH).publish = Some(StoredPublish {
                        branch: if setup.branch.trim().is_empty() {
                            "main".into()
                        } else {
                            setup.branch.trim().into()
                        },
                        token,
                        site_url: site,
                        every_minutes: setup.every_minutes.clamp(10, 1_440),
                        files: if old.repo == repo {
                            old.files
                        } else {
                            Vec::new()
                        },
                        generation: old.generation,
                        repo,
                        asked: true,
                        ..Default::default()
                    });
                    s.log(c.id, "publish.setup", "", now);
                    Ok(publish_view(s))
                })?;
                reply.wire(req, &view);
            }
            ("POST", ["admin", "publish", "now"]) => {
                self.admin(req)?;
                let view = self.mutate(|inner| {
                    let s = &mut inner.state;
                    match s.channel_mut(PUBLISH).publish.as_mut() {
                        Some(p) if now >= p.last_attempt + 60 => p.asked = true,
                        Some(_) => {
                            return Err(ApiError::new(
                                429,
                                "too_soon",
                                "Published a moment ago; try in a minute",
                            ))
                        }
                        None => {
                            return Err(ApiError::new(
                                409,
                                "not_set_up",
                                "Set publication up first",
                            ))
                        }
                    }
                    Ok(publish_view(s))
                })?;
                reply.wire(req, &view);
            }
            ("DELETE", ["admin", "publish"]) => {
                let c = self.admin(req)?;
                self.mutate(|inner| {
                    inner.state.channel_mut(PUBLISH).publish = None;
                    inner.state.log(c.id, "publish.remove", "", now);
                    Ok(())
                })?;
                reply.wire(req, &Done { ok: true });
            }
            ("GET", ["sync", "status"]) => {
                let inner = self.lock();
                let s = &inner.state;
                reply.wire(
                    req,
                    &SyncStatus {
                        published: s.publish().map_or(0, |p| p.last_published),
                        generation: s.publish().map_or(0, |p| p.generation),
                        bot_polled: s.bot().map_or(0, |b| b.last_poll),
                        site_url: s.publish().map(|p| p.site_url.clone()).unwrap_or_default(),
                    },
                );
            }
            _ => return Err(ApiError::not_found("No such API route")),
        }
        Ok(())
    }
}
