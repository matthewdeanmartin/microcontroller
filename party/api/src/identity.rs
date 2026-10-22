//! Mastodon identities (`spec/02-identity-auth.md#sign-in-methods`): the
//! instances people may sign in with, and what a verified identity means
//! for this board. Signing in never makes a second person by accident: an
//! identity belongs to at most one person, and a person has at most three.
use crate::invites::INVITED;
use crate::messages::{ProviderInput, StoredIdentity, StoredProvider};
use crate::outbound::PROVIDERS;
use crate::state::{State, ACTIVE};
use miniframework::oauth::{Identity, Provider, Trust};
use miniframework::ApiError;

pub const MAX_PROVIDERS: usize = 8;
pub const MAX_IDENTITIES: usize = 3;

/// `https://host` with no path or trailing slash.
pub fn clean_base(raw: &str) -> Result<String, ApiError> {
    let b = raw.trim().trim_end_matches('/');
    let b = if b.contains("://") {
        b.to_string()
    } else {
        format!("https://{b}")
    };
    let host = b.strip_prefix("https://").unwrap_or("");
    if host.is_empty() || host.contains('/') || !host.contains('.') {
        return Err(ApiError::bad_request(
            "An instance looks like https://mastodon.social",
        ));
    }
    Ok(b.to_lowercase())
}

pub fn host_of(base: &str) -> &str {
    base.split("://").nth(1).unwrap_or(base)
}

pub fn to_provider(p: &StoredProvider) -> Provider {
    Provider {
        id: p.id.to_string(),
        base: p.base.clone(),
        trust: if p.trust == "household" {
            Trust::Household
        } else {
            Trust::Public
        },
    }
}

/// What a finished sign-in means.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Sign this person in.
    SignedIn(u32),
    /// Linked to the signed-in person.
    Linked(u32),
}

impl State {
    pub fn provider(&self, id: u32) -> Result<&StoredProvider, ApiError> {
        self.providers()
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ApiError::not_found("No such instance"))
    }

    pub fn add_provider(
        &mut self,
        actor: u32,
        input: &ProviderInput,
        now: u64,
    ) -> Result<u32, ApiError> {
        let base = clean_base(&input.base)?;
        let name = input.name.trim();
        let name = if name.is_empty() {
            host_of(&base).to_string()
        } else {
            name.chars().take(40).collect()
        };
        let trust = match input.trust.as_str() {
            "" | "public" => "public",
            "household" => "household",
            _ => return Err(ApiError::bad_request("trust is public or household")),
        };
        let list = &self.providers();
        if list.iter().any(|p| p.base == base) {
            return Err(ApiError::new(
                409,
                "exists",
                "That instance is already listed",
            ));
        }
        if list.len() >= MAX_PROVIDERS {
            return Err(crate::state::full("instances", MAX_PROVIDERS));
        }
        let id = list.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        self.channel_mut(PROVIDERS).providers.push(StoredProvider {
            id,
            name,
            base,
            trust: trust.into(),
        });
        self.log(actor, "provider.add", format!("provider {id}"), now);
        Ok(id)
    }

    pub fn delete_provider(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        self.provider(id)?;
        self.channel_mut(PROVIDERS).providers.retain(|p| p.id != id);
        self.log(actor, "provider.delete", format!("provider {id}"), now);
        Ok(())
    }

    pub fn person_with(&self, host: &str, subject: &str) -> Option<u32> {
        self.people
            .iter()
            .find(|p| {
                p.identities
                    .iter()
                    .any(|i| i.host.eq_ignore_ascii_case(host) && i.subject == subject)
            })
            .map(|p| p.id)
    }

    /// Who sent a DM: on the bot's own instance the account id is theirs;
    /// from elsewhere the instance only knows the handle, which Mastodon
    /// never reassigns.
    pub fn person_by_sender(&self, bot_host: &str, account_id: &str, acct: &str) -> Option<u32> {
        if !acct.contains('@') {
            return self.person_with(bot_host, account_id);
        }
        let handle = format!("@{}", acct.trim_start_matches('@').to_lowercase());
        self.people
            .iter()
            .find(|p| p.identities.iter().any(|i| i.handle == handle))
            .map(|p| p.id)
    }

    fn stored(identity: &Identity) -> StoredIdentity {
        StoredIdentity {
            host: identity.host.to_lowercase(),
            subject: identity.subject.clone(),
            handle: identity.handle.to_lowercase(),
        }
    }

    /// Links `identity` to `person` (who proved who they are some other way).
    pub fn link_identity(
        &mut self,
        person: u32,
        identity: &Identity,
        now: u64,
    ) -> Result<(), ApiError> {
        match self.person_with(&identity.host, &identity.subject) {
            Some(p) if p == person => return Ok(()),
            Some(_) => {
                return Err(ApiError::new(
                    409,
                    "identity_taken",
                    "That Mastodon account is linked to someone else here",
                ))
            }
            None => {}
        }
        let p = self.person_mut(person)?;
        if p.identities.len() >= MAX_IDENTITIES {
            return Err(crate::state::full(
                "linked accounts per person",
                MAX_IDENTITIES,
            ));
        }
        p.identities.push(Self::stored(identity));
        self.log(person, "identity.link", identity.handle.clone(), now);
        Ok(())
    }

    pub fn unlink_identity(&mut self, person: u32, handle: &str, now: u64) -> Result<(), ApiError> {
        let p = self.person_mut(person)?;
        let before = p.identities.len();
        p.identities.retain(|i| i.handle != handle);
        if p.identities.len() == before {
            return Err(ApiError::not_found("No such linked account"));
        }
        if p.pin.is_empty() && p.identities.is_empty() {
            return Err(ApiError::new(
                409,
                "last_way_in",
                "Set a PIN first: this is your only way in",
            ));
        }
        self.log(person, "identity.unlink", handle.to_string(), now);
        Ok(())
    }

    /// The finish of a sign-in, by its intent: `signin`, `link:<person>` or
    /// `redeem:<token>`.
    pub fn finish_sign_in(
        &mut self,
        identity: &Identity,
        intent: &str,
        now: u64,
    ) -> Result<Outcome, ApiError> {
        if let Some(person) = intent.strip_prefix("link:") {
            let person: u32 = person
                .parse()
                .map_err(|_| ApiError::bad_request("Bad sign-in"))?;
            self.link_identity(person, identity, now)?;
            return Ok(Outcome::Linked(person));
        }
        if let Some(token) = intent.strip_prefix("redeem:") {
            let person = match self.person_with(&identity.host, &identity.subject) {
                Some(p) => p,
                None => match self.invite_person(token, now) {
                    // The email-only relative the invitation names.
                    Some(p) => {
                        self.link_identity(p, identity, now)?;
                        p
                    }
                    None => self.person_from(identity, now)?,
                },
            };
            let p = self.person_mut(person)?;
            if p.status == INVITED {
                p.status = ACTIVE.into();
            }
            if p.status != ACTIVE {
                return Err(ApiError::new(403, "disabled", "This account is disabled"));
            }
            self.redeem(person, token, now)?;
            return Ok(Outcome::SignedIn(person));
        }
        let person = self.person_with(&identity.host, &identity.subject).ok_or_else(|| {
            ApiError::new(
                403,
                "not_linked",
                "That Mastodon account isn't linked here yet: sign in with your PIN and link it on your profile, or use an invitation",
            )
        })?;
        let p = self.person_mut(person)?;
        if p.status != ACTIVE {
            return Err(ApiError::new(403, "disabled", "This account is disabled"));
        }
        if let Some(i) = p
            .identities
            .iter_mut()
            .find(|i| i.subject == identity.subject)
        {
            // Handles can change by instance migration; keep the newest.
            i.handle = identity.handle.to_lowercase();
        }
        p.last_seen = now;
        Ok(Outcome::SignedIn(person))
    }

    /// A new person for an invited Mastodon account (no PIN).
    fn person_from(&mut self, identity: &Identity, now: u64) -> Result<u32, ApiError> {
        if self.people.len() >= self.limits.people {
            return Err(crate::state::full("people", self.limits.people));
        }
        // Their display name, made unique if someone here has it already.
        let base: String = identity
            .display_name
            .trim()
            .chars()
            .filter(|c| *c != '@' && !c.is_control())
            .take(36)
            .collect();
        let base = if base.trim().is_empty() {
            identity
                .handle
                .trim_start_matches('@')
                .split('@')
                .next()
                .unwrap_or("guest")
                .to_string()
        } else {
            base.trim().to_string()
        };
        let mut name = base.clone();
        let mut n = 2;
        while self.check_unique(0, &name, "").is_err() {
            name = format!("{base} {n}");
            n += 1;
        }
        let id = self.next_person;
        self.next_person += 1;
        self.people.push(crate::messages::StoredPerson {
            id,
            display_name: name,
            status: ACTIVE.into(),
            discoverable: "members".into(),
            created: now,
            last_seen: now,
            identities: vec![Self::stored(identity)],
            ..Default::default()
        });
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{GroupInput, InviteInput, NewPerson};
    use crate::state::Limits;

    fn id(subject: &str, handle: &str) -> Identity {
        Identity {
            provider: "1".into(),
            host: "mastodon.social".into(),
            subject: subject.into(),
            handle: handle.into(),
            display_name: "Sam".into(),
            avatar: String::new(),
        }
    }

    #[test]
    fn sign_in_link_and_redeem() {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            ..Limits::board()
        });
        let dad = s.setup("Dad", "", "1234", 0).unwrap();
        s.signup("Sam", "", "1234", 0).unwrap();
        assert_eq!(
            s.finish_sign_in(&id("7", "@sam@mastodon.social"), "signin", 1)
                .unwrap_err()
                .code,
            "not_linked"
        );
        assert_eq!(
            s.finish_sign_in(&id("7", "@sam@mastodon.social"), &format!("link:{dad}"), 1)
                .unwrap(),
            Outcome::Linked(dad)
        );
        assert_eq!(
            s.finish_sign_in(&id("7", "@sam@mastodon.social"), "signin", 2)
                .unwrap(),
            Outcome::SignedIn(dad)
        );
        let kid = s.signup("Kid", "", "1234", 0).unwrap();
        assert_eq!(
            s.link_identity(kid, &id("7", "@sam@mastodon.social"), 3)
                .unwrap_err()
                .code,
            "identity_taken"
        );
        // DMs: by id on the bot's instance, by handle from elsewhere.
        assert_eq!(s.person_by_sender("mastodon.social", "7", "sam"), Some(dad));
        assert_eq!(
            s.person_by_sender("other.example", "999", "sam@mastodon.social"),
            Some(dad)
        );
        assert_eq!(
            s.person_by_sender("other.example", "7", "sam@evil.example"),
            None
        );
        // An invitation made for an email-only person becomes theirs.
        let g = s
            .create_group(
                dad,
                &GroupInput {
                    name: Some("Family".into()),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let gran = s
            .add_person(
                dad,
                &NewPerson {
                    display_name: "Gran".into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let inv = s
            .create_invite(
                dad,
                &InviteInput {
                    target_kind: "group".into(),
                    target: g,
                    person: gran,
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let out = s
            .finish_sign_in(
                &id("42", "@gran@mastodon.social"),
                &format!("redeem:{}", inv.token),
                5,
            )
            .unwrap();
        assert_eq!(out, Outcome::SignedIn(gran));
        assert_eq!(s.person(gran).unwrap().status, ACTIVE);
        // A stranger with an open invitation gets a new person, named uniquely.
        let open = s
            .create_invite(
                dad,
                &InviteInput {
                    target_kind: "group".into(),
                    target: g,
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let out = s
            .finish_sign_in(
                &id("43", "@sam2@mastodon.social"),
                &format!("redeem:{}", open.token),
                6,
            )
            .unwrap();
        let Outcome::SignedIn(new) = out else {
            panic!()
        };
        assert_eq!(s.person(new).unwrap().display_name, "Sam 2");
        assert!(s.membership(s.group(g).unwrap(), new).is_some());
        assert_eq!(
            clean_base("Mastodon.Social/").unwrap(),
            "https://mastodon.social"
        );
    }
}
