//! Groups, membership and who may do what (`spec/03-domain.md#groups`,
//! `spec/02-identity-auth.md#roles`).
//!
//! Every permission question goes through [`State::access`], whatever the
//! channel a request came from, so there is one table to test.
//!
//! | | see | list/search | manage (edit, members) | own (archive, delete, organizers, transfer) |
//! |---|---|---|---|---|
//! | admin | ✓ | ✓ | ✓ | ✓ |
//! | organizer | ✓ | ✓ | ✓ | ✓ |
//! | co-organizer | ✓ | ✓ | ✓ | |
//! | member | ✓ | ✓ | | |
//! | requested | public, unlisted | public, unlisted | | |
//! | other signed in | public, unlisted | public | | |
//! | signed out | public | public | | |
//!
//! Archived groups are seen only by their members and admins.
use crate::messages::{GroupInput, GroupView, MemberView, StoredGroup, StoredMember};
use crate::state::{fold, full, State, ACTIVE, DISABLED};
use miniframework::ApiError;

pub const ORGANIZER: &str = "organizer";
pub const CO_ORGANIZER: &str = "co-organizer";
pub const MEMBER: &str = "member";
pub const REQUESTED: &str = "requested";
pub const REMOVED: &str = "removed";
pub const ARCHIVED: &str = "archived";

/// What one person may do with one group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub see: bool,
    pub list: bool,
    pub manage: bool,
    pub own: bool,
}

fn pick<'a>(raw: &str, allowed: &[&'a str], what: &str) -> Result<&'a str, ApiError> {
    allowed.iter().find(|a| **a == raw).copied().ok_or_else(|| {
        ApiError::bad_request(format!("{what} must be one of: {}", allowed.join(", ")))
    })
}

const KINDS: &[&str] = &["club", "circle", "one-off"];
const VISIBILITIES: &[&str] = &["public", "unlisted", "private"];
const JOINS: &[&str] = &["open", "approval", "invite"];

fn group_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    let len = name.chars().count();
    if (1..=60).contains(&len) && !name.chars().any(char::is_control) {
        Ok(name.to_string())
    } else {
        Err(ApiError::bad_request("Group name: 1 to 60 characters"))
    }
}

fn group_about(raw: &str) -> Result<String, ApiError> {
    let about = raw.trim();
    if about.chars().count() > 2_000 {
        return Err(ApiError::bad_request("About: at most 2,000 characters"));
    }
    Ok(about.to_string())
}

/// `Sunday Dinners!` → `sunday-dinners`.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(40).collect();
    if slug.is_empty() {
        "group".into()
    } else {
        slug
    }
}

fn stale() -> ApiError {
    ApiError::new(
        409,
        "stale",
        "Someone changed this group meanwhile; reload and try again",
    )
}

impl State {
    pub fn group(&self, id: u32) -> Option<&StoredGroup> {
        self.groups.get(id)
    }

    /// A group by id or slug.
    pub fn group_ref(&self, raw: &str) -> Option<u32> {
        match raw.parse::<u32>() {
            Ok(id) => self.group(id).map(|g| g.id),
            Err(_) => self.groups.iter().find(|g| g.slug == raw).map(|g| g.id),
        }
    }

    pub fn membership<'a>(&self, g: &'a StoredGroup, person: u32) -> Option<&'a StoredMember> {
        g.members.iter().find(|m| m.person == person)
    }

    /// Both are active members of some group.
    pub fn share_group(&self, a: u32, b: u32) -> bool {
        self.groups.iter().any(|g| {
            let active = |p| {
                g.members
                    .iter()
                    .any(|m| m.person == p && m.status == ACTIVE)
            };
            active(a) && active(b)
        })
    }

    fn is_admin(&self, viewer: Option<u32>) -> bool {
        viewer
            .and_then(|v| self.person(v))
            .is_some_and(|p| p.admin && p.status == ACTIVE)
    }

    /// The permission table at the top of this file.
    pub fn access(&self, viewer: Option<u32>, g: &StoredGroup) -> Access {
        if self.is_admin(viewer) {
            return Access {
                see: true,
                list: true,
                manage: true,
                own: true,
            };
        }
        let m = viewer.and_then(|v| self.membership(g, v));
        let active = m.filter(|m| m.status == ACTIVE);
        let requested = m.is_some_and(|m| m.status == REQUESTED);
        if let Some(m) = active {
            let own = m.role == ORGANIZER;
            return Access {
                see: true,
                list: true,
                manage: own || m.role == CO_ORGANIZER,
                own,
            };
        }
        if g.status == ARCHIVED {
            return Access::default();
        }
        let signed_in = viewer.is_some();
        match g.visibility.as_str() {
            "public" => Access {
                see: true,
                list: true,
                ..Access::default()
            },
            "unlisted" => Access {
                see: signed_in,
                list: requested,
                ..Access::default()
            },
            _ => Access::default(),
        }
    }

    fn visible(&self, viewer: Option<u32>, id: u32) -> Result<(&StoredGroup, Access), ApiError> {
        let g = self
            .group(id)
            .ok_or_else(|| ApiError::not_found("No such group"))?;
        let access = self.access(viewer, g);
        if access.see {
            Ok((g, access))
        } else {
            // A private group's existence is not confirmed to outsiders.
            Err(ApiError::not_found("No such group"))
        }
    }

    fn need(allowed: bool, message: &str) -> Result<(), ApiError> {
        if allowed {
            Ok(())
        } else {
            Err(ApiError::new(403, "not_allowed", message))
        }
    }

    fn active_organizers(g: &StoredGroup) -> usize {
        g.members
            .iter()
            .filter(|m| m.role == ORGANIZER && m.status == ACTIVE)
            .count()
    }

    fn unique_slug(&self, name: &str) -> String {
        let base = slugify(name);
        let mut slug = base.clone();
        let mut n = 2;
        while self.groups.iter().any(|g| g.slug == slug) || slug.parse::<u32>().is_ok() {
            slug = format!("{base}-{n}");
            n += 1;
        }
        slug
    }

    pub fn create_group(
        &mut self,
        actor: u32,
        input: &GroupInput,
        now: u64,
    ) -> Result<u32, ApiError> {
        if self.groups_admin_only && !self.is_admin(Some(actor)) {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only admins can start groups here",
            ));
        }
        let name = group_name(input.name.as_deref().unwrap_or(""))?;
        let about = group_about(input.about.as_deref().unwrap_or(""))?;
        let kind = pick(input.kind.as_deref().unwrap_or("circle"), KINDS, "kind")?;
        let visibility = pick(
            input.visibility.as_deref().unwrap_or("unlisted"),
            VISIBILITIES,
            "visibility",
        )?;
        let join = pick(input.join.as_deref().unwrap_or("approval"), JOINS, "join")?;
        if self.groups.len() >= self.limits.groups {
            return Err(full("groups", self.limits.groups));
        }
        let id = self.next_group;
        self.next_group += 1;
        let slug = self.unique_slug(&name);
        self.groups.push(StoredGroup {
            id,
            slug,
            name,
            about,
            kind: kind.into(),
            visibility: visibility.into(),
            join: join.into(),
            members: vec![StoredMember {
                person: actor,
                role: ORGANIZER.into(),
                status: ACTIVE.into(),
                joined: now,
            }],
            status: ACTIVE.into(),
            created: now,
            updated: now,
            rev: 1,
            created_by: actor,
            members_create_events: false,
        });
        self.log(actor, "group.create", format!("group {id}"), now);
        Ok(id)
    }

    pub fn update_group(
        &mut self,
        actor: u32,
        id: u32,
        input: &GroupInput,
        now: u64,
    ) -> Result<(), ApiError> {
        let (g, access) = self.visible(Some(actor), id)?;
        Self::need(access.manage, "Only the group's organizers can change it")?;
        if input.rev.is_some_and(|r| r != g.rev) {
            return Err(stale());
        }
        let mut next = g.clone();
        if let Some(n) = &input.name {
            next.name = group_name(n)?;
        }
        if let Some(a) = &input.about {
            next.about = group_about(a)?;
        }
        if let Some(k) = &input.kind {
            next.kind = pick(k, KINDS, "kind")?.into();
        }
        if let Some(v) = &input.visibility {
            next.visibility = pick(v, VISIBILITIES, "visibility")?.into();
        }
        if let Some(j) = &input.join {
            next.join = pick(j, JOINS, "join")?.into();
        }
        if let Some(m) = input.members_create_events {
            next.members_create_events = m;
        }
        if let Some(s) = &input.status {
            let s = pick(s, &[ACTIVE, ARCHIVED], "status")?;
            if s != next.status {
                Self::need(
                    access.own,
                    "Only an organizer can archive or restore the group",
                )?;
                next.status = s.into();
            }
        }
        if next == *g {
            return Ok(());
        }
        next.rev += 1;
        next.updated = now;
        *self.groups.get_mut(id).expect("checked above") = next;
        self.log(actor, "group.update", format!("group {id}"), now);
        Ok(())
    }

    pub fn delete_group(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let (_, access) = self.visible(Some(actor), id)?;
        Self::need(access.own, "Only an organizer can delete the group")?;
        let i = self.groups.position(id).expect("checked above");
        self.groups.remove(i);
        self.log(actor, "group.delete", format!("group {id}"), now);
        Ok(())
    }

    /// Joins (open), asks to join (approval), or is refused (invite-only).
    /// Returns the caller's membership status afterwards.
    pub fn join_group(&mut self, actor: u32, id: u32, now: u64) -> Result<String, ApiError> {
        let (g, _) = self.visible(Some(actor), id)?;
        let status = match self.membership(g, actor).map(|m| m.status.as_str()) {
            Some(s @ (ACTIVE | REQUESTED)) => return Ok(s.to_string()),
            Some(REMOVED) => {
                return Err(ApiError::new(
                    403,
                    "removed",
                    "An organizer removed you from this group; ask them to add you back",
                ))
            }
            _ if g.status == ARCHIVED => {
                return Err(ApiError::new(409, "archived", "This group is archived"))
            }
            _ => match g.join.as_str() {
                "open" => ACTIVE,
                "approval" => REQUESTED,
                _ => {
                    return Err(ApiError::new(
                        403,
                        "invite_only",
                        "This group is invitation-only; ask an organizer",
                    ))
                }
            },
        };
        if g.members.len() >= self.limits.members_per_group {
            return Err(full("members in a group", self.limits.members_per_group));
        }
        let g = self.groups.get_mut(id).expect("checked above");
        g.members.push(StoredMember {
            person: actor,
            role: MEMBER.into(),
            status: status.into(),
            joined: now,
        });
        if status == REQUESTED {
            let managers: Vec<u32> = g
                .members
                .iter()
                .filter(|m| m.status == ACTIVE && (m.role == ORGANIZER || m.role == CO_ORGANIZER))
                .map(|m| m.person)
                .collect();
            for m in managers {
                self.notify_group(m, "request", 0, id, actor, now);
            }
        }
        Ok(status.into())
    }

    /// Leaves (or withdraws a request). The last organizer can't.
    pub fn leave_group(&mut self, actor: u32, id: u32) -> Result<(), ApiError> {
        let g = self
            .group(id)
            .ok_or_else(|| ApiError::not_found("No such group"))?;
        let Some(m) = self.membership(g, actor).filter(|m| m.status != REMOVED) else {
            return Ok(());
        };
        if m.role == ORGANIZER && m.status == ACTIVE && Self::active_organizers(g) <= 1 {
            return Err(ApiError::new(
                409,
                "last_organizer",
                "You're the only organizer: hand the group to someone else first",
            ));
        }
        let g = self.groups.get_mut(id).expect("checked above");
        g.members.retain(|m| m.person != actor);
        Ok(())
    }

    /// Approves, removes, adds or re-roles `person` in group `id`.
    pub fn set_member(
        &mut self,
        actor: u32,
        id: u32,
        person: u32,
        role: Option<&str>,
        status: Option<&str>,
        now: u64,
    ) -> Result<(), ApiError> {
        let (g, access) = self.visible(Some(actor), id)?;
        Self::need(
            access.manage,
            "Only the group's organizers can manage members",
        )?;
        if !self.person(person).is_some_and(|p| p.status != DISABLED) {
            return Err(ApiError::not_found("No such person"));
        }
        let role = role
            .map(|r| pick(r, &[ORGANIZER, CO_ORGANIZER, MEMBER], "role"))
            .transpose()?;
        let status = status
            .map(|s| pick(s, &[ACTIVE, REMOVED], "status"))
            .transpose()?;
        let current = self.membership(g, person).cloned();
        let mut next = current.clone().unwrap_or(StoredMember {
            person,
            role: MEMBER.into(),
            status: ACTIVE.into(),
            joined: now,
        });
        if let Some(r) = role {
            next.role = r.into();
        }
        if let Some(s) = status {
            next.status = s.into();
            if s == REMOVED {
                next.role = MEMBER.into();
            }
        }
        if current.as_ref() == Some(&next) {
            return Ok(());
        }
        // Organizers are appointed, removed and demoted only by owners.
        let was_organizer = current.as_ref().is_some_and(|m| m.role == ORGANIZER);
        if (was_organizer || next.role == ORGANIZER) && !access.own {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only an organizer can appoint or remove organizers",
            ));
        }
        let was_active_organizer = current
            .as_ref()
            .is_some_and(|m| m.role == ORGANIZER && m.status == ACTIVE);
        let still = next.role == ORGANIZER && next.status == ACTIVE;
        if was_active_organizer && !still && Self::active_organizers(g) <= 1 {
            return Err(ApiError::new(
                409,
                "last_organizer",
                "A group needs at least one organizer",
            ));
        }
        if current.is_none() && g.members.len() >= self.limits.members_per_group {
            return Err(full("members in a group", self.limits.members_per_group));
        }
        if next.status == ACTIVE && current.as_ref().is_none_or(|m| m.status != ACTIVE) {
            next.joined = now;
        }
        let g = self.groups.get_mut(id).expect("checked above");
        match g.members.iter_mut().find(|m| m.person == person) {
            Some(m) => *m = next,
            None => g.members.push(next),
        }
        self.log(
            actor,
            "group.member",
            format!("group {id} person {person}"),
            now,
        );
        Ok(())
    }

    /// `to` becomes an organizer; the caller (if an organizer) steps down to
    /// co-organizer.
    pub fn transfer_group(
        &mut self,
        actor: u32,
        id: u32,
        to: u32,
        now: u64,
    ) -> Result<(), ApiError> {
        let (g, access) = self.visible(Some(actor), id)?;
        Self::need(access.own, "Only an organizer can hand the group over")?;
        if to == actor {
            return Err(ApiError::bad_request("Pick someone else"));
        }
        if !self.membership(g, to).is_some_and(|m| m.status == ACTIVE) {
            return Err(ApiError::new(
                409,
                "not_a_member",
                "Hand the group to someone who is already a member",
            ));
        }
        let g = self.groups.get_mut(id).expect("checked above");
        for m in &mut g.members {
            if m.person == to {
                m.role = ORGANIZER.into();
            } else if m.person == actor && m.role == ORGANIZER {
                m.role = CO_ORGANIZER.into();
            }
        }
        g.rev += 1;
        g.updated = now;
        self.log(actor, "group.transfer", format!("group {id} to {to}"), now);
        Ok(())
    }

    // ---- views ----------------------------------------------------------------

    pub fn group_view(&self, viewer: Option<u32>, g: &StoredGroup) -> GroupView {
        let access = self.access(viewer, g);
        let mine = viewer.and_then(|v| self.membership(g, v));
        let count = |status: &str| g.members.iter().filter(|m| m.status == status).count() as u32;
        GroupView {
            id: g.id,
            slug: g.slug.clone(),
            name: g.name.clone(),
            about: g.about.clone(),
            kind: g.kind.clone(),
            visibility: g.visibility.clone(),
            join: g.join.clone(),
            status: g.status.clone(),
            members: count(ACTIVE),
            my_role: mine
                .filter(|m| m.status == ACTIVE)
                .map(|m| m.role.clone())
                .unwrap_or_default(),
            my_status: mine.map(|m| m.status.clone()).unwrap_or_default(),
            can_manage: access.manage,
            can_own: access.own,
            created: g.created,
            rev: g.rev,
            requests: if access.manage { count(REQUESTED) } else { 0 },
            members_create_events: g.members_create_events,
            can_create_events: viewer.is_some_and(|v| self.can_create_event(v, g.id).is_ok()),
        }
    }

    pub fn view_group(&self, viewer: Option<u32>, id: u32) -> Result<GroupView, ApiError> {
        let (g, _) = self.visible(viewer, id)?;
        Ok(self.group_view(viewer, g))
    }

    /// Groups the viewer may list, matching `q` (name or about), ascending
    /// id after `after`. `mine` keeps only groups they belong to or asked
    /// to join. Returns the page and the `after` for the next one (0 = end).
    pub fn list_groups(
        &self,
        viewer: Option<u32>,
        q: &str,
        mine: bool,
        after: u32,
        limit: usize,
    ) -> (Vec<GroupView>, u32) {
        let q = fold(q);
        let mut page = Vec::new();
        let mut next = 0;
        for g in self.groups.iter().filter(|g| g.id > after) {
            if !self.access(viewer, g).list {
                continue;
            }
            let member = viewer
                .and_then(|v| self.membership(g, v))
                .is_some_and(|m| m.status != REMOVED);
            if mine && !member {
                continue;
            }
            if !q.is_empty() && !fold(&g.name).contains(&q) && !fold(&g.about).contains(&q) {
                continue;
            }
            if page.len() == limit {
                next = page.last().map_or(0, |v: &GroupView| v.id);
                break;
            }
            page.push(self.group_view(viewer, g));
        }
        (page, next)
    }

    /// Who is in a group. Active members are shown to members, to anyone
    /// signed in for a public group, and to managers; requests and removals
    /// to managers only.
    pub fn group_members(&self, viewer: u32, id: u32) -> Result<Vec<MemberView>, ApiError> {
        let (g, access) = self.visible(Some(viewer), id)?;
        let member = self
            .membership(g, viewer)
            .is_some_and(|m| m.status == ACTIVE);
        Self::need(
            access.manage || member || g.visibility == "public",
            "Only members can see who's in this group",
        )?;
        Ok(g.members
            .iter()
            .filter(|m| access.manage || m.status == ACTIVE)
            .map(|m| MemberView {
                person: m.person,
                display_name: self.name(m.person),
                role: m.role.clone(),
                status: m.status.clone(),
                joined: m.joined,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Limits;

    fn limits() -> Limits {
        Limits {
            pin_rounds: 2,
            ..Limits::board()
        }
    }

    struct World {
        s: State,
        admin: u32,
        org: u32,
        co: u32,
        member: u32,
        requester: u32,
        outsider: u32,
    }

    /// One group per visibility × join policy, each with the same cast.
    fn world() -> (World, Vec<u32>) {
        let mut s = State::new(limits());
        let admin = s.setup("Admin", "", "1234", 0).unwrap();
        let org = s.signup("Org", "", "1234", 0).unwrap();
        let co = s.signup("Co", "", "1234", 0).unwrap();
        let member = s.signup("Member", "", "1234", 0).unwrap();
        let requester = s.signup("Requester", "", "1234", 0).unwrap();
        let outsider = s.signup("Outsider", "", "1234", 0).unwrap();
        let mut groups = Vec::new();
        for visibility in VISIBILITIES {
            for join in JOINS {
                let id = s
                    .create_group(
                        org,
                        &GroupInput {
                            name: Some(format!("{visibility} {join}")),
                            visibility: Some(visibility.to_string()),
                            join: Some(join.to_string()),
                            ..Default::default()
                        },
                        0,
                    )
                    .unwrap();
                s.set_member(org, id, co, Some(CO_ORGANIZER), None, 0)
                    .unwrap();
                s.set_member(org, id, member, None, None, 0).unwrap();
                s.groups.get_mut(id).unwrap().members.push(StoredMember {
                    person: requester,
                    role: MEMBER.into(),
                    status: REQUESTED.into(),
                    joined: 0,
                });
                groups.push(id);
            }
        }
        let w = World {
            s,
            admin,
            org,
            co,
            member,
            requester,
            outsider,
        };
        (w, groups)
    }

    fn a(see: bool, list: bool, manage: bool, own: bool) -> Access {
        Access {
            see,
            list,
            manage,
            own,
        }
    }

    #[test]
    fn the_permission_table() {
        let (w, groups) = world();
        for &id in &groups {
            let g = w.s.group(id).unwrap();
            let vis = g.visibility.as_str();
            let at = |v| w.s.access(v, g);
            assert_eq!(at(Some(w.admin)), a(true, true, true, true), "{vis}");
            assert_eq!(at(Some(w.org)), a(true, true, true, true), "{vis}");
            assert_eq!(at(Some(w.co)), a(true, true, true, false), "{vis}");
            assert_eq!(at(Some(w.member)), a(true, true, false, false), "{vis}");
            let (requester, outsider, anon) = match vis {
                "public" => (
                    a(true, true, false, false),
                    a(true, true, false, false),
                    a(true, true, false, false),
                ),
                "unlisted" => (
                    a(true, true, false, false),
                    a(true, false, false, false),
                    a(false, false, false, false),
                ),
                _ => (Access::default(), Access::default(), Access::default()),
            };
            assert_eq!(at(Some(w.requester)), requester, "{vis} requester");
            assert_eq!(at(Some(w.outsider)), outsider, "{vis} outsider");
            assert_eq!(at(None), anon, "{vis} signed out");
        }
    }

    #[test]
    fn private_groups_never_leak() {
        let (w, groups) = world();
        let (listed, _) = w.s.list_groups(Some(w.outsider), "", false, 0, 50);
        assert!(listed.iter().all(|g| g.visibility == "public"));
        let (found, _) = w.s.list_groups(Some(w.outsider), "private", false, 0, 50);
        assert!(found.is_empty(), "search doesn't find them either");
        for &id in &groups {
            if w.s.group(id).unwrap().visibility == "private" {
                assert_eq!(
                    w.s.view_group(Some(w.outsider), id).unwrap_err().status,
                    404
                );
                assert_eq!(w.s.view_group(None, id).unwrap_err().status, 404);
            }
        }
        let (members_see, _) = w.s.list_groups(Some(w.member), "", true, 0, 50);
        assert_eq!(members_see.len(), groups.len());
    }

    #[test]
    fn join_policies() {
        let (mut w, groups) = world();
        let mut results = Vec::new();
        for &id in &groups {
            let g = w.s.group(id).unwrap();
            let label = format!("{} {}", g.visibility, g.join);
            results.push((label, w.s.join_group(w.outsider, id, 1).map_err(|e| e.code)));
        }
        let get = |l: &str| results.iter().find(|(k, _)| k == l).unwrap().1.clone();
        assert_eq!(get("public open"), Ok(ACTIVE.into()));
        assert_eq!(get("public approval"), Ok(REQUESTED.into()));
        assert_eq!(get("public invite"), Err("invite_only"));
        assert_eq!(get("unlisted open"), Ok(ACTIVE.into()));
        assert_eq!(get("private open"), Err("not_found"));
        // Joining twice is a no-op.
        let public_open = groups[0];
        assert_eq!(w.s.join_group(w.outsider, public_open, 2).unwrap(), ACTIVE);
        assert_eq!(
            w.s.group(public_open)
                .unwrap()
                .members
                .iter()
                .filter(|m| m.person == w.outsider)
                .count(),
            1
        );
    }

    #[test]
    fn managers_approve_and_remove_but_only_owners_touch_organizers() {
        let (mut w, groups) = world();
        let id = groups[1]; // public, approval
        w.s.set_member(w.co, id, w.requester, None, Some(ACTIVE), 1)
            .unwrap();
        assert_eq!(
            w.s.membership(w.s.group(id).unwrap(), w.requester)
                .unwrap()
                .status,
            ACTIVE
        );
        assert_eq!(
            w.s.set_member(w.member, id, w.outsider, None, None, 1)
                .unwrap_err()
                .status,
            403,
            "plain members can't add people"
        );
        assert_eq!(
            w.s.set_member(w.co, id, w.member, Some(ORGANIZER), None, 1)
                .unwrap_err()
                .code,
            "not_allowed"
        );
        assert_eq!(
            w.s.set_member(w.co, id, w.org, None, Some(REMOVED), 1)
                .unwrap_err()
                .code,
            "not_allowed"
        );
        w.s.set_member(w.co, id, w.member, None, Some(REMOVED), 1)
            .unwrap();
        assert_eq!(w.s.join_group(w.member, id, 2).unwrap_err().code, "removed");
        w.s.set_member(w.org, id, w.co, Some(ORGANIZER), None, 1)
            .unwrap();
        w.s.set_member(w.org, id, w.org, Some(MEMBER), None, 1)
            .unwrap();
        assert!(!w.s.access(Some(w.org), w.s.group(id).unwrap()).own);
    }

    #[test]
    fn a_group_always_keeps_an_organizer() {
        let (mut w, groups) = world();
        let id = groups[0];
        assert_eq!(
            w.s.leave_group(w.org, id).unwrap_err().code,
            "last_organizer"
        );
        assert_eq!(
            w.s.set_member(w.org, id, w.org, Some(MEMBER), None, 1)
                .unwrap_err()
                .code,
            "last_organizer"
        );
        assert_eq!(
            w.s.set_member(w.admin, id, w.org, None, Some(REMOVED), 1)
                .unwrap_err()
                .code,
            "last_organizer"
        );
        w.s.transfer_group(w.org, id, w.member, 1).unwrap();
        let g = w.s.group(id).unwrap();
        assert_eq!(w.s.membership(g, w.member).unwrap().role, ORGANIZER);
        assert_eq!(w.s.membership(g, w.org).unwrap().role, CO_ORGANIZER);
        w.s.leave_group(w.org, id).unwrap();
        assert!(w.s.membership(w.s.group(id).unwrap(), w.org).is_none());
        assert_eq!(
            w.s.transfer_group(w.member, id, w.outsider, 1)
                .unwrap_err()
                .code,
            "not_a_member"
        );
    }

    #[test]
    fn edits_check_rev_and_archive_needs_an_owner() {
        let (mut w, groups) = world();
        let id = groups[0];
        let rev = w.s.group(id).unwrap().rev;
        let rename = GroupInput {
            name: Some("Sunday dinners".into()),
            rev: Some(rev),
            ..Default::default()
        };
        w.s.update_group(w.co, id, &rename, 1).unwrap();
        assert_eq!(
            w.s.update_group(w.co, id, &rename, 1).unwrap_err().code,
            "stale"
        );
        assert_eq!(w.s.group(id).unwrap().rev, rev + 1);
        let archive = GroupInput {
            status: Some(ARCHIVED.into()),
            ..Default::default()
        };
        assert_eq!(
            w.s.update_group(w.co, id, &archive, 1).unwrap_err().code,
            "not_allowed"
        );
        assert_eq!(
            w.s.update_group(w.member, id, &rename, 1).unwrap_err().code,
            "not_allowed"
        );
        w.s.update_group(w.org, id, &archive, 1).unwrap();
        assert_eq!(
            w.s.view_group(Some(w.outsider), id).unwrap_err().status,
            404
        );
        assert_eq!(
            w.s.join_group(w.member, id, 1).unwrap(),
            ACTIVE,
            "already in"
        );
        assert_eq!(
            w.s.delete_group(w.co, id, 1).unwrap_err().code,
            "not_allowed"
        );
        w.s.delete_group(w.org, id, 1).unwrap();
        assert!(w.s.group(id).is_none());
    }

    #[test]
    fn slugs_are_unique_and_never_numbers() {
        let mut s = State::new(limits());
        let me = s.setup("Me", "", "1234", 0).unwrap();
        let input = |n: &str| GroupInput {
            name: Some(n.into()),
            ..Default::default()
        };
        let a = s.create_group(me, &input("Sunday Dinners!"), 0).unwrap();
        let b = s.create_group(me, &input("sunday dinners"), 0).unwrap();
        let c = s.create_group(me, &input("2026"), 0).unwrap();
        assert_eq!(s.group(a).unwrap().slug, "sunday-dinners");
        assert_eq!(s.group(b).unwrap().slug, "sunday-dinners-2");
        assert_eq!(s.group(c).unwrap().slug, "2026-2");
        assert_eq!(s.group_ref("sunday-dinners"), Some(a));
        assert_eq!(s.group_ref(&b.to_string()), Some(b));
    }

    #[test]
    fn people_are_found_by_discoverability() {
        let (mut w, groups) = world();
        let stranger = w.s.signup("Stranger", "", "1234", 0).unwrap();
        let member = w.s.person(w.member).unwrap().clone();
        assert!(
            w.s.can_see_person(Some(w.co), &member),
            "they share a group"
        );
        assert!(!w.s.can_see_person(Some(stranger), &member));
        w.s.person_mut(w.member).unwrap().discoverable = "public".into();
        let member = w.s.person(w.member).unwrap().clone();
        assert!(w.s.can_see_person(Some(stranger), &member));
        w.s.person_mut(w.member).unwrap().discoverable = "nobody".into();
        let member = w.s.person(w.member).unwrap().clone();
        assert!(!w.s.can_see_person(Some(w.co), &member));
        assert!(w.s.can_see_person(Some(w.admin), &member));
        assert!(w.s.can_see_person(Some(w.member), &member));
        let _ = groups;
    }

    #[test]
    fn creating_groups_can_be_admin_only() {
        let (mut w, _) = world();
        w.s.groups_admin_only = true;
        let input = GroupInput {
            name: Some("Book club".into()),
            ..Default::default()
        };
        assert_eq!(
            w.s.create_group(w.member, &input, 0).unwrap_err().code,
            "not_allowed"
        );
        w.s.create_group(w.admin, &input, 0).unwrap();
    }
}
