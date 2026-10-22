//! Invitations (`spec/02-identity-auth.md#invitations`): a token (only its
//! hash is stored) with a target, an expiry and a use count, delivered by
//! email, DM or a link the organizer copies.
//!
//! An invitation may name a person (an email-only relative an organizer
//! made): they're added at once, and redeeming it later activates that
//! person rather than making a second one.
use crate::messages::{InviteInput, InvitePreview, InviteView, NewPerson, StoredInvite, StoredJob};
use crate::state::{full, normalize_email, normalize_name, State, ACTIVE};
use miniframework::ApiError;

pub const OPEN: &str = "open";
pub const USED: &str = "used";
pub const REVOKED: &str = "revoked";
pub const INVITED: &str = "invited";

/// SHA-256 (hex) of a token.
pub fn token_hash(token: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn new_token() -> Result<String, ApiError> {
    miniframework::session::random_token()
        .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))
}

/// `@user@host`, lower-cased; "" stays "".
pub fn normalize_handle(raw: &str) -> Result<String, ApiError> {
    let h = raw.trim().trim_start_matches('@').to_lowercase();
    if h.is_empty() {
        return Ok(h);
    }
    match h.split_once('@') {
        Some((user, host)) if !user.is_empty() && host.contains('.') && !host.contains('@') => {
            Ok(format!("@{h}"))
        }
        _ => Err(ApiError::bad_request(
            "A Mastodon handle looks like @name@instance.social",
        )),
    }
}

fn status(i: &StoredInvite, now: u64) -> &'static str {
    match i.status.as_str() {
        REVOKED => REVOKED,
        USED => USED,
        _ if now >= i.expires => "expired",
        _ => OPEN,
    }
}

impl State {
    fn target_name(&self, kind: &str, id: u32) -> String {
        match kind {
            "group" => self.group(id).map(|g| g.name.clone()),
            _ => self.event(id).map(|e| e.title.clone()),
        }
        .unwrap_or_default()
    }

    /// May `actor` invite people to this target?
    fn can_invite(&self, actor: u32, kind: &str, target: u32) -> Result<(), ApiError> {
        let ok = match kind {
            "group" => {
                let g = self
                    .group(target)
                    .ok_or_else(|| ApiError::not_found("No such group"))?;
                self.access(Some(actor), g).manage
            }
            "event" => {
                let (_, a) = self.visible_event(Some(actor), target)?;
                a.edit
            }
            _ => return Err(ApiError::bad_request("Invite to a group or an event")),
        };
        if ok {
            Ok(())
        } else {
            Err(ApiError::new(
                403,
                "not_allowed",
                "Only organizers and hosts can invite",
            ))
        }
    }

    /// Someone without a PIN (an email-only relative), made by an organizer.
    pub fn add_person(&mut self, actor: u32, input: &NewPerson, now: u64) -> Result<u32, ApiError> {
        let manages = self.is_any_manager(actor);
        if !manages {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only admins and organizers can add people",
            ));
        }
        let name = normalize_name(&input.display_name)?;
        let email = normalize_email(&input.email)?;
        if input.household != 0 && self.households.get(input.household).is_none() {
            return Err(ApiError::not_found("No such household"));
        }
        if self.people.len() >= self.limits.people {
            return Err(full("people", self.limits.people));
        }
        self.check_unique(0, &name, &email)?;
        let id = self.next_person;
        self.next_person += 1;
        self.people.push(crate::messages::StoredPerson {
            id,
            display_name: name,
            email,
            status: INVITED.into(),
            household: input.household,
            discoverable: "members".into(),
            created: now,
            ..Default::default()
        });
        self.log(actor, "person.add", format!("person {id}"), now);
        Ok(id)
    }

    /// Admin, or an organizer/co-organizer of any group.
    pub fn is_any_manager(&self, actor: u32) -> bool {
        self.person(actor).is_some_and(|p| p.admin)
            || self
                .groups
                .iter()
                .any(|g| self.access(Some(actor), g).manage)
    }

    /// Creates an invitation and queues its email/DM. Returns its view with
    /// the token (shown only now).
    pub fn create_invite(
        &mut self,
        actor: u32,
        input: &InviteInput,
        now: u64,
    ) -> Result<InviteView, ApiError> {
        self.can_invite(actor, &input.target_kind, input.target)?;
        let email = normalize_email(&input.email)?;
        let handle = normalize_handle(&input.handle)?;
        let days = if input.days == 0 { 14 } else { input.days };
        let uses = if input.uses == 0 { 1 } else { input.uses };
        if days > 90 || uses > 50 {
            return Err(ApiError::bad_request("At most 90 days and 50 uses"));
        }
        if input.person != 0
            && !self
                .person(input.person)
                .is_some_and(|p| p.status != crate::state::DISABLED)
        {
            return Err(ApiError::not_found("No such person"));
        }
        if self.invites.len() >= self.limits.invites {
            // Make room by forgetting the oldest invitation that's no use any more.
            let gone = self
                .invites
                .iter()
                .filter(|i| status(i, now) != OPEN)
                .map(|i| i.id)
                .next();
            match gone {
                Some(id) => {
                    let i = self.invites.position(id).expect("listed");
                    self.invites.remove(i);
                }
                None => return Err(full("open invitations", self.limits.invites)),
            }
        }
        let token = new_token()?;
        let id = self.next_invite;
        self.next_invite += 1;
        let invite = StoredInvite {
            id,
            token_hash: token_hash(&token),
            target_kind: input.target_kind.clone(),
            target: input.target,
            issuer: actor,
            email: email.clone(),
            handle: handle.clone(),
            person: input.person,
            created: now,
            expires: now + u64::from(days) * 86_400,
            uses_left: uses,
            status: OPEN.into(),
            used_by: Vec::new(),
        };
        self.invites.push(invite);
        // A named person is in at once (an event invitation tells them,
        // with a calendar invite, through the usual notification).
        if input.person != 0 {
            self.apply_invite(actor, input.person, &input.target_kind, input.target, now)?;
        }
        let link = format!("{}/invite/{token}", self.origin);
        let target_name = self.target_name(&input.target_kind, input.target);
        let issuer = self.name(actor);
        let to_email = if email.is_empty() {
            self.person(input.person)
                .map(|p| p.email.clone())
                .unwrap_or_default()
        } else {
            email.clone()
        };
        let mut emailed = false;
        let mail_for_person = input.person == 0 || input.target_kind == "group";
        if !to_email.is_empty() && self.mail().is_some() && mail_for_person {
            let (subject, text) = crate::mail::invite_email(&issuer, &target_name, &link, days);
            emailed = self.enqueue(
                StoredJob {
                    kind: "mail".into(),
                    to: to_email,
                    subject,
                    text,
                    reply_to: self
                        .person(actor)
                        .map(|p| p.email.clone())
                        .unwrap_or_default(),
                    priority: 1,
                    ..Default::default()
                },
                now,
            );
        }
        let mut messaged = false;
        if !handle.is_empty() && self.bot().is_some() {
            messaged = self.enqueue(
                StoredJob {
                    kind: "dm".into(),
                    to: handle.clone(),
                    subject: format!("Invitation to {target_name}"),
                    text: format!(
                        "{handle} {issuer} invited you to {target_name}. To accept, reply \"redeem {token}\" (or open {link} at the house)."
                    ),
                    priority: 1,
                    ..Default::default()
                },
                now,
            );
        }
        self.log(
            actor,
            "invite.create",
            format!("invite {id} to {} {}", input.target_kind, input.target),
            now,
        );
        let mut view = self.invite_view(self.invites.get(id).expect("just made"), now);
        view.token = token;
        view.link = link;
        view.emailed = emailed;
        view.messaged = messaged;
        Ok(view)
    }

    /// Puts `person` into the invitation's target.
    fn apply_invite(
        &mut self,
        actor: u32,
        person: u32,
        kind: &str,
        target: u32,
        now: u64,
    ) -> Result<(), ApiError> {
        match kind {
            "group" => {
                let present = self
                    .group(target)
                    .and_then(|g| self.membership(g, person))
                    .is_some_and(|m| m.status == ACTIVE);
                if !present {
                    // The issuer could add them; an invitation lifts a removal too.
                    let issuer_ok = self
                        .group(target)
                        .is_some_and(|g| self.access(Some(actor), g).manage);
                    if issuer_ok {
                        self.set_member(actor, target, person, None, Some(ACTIVE), now)?;
                    } else {
                        return Err(ApiError::new(
                            403,
                            "not_allowed",
                            "The invitation's sender can't add people any more",
                        ));
                    }
                }
            }
            _ => {
                let e = self
                    .event(target)
                    .ok_or_else(|| ApiError::not_found("No such event"))?;
                if !e.invitees.contains(&person) {
                    let mut invitees = e.invitees.clone();
                    invitees.push(person);
                    let input = crate::messages::EventInput {
                        invitees: Some(invitees),
                        ..Default::default()
                    };
                    self.update_event(actor, target, &input, now)?;
                }
            }
        }
        Ok(())
    }

    fn find_invite(&self, token: &str) -> Option<&StoredInvite> {
        let hash = token_hash(token.trim());
        self.invites.iter().find(|i| i.token_hash == hash)
    }

    pub fn preview_invite(&self, token: &str, now: u64) -> InvitePreview {
        let Some(i) = self.find_invite(token) else {
            return InvitePreview {
                reason: "This invitation doesn't exist (check the link)".into(),
                ..Default::default()
            };
        };
        let st = status(i, now);
        InvitePreview {
            target_kind: i.target_kind.clone(),
            target: i.target,
            target_name: self.target_name(&i.target_kind, i.target),
            issuer_name: self.name(i.issuer),
            valid: st == OPEN,
            reason: match st {
                OPEN => String::new(),
                USED => "This invitation has been used".into(),
                REVOKED => "This invitation was withdrawn".into(),
                _ => "This invitation has expired; ask for a new one".into(),
            },
            providers: Vec::new(),
        }
    }

    /// The invitation's person, if it names one who has no PIN yet (their
    /// sign-up or Mastodon sign-in then activates them).
    pub fn invite_person(&self, token: &str, now: u64) -> Option<u32> {
        let i = self.find_invite(token).filter(|i| status(i, now) == OPEN)?;
        (i.person != 0)
            .then_some(i.person)
            .filter(|p| self.person(*p).is_some_and(|p| p.status == INVITED))
    }

    /// Uses invitation `token` for `person`.
    pub fn redeem(
        &mut self,
        person: u32,
        token: &str,
        now: u64,
    ) -> Result<InvitePreview, ApiError> {
        let i = self
            .find_invite(token)
            .cloned()
            .ok_or_else(|| ApiError::not_found("No such invitation"))?;
        if status(&i, now) != OPEN {
            return Err(ApiError::new(
                410,
                "invite_closed",
                self.preview_invite(token, now).reason,
            ));
        }
        if i.person != 0 && i.person != person {
            return Err(ApiError::new(
                403,
                "not_yours",
                "This invitation is for someone else",
            ));
        }
        if i.used_by.contains(&person) {
            return Ok(self.preview_invite(token, now));
        }
        self.apply_invite(i.issuer, person, &i.target_kind, i.target, now)?;
        let row = self.invites.get_mut(i.id).expect("found");
        row.uses_left = row.uses_left.saturating_sub(1);
        row.used_by.push(person);
        if row.uses_left == 0 {
            row.status = USED.into();
        }
        self.log(person, "invite.redeem", format!("invite {}", i.id), now);
        Ok(self.preview_invite(token, now))
    }

    /// Sign-up with an invitation: open even when sign-up is closed. If the
    /// invitation names an email-only person, this activates them instead
    /// of making someone new.
    pub fn signup_invited(
        &mut self,
        name: &str,
        email: &str,
        pin: &str,
        token: &str,
        now: u64,
    ) -> Result<u32, ApiError> {
        if self.people.is_empty() {
            return Err(ApiError::new(409, "setup_needed", "Set party up first"));
        }
        let preview = self.preview_invite(token, now);
        if !preview.valid {
            return Err(ApiError::new(410, "invite_closed", preview.reason));
        }
        let id = match self.invite_person(token, now) {
            Some(p) => {
                let name = normalize_name(name)?;
                let email = normalize_email(email)?;
                crate::state::check_pin(pin)?;
                self.check_unique(p, &name, &email)?;
                let hash = crate::pin::hash(pin, self.limits.pin_rounds)
                    .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
                let person = self.person_mut(p)?;
                person.display_name = name;
                if !email.is_empty() {
                    person.email = email;
                }
                person.pin = hash;
                person.status = ACTIVE.into();
                person.last_seen = now;
                p
            }
            None => self.create_person(name, email, pin, false, now)?,
        };
        self.redeem(id, token, now)?;
        Ok(id)
    }

    pub fn revoke_invite(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let i = self
            .invites
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("No such invitation"))?;
        if i.issuer != actor {
            self.can_invite(actor, &i.target_kind, i.target)?;
        }
        self.invites.get_mut(id).expect("found").status = REVOKED.into();
        self.log(actor, "invite.revoke", format!("invite {id}"), now);
        Ok(())
    }

    pub fn invite_view(&self, i: &StoredInvite, now: u64) -> InviteView {
        InviteView {
            id: i.id,
            target_kind: i.target_kind.clone(),
            target: i.target,
            target_name: self.target_name(&i.target_kind, i.target),
            issuer_name: self.name(i.issuer),
            email: i.email.clone(),
            handle: i.handle.clone(),
            person: i.person,
            expires: i.expires,
            uses_left: i.uses_left,
            status: status(i, now).into(),
            ..Default::default()
        }
    }

    /// Invitations to a target, for those who may invite to it.
    pub fn list_invites(
        &self,
        actor: u32,
        kind: &str,
        target: u32,
        now: u64,
    ) -> Result<Vec<InviteView>, ApiError> {
        self.can_invite(actor, kind, target)?;
        Ok(self
            .invites
            .iter()
            .filter(|i| i.target_kind == kind && i.target == target)
            .map(|i| self.invite_view(i, now))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::GroupInput;
    use crate::state::Limits;

    fn world() -> (State, u32, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            ..Limits::board()
        });
        let dad = s.setup("Dad", "dad@home.example", "1234", 0).unwrap();
        let kid = s.signup("Kid", "", "1234", 0).unwrap();
        let g = s
            .create_group(
                dad,
                &GroupInput {
                    name: Some("Family".into()),
                    visibility: Some("private".into()),
                    join: Some("invite".into()),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        (s, dad, kid, g)
    }

    fn invite(s: &mut State, actor: u32, g: u32, uses: u32) -> Result<InviteView, ApiError> {
        s.create_invite(
            actor,
            &InviteInput {
                target_kind: "group".into(),
                target: g,
                uses,
                ..Default::default()
            },
            100,
        )
    }

    #[test]
    fn tokens_redeem_once_expire_and_revoke() {
        let (mut s, dad, kid, g) = world();
        assert_eq!(
            invite(&mut s, kid, g, 1).unwrap_err().status,
            403,
            "members can't invite"
        );
        let v = invite(&mut s, dad, g, 1).unwrap();
        assert!(v.link.ends_with(&v.token));
        assert!(
            !s.invites.iter().any(|i| i.token_hash == v.token),
            "only the hash is stored"
        );
        assert!(s.preview_invite(&v.token, 101).valid);
        s.redeem(kid, &v.token, 101).unwrap();
        assert_eq!(
            s.membership(s.group(g).unwrap(), kid).unwrap().status,
            ACTIVE
        );
        let other = s.signup("Other", "", "1234", 0).unwrap();
        assert_eq!(
            s.redeem(other, &v.token, 102).unwrap_err().code,
            "invite_closed"
        );
        let v2 = invite(&mut s, dad, g, 5).unwrap();
        assert!(
            !s.preview_invite(&v2.token, 100 + 15 * 86_400).valid,
            "expired after 14 days"
        );
        s.revoke_invite(dad, v2.id, 103).unwrap();
        assert_eq!(
            s.redeem(other, &v2.token, 104).unwrap_err().code,
            "invite_closed"
        );
        assert_eq!(s.redeem(other, "nope", 104).unwrap_err().status, 404);
    }

    #[test]
    fn an_email_only_person_is_added_and_their_invite_is_theirs() {
        let (mut s, dad, kid, g) = world();
        let gran = s
            .add_person(
                dad,
                &NewPerson {
                    display_name: "Grandma".into(),
                    email: "gran@example.com".into(),
                    household: 0,
                },
                1,
            )
            .unwrap();
        assert_eq!(s.person(gran).unwrap().status, INVITED);
        assert_eq!(
            s.add_person(
                kid,
                &NewPerson {
                    display_name: "X".into(),
                    ..Default::default()
                },
                1
            )
            .unwrap_err()
            .status,
            403
        );
        let v = s
            .create_invite(
                dad,
                &InviteInput {
                    target_kind: "group".into(),
                    target: g,
                    person: gran,
                    ..Default::default()
                },
                2,
            )
            .unwrap();
        assert_eq!(
            s.membership(s.group(g).unwrap(), gran).unwrap().status,
            ACTIVE,
            "in at once"
        );
        assert_eq!(s.invite_person(&v.token, 3), Some(gran));
        assert_eq!(s.redeem(kid, &v.token, 3).unwrap_err().code, "not_yours");
        assert_eq!(
            normalize_handle(" @Son@Mastodon.Social ").unwrap(),
            "@son@mastodon.social"
        );
        assert!(normalize_handle("son").is_err());
    }
}
