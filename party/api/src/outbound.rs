//! The outbound queue and channel settings (`spec/04-channels.md`).
//!
//! Deliveries are rows in the state (`o<id>`), so "the change happened" and
//! "a message about it is queued" commit together, and survive a reboot.
//! The worker (`App::work`) picks a due job, does the HTTP without holding
//! the state lock, and records the result here. Nothing in this module
//! talks to the network.
use crate::messages::{
    JobView, StoredBot, StoredChannel, StoredJob, StoredMail, StoredProvider, StoredPublish,
};
use crate::state::State;

pub const MAIL: u32 = 1;
pub const BOT: u32 = 2;
pub const PUBLISH: u32 = 3;
pub const PROVIDERS: u32 = 4;

pub const QUEUED: &str = "queued";
pub const SENT: &str = "sent";
pub const FAILED: &str = "failed";

/// Retries back off from 2 minutes to an hour, and give up after a day.
const FIRST_RETRY: u64 = 120;
const MAX_RETRY: u64 = 3_600;
const GIVE_UP: u64 = 86_400;

/// The last four characters of a secret, for "set, ends in …".
pub fn hint(secret: &str) -> String {
    let n = secret.chars().count();
    if n == 0 {
        String::new()
    } else {
        format!(
            "…{}",
            secret.chars().skip(n.saturating_sub(4)).collect::<String>()
        )
    }
}

impl State {
    fn channel(&self, id: u32) -> Option<&StoredChannel> {
        self.channels.get(id)
    }

    /// The channel row `id`, created empty if missing.
    pub fn channel_mut(&mut self, id: u32) -> &mut StoredChannel {
        if self.channels.get(id).is_none() {
            self.channels.push(StoredChannel {
                id,
                ..Default::default()
            });
        }
        self.channels.get_mut(id).expect("just made")
    }

    pub fn mail(&self) -> Option<&StoredMail> {
        self.channel(MAIL).and_then(|c| c.mail.as_ref())
    }

    pub fn mail_ready(&self) -> bool {
        self.mail().is_some_and(|m| m.state == "ready")
    }

    pub fn bot(&self) -> Option<&StoredBot> {
        self.channel(BOT).and_then(|c| c.bot.as_ref())
    }

    pub fn bot_ready(&self) -> bool {
        self.bot().is_some_and(|b| b.state == "ready")
    }

    pub fn publish(&self) -> Option<&StoredPublish> {
        self.channel(PUBLISH).and_then(|c| c.publish.as_ref())
    }

    pub fn providers(&self) -> &[StoredProvider] {
        self.channel(PROVIDERS)
            .map_or(&[], |c| c.providers.as_slice())
    }

    /// Queues a delivery. When the queue is full, finished jobs make room;
    /// failing that, notifications (priority 2) are dropped and counted,
    /// and anything more important still goes in.
    pub fn enqueue(&mut self, mut job: StoredJob, now: u64) -> bool {
        if self.jobs.len() >= self.limits.jobs {
            let done: Vec<u32> = {
                let mut done: Vec<(u64, u32)> = self
                    .jobs
                    .iter()
                    .filter(|j| j.state != QUEUED)
                    .map(|j| (j.sent_at.max(j.created), j.id))
                    .collect();
                done.sort_unstable();
                done.into_iter().map(|(_, id)| id).collect()
            };
            match done.first() {
                Some(&oldest) => {
                    let i = self.jobs.position(oldest).expect("listed");
                    self.jobs.remove(i);
                }
                None if job.priority >= 2 => {
                    self.channel_mut(MAIL).dropped += 1;
                    return false;
                }
                None => {}
            }
        }
        job.id = self.next_job;
        self.next_job += 1;
        job.state = QUEUED.into();
        job.created = now;
        job.next_at = now;
        self.jobs.push(job);
        true
    }

    /// The next job due now, most important first, if its channel is ready.
    pub fn next_job(&self, now: u64) -> Option<StoredJob> {
        self.jobs
            .iter()
            .filter(|j| j.state == QUEUED && j.next_at <= now)
            .filter(|j| match j.kind.as_str() {
                "mail" => self.mail_ready(),
                "dm" => self.bot_ready(),
                _ => false,
            })
            .min_by_key(|j| (j.priority, j.id))
            .cloned()
    }

    /// Mail sent today, against the daily cap.
    pub fn mail_budget_left(&self, now: u64) -> bool {
        let today = (now / 86_400) as u32;
        self.mail()
            .is_none_or(|m| m.day != today || m.sent < self.limits.mail_per_day)
    }

    /// Records the outcome of a delivery attempt.
    pub fn job_done(&mut self, id: u32, result: Result<(), String>, now: u64) {
        let kind = self.jobs.get(id).map(|j| j.kind.clone());
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        job.attempts += 1;
        match result {
            Ok(()) => {
                job.state = SENT.into();
                job.sent_at = now;
                job.error.clear();
                // The text has done its job; don't keep messages around.
                job.text.clear();
                job.ics.clear();
            }
            Err(e) => {
                job.error = e.chars().take(200).collect();
                let wait = (FIRST_RETRY << job.attempts.min(10).saturating_sub(1)).min(MAX_RETRY);
                if now.saturating_sub(job.created) + wait > GIVE_UP || job.attempts >= 12 {
                    job.state = FAILED.into();
                } else {
                    job.next_at = now + wait;
                }
            }
        }
        if kind.as_deref() == Some("mail") {
            let today = (now / 86_400) as u32;
            if let Some(m) = self.channel_mut(MAIL).mail.as_mut() {
                if m.day != today {
                    m.day = today;
                    m.sent = 0;
                }
                m.sent += 1;
            }
        }
    }

    /// Admin "retry now" for a failed job.
    pub fn retry_job(&mut self, id: u32, now: u64) -> bool {
        match self.jobs.get_mut(id) {
            Some(j) if j.state == FAILED => {
                j.state = QUEUED.into();
                j.next_at = now;
                j.attempts = 0;
                j.created = now;
                true
            }
            _ => false,
        }
    }

    pub fn delivery(&self) -> Vec<JobView> {
        let mut jobs: Vec<JobView> = self
            .jobs
            .iter()
            .map(|j| JobView {
                id: j.id,
                kind: j.kind.clone(),
                to: j.to.clone(),
                subject: j.subject.clone(),
                state: j.state.clone(),
                attempts: j.attempts,
                next_at: j.next_at,
                error: j.error.clone(),
                created: j.created,
            })
            .collect();
        jobs.sort_by_key(|j| std::cmp::Reverse(j.id));
        jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Limits;

    fn job(priority: u32) -> StoredJob {
        StoredJob {
            kind: "mail".into(),
            to: "a@b.c".into(),
            priority,
            ..Default::default()
        }
    }

    #[test]
    fn a_full_queue_drops_notifications_not_replies() {
        let mut s = State::new(Limits {
            jobs: 2,
            ..Limits::board()
        });
        assert!(s.enqueue(job(2), 0));
        assert!(s.enqueue(job(2), 0));
        assert!(!s.enqueue(job(2), 0), "a third notification is dropped");
        assert_eq!(s.channels.get(MAIL).unwrap().dropped, 1);
        assert!(s.enqueue(job(0), 0), "a command reply still goes in");
        // Finished jobs make room.
        let first = s.jobs[0].id;
        s.job_done(first, Ok(()), 5);
        assert!(s.enqueue(job(2), 6));
        assert!(s.jobs.get(first).is_none());
    }

    #[test]
    fn retries_back_off_then_give_up() {
        let mut s = State::new(Limits::board());
        s.channel_mut(MAIL).mail = Some(StoredMail {
            state: "ready".into(),
            ..Default::default()
        });
        s.enqueue(job(1), 1_000);
        let id = s.jobs[0].id;
        assert_eq!(s.next_job(1_000).map(|j| j.id), Some(id));
        s.job_done(id, Err("timeout".into()), 1_000);
        assert_eq!(s.jobs.get(id).unwrap().next_at, 1_120);
        assert!(s.next_job(1_100).is_none(), "waits for its retry");
        s.job_done(id, Err("timeout".into()), 1_120);
        assert_eq!(s.jobs.get(id).unwrap().next_at, 1_360, "doubles");
        s.job_done(id, Err("timeout".into()), 90_000);
        assert_eq!(s.jobs.get(id).unwrap().state, FAILED, "a day is enough");
        assert!(s.retry_job(id, 90_001));
        assert_eq!(s.jobs.get(id).unwrap().state, QUEUED);
        assert_eq!(hint("xkeysib-abcd1234"), "…1234");
    }
}
