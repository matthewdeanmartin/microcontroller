//! Mastodon sign-in on the Game Matcher side (`spec/02-accounts-auth.md`):
//! which instances, who from them may sign in, and how a Mastodon identity
//! maps to a local account. The OAuth itself is the framework's.
use crate::messages::{ProviderInput, StoredAccount, StoredIdentity, StoredProvider};
use crate::state::{normalize_name, State};
use miniframework::oauth::{Identity, Provider, Trust};
use miniframework::ApiError;

pub const TRUSTS: [&str; 2] = ["household", "invited"];
pub const ANCHORS: [&str; 2] = ["household-ca", "public"];
pub const MAX_PROVIDERS: usize = 8;
pub const MAX_INVITES: usize = 32;

/// `@User@Host` → `@user@host`; `user@host` gets its `@`.
pub fn normalize_handle(raw: &str) -> Result<String, ApiError> {
    let h = raw.trim().trim_start_matches('@').to_lowercase();
    match h.split_once('@') {
        Some((user, host)) if !user.is_empty() && host.contains('.') || host_is_local(&h) => {
            Ok(format!("@{h}"))
        }
        _ => Err(ApiError::bad_request(format!(
            "{raw:?} isn't a handle like @user@instance"
        ))),
    }
}

fn host_is_local(h: &str) -> bool {
    h.split_once('@')
        .is_some_and(|(u, host)| !u.is_empty() && !host.is_empty())
}

fn clean_base(raw: &str) -> Result<String, ApiError> {
    let base = raw.trim().trim_end_matches('/').to_string();
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))
        .ok_or_else(|| ApiError::bad_request("The instance address starts with https://"))?;
    if rest.is_empty() || rest.contains('/') || rest.contains(char::is_whitespace) {
        return Err(ApiError::bad_request(
            "Just the instance's address, like https://mastodon.social",
        ));
    }
    Ok(base)
}

/// The framework's view of a stored provider.
pub fn to_provider(p: &StoredProvider) -> Provider {
    Provider {
        id: p.id.to_string(),
        base: p.base.clone(),
        trust: if p.anchor == "household-ca" {
            Trust::Household
        } else {
            Trust::Public
        },
    }
}

fn host_of(base: &str) -> &str {
    base.split_once("://")
        .map_or(base, |(_, r)| r)
        .split(':')
        .next()
        .unwrap_or("")
}

impl State {
    pub fn provider(&self, id: u32) -> Result<&StoredProvider, ApiError> {
        self.providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ApiError::not_found("No such instance"))
    }

    fn apply_provider(p: &mut StoredProvider, input: &ProviderInput) -> Result<(), ApiError> {
        if let Some(name) = &input.name {
            p.name = normalize_name(name)?;
        }
        if let Some(base) = &input.base {
            p.base = clean_base(base)?;
        }
        if let Some(trust) = &input.trust {
            if !TRUSTS.contains(&trust.as_str()) {
                return Err(ApiError::bad_request("trust: household or invited"));
            }
            p.trust = trust.clone();
        }
        if let Some(anchor) = &input.anchor {
            if !ANCHORS.contains(&anchor.as_str()) {
                return Err(ApiError::bad_request("anchor: household-ca or public"));
            }
            p.anchor = anchor.clone();
        }
        if let Some(invites) = &input.invites {
            let mut list: Vec<String> = Vec::new();
            for raw in invites.iter().filter(|h| !h.trim().is_empty()) {
                let h = normalize_handle(raw)?;
                if !list.contains(&h) {
                    list.push(h);
                }
            }
            if list.len() > MAX_INVITES {
                return Err(ApiError::bad_request(format!(
                    "Up to {MAX_INVITES} invited handles"
                )));
            }
            p.invites = list;
        }
        Ok(())
    }

    pub fn add_provider(&mut self, input: &ProviderInput) -> Result<u32, ApiError> {
        if self.providers.len() >= MAX_PROVIDERS {
            return Err(ApiError::new(
                409,
                "full",
                format!("Up to {MAX_PROVIDERS} instances"),
            ));
        }
        let base = input
            .base
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("Which instance? (https://...)"))?;
        let host = clean_base(base)?;
        let mut p = StoredProvider {
            id: self.next_provider.max(1),
            name: host_of(&host).to_string(),
            trust: "invited".into(),
            anchor: "public".into(),
            ..Default::default()
        };
        Self::apply_provider(&mut p, input)?;
        if self.providers.iter().any(|q| q.base == p.base) {
            return Err(ApiError::new(
                409,
                "duplicate",
                "That instance is already set up",
            ));
        }
        self.next_provider = p.id + 1;
        let id = p.id;
        self.providers.push(p);
        Ok(id)
    }

    pub fn edit_provider(&mut self, id: u32, input: &ProviderInput) -> Result<(), ApiError> {
        let mut p = self.provider(id)?.clone();
        Self::apply_provider(&mut p, input)?;
        if self
            .providers
            .iter()
            .any(|q| q.id != id && q.base == p.base)
        {
            return Err(ApiError::new(
                409,
                "duplicate",
                "That instance is already set up",
            ));
        }
        *self
            .providers
            .iter_mut()
            .find(|q| q.id == id)
            .expect("found above") = p;
        Ok(())
    }

    pub fn delete_provider(&mut self, id: u32) -> Result<(), ApiError> {
        self.provider(id)?;
        self.providers.retain(|p| p.id != id);
        Ok(())
    }

    fn account_with(&self, identity: &Identity) -> Option<&StoredAccount> {
        self.accounts.iter().find(|a| {
            a.mastodon
                .as_ref()
                .is_some_and(|m| m.host == identity.host && m.subject == identity.subject)
        })
    }

    /// Signs in with a Mastodon identity: its linked account, or a new one if
    /// the instance's trust allows. Returns the account id.
    pub fn sign_in_with(
        &mut self,
        provider: u32,
        identity: &Identity,
        now: u64,
    ) -> Result<u32, ApiError> {
        if let Some(a) = self.account_with(identity) {
            if a.disabled {
                return Err(ApiError::new(403, "disabled", "This account is disabled"));
            }
            let id = a.id;
            // Keep the display handle current (usernames can change).
            if let Ok(a) = self.account_mut(id) {
                if let Some(m) = a.mastodon.as_mut() {
                    m.handle = identity.handle.clone();
                }
                a.last_seen = now;
            }
            return Ok(id);
        }
        if self.accounts.is_empty() {
            return Err(ApiError::new(
                409,
                "setup_needed",
                "Set up Game Matcher with a PIN first",
            ));
        }
        let p = self.provider(provider)?;
        let handle = normalize_handle(&identity.handle)?;
        let allowed = match p.trust.as_str() {
            "household" => !self.signup_closed || p.invites.contains(&handle),
            _ => p.invites.contains(&handle),
        };
        if !allowed {
            return Err(ApiError::new(
                403,
                "not_invited",
                format!("{} isn't invited here yet; ask the admin", identity.handle),
            ));
        }
        if self.accounts.len() >= self.limits.accounts {
            return Err(ApiError::new(
                409,
                "full",
                "This board has no room for more accounts",
            ));
        }
        let display_name = normalize_name(&identity.display_name)
            .or_else(|_| normalize_name(&identity.handle.chars().take(32).collect::<String>()))?;
        let id = self.next_account;
        self.accounts.push(StoredAccount {
            id,
            display_name,
            created: now,
            last_seen: now,
            mastodon: Some(StoredIdentity {
                host: identity.host.clone(),
                subject: identity.subject.clone(),
                handle: identity.handle.clone(),
            }),
            ..Default::default()
        });
        self.next_account += 1;
        Ok(id)
    }

    /// Links a Mastodon identity to `account` (replacing any earlier link).
    pub fn link_identity(&mut self, account: u32, identity: &Identity) -> Result<(), ApiError> {
        if let Some(other) = self.account_with(identity) {
            if other.id != account {
                return Err(ApiError::new(
                    409,
                    "linked_elsewhere",
                    format!(
                        "{} is already linked to {}",
                        identity.handle, other.display_name
                    ),
                ));
            }
        }
        self.account_mut(account)?.mastodon = Some(StoredIdentity {
            host: identity.host.clone(),
            subject: identity.subject.clone(),
            handle: identity.handle.clone(),
        });
        Ok(())
    }

    /// Unlinks; refused for an account with no PIN (it would be locked out).
    pub fn unlink_identity(&mut self, account: u32) -> Result<(), ApiError> {
        let a = self.account_mut(account)?;
        if a.pin.is_empty() {
            return Err(ApiError::new(
                409,
                "needs_pin",
                "Set a PIN first, or you'd have no way to sign in",
            ));
        }
        a.mastodon = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Limits;

    fn identity(subject: &str, handle: &str) -> Identity {
        Identity {
            provider: "1".into(),
            host: handle.rsplit('@').next().unwrap().into(),
            subject: subject.into(),
            handle: handle.into(),
            display_name: "Sam S.".into(),
            avatar: String::new(),
        }
    }

    fn world() -> (State, u32, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            ..Limits::board()
        });
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let home = s
            .add_provider(&ProviderInput {
                name: Some("Home".into()),
                base: Some("https://mastomini.local/".into()),
                trust: Some("household".into()),
                anchor: Some("household-ca".into()),
                ..Default::default()
            })
            .unwrap();
        let social = s
            .add_provider(&ProviderInput {
                base: Some("https://mastodon.social".into()),
                invites: Some(vec!["Bob@Mastodon.Social".into()]),
                ..Default::default()
            })
            .unwrap();
        (s, dad, home, social)
    }

    #[test]
    fn providers_validate() {
        let (mut s, _, home, social) = world();
        assert_eq!(s.provider(home).unwrap().base, "https://mastomini.local");
        let p = s.provider(social).unwrap();
        assert_eq!(
            (p.name.as_str(), p.trust.as_str(), p.anchor.as_str()),
            ("mastodon.social", "invited", "public")
        );
        assert_eq!(p.invites, vec!["@bob@mastodon.social"]);
        let dup = ProviderInput {
            base: Some("https://mastodon.social/".into()),
            ..Default::default()
        };
        assert_eq!(s.add_provider(&dup).unwrap_err().code, "duplicate");
        let bad = ProviderInput {
            base: Some("mastodon.social".into()),
            ..Default::default()
        };
        assert_eq!(s.add_provider(&bad).unwrap_err().status, 400);
        let bad = ProviderInput {
            trust: Some("everyone".into()),
            ..Default::default()
        };
        assert_eq!(s.edit_provider(home, &bad).unwrap_err().status, 400);
        assert!(matches!(
            to_provider(s.provider(home).unwrap()).trust,
            Trust::Household
        ));
    }

    #[test]
    fn household_instance_creates_accounts_once() {
        let (mut s, _, home, _) = world();
        let sam = identity("42", "@sam@mastomini.local");
        let id = s.sign_in_with(home, &sam, 5).unwrap();
        let a = s.account(id).unwrap();
        assert_eq!(
            (a.display_name.as_str(), a.email.as_str(), a.pin.as_str()),
            ("Sam S.", "", "")
        );
        // Same id, renamed: the same account, handle updated.
        let renamed = identity("42", "@samuel@mastomini.local");
        assert_eq!(s.sign_in_with(home, &renamed, 6).unwrap(), id);
        assert_eq!(
            s.account(id).unwrap().mastodon.as_ref().unwrap().handle,
            "@samuel@mastomini.local"
        );
        // A closed sign-up stops new household accounts too.
        s.signup_closed = true;
        assert_eq!(
            s.sign_in_with(home, &identity("43", "@kim@mastomini.local"), 7)
                .unwrap_err()
                .code,
            "not_invited"
        );
        // An empty email never matches PIN sign-in.
        assert!(s.by_email("").is_none());
    }

    #[test]
    fn invited_instances_need_the_handle() {
        let (mut s, _, _, social) = world();
        let stranger = identity("9", "@eve@mastodon.social");
        assert_eq!(
            s.sign_in_with(social, &stranger, 1).unwrap_err().code,
            "not_invited"
        );
        let bob = identity("7", "@Bob@mastodon.social");
        assert!(s.sign_in_with(social, &bob, 1).is_ok());
    }

    #[test]
    fn link_and_unlink() {
        let (mut s, dad, home, _) = world();
        let mine = identity("1", "@dad@mastomini.local");
        s.link_identity(dad, &mine).unwrap();
        assert_eq!(s.sign_in_with(home, &mine, 2).unwrap(), dad);
        let kid = s.signup("kid@h", "Kid", "1111", 0).unwrap();
        assert_eq!(
            s.link_identity(kid, &mine).unwrap_err().code,
            "linked_elsewhere"
        );
        s.unlink_identity(dad).unwrap();
        let sam = s
            .sign_in_with(home, &identity("42", "@sam@mastomini.local"), 3)
            .unwrap();
        assert_eq!(s.unlink_identity(sam).unwrap_err().code, "needs_pin");
        s.account_mut(dad).unwrap().disabled = true;
        s.link_identity(dad, &mine).unwrap();
        assert_eq!(s.sign_in_with(home, &mine, 4).unwrap_err().code, "disabled");
    }
}
