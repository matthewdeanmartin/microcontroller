//! What email says (`spec/04-channels.md#email`), the calendar invitations
//! it carries, and Brevo's request body. Email is outbound only: replies go
//! to the organizer (Reply-To), not to the board.
use crate::events::CANCELLED;
use crate::messages::{StoredEvent, StoredJob};
use crate::state::State;

/// Standard base64 (attachments).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A JSON string literal.
pub fn json_string(out: &mut String, s: &str) {
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

/// `POST /v3/smtp/email`'s body for one job.
pub fn brevo_body(sender_name: &str, sender_email: &str, job: &StoredJob) -> Vec<u8> {
    let mut out = String::from("{\"sender\":{\"name\":");
    json_string(&mut out, sender_name);
    out.push_str(",\"email\":");
    json_string(&mut out, sender_email);
    out.push_str("},\"to\":[{\"email\":");
    json_string(&mut out, &job.to);
    if !job.to_name.is_empty() {
        out.push_str(",\"name\":");
        json_string(&mut out, &job.to_name);
    }
    out.push_str("}]");
    if !job.reply_to.is_empty() {
        out.push_str(",\"replyTo\":{\"email\":");
        json_string(&mut out, &job.reply_to);
        out.push('}');
    }
    out.push_str(",\"subject\":");
    json_string(&mut out, &job.subject);
    out.push_str(",\"textContent\":");
    json_string(&mut out, &job.text);
    if !job.ics.is_empty() {
        out.push_str(",\"attachment\":[{\"name\":");
        json_string(&mut out, &job.ics_name);
        out.push_str(",\"content\":");
        json_string(&mut out, &base64(job.ics.as_bytes()));
        out.push_str("}]");
    }
    out.push('}');
    out.into_bytes()
}

/// Whose email replies should reach: the first host with an address.
pub fn reply_to(state: &State, e: &StoredEvent) -> String {
    e.hosts
        .iter()
        .filter_map(|h| state.person(*h))
        .map(|p| p.email.clone())
        .find(|m| !m.is_empty())
        .unwrap_or_default()
}

fn when(state: &State, secs: u64) -> String {
    if secs == 0 {
        return "time to be decided".into();
    }
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let l = state.zone.local(secs);
    format!(
        "{} {} {} at {:02}:{:02}",
        DAYS[l.weekday as usize],
        MONTHS[(l.month - 1) as usize],
        l.day,
        l.hour,
        l.minute
    )
}

/// The subject and text of a notification email about event `e`, or None
/// for kinds that don't go by email.
pub fn notice_email(
    state: &State,
    kind: &str,
    e: &StoredEvent,
    actor: u32,
    origin: &str,
) -> Option<(String, String)> {
    let who = if actor == 0 {
        "party".to_string()
    } else {
        state.name(actor)
    };
    let group = state
        .group(e.group)
        .map(|g| g.name.clone())
        .unwrap_or_default();
    let at = when(state, e.start);
    let (subject, lead) = match kind {
        "invited" => (
            format!("Invitation: {}", e.title),
            format!("{who} invited you to {} ({group}), {at}.", e.title),
        ),
        "changed" => (
            format!("Changed: {}", e.title),
            format!("{who} changed {}. It's now {at}.", e.title),
        ),
        "cancelled" => (
            format!("Cancelled: {}", e.title),
            format!("{} on {at} is cancelled.", e.title),
        ),
        "reminder" => (
            format!("Reminder: {}", e.title),
            format!("{} is coming up: {at}.", e.title),
        ),
        "promoted" => (
            format!("You're in: {}", e.title),
            format!("A place opened up: you're going to {}, {at}.", e.title),
        ),
        "answered_for" => (
            format!("Answered for you: {}", e.title),
            format!("{who} answered {} for you.", e.title),
        ),
        _ => return None,
    };
    let mut text = lead;
    if !e.about.is_empty() && kind == "invited" {
        text.push_str(&format!("\n\n{}", e.about));
    }
    if !e.address.is_empty() && kind != "cancelled" {
        text.push_str(&format!("\n\nWhere: {}", e.address));
    }
    text.push_str(&format!(
        "\n\nAnswer at {origin}/events/{} on the home Wi-Fi, or reply to this email to reach the host.\n\n(You get these emails because you're invited. Turn them off on your profile at {origin}/me.)",
        e.id
    ));
    Some((subject, text))
}

/// An iCalendar invitation (`REQUEST`) or cancellation (`CANCEL`) of `e`
/// for one attendee: calendar apps add or update the event from the email.
pub fn invitation(
    state: &State,
    e: &StoredEvent,
    to: &str,
    to_name: &str,
    organizer: &str,
) -> String {
    let cancel = e.status == CANCELLED;
    let mut out = String::new();
    let mut line = |s: &str| crate::ics::push_line(&mut out, s);
    line("BEGIN:VCALENDAR");
    line("VERSION:2.0");
    line("PRODID:-//party.local//party//EN");
    line(if cancel {
        "METHOD:CANCEL"
    } else {
        "METHOD:REQUEST"
    });
    line("BEGIN:VEVENT");
    line(&format!("UID:e{}@party.local", e.id));
    line(&format!(
        "DTSTAMP:{}",
        crate::ics::utc(e.updated.max(e.created))
    ));
    line(&format!("DTSTART:{}", crate::ics::utc(e.start)));
    line(&format!("DTEND:{}", crate::ics::utc(e.effective_end())));
    line(&format!("SUMMARY:{}", crate::ics::escape(&e.title)));
    if !e.about.is_empty() {
        line(&format!("DESCRIPTION:{}", crate::ics::escape(&e.about)));
    }
    let place = [e.place.as_str(), e.address.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if !place.is_empty() {
        line(&format!("LOCATION:{}", crate::ics::escape(&place)));
    }
    let host = e.hosts.first().map(|h| state.name(*h)).unwrap_or_default();
    line(&format!(
        "ORGANIZER;CN={}:mailto:{organizer}",
        crate::ics::escape(&host)
    ));
    line(&format!(
        "ATTENDEE;CN={};ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=FALSE:mailto:{to}",
        crate::ics::escape(to_name)
    ));
    line(&format!("SEQUENCE:{}", e.edits));
    line(if cancel {
        "STATUS:CANCELLED"
    } else {
        "STATUS:CONFIRMED"
    });
    line("END:VEVENT");
    line("END:VCALENDAR");
    out
}

/// The email that carries an invitation link.
pub fn invite_email(issuer: &str, target: &str, link: &str, days: u32) -> (String, String) {
    (
        format!("{issuer} invited you to {target}"),
        format!(
            "{issuer} invited you to {target} on party.\n\nOpen this link to accept:\n{link}\n\nIt works for {days} days. At the house, it opens on the home Wi-Fi; from elsewhere you'll be able to accept with a Mastodon account."
        ),
    )
}

/// The "forgot PIN" email.
pub fn reset_email(code: &str) -> (String, String) {
    (
        "Your party PIN code".into(),
        format!("Your code to set a new PIN is {code}. It works for 15 minutes.\n\nIf you didn't ask for it, ignore this email."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want);
        }
    }

    #[test]
    fn brevo_body_escapes_and_attaches() {
        let job = StoredJob {
            to: "gran@example.com".into(),
            to_name: "Gran \"G\"".into(),
            subject: "Hi".into(),
            text: "line 1\nline 2".into(),
            ics: "BEGIN:VCALENDAR".into(),
            ics_name: "invite.ics".into(),
            reply_to: "dad@example.com".into(),
            ..Default::default()
        };
        let body = String::from_utf8(brevo_body("party", "bot@example.com", &job)).unwrap();
        assert!(body.contains(r#""name":"Gran \"G\"""#), "{body}");
        assert!(body.contains(r#""textContent":"line 1\nline 2""#));
        assert!(body.contains(r#""replyTo":{"email":"dad@example.com"}"#));
        assert!(body.contains(&format!(r#""content":"{}""#, base64(b"BEGIN:VCALENDAR"))));
    }
}
