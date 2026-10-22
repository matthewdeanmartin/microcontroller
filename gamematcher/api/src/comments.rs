//! Comment threads (`spec/03-domain.md#comments`). Each plan's thread is its
//! own blob in the Kv (`c<plan id>`), so posting rewrites one small blob,
//! not the snapshot; the plan keeps the count. `api.rs` loads and saves the
//! blob; the rules are here.
use crate::messages::{StoredComment, StoredThread};
use crate::notify;
use crate::plans::{active, CANCELLED};
use crate::state::State;
use miniframework::ApiError;

pub const MAX_LEN: usize = 500;
/// Comments stay open this long after a plan is done ("thanks for coming").
pub const OPEN_AFTER_DONE: u64 = 7 * 86_400;

/// The Kv key of a plan's thread.
pub fn key(plan: u32) -> String {
    format!("c{plan}")
}

fn text(raw: &str) -> Result<String, ApiError> {
    let t = raw.trim();
    let len = t.chars().count();
    if len == 0 || len > MAX_LEN || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad_request(format!(
            "A comment is 1 to {MAX_LEN} characters"
        )));
    }
    Ok(t.to_string())
}

impl State {
    /// Whether a plan's thread takes new comments and edits now.
    pub fn comments_open(&self, plan: u32, now: u64) -> bool {
        self.plan(plan).is_ok_and(|p| {
            active(p) || (p.status != CANCELLED && now < p.finished.saturating_add(OPEN_AFTER_DONE))
        })
    }

    fn check_open(&self, plan: u32, now: u64) -> Result<(), ApiError> {
        self.plan(plan)?;
        if self.comments_open(plan, now) {
            Ok(())
        } else {
            Err(ApiError::new(
                409,
                "comments_closed",
                "This plan's comments are closed",
            ))
        }
    }

    pub fn comment_total(&self) -> usize {
        self.plans.iter().map(|p| p.comments as usize).sum()
    }

    pub fn add_comment(
        &mut self,
        thread: &mut StoredThread,
        author: u32,
        plan: u32,
        raw: &str,
        now: u64,
    ) -> Result<u32, ApiError> {
        self.check_open(plan, now)?;
        let text = text(raw)?;
        if thread.comments.len() >= self.limits.comments_per_plan {
            return Err(ApiError::new(
                409,
                "full",
                format!(
                    "A plan holds at most {} comments",
                    self.limits.comments_per_plan
                ),
            ));
        }
        if self.comment_total() >= self.limits.comments_total {
            return Err(ApiError::new(
                409,
                "full",
                "The board is out of room for comments; old plans' threads go as plans age out",
            ));
        }
        let p = self
            .plans
            .iter_mut()
            .find(|p| p.id == plan)
            .expect("checked");
        p.next_comment += 1;
        let id = p.next_comment;
        thread.comments.push(StoredComment {
            id,
            author,
            text,
            created: now,
            edited: 0,
        });
        p.comments = thread.comments.len() as u32;
        self.on_plan_event(plan, notify::COMMENTS, author, now);
        Ok(id)
    }

    pub fn edit_comment(
        &mut self,
        thread: &mut StoredThread,
        actor: u32,
        plan: u32,
        id: u32,
        raw: &str,
        now: u64,
    ) -> Result<(), ApiError> {
        self.check_open(plan, now)?;
        let text = text(raw)?;
        let c = thread
            .comments
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| ApiError::not_found("No such comment"))?;
        if c.author != actor {
            return Err(ApiError::new(
                403,
                "not_yours",
                "You can only edit your own comments",
            ));
        }
        c.text = text;
        c.edited = now;
        Ok(())
    }

    /// The author or an admin may delete, even on a closed thread.
    pub fn delete_comment(
        &mut self,
        thread: &mut StoredThread,
        actor: u32,
        plan: u32,
        id: u32,
    ) -> Result<(), ApiError> {
        self.plan(plan)?;
        let i = thread
            .comments
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| ApiError::not_found("No such comment"))?;
        let admin = self.account(actor).is_some_and(|a| a.admin);
        if thread.comments[i].author != actor && !admin {
            return Err(ApiError::new(
                403,
                "not_yours",
                "You can only delete your own comments",
            ));
        }
        thread.comments.remove(i);
        if let Some(p) = self.plans.get_mut(plan) {
            p.comments = thread.comments.len() as u32;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{GameInput, PlanInput, ResponseInput};
    use crate::state::Limits;

    fn world() -> (State, u32, u32, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            comments_per_plan: 3,
            comments_total: 4,
            ..Limits::board()
        });
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let kid = s.signup("kid@h", "Sam", "1234", 0).unwrap();
        let mom = s.signup("mom@h", "Mom", "1234", 0).unwrap();
        let g = s
            .add_game(
                dad,
                &GameInput {
                    title: Some("Catan".into()),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let p = s
            .create_plan(
                kid,
                &PlanInput {
                    games: Some(vec![g]),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        (s, dad, kid, mom, p)
    }

    #[test]
    fn post_edit_delete_and_permissions() {
        let (mut s, dad, kid, mom, p) = world();
        let mut t = StoredThread::default();
        let id = s
            .add_comment(&mut t, mom, p, "  bringing snacks ", 5)
            .unwrap();
        assert_eq!(t.comments[0].text, "bringing snacks");
        assert_eq!(s.plan(p).unwrap().comments, 1);
        assert_eq!(
            s.add_comment(&mut t, mom, p, "   ", 5).unwrap_err().status,
            400
        );
        assert_eq!(
            s.edit_comment(&mut t, kid, p, id, "x", 6).unwrap_err().code,
            "not_yours"
        );
        s.edit_comment(&mut t, mom, p, id, "bringing chips", 6)
            .unwrap();
        assert_eq!(t.comments[0].edited, 6);
        assert_eq!(
            s.delete_comment(&mut t, kid, p, id).unwrap_err().code,
            "not_yours"
        );
        s.delete_comment(&mut t, dad, p, id).unwrap();
        assert_eq!(s.plan(p).unwrap().comments, 0);
    }

    #[test]
    fn comments_notify_the_engaged_coalesced() {
        let (mut s, dad, kid, mom, p) = world();
        s.respond(
            dad,
            p,
            &ResponseInput {
                overall: "in".into(),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        let mut t = StoredThread::default();
        s.add_comment(&mut t, mom, p, "one", 2).unwrap();
        s.add_comment(&mut t, mom, p, "two", 3).unwrap();
        let comments = |who: u32| {
            s.account(who)
                .unwrap()
                .notifications
                .iter()
                .find(|n| n.kind == notify::COMMENTS)
                .map(|n| n.count)
        };
        assert_eq!(comments(kid), Some(2), "the host, coalesced");
        assert_eq!(comments(dad), Some(2), "said in");
        assert_eq!(comments(mom), None, "the author");
    }

    #[test]
    fn bounds_and_closing() {
        let (mut s, _dad, kid, mom, p) = world();
        let mut t = StoredThread::default();
        for n in 0..3 {
            s.add_comment(&mut t, mom, p, &format!("c{n}"), 1).unwrap();
        }
        assert_eq!(
            s.add_comment(&mut t, mom, p, "four", 1).unwrap_err().code,
            "full"
        );
        s.cancel_plan(kid, p, 2).unwrap();
        let mut t2 = StoredThread::default();
        assert_eq!(
            s.add_comment(&mut t2, mom, p, "late", 3).unwrap_err().code,
            "comments_closed"
        );
        assert!(!s.comments_open(p, 3));
    }
}
