//! Notifications (`spec/03-domain.md#notifications`): one inbox record per
//! person (`n<id>`), newest first, at most `Limits::notices`, coalesced per
//! event while unread.
//!
//! "A new event in your group" is not stored: the summary counts upcoming
//! events a person can answer but hasn't, so creating an event in a group of
//! 500 writes one record, not 500 inboxes.
use crate::messages::{NoticeView, StoredInbox, StoredJob, StoredNotice, Summary};
use crate::state::{State, ACTIVE, DISABLED};

/// Kinds that also go out by email and Mastodon DM.
const DELIVERED: &[&str] = &[
    "invited",
    "changed",
    "cancelled",
    "reminder",
    "promoted",
    "answered_for",
];

impl State {
    /// Tells `to` about `kind` on `event`, done by `actor` (0 = the board).
    /// Nobody is told about their own actions; disabled people collect nothing.
    pub fn notify(&mut self, to: u32, kind: &str, event: u32, actor: u32, now: u64) {
        if to == actor || !self.person(to).is_some_and(|p| p.status != DISABLED) {
            return;
        }
        let group = self.event(event).map_or(0, |e| e.group);
        // People without a PIN yet (email-only) have no inbox to read, but
        // still get email.
        let fresh = !self.person(to).is_some_and(|p| p.status == ACTIVE)
            || self.notify_group(to, kind, event, group, actor, now);
        if fresh && DELIVERED.contains(&kind) {
            self.deliver(to, kind, event, actor, now);
        }
    }

    /// Queues the email and DM for a new notice, by the person's settings.
    fn deliver(&mut self, to: u32, kind: &str, event: u32, actor: u32, now: u64) {
        let (Some(p), Some(e)) = (self.person(to).cloned(), self.event(event).cloned()) else {
            return;
        };
        let origin = self.origin.clone();
        if !p.email.is_empty() && !p.email_off && self.mail().is_some() {
            if let Some((subject, text)) = crate::mail::notice_email(self, kind, &e, actor, &origin)
            {
                let organizer = self
                    .mail()
                    .map(|m| m.sender_email.clone())
                    .unwrap_or_default();
                let ics = if matches!(kind, "invited" | "changed" | "cancelled") && !e.polling() {
                    crate::mail::invitation(self, &e, &p.email, &p.display_name, &organizer)
                } else {
                    String::new()
                };
                let job = StoredJob {
                    kind: "mail".into(),
                    to: p.email.clone(),
                    to_name: p.display_name.clone(),
                    subject,
                    text,
                    ics_name: if ics.is_empty() {
                        String::new()
                    } else {
                        format!("party-{event}.ics")
                    },
                    ics,
                    reply_to: crate::mail::reply_to(self, &e),
                    priority: if kind == "invited" { 1 } else { 2 },
                    ..Default::default()
                };
                self.enqueue(job, now);
            }
        }
        if let Some(id) = p.identities.first() {
            if !p.dm_off && self.bot().is_some() {
                if let Some((subject, _)) =
                    crate::mail::notice_email(self, kind, &e, actor, &origin)
                {
                    let text = format!(
                        "{} {subject}. Reply \"going E{event}\", \"maybe E{event}\" or \"no E{event}\", or \"help\".",
                        id.handle
                    );
                    let job = StoredJob {
                        kind: "dm".into(),
                        to: id.handle.clone(),
                        to_name: p.display_name.clone(),
                        subject: subject.clone(),
                        text,
                        priority: 2,
                        ..Default::default()
                    };
                    self.enqueue(job, now);
                }
            }
        }
    }

    /// As [`State::notify`], for something about a group (join requests).
    /// Returns whether it's a new notice (not coalesced into an unread one).
    pub fn notify_group(
        &mut self,
        to: u32,
        kind: &str,
        event: u32,
        group: u32,
        actor: u32,
        now: u64,
    ) -> bool {
        if to == actor || !self.person(to).is_some_and(|p| p.status == ACTIVE) {
            return false;
        }
        let limit = self.limits.notices.max(1);
        let id = self.next_notice;
        let coalesces = kind != "reminder";
        match self.inboxes.get_mut(to) {
            Some(inbox) => {
                let list = &mut inbox.notices;
                let same = list.iter().position(|n| {
                    coalesces && !n.read && n.kind == kind && n.event == event && n.group == group
                });
                if let Some(i) = same {
                    let mut n = list.remove(i);
                    n.count += 1;
                    n.actor = actor;
                    n.created = now;
                    list.insert(0, n);
                    return false;
                }
                list.insert(
                    0,
                    StoredNotice {
                        id,
                        kind: kind.into(),
                        event,
                        group,
                        actor,
                        created: now,
                        read: false,
                        count: 1,
                    },
                );
                list.truncate(limit);
            }
            None => self.inboxes.push(StoredInbox {
                id: to,
                notices: vec![StoredNotice {
                    id,
                    kind: kind.into(),
                    event,
                    group,
                    actor,
                    created: now,
                    read: false,
                    count: 1,
                }],
            }),
        }
        self.next_notice += 1;
        true
    }

    /// Marks notifications read, up to and including `up_to` (0 = all).
    /// Touches the inbox only if something changes.
    pub fn mark_read(&mut self, person: u32, up_to: u32) -> bool {
        let unread = |n: &StoredNotice| !n.read && (up_to == 0 || n.id <= up_to);
        if !self
            .inboxes
            .get(person)
            .is_some_and(|i| i.notices.iter().any(unread))
        {
            return false;
        }
        let inbox = self.inboxes.get_mut(person).expect("checked above");
        for n in &mut inbox.notices {
            if unread(n) {
                n.read = true;
            }
        }
        true
    }

    pub fn notices(&self, person: u32) -> Vec<NoticeView> {
        let Some(inbox) = self.inboxes.get(person) else {
            return Vec::new();
        };
        inbox
            .notices
            .iter()
            .map(|n| NoticeView {
                id: n.id,
                kind: n.kind.clone(),
                event: n.event,
                event_title: self
                    .event(n.event)
                    .map(|e| e.title.clone())
                    .unwrap_or_default(),
                group: n.group,
                group_name: self
                    .group(n.group)
                    .map(|g| g.name.clone())
                    .unwrap_or_default(),
                actor_name: if n.actor == 0 {
                    String::new()
                } else {
                    self.name(n.actor)
                },
                created: n.created,
                read: n.read,
                count: n.count,
            })
            .collect()
    }

    pub fn summary(&self, person: u32, now: u64) -> Summary {
        let inbox = self.inboxes.get(person);
        let awaiting = self
            .events
            .iter()
            .filter(|e| !e.locked(now) && self.rsvp(e.id, person).is_none())
            .filter(|e| self.eligible(person, e))
            .count() as u32;
        Summary {
            unread: inbox.map_or(0, |i| i.notices.iter().filter(|n| !n.read).count() as u32),
            awaiting,
            latest: inbox.and_then(|i| i.notices.first()).map_or(0, |n| n.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::{Limits, State};

    #[test]
    fn coalesces_while_unread_and_keeps_the_newest() {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            notices: 3,
            ..Limits::board()
        });
        let dad = s.setup("Dad", "", "1234", 0).unwrap();
        let kid = s.signup("Kid", "", "1234", 0).unwrap();
        s.notify_group(dad, "request", 0, 1, kid, 1);
        s.notify_group(dad, "request", 0, 1, kid, 2);
        assert_eq!(s.notices(dad).len(), 1);
        assert_eq!(s.notices(dad)[0].count, 2);
        s.notify_group(dad, "request", 0, 1, kid, 2);
        assert!(s.mark_read(dad, 0));
        assert!(!s.mark_read(dad, 0), "nothing left to mark: no write");
        s.notify_group(dad, "request", 0, 1, kid, 3);
        assert_eq!(s.notices(dad).len(), 2, "a read one isn't coalesced into");
        for g in 2..6 {
            s.notify_group(dad, "request", 0, g, kid, 4);
        }
        assert_eq!(s.notices(dad).len(), 3, "the ring keeps the newest");
        s.notify_group(kid, "request", 0, 1, kid, 5);
        assert!(
            s.notices(kid).is_empty(),
            "nobody is told about their own action"
        );
    }
}
