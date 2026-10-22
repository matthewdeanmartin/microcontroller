//! The bot's command language (`spec/04-channels.md#the-command-dsl`).
//!
//! Plain words, one command per line, case-insensitive; mentions are
//! ignored. `E12`, `G3`, `S4` name an event, group or series exactly; other
//! words are resolved later against what the sender can see. Each command
//! has one canonical text form ([`Cmd::canonical`]), which the shared test
//! vectors in `spec/dsl-vectors.json` pin down for the companion site too.

/// An event, group or series: an exact id, or words to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    Id(u32),
    Name(String),
}

impl Ref {
    fn canonical(&self, prefix: char) -> String {
        match self {
            Ref::Id(id) => format!("{prefix}{id}"),
            Ref::Name(n) => n.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Help(String),
    Me,
    Events(Option<Ref>),
    Event(Ref),
    Groups,
    Find(String),
    Join(Ref),
    Leave(Ref),
    Rsvp {
        event: Ref,
        status: String,
        plus: Option<u32>,
        kids: Option<u32>,
        note: String,
    },
    Bring {
        event: Ref,
        item: String,
    },
    Unbring {
        event: Ref,
        item: String,
    },
    Ride {
        event: Ref,
        kind: String,
        seats: u32,
    },
    When {
        event: Ref,
        options: Vec<u32>,
    },
    Shift {
        event: Ref,
        name: String,
    },
    Answer {
        event: Ref,
        question: u32,
        value: String,
    },
    Regular {
        series: Ref,
        on: bool,
    },
    Redeem(String),
    Link,
    NewLink,
    Stop,
    Start,
    Undo,
    Confirm(String),
    Cancel,
}

impl Cmd {
    /// The one way to write this command.
    pub fn canonical(&self) -> String {
        let opt =
            |v: &Option<u32>, prefix: &str| v.map(|n| format!(" {prefix}{n}")).unwrap_or_default();
        match self {
            Cmd::Help(topic) if topic.is_empty() => "help".into(),
            Cmd::Help(topic) => format!("help {topic}"),
            Cmd::Me => "me".into(),
            Cmd::Events(None) => "events".into(),
            Cmd::Events(Some(g)) => format!("events {}", g.canonical('G')),
            Cmd::Event(e) => format!("event {}", e.canonical('E')),
            Cmd::Groups => "groups".into(),
            Cmd::Find(words) => format!("find {words}"),
            Cmd::Join(g) => format!("join {}", g.canonical('G')),
            Cmd::Leave(g) => format!("leave {}", g.canonical('G')),
            Cmd::Rsvp {
                event,
                status,
                plus,
                kids,
                note,
            } => {
                // By id: "rsvp E12 going"; by name: "going saturday dinner".
                let head = match event {
                    Ref::Id(_) => format!("rsvp {} {status}", event.canonical('E')),
                    Ref::Name(n) => format!("{status} {n}"),
                };
                let mut s = format!("{head}{}{}", opt(plus, "+"), opt(kids, "kids "));
                if !note.is_empty() {
                    s.push_str(&format!(" note {note}"));
                }
                s
            }
            Cmd::Bring { event, item } => format!("bring {} {item}", event.canonical('E')),
            Cmd::Unbring { event, item } => format!("unbring {} {item}", event.canonical('E')),
            Cmd::Ride { event, kind, .. } if kind == "own" => {
                format!("ride {} own", event.canonical('E'))
            }
            Cmd::Ride { event, kind, seats } => {
                format!("ride {} {kind} {seats}", event.canonical('E'))
            }
            Cmd::When { event, options } => format!(
                "when {} {}",
                event.canonical('E'),
                options
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Cmd::Shift { event, name } => format!("shift {} {name}", event.canonical('E')),
            Cmd::Answer {
                event,
                question,
                value,
            } => format!("answer {} {question} {value}", event.canonical('E')),
            Cmd::Regular { series, on } => format!(
                "regular {} {}",
                series.canonical('S'),
                if *on { "on" } else { "off" }
            ),
            Cmd::Redeem(t) => format!("redeem {t}"),
            Cmd::Link => "link".into(),
            Cmd::NewLink => "newlink".into(),
            Cmd::Stop => "stop".into(),
            Cmd::Start => "start".into(),
            Cmd::Undo => "undo".into(),
            Cmd::Confirm(code) => format!("confirm {code}"),
            Cmd::Cancel => "cancel".into(),
        }
    }

    /// Whether it changes anything (and so needs a known sender).
    pub fn changes(&self) -> bool {
        !matches!(
            self,
            Cmd::Help(_)
                | Cmd::Me
                | Cmd::Events(_)
                | Cmd::Event(_)
                | Cmd::Groups
                | Cmd::Find(_)
                | Cmd::Link
        )
    }
}

/// Mastodon status HTML → plain lines, mentions dropped.
pub fn plain_lines(html: &str) -> Vec<String> {
    let mut plain = String::new();
    let mut tag = false;
    let mut markup = String::new();
    for c in html.chars().take(4_096) {
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
                    plain.push('\n');
                }
            }
            _ if !tag => plain.push(c),
            _ => markup.push(c),
        }
    }
    let plain = plain
        .replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    plain
        .lines()
        .map(|l| {
            l.split_whitespace()
                .filter(|w| !w.starts_with('@'))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|l| !l.is_empty())
        .collect()
}

fn id_of(word: &str, prefix: char) -> Option<u32> {
    let w = word.trim_start_matches('#');
    let digits = w
        .strip_prefix(prefix)
        .or_else(|| w.strip_prefix(prefix.to_ascii_lowercase()))
        .unwrap_or(w);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // A bare number counts only with the prefix or a '#'.
    if digits.len() == w.len() && !word.starts_with('#') {
        return None;
    }
    digits.parse().ok()
}

fn reference(words: &[&str], prefix: char) -> Result<Ref, String> {
    if words.is_empty() {
        return Err(format!("which one? Name it, or use its id ({prefix}12)"));
    }
    if words.len() == 1 {
        if let Some(id) = id_of(words[0], prefix) {
            return Ok(Ref::Id(id));
        }
    }
    Ok(Ref::Name(words.join(" ").to_lowercase()))
}

fn count(word: &str) -> Option<u32> {
    word.strip_prefix('+')
        .and_then(|n| n.parse().ok())
        .filter(|n| *n <= 99)
}

/// One line → a command, or a short reason it isn't one.
pub fn parse(line: &str) -> Result<Cmd, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(first) = words.first() else {
        return Err("empty".into());
    };
    let verb = first.to_lowercase();
    let rest = &words[1..];
    let text = |ws: &[&str]| ws.join(" ");
    // An event first, then the rest.
    let event_then = |ws: &[&str]| -> Result<(Ref, Vec<String>), String> {
        let first = ws.first().ok_or("which event? Use its id (E12)")?;
        match id_of(first, 'E') {
            Some(id) => Ok((Ref::Id(id), ws[1..].iter().map(|s| s.to_string()).collect())),
            None => Err("start with the event's id (E12), then the rest".into()),
        }
    };
    let cmd = match verb.as_str() {
        "help" | "?" => Cmd::Help(text(rest).to_lowercase()),
        "me" => Cmd::Me,
        "events" | "upcoming" => {
            if rest.is_empty() {
                Cmd::Events(None)
            } else {
                Cmd::Events(Some(reference(rest, 'G')?))
            }
        }
        "event" => Cmd::Event(reference(rest, 'E')?),
        "groups" => Cmd::Groups,
        "find" => {
            if rest.is_empty() {
                return Err("find what? e.g. find hiking".into());
            }
            Cmd::Find(text(rest).to_lowercase())
        }
        "join" => Cmd::Join(reference(rest, 'G')?),
        "leave" => Cmd::Leave(reference(rest, 'G')?),
        "going" | "maybe" | "no" | "rsvp" => {
            let (status, ws) = if verb == "rsvp" {
                let (Some(e), Some(s)) = (rest.first(), rest.get(1)) else {
                    return Err("rsvp E12 going|maybe|no".into());
                };
                let s = s.to_lowercase();
                if !matches!(s.as_str(), "going" | "maybe" | "no") {
                    return Err("answer going, maybe or no".into());
                }
                let mut ws = vec![*e];
                ws.extend_from_slice(&rest[2..]);
                (s, ws)
            } else {
                (verb.clone(), rest.to_vec())
            };
            let mut plus = None;
            let mut kids = None;
            let mut note = Vec::new();
            let mut event = Vec::new();
            let mut i = 0;
            let mut by_id = false;
            while i < ws.len() {
                let w = ws[i];
                if i == 0 {
                    if let Some(id) = id_of(w, 'E') {
                        event.push(format!("E{id}"));
                        by_id = true;
                        i += 1;
                        continue;
                    }
                }
                if let Some(n) = count(w) {
                    plus = Some(n);
                } else if w.eq_ignore_ascii_case("kids")
                    && ws.get(i + 1).and_then(|n| n.parse::<u32>().ok()).is_some()
                {
                    kids = ws[i + 1].parse().ok();
                    i += 1;
                } else if w.eq_ignore_ascii_case("note") && note.is_empty() {
                    note.extend(ws[i + 1..].iter().map(|s| s.to_string()));
                    break;
                } else if by_id {
                    note.extend(ws[i..].iter().map(|s| s.to_string()));
                    break;
                } else {
                    event.push(w.to_lowercase());
                }
                i += 1;
            }
            let event = if by_id {
                Ref::Id(event[0][1..].parse().expect("digits"))
            } else if event.is_empty() {
                return Err("which event? e.g. going E12".into());
            } else {
                Ref::Name(event.join(" "))
            };
            Cmd::Rsvp {
                event,
                status,
                plus,
                kids,
                note: note.join(" "),
            }
        }
        "bring" | "unbring" => {
            let (event, item) = event_then(rest)?;
            if item.is_empty() {
                return Err(format!("{verb} E12 <what>"));
            }
            let item = item.join(" ");
            if verb == "bring" {
                Cmd::Bring { event, item }
            } else {
                Cmd::Unbring { event, item }
            }
        }
        "ride" => {
            let (event, more) = event_then(rest)?;
            let kind = more.first().map(|k| k.to_lowercase()).unwrap_or_default();
            let seats: u32 = more.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            match kind.as_str() {
                "need" | "offer" if (1..=8).contains(&seats) => Cmd::Ride { event, kind, seats },
                "need" | "offer" => Cmd::Ride {
                    event,
                    kind,
                    seats: 1,
                },
                "own" => Cmd::Ride {
                    event,
                    kind,
                    seats: 0,
                },
                _ => return Err("ride E12 need 1 | offer 3 | own".into()),
            }
        }
        "when" => {
            let (event, more) = event_then(rest)?;
            let options: Result<Vec<u32>, _> = more
                .join(",")
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().parse::<u32>())
                .collect();
            match options {
                Ok(mut o) if !o.is_empty() => {
                    o.sort_unstable();
                    o.dedup();
                    Cmd::When { event, options: o }
                }
                _ => return Err("when E12 1,3 (the numbers of the times that work)".into()),
            }
        }
        "shift" => {
            let (event, name) = event_then(rest)?;
            if name.is_empty() {
                return Err("shift E12 <name>".into());
            }
            Cmd::Shift {
                event,
                name: name.join(" "),
            }
        }
        "answer" => {
            let (event, more) = event_then(rest)?;
            let question = more
                .first()
                .and_then(|q| q.parse().ok())
                .ok_or("answer E12 <question number> <answer>")?;
            Cmd::Answer {
                event,
                question,
                value: more[1..].join(" "),
            }
        }
        "regular" => {
            let on = match rest.last().map(|w| w.to_lowercase()) {
                Some(w) if w == "on" => true,
                Some(w) if w == "off" => false,
                _ => return Err("regular S4 on|off".into()),
            };
            Cmd::Regular {
                series: reference(&rest[..rest.len() - 1], 'S')?,
                on,
            }
        }
        "redeem" => match rest {
            [token] => Cmd::Redeem(token.to_string()),
            _ => return Err("redeem <the code from your invitation>".into()),
        },
        "link" => Cmd::Link,
        "newlink" => Cmd::NewLink,
        "stop" => Cmd::Stop,
        "start" => Cmd::Start,
        "undo" => Cmd::Undo,
        "confirm" => match rest {
            [code] => Cmd::Confirm(code.to_string()),
            _ => return Err("confirm <code>".into()),
        },
        "cancel" => Cmd::Cancel,
        _ => return Err(format!("I don't know \"{verb}\"; try help")),
    };
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `spec/dsl-vectors.json`: the companion site's builder is tested
    /// against the same file.
    #[test]
    fn shared_vectors() {
        let raw = include_str!("../../spec/dsl-vectors.json");
        let mut checked = 0;
        for line in raw.lines() {
            let line = line.trim().trim_end_matches(',');
            let Some(body) = line
                .strip_prefix("{\"in\": ")
                .and_then(|l| l.strip_suffix('}'))
            else {
                continue;
            };
            // {"in": "...", "out": "..."} or {"in": "...", "error": true}
            let (input, rest) = split_json_string(body);
            let rest = rest.trim_start_matches(',').trim();
            let got = parse(&input);
            if let Some(out) = rest.strip_prefix("\"out\": ") {
                let (want, _) = split_json_string(out);
                assert_eq!(
                    got.as_ref().map(Cmd::canonical),
                    Ok(want.clone()),
                    "{input}"
                );
                // Canonical forms parse back to themselves.
                assert_eq!(
                    parse(&want).as_ref().map(Cmd::canonical),
                    Ok(want),
                    "round trip of {input}"
                );
            } else {
                assert!(got.is_err(), "{input} should not parse: {got:?}");
            }
            checked += 1;
        }
        assert!(checked >= 30, "only {checked} vectors");
    }

    fn split_json_string(s: &str) -> (String, &str) {
        let s = s.trim().strip_prefix('"').expect("a JSON string");
        let mut out = String::new();
        let mut chars = s.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, e)) = chars.next() {
                        out.push(e);
                    }
                }
                '"' => return (out, &s[i + 1..]),
                c => out.push(c),
            }
        }
        (out, "")
    }

    #[test]
    fn html_to_lines() {
        let html = "<p><span class=\"h-card\"><a href=\"x\">@<span>party</span></a></span> going E12 +1</p><p>bring E12 pie &amp; cream</p>";
        assert_eq!(plain_lines(html), ["going E12 +1", "bring E12 pie & cream"]);
    }
}
