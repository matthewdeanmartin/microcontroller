//! In-app notifications (`spec/03-domain.md#notifications`).
//!
//! Each person has a short ring, newest first. While a notification is
//! unread, another event of the same kind on the same plan bumps its count
//! instead of adding a row ("3 new comments on Game night"), so nobody gets
//! the same news twice.
use crate::messages::StoredNotification;
use crate::plans::ON;
use crate::state::State;

pub const NEW_PLAN: &str = "new_plan";
pub const RESPONSES: &str = "responses";
pub const LOCKED: &str = "locked";
pub const REOPENED: &str = "reopened";
pub const CHANGED: &str = "changed";
pub const CANCELLED: &str = "cancelled";
pub const COMMENTS: &str = "comments";
pub const HOST: &str = "host";
pub const REMINDER: &str = "reminder";

/// Reminders go out this long before a locked-in plan starts.
pub const REMIND_BEFORE: u64 = 15 * 60;

impl State {
    /// Tells `to` about `kind` on `plan`, done by `actor` (0 = the board).
    /// Nobody is told about their own actions, and disabled accounts
    /// collect nothing.
    pub fn notify(&mut self, to: u32, kind: &str, plan: u32, actor: u32, now: u64) {
        if to == actor {
            return;
        }
        let limit = self.limits.notifications.max(1);
        let id = self.next_notification;
        if !self.account(to).is_some_and(|a| !a.disabled) {
            return;
        }
        let Some(account) = self.accounts.get_mut(to) else {
            return;
        };
        let list = &mut account.notifications;
        if let Some(i) = list.iter().position(|n| {
            !n.read && n.kind == kind && n.plan == plan && kind != REMINDER && kind != CHANGED
        }) {
            let mut n = list.remove(i);
            n.count += 1;
            n.actor = actor;
            n.created = now;
            list.insert(0, n);
            return;
        }
        list.insert(
            0,
            StoredNotification {
                id,
                kind: kind.to_string(),
                plan,
                count: 1,
                actor,
                created: now,
                read: false,
            },
        );
        list.truncate(limit);
        self.next_notification += 1;
        // Queue delivery independently of email preferences: reminders and
        // updates also reach linked Mastodon accounts.
        if kind == REMINDER
            || kind == CHANGED
            || (crate::mail::EMAILED.contains(&kind)
                && !account.email.is_empty()
                && !account.email_off)
        {
            self.mail.push(crate::mail::Pending {
                to,
                kind: kind.to_string(),
                plan,
                actor,
            });
        }
    }

    fn notify_all(&mut self, to: &[u32], kind: &str, plan: u32, actor: u32, now: u64) {
        for &account in to {
            self.notify(account, kind, plan, actor, now);
        }
    }

    /// Everyone with a stake in a plan: its host and whoever said in or maybe.
    pub fn engaged(&self, plan: u32) -> Vec<u32> {
        let Ok(p) = self.plan(plan) else {
            return Vec::new();
        };
        let mut out = vec![p.host];
        for r in &p.responses {
            if (r.overall == "in" || r.overall == "maybe") && !out.contains(&r.account) {
                out.push(r.account);
            }
        }
        out
    }

    /// A new plan: tell everyone into one of its games.
    pub fn on_plan_created(&mut self, plan: u32, actor: u32, now: u64) {
        let Ok(p) = self.plan(plan) else { return };
        let games = p.games.clone();
        let fans: Vec<u32> = self
            .accounts
            .iter()
            .filter(|a| a.interests.iter().any(|g| games.contains(g)))
            .map(|a| a.id)
            .collect();
        self.notify_all(&fans, NEW_PLAN, plan, actor, now);
    }

    /// Something happened to a plan everyone engaged should hear about.
    pub fn on_plan_event(&mut self, plan: u32, kind: &str, actor: u32, now: u64) {
        let to = self.engaged(plan);
        self.notify_all(&to, kind, plan, actor, now);
    }

    /// Someone answered: the host hears (coalesced).
    pub fn on_response(&mut self, plan: u32, actor: u32, now: u64) {
        if let Ok(host) = self.plan(plan).map(|p| p.host) {
            self.notify(host, RESPONSES, plan, actor, now);
        }
    }

    /// Reminders for locked-in plans starting within [`REMIND_BEFORE`]:
    /// everyone who said in, and the host. Returns whether any went out.
    pub fn send_reminders(&mut self, now: u64) -> bool {
        let due: Vec<u32> = self
            .plans
            .iter()
            .filter(|p| p.status == ON && !p.reminded)
            .filter(|p| {
                p.times
                    .iter()
                    .find(|t| Some(t.id) == p.chosen_time)
                    .is_some_and(|t| t.start > now && t.start - now <= REMIND_BEFORE)
            })
            .map(|p| p.id)
            .collect();
        for &plan in &due {
            let p = self.plan(plan).expect("listed above");
            let mut to = vec![p.host];
            to.extend(
                p.responses
                    .iter()
                    .filter(|r| r.overall == "in")
                    .map(|r| r.account),
            );
            to.sort_unstable();
            to.dedup();
            self.notify_all(&to, REMINDER, plan, 0, now);
            if let Some(p) = self.plans.get_mut(plan) {
                p.reminded = true;
            }
        }
        !due.is_empty()
    }

    /// Marks `account`'s notifications read, up to and including `up_to`
    /// (ids grow over time), or all of them when `up_to` is 0.
    pub fn mark_read(&mut self, account: u32, up_to: u32) -> bool {
        let unread = |a: &crate::messages::StoredAccount| {
            a.notifications
                .iter()
                .any(|n| !n.read && (up_to == 0 || n.id <= up_to))
        };
        // Only an account with something to mark is touched (and saved).
        if !self.account(account).is_some_and(unread) {
            return false;
        }
        let Some(a) = self.accounts.get_mut(account) else {
            return false;
        };
        let mut changed = false;
        for n in &mut a.notifications {
            if !n.read && (up_to == 0 || n.id <= up_to) {
                n.read = true;
                changed = true;
            }
        }
        changed
    }

    pub fn unread(&self, account: u32) -> u32 {
        self.account(account).map_or(0, |a| {
            a.notifications.iter().filter(|n| !n.read).count() as u32
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{GameInput, LockIn, PlanInput, ResponseInput, TimeAnswer, TimeInput};
    use crate::state::Limits;

    const SAT: u64 = 1_793_000_000;

    fn world() -> (State, u32, u32, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            notifications: 3,
            ..Limits::board()
        });
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let kid = s.signup("kid@h", "Sam", "1234", 0).unwrap();
        let mom = s.signup("mom@h", "Mom", "1234", 0).unwrap();
        let game = s
            .add_game(
                dad,
                &GameInput {
                    title: Some("Catan".into()),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        (s, dad, kid, mom, game)
    }

    fn kinds(s: &State, who: u32) -> Vec<(String, u32)> {
        s.account(who)
            .unwrap()
            .notifications
            .iter()
            .map(|n| (n.kind.clone(), n.count))
            .collect()
    }

    fn plan(s: &mut State, host: u32, game: u32) -> u32 {
        let input = PlanInput {
            games: Some(vec![game]),
            times: Some(vec![TimeInput {
                id: 0,
                start: SAT,
                end: 0,
            }]),
            ..Default::default()
        };
        s.create_plan(host, &input, 1).unwrap()
    }

    fn answer(overall: &str) -> ResponseInput {
        ResponseInput {
            overall: overall.into(),
            ..Default::default()
        }
    }

    #[test]
    fn new_plan_tells_fans_but_not_the_host() {
        let (mut s, dad, kid, mom, game) = world();
        s.set_interest(mom, game, true).unwrap();
        s.set_interest(kid, game, true).unwrap();
        plan(&mut s, kid, game);
        assert_eq!(kinds(&s, mom), vec![(NEW_PLAN.into(), 1)]);
        assert!(kinds(&s, kid).is_empty(), "the host made it");
        assert!(kinds(&s, dad).is_empty(), "not into Catan");
    }

    #[test]
    fn responses_coalesce_while_unread() {
        let (mut s, dad, kid, mom, game) = world();
        let p = plan(&mut s, kid, game);
        s.respond(dad, p, &answer("in"), 2).unwrap();
        s.respond(mom, p, &answer("maybe"), 3).unwrap();
        s.respond(mom, p, &answer("in"), 4).unwrap();
        assert_eq!(kinds(&s, kid), vec![(RESPONSES.into(), 3)]);
        assert_eq!(s.unread(kid), 1);
        assert!(s.mark_read(kid, 0));
        assert_eq!(s.unread(kid), 0);
        s.respond(dad, p, &answer("out"), 5).unwrap();
        assert_eq!(kinds(&s, kid).len(), 2, "read ones don't coalesce");
    }

    #[test]
    fn lock_and_cancel_reach_the_engaged() {
        let (mut s, dad, kid, mom, game) = world();
        let p = plan(&mut s, kid, game);
        s.respond(dad, p, &answer("in"), 2).unwrap();
        s.respond(mom, p, &answer("out"), 2).unwrap();
        let t = s.plan(p).unwrap().times[0].id;
        s.lock_plan(
            kid,
            p,
            &LockIn {
                time: t,
                games: vec![],
            },
            3,
        )
        .unwrap();
        assert_eq!(kinds(&s, dad), vec![(LOCKED.into(), 1)]);
        assert!(kinds(&s, mom).is_empty(), "said out");
        s.cancel_plan(dad, p, 4).unwrap();
        assert_eq!(kinds(&s, kid)[0].0, CANCELLED, "host hears an admin cancel");
        assert_eq!(
            kinds(&s, dad)[0].0,
            LOCKED,
            "not told about their own cancel"
        );
    }

    #[test]
    fn ring_is_bounded() {
        let (mut s, _dad, kid, mom, game) = world();
        s.set_interest(mom, game, true).unwrap();
        for _ in 0..5 {
            let p = plan(&mut s, kid, game);
            s.cancel_plan(kid, p, 9).unwrap();
        }
        assert_eq!(kinds(&s, mom).len(), 3);
    }

    #[test]
    fn reminders_once_fifteen_minutes_before() {
        let (mut s, dad, kid, mom, game) = world();
        let p = plan(&mut s, kid, game);
        let t = s.plan(p).unwrap().times[0].id;
        s.respond(
            dad,
            p,
            &ResponseInput {
                overall: "in".into(),
                times: vec![TimeAnswer {
                    time: t,
                    answer: "yes".into(),
                }],
                ..Default::default()
            },
            2,
        )
        .unwrap();
        s.respond(mom, p, &answer("maybe"), 2).unwrap();
        s.lock_plan(
            kid,
            p,
            &LockIn {
                time: t,
                games: vec![],
            },
            3,
        )
        .unwrap();
        s.mark_read(dad, 0);
        s.mark_read(kid, 0);
        assert!(!s.send_reminders(SAT - REMIND_BEFORE - 1));
        assert!(s.send_reminders(SAT - 900));
        assert!(!s.send_reminders(SAT - 600), "only once");
        assert_eq!(kinds(&s, dad)[0].0, REMINDER);
        assert_eq!(kinds(&s, kid)[0].0, REMINDER, "the host too");
        assert_ne!(kinds(&s, mom)[0].0, REMINDER, "maybe isn't reminded");
    }
}
