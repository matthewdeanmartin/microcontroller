//! Email the board sends itself, through Brevo's transactional API
//! (`spec/08-mastodon.md` → "Email"): plan news for people who haven't
//! turned it off, invites typed on a plan, and "forgot PIN" codes. Pure
//! text here; queueing and sending are [`crate::outbound`]'s.
use crate::announce::coming;
use crate::messages::StoredPlan;
use crate::notify;
use crate::screen::clock;
use crate::state::State;

/// Notification kinds worth an email. Answers and comments stay in the
/// bell: they come in bursts and the plan page shows them.
pub const EMAILED: [&str; 6] = [
    notify::NEW_PLAN,
    notify::LOCKED,
    notify::CANCELLED,
    notify::CHANGED,
    notify::REMINDER,
    notify::HOST,
];

/// Brevo's free plan sends 300 a day; stay well inside it.
pub const DAILY_LIMIT: u32 = 100;
/// Addresses one invite may go to.
pub const MAX_INVITES: usize = 8;

/// A notification to email, recorded by `State::notify`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub to: u32,
    pub kind: String,
    pub plan: u32,
    pub actor: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Email {
    pub to: String,
    pub to_name: String,
    pub subject: String,
    pub text: String,
}

/// A plausible address: something@something.tld, no spaces, short.
pub fn valid_address(raw: &str) -> bool {
    let a = raw.trim();
    let Some((user, host)) = a.split_once('@') else {
        return false;
    };
    a.len() <= 254
        && !user.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !a
            .chars()
            .any(|c| c.is_whitespace() || c == ',' || c == '<' || c == '>' || c == '"')
}

pub(crate) fn plan_link(origin: &str, p: &StoredPlan) -> String {
    format!("{origin}/plans/{}", p.id)
}

fn games(state: &State, p: &StoredPlan) -> String {
    let ids = if p.chosen_games.is_empty() {
        &p.games
    } else {
        &p.chosen_games
    };
    ids.iter()
        .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn footer(origin: &str) -> String {
    format!(
        "\n\n-- \nGame Matcher at home ({origin}). The link works on the home Wi-Fi.\n\
         Don't want these? Turn plan emails off on your profile: {origin}/profile"
    )
}

pub(crate) fn times(p: &StoredPlan, now: u64, offset: i64) -> String {
    match p.times.iter().find(|t| Some(t.id) == p.chosen_time) {
        Some(t) => clock::when(t.start, now, offset),
        None if p.times.is_empty() => "time to be decided".into(),
        None => p
            .times
            .iter()
            .map(|t| clock::when(t.start, now, offset))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// The email for one pending notification, or `None` if its account or
/// plan is gone.
pub fn notification(
    state: &State,
    n: &Pending,
    origin: &str,
    now: u64,
    offset: i64,
) -> Option<Email> {
    let account = state
        .account(n.to)
        .filter(|a| !a.disabled && !a.email.is_empty())?;
    let p = state.plan(n.plan).ok()?;
    let actor = if n.actor == 0 {
        "Game Matcher".to_string()
    } else {
        state.name(n.actor)
    };
    let what = games(state, p);
    let when = times(p, now, offset);
    let place = if p.place.is_empty() {
        String::new()
    } else {
        format!("\nWhere: {}", p.place)
    };
    let link = plan_link(origin, p);
    let (subject, body) =
        match n.kind.as_str() {
            notify::NEW_PLAN => (
                format!("New game plan: {}", p.title),
                format!("{actor} wants to play {what}.\nWhen: {when}{place}\n\nAre you in? {link}"),
            ),
            notify::LOCKED => (
                format!("It's on: {} — {when}", p.title),
                format!(
                "{actor} locked in {}.\nWhen: {when}{place}\nPlaying: {what}\nComing: {}\n\n{link}",
                p.title,
                coming(p).iter().map(|&id| state.name(id)).collect::<Vec<_>>().join(", "),
            ),
            ),
            notify::CANCELLED => (
                format!("Cancelled: {}", p.title),
                format!("{actor} cancelled {}.\n\n{link}", p.title),
            ),
            notify::CHANGED => (
                format!("Plan updated: {}", p.title),
                format!(
                    "{actor} updated {}.\nWhen: {when}{place}\n\nCheck the plan and your answer: {link}",
                    p.title
                ),
            ),
            notify::REMINDER => (
                format!("Starting soon: {} — {when}", p.title),
                format!(
                    "{} starts {when}.{place}\nPlaying: {what}\n\n{link}",
                    p.title
                ),
            ),
            notify::HOST => (
                format!("You're hosting {}", p.title),
                format!("{actor} handed {} over to you.\n\n{link}", p.title),
            ),
            _ => return None,
        };
    Some(Email {
        to: account.email.clone(),
        to_name: account.display_name.clone(),
        subject,
        text: format!("Hi {},\n\n{body}{}", account.display_name, footer(origin)),
    })
}

/// An invite to a plan, from `from` (an account id) to `address`.
pub fn plan_invite(
    state: &State,
    p: &StoredPlan,
    from: u32,
    address: &str,
    origin: &str,
    now: u64,
    offset: i64,
) -> Email {
    let from = state.name(from);
    let place = if p.place.is_empty() {
        String::new()
    } else {
        format!("\nWhere: {}", p.place)
    };
    Email {
        to: address.trim().to_string(),
        to_name: String::new(),
        subject: format!("{from} invited you: {}", p.title),
        text: format!(
            "{from} wants to play {} and invited you.\nWhen: {}{place}\n\n\
             Say if you're in: {}\n(on the home Wi-Fi; first time? make an account with this email address)\n\n\
             -- \nGame Matcher at home ({origin})",
            games(state, p),
            times(p, now, offset),
            plan_link(origin, p),
        ),
    }
}

/// An invite to join the board.
pub fn site_invite(state: &State, from: u32, address: &str, origin: &str) -> Email {
    let from = state.name(from);
    Email {
        to: address.trim().to_string(),
        to_name: String::new(),
        subject: format!("{from} invited you to Game Matcher"),
        text: format!(
            "{from} invited you to the household's game-night planner: pick the games you're into, \
             say when you can play, see what's on.\n\nJoin on the home Wi-Fi: {origin}/signup\n\n\
             -- \nGame Matcher at home ({origin})"
        ),
    }
}

/// A "forgot PIN" code.
pub fn pin_code(name: &str, address: &str, code: &str, origin: &str) -> Email {
    Email {
        to: address.to_string(),
        to_name: name.to_string(),
        subject: format!("Your Game Matcher code: {code}"),
        text: format!(
            "Hi {name},\n\nYour code to set a new PIN is {code}. It works for 15 minutes.\n\n\
             Didn't ask? Ignore this; your PIN hasn't changed.\n\n-- \nGame Matcher at home ({origin})"
        ),
    }
}

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Brevo's `POST /v3/smtp/email` body (its `textContent` key isn't a
/// Rust-shaped field name, so this one is written by hand).
pub fn brevo_body(
    sender_name: &str,
    sender_email: &str,
    to: &str,
    to_name: &str,
    subject: &str,
    text: &str,
) -> Vec<u8> {
    let mut out = String::from("{\"sender\":{\"name\":");
    json_string(&mut out, sender_name);
    out.push_str(",\"email\":");
    json_string(&mut out, sender_email);
    out.push_str("},\"to\":[{\"email\":");
    json_string(&mut out, to);
    if !to_name.is_empty() {
        out.push_str(",\"name\":");
        json_string(&mut out, to_name);
    }
    out.push_str("}],\"subject\":");
    json_string(&mut out, subject);
    out.push_str(",\"textContent\":");
    json_string(&mut out, text);
    out.push('}');
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{StoredAccount, StoredTime};
    use crate::state::Limits;

    #[test]
    fn addresses() {
        assert!(valid_address("sam@example.com"));
        assert!(valid_address(" sam+games@home.example.org "));
        for bad in [
            "sam",
            "sam@",
            "@x.com",
            "sam@local",
            "a b@x.com",
            "a@x.com, b@y.com",
            "x@.com",
        ] {
            assert!(!valid_address(bad), "{bad}");
        }
    }

    #[test]
    fn brevo_json_escapes() {
        let body = brevo_body(
            "Game \"Matcher\"",
            "gm@x.com",
            "s@x.com",
            "",
            "Hi",
            "a\nb\\c",
        );
        let s = String::from_utf8(body).unwrap();
        assert_eq!(
            s,
            r#"{"sender":{"name":"Game \"Matcher\"","email":"gm@x.com"},"to":[{"email":"s@x.com"}],"subject":"Hi","textContent":"a\nb\\c"}"#
        );
    }

    #[test]
    fn a_new_notification_mails_once_until_read() {
        let mut s = State::new(Limits::board());
        for (id, name, email) in [
            (1, "Dad", "dad@x.com"),
            (2, "Sam", "sam@x.com"),
            (3, "Kid", ""),
        ] {
            s.accounts.push(StoredAccount {
                id,
                display_name: name.into(),
                email: email.into(),
                ..Default::default()
            });
        }
        s.plans.push(StoredPlan {
            id: 7,
            host: 1,
            title: "Catan night".into(),
            status: "on".into(),
            place: "Home".into(),
            times: vec![StoredTime {
                id: 1,
                start: 1_792_260_000 + 7200,
                end: 0,
            }],
            chosen_time: Some(1),
            ..Default::default()
        });
        s.notify(2, notify::LOCKED, 7, 1, 100);
        s.notify(2, notify::LOCKED, 7, 1, 101);
        s.notify(3, notify::LOCKED, 7, 1, 100);
        s.notify(2, notify::COMMENTS, 7, 1, 100);
        assert_eq!(
            s.mail.len(),
            1,
            "coalesced, no address, not emailed: {:?}",
            s.mail
        );
        let email =
            notification(&s, &s.mail[0], "http://gamematcher.local", 1_792_260_000, 0).unwrap();
        assert_eq!(email.to, "sam@x.com");
        assert!(
            email.subject.starts_with("It's on: Catan night — Tonight"),
            "{}",
            email.subject
        );
        assert!(
            email.text.contains("http://gamematcher.local/plans/7")
                && email.text.contains("Coming: Dad")
        );

        s.accounts[1].email_off = true;
        s.notify(2, notify::CANCELLED, 7, 1, 102);
        assert_eq!(s.mail.len(), 1, "turned off");
    }
}
