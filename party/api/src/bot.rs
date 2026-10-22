//! The Mastodon bot as a message queue (`spec/04-channels.md`): a batch of
//! mentions becomes commands, run through the same rules as the web app,
//! and one direct reply per sender. The worker fetches the batch; this
//! module only decides.
use crate::dsl::{parse, plain_lines, Cmd, Ref};
use crate::events::SCHEDULED;
use crate::messages::{InboxEntry, Mention, RsvpInput, StoredEvent, StoredJob};
use crate::state::{fold, State, ACTIVE};

/// Commands per DM.
const MAX_LINES: usize = 5;
/// Status ids remembered against replays.
const PROCESSED: usize = 128;
/// `undo` works this long.
const UNDO_SECS: u64 = 3_600;

const HELP: &str = "Commands: events · event E12 · going E12 [+1] · maybe E12 · no E12 · bring E12 pie · ride E12 need 1 · when E12 1,3 · groups · find <words> · join G3 · leave G3 · me · link · stop · start · undo. Ids like E12 are in every message I send.";

/// What a line resolved to, for the reply and the log.
type Line = Result<String, String>;

impl State {
    /// Events `person` can see that haven't happened, soonest first.
    fn upcoming_for(&self, person: u32, now: u64) -> Vec<&StoredEvent> {
        let mut v: Vec<&StoredEvent> = self
            .events
            .iter()
            .filter(|e| e.status == SCHEDULED && (e.polling() || e.effective_end() > now))
            .filter(|e| self.event_access(Some(person), e).see)
            .collect();
        v.sort_by_key(|e| (e.start, e.id));
        v
    }

    fn short(&self, e: &StoredEvent) -> String {
        if e.polling() {
            return format!("E{} {} (time being decided)", e.id, e.title);
        }
        let l = self.zone.local(e.start);
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        format!(
            "E{} {} ({} {}/{} {:02}:{:02})",
            e.id, e.title, DAYS[l.weekday as usize], l.month, l.day, l.hour, l.minute
        )
    }

    /// An event by id, or by words in its title (or a weekday) among what
    /// the person can see. Several matches ask which.
    fn find_event(&self, person: u32, r: &Ref, now: u64) -> Result<u32, String> {
        if let Ref::Id(id) = r {
            return self
                .event(*id)
                .filter(|e| self.event_access(Some(person), e).see)
                .map(|e| e.id)
                .ok_or_else(|| format!("I can't find E{id}"));
        }
        let Ref::Name(words) = r else { unreachable!() };
        const DAYS: [&str; 7] = [
            "sunday",
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
        ];
        let today = self.zone.local(now).days();
        let day = match words.as_str() {
            "today" | "tonight" => Some(today),
            "tomorrow" => Some(today + 1),
            w => DAYS.iter().position(|d| *d == w || d[..3] == *w).map(|wd| {
                let ahead = (wd as i64 - crate::tz::weekday(today) as i64).rem_euclid(7);
                today + ahead
            }),
        };
        let found: Vec<&StoredEvent> = self
            .upcoming_for(person, now)
            .into_iter()
            .filter(|e| match day {
                Some(d) => !e.polling() && self.zone.local(e.start).days() == d,
                None => fold(&e.title).contains(words.as_str()),
            })
            .collect();
        match found.as_slice() {
            [] => Err(format!(
                "no upcoming event matches \"{words}\" (try: events)"
            )),
            [one] => Ok(one.id),
            many => Err(format!(
                "which one? {}",
                many.iter()
                    .take(5)
                    .map(|e| self.short(e))
                    .collect::<Vec<_>>()
                    .join(" · ")
            )),
        }
    }

    fn find_group(&self, person: u32, r: &Ref) -> Result<u32, String> {
        let visible = |g: &&crate::messages::StoredGroup| self.access(Some(person), g).see;
        if let Ref::Id(id) = r {
            return self
                .group(*id)
                .filter(visible)
                .map(|g| g.id)
                .ok_or_else(|| format!("I can't find G{id}"));
        }
        let Ref::Name(words) = r else { unreachable!() };
        let all: Vec<&crate::messages::StoredGroup> = self.groups.iter().filter(visible).collect();
        if let Some(g) = all
            .iter()
            .find(|g| fold(&g.name) == *words || g.slug == *words)
        {
            return Ok(g.id);
        }
        let found: Vec<_> = all
            .iter()
            .filter(|g| fold(&g.name).contains(words.as_str()))
            .collect();
        match found.as_slice() {
            [] => Err(format!("no group matches \"{words}\" (try: find {words})")),
            [one] => Ok(one.id),
            many => Err(format!(
                "which one? {}",
                many.iter()
                    .take(5)
                    .map(|g| format!("G{} {}", g.id, g.name))
                    .collect::<Vec<_>>()
                    .join(" · ")
            )),
        }
    }

    /// The person's personal link for the companion site, making their key
    /// if needed ("" until publication is set up).
    pub fn personal_link(&mut self, person: u32, rotate: bool) -> Result<String, String> {
        let site = self
            .publish()
            .map(|p| p.site_url.clone())
            .unwrap_or_default();
        let Ok(p) = self.person_mut(person) else {
            return Err("unknown person".into());
        };
        if p.personal_key.is_empty() || rotate {
            let mut key = [0u8; 32];
            getrandom::getrandom(&mut key).map_err(|e| e.to_string())?;
            p.personal_key = key.iter().map(|b| format!("{b:02x}")).collect();
        }
        if site.is_empty() {
            return Ok(String::new());
        }
        Ok(format!(
            "{}#/k/{}.{}",
            site.trim_end_matches('/').to_string() + "/",
            person,
            p.personal_key
        ))
    }

    /// Answers through the web app's rules, remembering the old answer for `undo`.
    fn bot_answer(
        &mut self,
        person: u32,
        event: u32,
        change: impl FnOnce(&mut RsvpInput, &StoredEvent),
        now: u64,
    ) -> Line {
        let e = self.event(event).cloned().ok_or("that event is gone")?;
        let current = self.rsvp(event, person).cloned();
        let mut input = RsvpInput {
            status: current
                .as_ref()
                .map(|r| {
                    if r.status == "waitlist" {
                        "going".into()
                    } else {
                        r.status.clone()
                    }
                })
                .unwrap_or_else(|| "going".into()),
            rev_seen: e.rev,
            ..Default::default()
        };
        change(&mut input, &e);
        match self.answer(person, event, person, &input, now) {
            Ok(changed) => {
                if changed {
                    self.undo.insert(person, (event, current, now));
                }
                let r = self.rsvp(event, person).cloned().unwrap_or_default();
                let guests = if r.plus > 0 {
                    format!(" +{}", r.plus)
                } else {
                    String::new()
                };
                let status = match r.status.as_str() {
                    "going" => "going to",
                    "maybe" => "a maybe for",
                    "waitlist" => "on the waitlist for",
                    _ => "not going to",
                };
                Ok(format!("You're {status} {}{guests}.", self.short(&e)))
            }
            Err(err) => Err(err.message),
        }
    }

    /// Runs one command for `person` (None: a sender not linked here).
    pub fn run_command(&mut self, person: Option<u32>, cmd: &Cmd, now: u64) -> Line {
        let Some(me) = person else {
            return match cmd {
                Cmd::Help(_) => Ok(HELP.into()),
                Cmd::Redeem(_) => Err("open the invitation link in a browser to accept it with Mastodon".into()),
                _ => Err("I don't know you yet: accept an invitation first, or link this account on your profile at the house".into()),
            };
        };
        match cmd {
            Cmd::Help(_) => Ok(HELP.into()),
            Cmd::Me => {
                let groups: Vec<String> = self
                    .groups
                    .iter()
                    .filter(|g| self.membership(g, me).is_some_and(|m| m.status == ACTIVE))
                    .map(|g| format!("G{} {}", g.id, g.name))
                    .collect();
                let going: Vec<String> = self
                    .upcoming_for(me, now)
                    .into_iter()
                    .filter(|e| self.rsvp(e.id, me).is_some_and(|r| r.status == "going"))
                    .take(5)
                    .map(|e| self.short(e))
                    .collect();
                let dms = if self.person(me).is_some_and(|p| p.dm_off) {
                    "off"
                } else {
                    "on"
                };
                Ok(format!(
                    "Groups: {}. Going: {}. Messages: {dms}.",
                    if groups.is_empty() {
                        "none".into()
                    } else {
                        groups.join(", ")
                    },
                    if going.is_empty() {
                        "nothing yet".into()
                    } else {
                        going.join(", ")
                    }
                ))
            }
            Cmd::Events(group) => {
                let g = match group {
                    Some(r) => Some(self.find_group(me, r)?),
                    None => None,
                };
                let list: Vec<String> = self
                    .upcoming_for(me, now)
                    .into_iter()
                    .filter(|e| g.is_none_or(|g| e.group == g))
                    .take(8)
                    .map(|e| {
                        let mine = self
                            .rsvp(e.id, me)
                            .map(|r| format!(" [{}]", r.status))
                            .unwrap_or_default();
                        format!("{}{mine}", self.short(e))
                    })
                    .collect();
                Ok(if list.is_empty() {
                    "Nothing coming up.".into()
                } else {
                    list.join(" · ")
                })
            }
            Cmd::Event(r) => {
                let id = self.find_event(me, r, now)?;
                let e = self.event(id).expect("found").clone();
                let mut out = format!("{}. {} going", self.short(&e), self.heads(id));
                if e.capacity > 0 {
                    out.push_str(&format!(" of {}", e.capacity));
                }
                out.push('.');
                if !e.place.is_empty() {
                    out.push_str(&format!(" At {}.", e.place));
                }
                if !e.bring.is_empty() {
                    let items: Vec<String> = e.bring.iter().map(|s| s.name.clone()).collect();
                    out.push_str(&format!(" Bring list: {}.", items.join(", ")));
                }
                if e.polling() {
                    let opts: Vec<String> = e
                        .when
                        .iter()
                        .enumerate()
                        .map(|(i, o)| {
                            format!(
                                "{}) {}",
                                i + 1,
                                self.short(&StoredEvent {
                                    start: o.start,
                                    ..e.clone()
                                })
                                .split(" (")
                                .nth(1)
                                .unwrap_or("")
                                .trim_end_matches(')')
                            )
                        })
                        .collect();
                    out.push_str(&format!(
                        " Times: {}. Answer: when E{id} 1,2",
                        opts.join(" ")
                    ));
                }
                Ok(out)
            }
            Cmd::Groups => {
                let list: Vec<String> = self
                    .groups
                    .iter()
                    .filter(|g| self.membership(g, me).is_some_and(|m| m.status == ACTIVE))
                    .map(|g| format!("G{} {}", g.id, g.name))
                    .collect();
                Ok(if list.is_empty() {
                    "You're not in any groups yet.".into()
                } else {
                    list.join(" · ")
                })
            }
            Cmd::Find(words) => {
                let (found, _) = self.list_groups(Some(me), words, false, 0, 8);
                Ok(if found.is_empty() {
                    format!("No group matches \"{words}\".")
                } else {
                    found
                        .iter()
                        .map(|g| format!("G{} {} ({})", g.id, g.name, g.join))
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
            }
            Cmd::Join(r) => {
                let g = self.find_group(me, r)?;
                let status = self.join_group(me, g, now).map_err(|e| e.message)?;
                let name = self.group(g).map(|g| g.name.clone()).unwrap_or_default();
                Ok(if status == ACTIVE {
                    format!("You're in {name}.")
                } else {
                    format!("Asked to join {name}; an organizer will let you in.")
                })
            }
            Cmd::Leave(r) => {
                let g = self.find_group(me, r)?;
                self.leave_group(me, g).map_err(|e| e.message)?;
                Ok("You've left.".into())
            }
            Cmd::Rsvp {
                event,
                status,
                plus,
                kids,
                note,
            } => {
                let id = self.find_event(me, event, now)?;
                let (status, plus, kids, note) = (status.clone(), *plus, *kids, note.clone());
                self.bot_answer(
                    me,
                    id,
                    |i, _| {
                        i.status = status;
                        i.plus = plus;
                        i.kids = kids;
                        if !note.is_empty() {
                            i.note = Some(note);
                        }
                    },
                    now,
                )
            }
            Cmd::Bring { event, item } | Cmd::Unbring { event, item } => {
                let adding = matches!(cmd, Cmd::Bring { .. });
                let id = self.find_event(me, event, now)?;
                let e = self.event(id).expect("found").clone();
                let mine = self
                    .rsvp(id, me)
                    .map(|r| r.bring.clone())
                    .unwrap_or_default();
                let want = fold(item);
                let slot = e
                    .bring
                    .iter()
                    .find(|s| fold(&s.name) == want)
                    .or_else(|| e.bring.iter().find(|s| fold(&s.name).contains(&want)));
                let item = item.clone();
                self.bot_answer(
                    me,
                    id,
                    |i, _| match slot {
                        Some(s) => {
                            let mut list = mine.clone();
                            list.retain(|x| *x != s.id);
                            if adding {
                                list.push(s.id);
                            }
                            i.bring = Some(list);
                        }
                        None => i.bring_other = Some(if adding { item } else { String::new() }),
                    },
                    now,
                )
            }
            Cmd::Ride { event, kind, seats } => {
                let id = self.find_event(me, event, now)?;
                let (kind, seats) = (kind.clone(), *seats);
                self.bot_answer(
                    me,
                    id,
                    |i, _| {
                        i.ride = Some(kind);
                        i.seats = Some(seats);
                    },
                    now,
                )
            }
            Cmd::When { event, options } => {
                let id = self.find_event(me, event, now)?;
                let e = self.event(id).expect("found").clone();
                let picked: Option<Vec<u32>> = options
                    .iter()
                    .map(|n| e.when.get(*n as usize - 1).map(|o| o.id))
                    .collect();
                let picked = picked.ok_or(format!("pick numbers from 1 to {}", e.when.len()))?;
                self.bot_answer(me, id, |i, _| i.when = Some(picked), now)
            }
            Cmd::Shift { event, name } => {
                let id = self.find_event(me, event, now)?;
                let e = self.event(id).expect("found").clone();
                let want = fold(name);
                let s = e
                    .shifts
                    .iter()
                    .find(|s| fold(&s.name).contains(&want))
                    .ok_or(format!("no shift called \"{name}\""))?;
                let mut list = self
                    .rsvp(id, me)
                    .map(|r| r.shifts.clone())
                    .unwrap_or_default();
                list.push(s.id);
                self.bot_answer(me, id, |i, _| i.shifts = Some(list), now)
            }
            Cmd::Answer {
                event,
                question,
                value,
            } => {
                let id = self.find_event(me, event, now)?;
                let e = self.event(id).expect("found").clone();
                let q = e
                    .questions
                    .get((*question as usize).saturating_sub(1))
                    .ok_or(format!("there are {} questions", e.questions.len()))?;
                let mut answers = self
                    .rsvp(id, me)
                    .map(|r| r.answers.clone())
                    .unwrap_or_default();
                answers.retain(|a| a.question != q.id);
                answers.push(crate::messages::StoredAnswer {
                    question: q.id,
                    value: value.to_lowercase(),
                });
                self.bot_answer(me, id, |i, _| i.answers = Some(answers), now)
            }
            Cmd::Regular { series, on } => {
                let sid = match series {
                    Ref::Id(id) => *id,
                    Ref::Name(_) => {
                        let e = self.find_event(me, series, now)?;
                        self.event(e)
                            .map(|e| e.series)
                            .filter(|s| *s != 0)
                            .ok_or("that event doesn't repeat")?
                    }
                };
                self.set_regular(me, sid, *on, now).map_err(|e| e.message)?;
                Ok(if *on {
                    "You're a regular: going to each one.".into()
                } else {
                    "Not a regular any more.".into()
                })
            }
            Cmd::Redeem(token) => {
                let p = self.redeem(me, token, now).map_err(|e| e.message)?;
                Ok(format!("Welcome to {}!", p.target_name))
            }
            Cmd::Link | Cmd::NewLink => {
                let link = self.personal_link(me, matches!(cmd, Cmd::NewLink))?;
                Ok(if link.is_empty() {
                    "The companion site isn't set up yet.".into()
                } else if matches!(cmd, Cmd::NewLink) {
                    format!("Your new link (the old one stopped working): {link}")
                } else {
                    format!("Your link: {link}")
                })
            }
            Cmd::Stop | Cmd::Start => {
                let off = matches!(cmd, Cmd::Stop);
                self.person_mut(me).map_err(|e| e.message)?.dm_off = off;
                Ok(if off {
                    "No more messages from me (answer anytime).".into()
                } else {
                    "Messages back on.".into()
                })
            }
            Cmd::Undo => {
                let Some((event, before, at)) = self.undo.remove(&me) else {
                    return Err("nothing to undo".into());
                };
                if now > at + UNDO_SECS {
                    return Err("too late to undo (an hour has passed)".into());
                }
                let input = match before {
                    Some(r) => RsvpInput {
                        status: if r.status == "waitlist" {
                            "going".into()
                        } else {
                            r.status
                        },
                        plus: Some(r.plus),
                        kids: Some(r.kids),
                        names: Some(r.names),
                        bring: Some(r.bring),
                        bring_other: Some(r.bring_other),
                        ride: Some(r.ride),
                        seats: Some(r.seats),
                        when: Some(r.when),
                        shifts: Some(r.shifts),
                        note: Some(r.note),
                        answers: Some(r.answers),
                        diet: Some(r.diet),
                        rev_seen: self.event(event).map_or(0, |e| e.rev),
                    },
                    None => RsvpInput {
                        status: "no".into(),
                        rev_seen: self.event(event).map_or(0, |e| e.rev),
                        ..Default::default()
                    },
                };
                self.answer(me, event, me, &input, now)
                    .map_err(|e| e.message)?;
                Ok("Undone.".into())
            }
            Cmd::Confirm(_) | Cmd::Cancel => Err("nothing waiting to be confirmed".into()),
        }
    }

    /// One mention: its lines, run as the sender. Returns the reply job,
    /// or None when nothing should be said. `bot_host` is the bot's instance.
    pub fn handle_mention(&mut self, bot_host: &str, m: &Mention, now: u64) -> Option<StoredJob> {
        let status = m.status.as_ref()?;
        let acct = m.account.acct.to_lowercase();
        let handle = if acct.contains('@') {
            format!("@{acct}")
        } else {
            format!("@{acct}@{bot_host}")
        };
        let person = self
            .person_by_sender(bot_host, &m.account.id, &acct)
            .filter(|p| self.person(*p).is_some_and(|p| p.status == ACTIVE));
        if status.visibility != "direct" {
            // Commands only in private: a public mention gets a nudge, nothing runs.
            return Some(self.reply(&handle, "Send me commands privately (visibility: mentioned people only), so nobody else sees them.".into(), now));
        }
        let lines = plain_lines(&status.content);
        let mut out = Vec::new();
        let mut acks = Vec::new();
        for line in lines.iter().take(MAX_LINES) {
            let result = match parse(line) {
                Ok(cmd) => {
                    let r = self.run_command(person, &cmd, now);
                    acks.push(format!(
                        "{}={}",
                        cmd.canonical(),
                        if r.is_ok() { "ok" } else { "err" }
                    ));
                    r
                }
                Err(why) => {
                    acks.push("?=err".into());
                    Err(why)
                }
            };
            self.inbox_log.push_front(InboxEntry {
                at: now,
                from: handle.clone(),
                text: line.chars().take(120).collect(),
                result: match &result {
                    Ok(s) | Err(s) => s.chars().take(120).collect(),
                },
            });
            self.inbox_log.truncate(50);
            out.push(match result {
                Ok(s) => s,
                Err(e) => format!("Sorry: {e}."),
            });
        }
        if lines.len() > MAX_LINES {
            out.push(format!("(I read the first {MAX_LINES} lines.)"));
        }
        if out.is_empty() {
            out.push(HELP.into());
        }
        let text = format!(
            "{}\n⟨party ack {} {}⟩",
            out.join("\n"),
            m.id,
            acks.join(" ")
        );
        Some(self.reply(&handle, text, now))
    }

    fn reply(&self, handle: &str, text: String, _now: u64) -> StoredJob {
        StoredJob {
            kind: "dm".into(),
            to: handle.into(),
            subject: "reply".into(),
            text: format!("{handle} {text}"),
            priority: 0,
            ..Default::default()
        }
    }

    /// A polled batch: runs new mentions in order and advances the cursor,
    /// all in one commit with their effects. The first poll only finds the
    /// newest notification: nothing from before the bot existed runs.
    pub fn take_mentions(&mut self, mut batch: Vec<Mention>, now: u64) -> usize {
        let Some(bot) = self.bot().cloned() else {
            return 0;
        };
        let host = crate::identity::host_of(&bot.instance).to_string();
        batch.sort_by(|a, b| (a.id.len(), &a.id).cmp(&(b.id.len(), &b.id)));
        let newest = batch.last().map(|m| m.id.clone());
        let first_poll = bot.cursor.is_empty();
        let mut ran = 0;
        if !first_poll {
            for m in &batch {
                if bot.processed.contains(&m.id) || !newer(&m.id, &bot.cursor) {
                    continue;
                }
                if let Some(job) = self.handle_mention(&host, m, now) {
                    self.enqueue(job, now);
                }
                ran += 1;
            }
        }
        let b = self
            .channel_mut(crate::outbound::BOT)
            .bot
            .as_mut()
            .expect("checked above");
        if let Some(n) = newest {
            if newer(&n, &b.cursor) {
                b.cursor = n;
            }
        } else if b.cursor.is_empty() {
            b.cursor = "0".into();
        }
        for m in &batch {
            if !b.processed.contains(&m.id) {
                b.processed.push(m.id.clone());
            }
        }
        let extra = b.processed.len().saturating_sub(PROCESSED);
        b.processed.drain(..extra);
        b.last_poll = now;
        b.error.clear();
        ran
    }
}

/// Decimal snowflake comparison without assuming ids fit an integer.
pub fn newer(a: &str, b: &str) -> bool {
    !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && (a.len(), a) > (b.len(), b)
}
