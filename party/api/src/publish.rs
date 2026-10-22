//! Publication to the companion site's data repo (`spec/05-companion-site.md#publication`).
//!
//! The files are built from the same access rules as everything else: an
//! event is published only if a signed-out viewer could see it. Only
//! `public`-class fields exist in the published types ([`NOT_PUBLIC`] and the
//! test that walks them keep it so).
//!
//! The push is one parentless commit through GitHub's Git Data API, so the
//! branch is always one commit deep and keeps no history of old files.
use crate::mail::json_string;
use crate::messages::{
    GitCommit, GitObject, GitRef, Manifest, PublicEvent, PublicEvents, PublicGroup, PublicGroups,
    PublicPeople, PublicPerson, StoredFile, StoredPublish,
};
use crate::state::{State, ACTIVE};
use miniframework::fetch::{Fetch, Outgoing};
use miniframework::wire::{self, Format, Schema, Ty};

/// Field names that must never appear in a published type, at any depth.
pub const NOT_PUBLIC: &[&str] = &[
    "address",
    "email",
    "pin",
    "token",
    "token_hash",
    "diet",
    "invitees",
    "names",
    "note",
    "answers",
    "identities",
    "personal_key",
    "feed_hash",
    "api_key",
    "map",
    "link",
    "hosts",
    "household",
    "rsvps",
    "members",
];

/// Published events: this far ahead, and this long after they ended.
const AHEAD: u64 = 120 * 86_400;
const BEHIND: u64 = 7 * 86_400;

/// The names in `schema` (and messages inside it) that aren't public.
pub fn leaks(schema: &'static Schema) -> Vec<String> {
    fn walk(s: &'static Schema, path: &str, out: &mut Vec<String>, depth: u32) {
        if depth > 6 {
            return;
        }
        for f in s.fields {
            let here = format!("{path}{}.{}", s.name, f.name);
            if NOT_PUBLIC.contains(&f.name) {
                out.push(here.clone());
            }
            let mut ty = (f.ty)();
            loop {
                match ty {
                    Ty::List(t) | Ty::Opt(t) => ty = *t,
                    Ty::Msg(m) => {
                        walk(m(), &format!("{here}/"), out, depth + 1);
                        break;
                    }
                    _ => break,
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(schema, "", &mut out, 0);
    out
}

/// Every type the publisher writes.
pub fn published_schemas() -> [&'static Schema; 4] {
    use miniframework::Message;
    [
        Manifest::SCHEMA,
        PublicGroups::SCHEMA,
        PublicEvents::SCHEMA,
        PublicPeople::SCHEMA,
    ]
}

/// A publication to push: the settings, all files, which changed (by
/// index), and which paths went away.
pub type Due = (StoredPublish, Vec<File>, Vec<usize>, Vec<String>);

pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
}

fn json<M: miniframework::wire::Encode>(m: &M) -> Vec<u8> {
    wire::to_vec(Format::Json, m).unwrap_or_default()
}

impl State {
    /// The files a publication contains, the manifest last.
    pub fn published_files(&self, generation: u32, now: u64) -> Vec<File> {
        let groups: Vec<PublicGroup> = self
            .groups
            .iter()
            .filter(|g| g.visibility == "public" && g.status == ACTIVE)
            .map(|g| PublicGroup {
                id: g.id,
                slug: g.slug.clone(),
                name: g.name.clone(),
                about: g.about.clone(),
                kind: g.kind.clone(),
                join: g.join.clone(),
                member_count: g.members.iter().filter(|m| m.status == ACTIVE).count() as u32,
                hashtag: String::new(),
            })
            .collect();
        let events: Vec<PublicEvent> = self
            .events
            .iter()
            .filter(|e| self.event_access(None, e).see)
            .filter(|e| e.polling() || (e.start < now + AHEAD && e.effective_end() + BEHIND > now))
            .map(|e| PublicEvent {
                id: e.id,
                group: e.group,
                title: e.title.clone(),
                about: e.about.clone(),
                kind: e.kind.clone(),
                start: e.start,
                end: e.end,
                area: if e.where_kind == "place" {
                    e.place.clone()
                } else {
                    String::new()
                },
                status: e.status.clone(),
                going: self.heads(e.id),
                capacity: e.capacity,
                series: e.series,
            })
            .collect();
        let people: Vec<PublicPerson> = self
            .people
            .iter()
            .filter(|p| p.status == ACTIVE && p.discoverable == "public")
            .map(|p| PublicPerson {
                id: p.id,
                display_name: p.display_name.clone(),
                bio: p.bio.clone(),
            })
            .collect();
        let mut files = vec![
            // GitHub Pages serves the files as they are (no Jekyll).
            File {
                path: ".nojekyll".into(),
                bytes: Vec::new(),
            },
            File {
                path: "data/public/groups.json".into(),
                bytes: json(&PublicGroups {
                    groups: groups.clone(),
                }),
            },
            File {
                path: "data/public/events.json".into(),
                bytes: json(&PublicEvents { events }),
            },
            File {
                path: "data/public/people.json".into(),
                bytes: json(&PublicPeople { people }),
            },
        ];
        for g in &groups {
            let list: Vec<&crate::messages::StoredEvent> = self
                .events
                .iter()
                .filter(|e| e.group == g.id && e.status != "draft" && !e.polling())
                .filter(|e| self.event_access(None, e).see)
                .collect();
            files.push(File {
                path: format!("data/ics/g{}.ics", g.id),
                bytes: crate::ics::public_feed(self, &g.name, &list).into_bytes(),
            });
        }
        let manifest = Manifest {
            v: 1,
            generation,
            created: now,
            instance: "party".into(),
            bot: self
                .bot()
                .filter(|b| b.state == "ready")
                .map(|b| b.handle.clone())
                .unwrap_or_default(),
            public: files.iter().map(|f| f.path.clone()).collect(),
        };
        files.push(File {
            path: "data/manifest.json".into(),
            bytes: json(&manifest),
        });
        files
    }

    /// Whether a publication is due; when it is, what to push: all files,
    /// which of them changed, and which paths went away.
    pub fn publication_due(&self, now: u64) -> Option<Due> {
        let p = self.publish()?.clone();
        if p.token.is_empty() || p.repo.is_empty() {
            return None;
        }
        let every = u64::from(p.every_minutes.max(10)) * 60;
        if !p.asked && now < p.last_attempt + every {
            return None;
        }
        let files = self.published_files(p.generation + 1, now);
        let crc_of = |path: &str| p.files.iter().find(|f| f.path == path).map(|f| f.crc);
        // The manifest's timestamp always changes; it only matters with something else.
        let changed: Vec<usize> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.path == "data/manifest.json" || crc_of(&f.path) != Some(crc32fast::hash(&f.bytes))
            })
            .map(|(i, _)| i)
            .collect();
        let removed: Vec<String> = p
            .files
            .iter()
            .filter(|f| !files.iter().any(|n| n.path == f.path))
            .map(|f| f.path.clone())
            .collect();
        let content_changed = changed.len() > 1 || !removed.is_empty() || p.files.is_empty();
        if !content_changed && !p.asked {
            return None;
        }
        Some((p, files, changed, removed))
    }

    /// Records a publication attempt (and its files' hashes when it worked).
    pub fn publication_done(&mut self, files: &[File], result: Result<String, String>, now: u64) {
        let ch = self.channel_mut(crate::outbound::PUBLISH);
        let Some(p) = ch.publish.as_mut() else { return };
        p.last_attempt = now;
        p.asked = false;
        match result {
            Ok(commit) => {
                p.generation += 1;
                p.last_published = now;
                p.last_commit = commit;
                p.last_error.clear();
                p.files = files
                    .iter()
                    .map(|f| StoredFile {
                        path: f.path.clone(),
                        crc: crc32fast::hash(&f.bytes),
                    })
                    .collect();
            }
            Err(e) => p.last_error = e.chars().take(200).collect(),
        }
    }

    /// No content changed since the last check: just note the check (RAM-free:
    /// the attempt time only matters for the cadence).
    pub fn publication_checked(&mut self, now: u64) {
        if let Some(p) = self.channel_mut(crate::outbound::PUBLISH).publish.as_mut() {
            p.last_attempt = now;
        }
    }
}

const GITHUB: &str = "https://api.github.com";

fn call(
    fetch: &dyn Fetch,
    token: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Result<(u16, Vec<u8>), String> {
    let auth = format!("Bearer {token}");
    let url = format!("{GITHUB}{path}");
    let reply = fetch
        .send(&Outgoing {
            method,
            url: &url,
            headers: &[
                ("Authorization", &auth),
                ("Accept", "application/vnd.github+json"),
                ("X-GitHub-Api-Version", "2022-11-28"),
                ("User-Agent", "party.local"),
                ("Content-Type", "application/json"),
            ],
            body: body.as_bytes(),
            limit: 256 * 1024,
        })
        .map_err(|e| format!("{method} {path}: {e}"))?;
    Ok((reply.status, reply.body))
}

fn ok(step: &str, (status, body): (u16, Vec<u8>)) -> Result<Vec<u8>, String> {
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        let detail: String = String::from_utf8_lossy(&body).chars().take(160).collect();
        Err(match status {
            401 => format!("{step}: GitHub refused the token (expired or wrong?)"),
            403 | 404 => format!("{step}: HTTP {status}; does the token have Contents read/write on this repo? {detail}"),
            _ => format!("{step}: HTTP {status} {detail}"),
        })
    }
}

fn sha<M: miniframework::Message>(
    body: &[u8],
    pick: impl Fn(M) -> String,
) -> Result<String, String> {
    wire::decode::<M>(Format::Json, body)
        .map(pick)
        .map_err(|e| e.to_string())
}

/// Pushes `files` (only the `changed` ones are uploaded; `removed` paths are
/// deleted) as one parentless commit, then points the branch at it.
/// Returns the commit's sha.
pub fn push(
    fetch: &dyn Fetch,
    p: &StoredPublish,
    files: &[File],
    changed: &[usize],
    removed: &[String],
) -> Result<String, String> {
    let repo = format!("/repos/{}", p.repo.trim_matches('/'));
    let branch = if p.branch.is_empty() {
        "main"
    } else {
        p.branch.as_str()
    };
    let (status, body) = call(
        fetch,
        &p.token,
        "GET",
        &format!("{repo}/git/ref/heads/{branch}"),
        "",
    )?;
    let base_tree = match status {
        200 => {
            let head = sha::<GitRef>(&body, |r| r.object.sha)?;
            let commit = ok(
                "read the head commit",
                call(
                    fetch,
                    &p.token,
                    "GET",
                    &format!("{repo}/git/commits/{head}"),
                    "",
                )?,
            )?;
            Some(sha::<GitCommit>(&commit, |c| c.tree.sha)?)
        }
        404 | 409 => None,
        _ => return Err(ok("read the branch", (status, body)).unwrap_err()),
    };
    // Without a tree to build on, every file goes up.
    let upload: Vec<usize> = if base_tree.is_some() {
        changed.to_vec()
    } else {
        (0..files.len()).collect()
    };
    let mut entries = Vec::new();
    for &i in &upload {
        let f = &files[i];
        let mut body = String::from("{\"encoding\":\"base64\",\"content\":");
        json_string(&mut body, &crate::mail::base64(&f.bytes));
        body.push('}');
        let blob = ok(
            "upload a file",
            call(fetch, &p.token, "POST", &format!("{repo}/git/blobs"), &body)?,
        )?;
        let blob = sha::<GitObject>(&blob, |o| o.sha)?;
        let mut e = String::from("{\"path\":");
        json_string(&mut e, &f.path);
        e.push_str(&format!(
            ",\"mode\":\"100644\",\"type\":\"blob\",\"sha\":\"{blob}\"}}"
        ));
        entries.push(e);
    }
    if base_tree.is_some() {
        for path in removed {
            let mut e = String::from("{\"path\":");
            json_string(&mut e, path);
            e.push_str(",\"mode\":\"100644\",\"type\":\"blob\",\"sha\":null}");
            entries.push(e);
        }
    }
    let mut tree = String::from("{");
    if let Some(base) = &base_tree {
        tree.push_str(&format!("\"base_tree\":\"{base}\","));
    }
    tree.push_str(&format!("\"tree\":[{}]}}", entries.join(",")));
    let tree = ok(
        "write the tree",
        call(fetch, &p.token, "POST", &format!("{repo}/git/trees"), &tree)?,
    )?;
    let tree = sha::<GitObject>(&tree, |o| o.sha)?;
    let commit =
        format!("{{\"message\":\"party publication\",\"tree\":\"{tree}\",\"parents\":[]}}");
    let commit = ok(
        "write the commit",
        call(
            fetch,
            &p.token,
            "POST",
            &format!("{repo}/git/commits"),
            &commit,
        )?,
    )?;
    let commit = sha::<GitObject>(&commit, |o| o.sha)?;
    if base_tree.is_some() {
        let body = format!("{{\"sha\":\"{commit}\",\"force\":true}}");
        ok(
            "move the branch",
            call(
                fetch,
                &p.token,
                "PATCH",
                &format!("{repo}/git/refs/heads/{branch}"),
                &body,
            )?,
        )?;
    } else {
        let body = format!("{{\"ref\":\"refs/heads/{branch}\",\"sha\":\"{commit}\"}}");
        ok(
            "create the branch",
            call(fetch, &p.token, "POST", &format!("{repo}/git/refs"), &body)?,
        )?;
    }
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniframework::Message;

    #[test]
    fn published_types_hold_only_public_fields() {
        for s in published_schemas() {
            assert!(leaks(s).is_empty(), "{} leaks {:?}", s.name, leaks(s));
        }
        // The check catches what it should (a deliberate failing case).
        let stored = leaks(crate::messages::StoredEvent::SCHEMA);
        assert!(stored.iter().any(|f| f.ends_with(".address")), "{stored:?}");
        let nested = leaks(crate::messages::StoredChannel::SCHEMA);
        assert!(
            nested.iter().any(|f| f.contains("/StoredMail.api_key")),
            "{nested:?}"
        );
    }
}
