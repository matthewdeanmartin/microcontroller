//! Strict RSVP commands. Instances authenticate senders; local accounts must be linked.
use miniframework::message;

message! {
    pub struct PostLink {
        1 status: String,
        2 plan: u32,
    }
}
message! {
    pub struct RemoteAccount {
        1 id: String,
        2 acct: String,
    }
}
message! {
    pub struct RemoteStatus {
        1 content: String,
        2 in_reply_to_id: Option<String>,
    }
}
message! {
    pub struct Mention {
        1 id: String,
        2 account: RemoteAccount,
        3 status: Option<RemoteStatus>,
    }
}
message! {
    pub struct Mentions {
        1 items: Vec<Mention>,
    }
}
message! {
    pub struct Inbox {
        1 cursor: String,
        2 pending: Vec<Mention>,
        3 initialized: bool,
    }
}

/// Decimal snowflake comparison without assuming ids fit a machine integer.
pub fn newer(a: &str, b: &str) -> bool {
    !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()) && (a.len(), a) > (b.len(), b)
}

pub fn linked_plan(text: &str) -> Option<u32> {
    let (_, rest) = text.rsplit_once("/plans/")?;
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// Strip markup, preserve word boundaries, and accept only an exact command.
pub fn command(html: &str) -> Option<(String, Option<u32>)> {
    if html.len() > 4096 {
        return None;
    }
    let mut plain = String::new();
    let mut tag = false;
    let mut markup = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                tag = true;
                markup.clear();
            }
            '>' => {
                tag = false;
                let name = markup
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/')
                    .to_ascii_lowercase();
                if matches!(name.as_str(), "p" | "/p" | "br" | "div" | "/div") {
                    plain.push(' ');
                }
            }
            _ if !tag => plain.push(c),
            _ => markup.push(c),
        }
    }
    let plain = plain
        .replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let words: Vec<_> = plain
        .split_whitespace()
        .filter(|s| !s.starts_with('@'))
        .collect();
    let answer = words.first()?.to_ascii_lowercase();
    if !matches!(answer.as_str(), "in" | "maybe" | "out") {
        return None;
    }
    match words.as_slice() {
        [_] => Some((answer, None)),
        [_, id] => Some((answer, Some(id.trim_start_matches('#').parse().ok()?))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_unambiguous_commands() {
        assert_eq!(
            command("<p><a>@games</a> IN #42</p>"),
            Some(("in".into(), Some(42)))
        );
        assert_eq!(command("<p>@games maybe</p>"), Some(("maybe".into(), None)));
        assert_eq!(command("<p>I think I'm in 42</p>"), None);
        assert_eq!(command("<p>in 42 out</p>"), None);
        assert_eq!(command("<p>in nope</p>"), None);
        assert_eq!(
            command("<p><a>@<span>games</span></a> in</p>"),
            Some(("in".into(), None))
        );
        assert!(newer("100", "99"));
        assert!(!newer("x", "99"));
    }
}
