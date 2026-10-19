//! What the household bot says (`spec/08-mastodon.md`): plan news, the
//! morning digest and comment DMs. Pure text from the state; queueing and
//! sending are [`crate::outbound`]'s.
use crate::messages::StoredPlan;
use crate::screen::clock::{self, Local};
use crate::state::State;

/// Mastodon's default status length.
pub const LIMIT: usize = 500;
const WEEK: u64 = 7 * 86_400;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum News {
    New,
    Locked,
    Cancelled,
    Updated,
}

/// At most `limit` characters, ending in "…" when cut.
pub fn fit(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A line of its own, or nothing.
fn lines(text: String) -> String {
    if text.is_empty() {
        text
    } else {
        text + "
"
    }
}

fn game_list(state: &State, ids: &[u32]) -> String {
    let titles: Vec<String> = ids
        .iter()
        .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
        .collect();
    match titles.len() {
        0 => String::new(),
        n if n > 4 => format!("{} +{}", titles[..4].join(", "), n - 4),
        _ => titles.join(", "),
    }
}

/// Who's coming: the host (unless they said out) and everyone in.
pub fn coming(p: &StoredPlan) -> Vec<u32> {
    let mut out = Vec::new();
    let host_out = p
        .responses
        .iter()
        .any(|r| r.account == p.host && r.overall == "out");
    if !host_out {
        out.push(p.host);
    }
    for r in &p.responses {
        if r.overall == "in" && !out.contains(&r.account) {
            out.push(r.account);
        }
    }
    out
}

fn chosen_start(p: &StoredPlan) -> Option<u64> {
    p.times
        .iter()
        .find(|t| Some(t.id) == p.chosen_time)
        .map(|t| t.start)
}

fn link(origin: &str, p: &StoredPlan) -> String {
    format!("{origin}/plans/{}", p.id)
}

/// A post about one plan.
pub fn plan_post(
    state: &State,
    p: &StoredPlan,
    news: News,
    origin: &str,
    now: u64,
    offset: i64,
) -> String {
    let when = |t: u64| clock::when(t, now, offset);
    let place = if p.place.is_empty() || p.place == "Home" {
        String::new()
    } else {
        format!(" at {}", p.place)
    };
    let text = match news {
        News::New | News::Updated => {
            let mut times: Vec<String> = p.times.iter().take(3).map(|t| when(t.start)).collect();
            if p.times.len() > 3 {
                times.push(format!("+{} more", p.times.len() - 3));
            }
            format!(
                "🎲 New game plan: {}\n{}\nWhen: {}{place}\n{} is hosting. Who's in? {}",
                p.title,
                lines(game_list(state, &p.games)),
                if times.is_empty() {
                    "to be decided".into()
                } else {
                    times.join(" · ")
                },
                state.name(p.host),
                link(origin, p),
            )
        }
        News::Locked => {
            let games = if p.chosen_games.is_empty() {
                &p.games
            } else {
                &p.chosen_games
            };
            format!(
                "✅ It's on: {} — {}{place}\n{}\n{} coming. {}",
                p.title,
                chosen_start(p).map_or_else(|| "time to be decided".into(), when),
                lines(game_list(state, games)),
                coming(p).len(),
                link(origin, p),
            )
        }
        News::Cancelled => format!("❌ Cancelled: {}. {}", p.title, link(origin, p)),
    };
    let text = if news == News::Updated {
        text.replacen("New game plan:", "Plan updated:", 1)
    } else {
        text
    };
    fit(&text, LIMIT)
}

/// The morning post: today's and this week's locked-in plans, then the
/// ones still being decided. `None` when nothing is planned.
pub fn digest(state: &State, origin: &str, now: u64, offset: i64) -> Option<String> {
    digest_for(state, origin, now, offset, false)
}

pub fn digest_for(
    state: &State,
    origin: &str,
    now: u64,
    offset: i64,
    public: bool,
) -> Option<String> {
    let today = Local::at(now, offset);
    let mut on: Vec<(u64, &StoredPlan)> = state
        .plans
        .iter()
        .filter(|p| p.status == "on" && (!public || p.announce_public))
        .filter_map(|p| chosen_start(p).map(|s| (s, p)))
        .filter(|(s, _)| *s + 3600 >= now && *s < now + WEEK)
        .collect();
    on.sort_by_key(|(s, _)| *s);
    let open: Vec<&StoredPlan> = state
        .plans
        .iter()
        .filter(|p| p.status == "open" && (!public || p.announce_public))
        .collect();
    if on.is_empty() && open.is_empty() {
        return None;
    }
    let line = |s: u64, p: &StoredPlan| {
        format!(
            "• {} – {}, {} coming",
            p.title,
            clock::when(s, now, offset),
            coming(p).len()
        )
    };
    let mut out = format!(
        "🎲 Game night news for {} {}\n",
        today.day_name(),
        today.day
    );
    let (tonight, later): (Vec<_>, Vec<_>) = on.iter().partition(|(s, _)| {
        let at = Local::at(*s, offset);
        (at.year, at.month, at.day) == (today.year, today.month, today.day)
    });
    if !tonight.is_empty() {
        out.push_str("\nToday\n");
        for (s, p) in &tonight {
            out.push_str(&line(*s, p));
            out.push('\n');
        }
    }
    if !later.is_empty() {
        out.push_str("\nThis week\n");
        for (s, p) in &later {
            out.push_str(&line(*s, p));
            out.push('\n');
        }
    }
    if !open.is_empty() {
        out.push_str("\nStill deciding\n");
        for p in &open {
            out.push_str(&format!("• {} – {} answered\n", p.title, p.responses.len()));
        }
    }
    out.push('\n');
    out.push_str(origin);
    Some(fit(&out, LIMIT))
}

/// Mastodon handles the bot can DM about `p`: everyone coming or maybe
/// (and the host), except `author`, whose linked account is on the bot's
/// host, or anywhere but a `.local` instance when `federated`.
pub fn recipients(
    state: &State,
    p: &StoredPlan,
    author: u32,
    bot_host: &str,
    federated: bool,
) -> Vec<String> {
    let mut people = coming(p);
    for r in &p.responses {
        if r.overall == "maybe" && !people.contains(&r.account) {
            people.push(r.account);
        }
    }
    people
        .into_iter()
        .filter(|&id| id != author)
        .filter_map(|id| state.account(id).filter(|a| !a.disabled)?.mastodon.as_ref())
        .filter(|m| {
            m.host.eq_ignore_ascii_case(bot_host) || (federated && !m.host.ends_with(".local"))
        })
        .map(|m| m.handle.clone())
        .filter(|h| h.starts_with('@'))
        .collect()
}

/// A comment as a direct message to `mentions`.
pub fn comment_dm(
    state: &State,
    p: &StoredPlan,
    author: u32,
    comment: &str,
    mentions: &[String],
    origin: &str,
) -> String {
    let head = format!(
        "{}\n{} on “{}”:\n",
        mentions.join(" "),
        state.name(author),
        p.title
    );
    let tail = format!("\n{}", link(origin, p));
    let room = LIMIT.saturating_sub(head.chars().count() + tail.chars().count());
    format!("{head}{}{tail}", fit(comment, room))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{StoredAccount, StoredIdentity, StoredResponse, StoredTime};
    use crate::state::Limits;

    #[test]
    fn fit_cuts_with_an_ellipsis_only_when_needed() {
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(fit("abcd", 3), "ab…");
    }

    fn state_with_plan() -> State {
        let mut s = State::new(Limits::board());
        for (id, name) in [(1, "Dad"), (2, "Sam"), (3, "Ana")] {
            s.accounts.push(StoredAccount {
                id,
                display_name: name.into(),
                ..Default::default()
            });
        }
        s.accounts[1].mastodon = Some(StoredIdentity {
            host: "mastomini.local".into(),
            subject: "2".into(),
            handle: "@sam@mastomini.local".into(),
        });
        s.accounts[2].mastodon = Some(StoredIdentity {
            host: "mastodon.social".into(),
            subject: "9".into(),
            handle: "@ana@mastodon.social".into(),
        });
        s.plans.push(StoredPlan {
            id: 1,
            host: s.accounts[0].id,
            title: "Catan night".into(),
            status: "on".into(),
            place: "Home".into(),
            times: vec![StoredTime {
                id: 1,
                start: 1_792_260_000 + 3600,
                end: 0,
            }],
            chosen_time: Some(1),
            responses: vec![
                StoredResponse {
                    account: s.accounts[1].id,
                    overall: "in".into(),
                    ..Default::default()
                },
                StoredResponse {
                    account: s.accounts[2].id,
                    overall: "maybe".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        s
    }

    #[test]
    fn dms_reach_the_bots_instance_and_skip_the_author() {
        let s = state_with_plan();
        let p = &s.plans[0];
        let (dad, sam) = (s.accounts[0].id, s.accounts[1].id);
        assert_eq!(
            recipients(&s, p, dad, "mastomini.local", false),
            ["@sam@mastomini.local"]
        );
        assert_eq!(
            recipients(&s, p, dad, "mastodon.social", true),
            ["@ana@mastodon.social"],
            "a public bot can't reach a .local instance"
        );
        assert!(recipients(&s, p, sam, "mastomini.local", false).is_empty());
        let dm = comment_dm(
            &s,
            p,
            dad,
            &"long ".repeat(200),
            &["@sam@mastomini.local".into()],
            "http://gamematcher.local",
        );
        assert!(dm.starts_with("@sam@mastomini.local\nDad on “Catan night”:"));
        assert!(dm.ends_with("http://gamematcher.local/plans/1") && dm.chars().count() <= LIMIT);
    }

    #[test]
    fn posts_and_the_digest_say_what_and_when() {
        let s = state_with_plan();
        let now = 1_792_260_000;
        let p = &s.plans[0];
        let locked = plan_post(&s, p, News::Locked, "http://g.local", now, 0);
        assert!(
            locked.starts_with("✅ It's on: Catan night — Tonight"),
            "{locked}"
        );
        assert!(
            locked.contains("2 coming"),
            "host + in, not maybe: {locked}"
        );
        let digest = digest(&s, "http://g.local", now, 0).unwrap();
        assert!(digest.contains("Today\n• Catan night"), "{digest}");
        let empty = State::new(Limits::board());
        assert!(
            super::digest(&empty, "x", now, 0).is_none(),
            "nothing planned, nothing posted"
        );
    }
}
