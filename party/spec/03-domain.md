# 03 — Domain

The owner asked for a domain model that isn't as anemic as Meetup's. The way to
get richness without a schema engine on a microcontroller is **modules**: an
event is a small core plus a fixed menu of typed modules (bring list, rides,
questions …). An **event kind** is a preset of modules with sensible defaults.
The RSVP form is whatever the enabled modules ask for. Every module is a
hand-written Rust type with its own codec. No reflection, no dynamic schemas.

## Groups

| Field | Notes |
|---|---|
| `id`, `slug` | slug is for URLs on `party.local`; published files use ids |
| `name`, `about` | ≤ 60 / ≤ 2,000 chars |
| `kind` | `club` (ongoing, recurring events), `circle` (family/friends, ad hoc), `one-off` (exists for one event) |
| `visibility` | `public` (published plaintext), `unlisted` (members and invitees, sealed), `private` (sealed, invisible to non-members) |
| `join` | `open`, `approval`, `invite` |
| `members` | `(person, role, joined, status)`; role ∈ `organizer`, `co-organizer`, `member`; status ∈ `active`, `requested`, `left`, `removed` |
| `defaults` | default event kind, plus-one policy, reminder lead time, where |
| `hashtag` | optional, for the companion site's public Mastodon strip |
| `status` | `active`, `archived` |

Any active member can create a group (admin setting can restrict this to admins).
The creator is the first organizer. A group always has ≥ 1 organizer.

## Events

### Core

| Field | Notes |
|---|---|
| `id` | u32. Shown to people as `E<id>` |
| `group`, `series` | series id if it's an occurrence of a recurring event |
| `kind` | which preset it started from (below) |
| `title`, `about` | ≤ 80 / ≤ 4,000 chars |
| `hosts` | 1–4 people. Hosts can edit the event. Organizers always can |
| `start`, `end` | UTC seconds. `end` optional. v1 uses the **instance time zone** (POSIX TZ string) for display defaults and recurrence; browsers display in local time |
| `where` | `place` (name, optional address, optional map link), `someone's house` (a person or household; the address comes from them), or `tbd`. Address class defaults to `direct` ([05](05-companion-site.md#data-classes)) |
| `visibility` | `group` (inherits), `invite-only` (only invitees and organizers), `public` (only in public groups) |
| `capacity` | heads, optional. Full → waitlist |
| `rsvp_by` | optional deadline. After it, answers are locked except by organizers |
| `plus_ones` | `none`, `up-to N` (1–9), `unlimited`. Default from the group |
| `status` | `draft`, `scheduled`, `cancelled`, `done` (from the clock) |
| `modules` | the enabled modules and their data |
| `rev` | increments on every edit that affects answers (time, place, modules). Answers carry the rev they saw |

### Modules

| Module | What it adds to the event | What it adds to an RSVP |
|---|---|---|
| `headcount` | always on | `+N` guests, optional names, optional adults/kids split |
| `bring` | a list of slots ("Salad", "Dessert × 2", "Drinks"), each claimable by N people, plus "something else" | which slots you claim, free text for "something else" |
| `diet` | a prompt ("Allergies or diet?") | short text, visible only to hosts and organizers |
| `rides` | meeting point, notes | `need N seats` / `offering N seats` / `own way`. Hosts see a match list |
| `when` | 2–6 candidate times instead of one fixed time (Game Matcher's time poll) | which times work. The host locks one in, which turns it into a fixed-time event |
| `shifts` | named slots with times and a count ("Setup 5–6pm × 3") | which shifts you take |
| `questions` | up to 4 organizer questions: yes/no, choice (≤ 6), short text | answers |
| `cost` | text only ("$10 for pizza, pay Sam"). No payments | — |
| `link` | URL for an online part (roadmap: virtual events). Class `sealed` by default | — |

### Event kinds (presets)

| Kind | Modules on | Defaults |
|---|---|---|
| `generic` | headcount | plus-ones unlimited |
| `party` | headcount, bring, diet | plus-ones up to 2, reminder 1 day before |
| `dinner` | headcount, diet | capacity = seats, plus-ones none, rsvp_by 2 days before |
| `potluck` | headcount, bring, diet | plus-ones up to 2 |
| `outing` | headcount, rides, cost | where = place |
| `meeting` | headcount, questions | club default, recurring weekly |
| `workday` | shifts, bring | plus-ones unlimited |
| `poll` | headcount, when | for "when are we doing Thanksgiving?" |

A kind is only a starting point: hosts can switch modules on and off later.
Turning a module off hides its answers but keeps them until the event is done.

## RSVPs

One answer per person per event:

| Field | Notes |
|---|---|
| `status` | `going`, `maybe`, `no`, or no answer. `waitlist` is set by the board when capacity is full |
| `plus` | guests (count, optional names, optional kids count), within the event's policy |
| `modules` | per-module answer, as above |
| `note` | ≤ 200 chars, visible to the group |
| `rev_seen` | event rev the answer was made against |
| `by` | who made it: the person, a household member, or an organizer ("answered for you by Sam") |
| `via` | `web`, `dm`, `relay`, `email-proxy` (an organizer recorded what an email-only person said) |
| `updated` | UTC seconds |

Rules:

- **Idempotent.** Setting the same answer again changes nothing and sends no
  notifications. Commands from external channels carry a message id the board
  remembers ([04](04-channels.md#durable-processing)).
- **Stale revs.** An answer made against an older rev of an event with `when`
  or `bring` is rejected with "the event changed, here's what's new" (web:
  409, DM: a reply). Plain going/maybe/no against a changed time is accepted
  and the person gets notified of the change, as in Game Matcher.
- **Capacity counts heads** (person + plus-ones). An answer that would overflow
  goes to the waitlist. When someone drops, the oldest waitlisted answer that
  fits is promoted and notified.
- **Plus-one policy changes** don't rewrite existing answers. Over-policy
  answers are flagged to hosts.

### Answering for someone

- A household member can answer for others in their household.
- Hosts and organizers can answer for anyone invited (the email-only relative
  who phoned). The answer shows `by` and `via`, and the person is notified by
  their channel.

## Recurring events

A **series** holds the rule, and **occurrences** are real events.

- Rules: weekly / every N weeks on a weekday, monthly on the Nth weekday
  ("first Friday", "last Sunday"), monthly on a date. Start date, optional end
  date or count. Times are wall-clock in the instance time zone, so DST works.
- Occurrences are materialized on a **rolling window**: the next 4 (configurable
  1–8), created as each one passes. Open-ended series cost bounded storage.
- Editing a series asks "this one / this and following". "This one" turns the
  occurrence into an exception (moved or cancelled) that the series remembers.
- RSVPs are per occurrence. Optional "I'm a regular": a standing `going` that
  each new occurrence starts with, which the person can override.
- Builds on Game Matcher's `recurring.rs`, which does finite series. The
  rolling window, exceptions and "regulars" are new.

## Comments

Not in v1. The draft excluded forums, and an RSVP note covers "running late".
If people ask for threads, Game Matcher's per-plan comment blobs are the design.

## Notifications

In-app bell (polled `GET /api/v1/me/summary`, as in Game Matcher) and, by each
person's settings, DM and email.

| Trigger | Who |
|---|---|
| Invited to a group or event | the invitee |
| New event in your group | members with notifications on for that group |
| Time, place or module change (rev bump) | everyone who answered going/maybe |
| Cancelled | everyone who answered, plus invitees |
| Promoted from the waitlist | that person |
| Reminder (event's lead time) | going + maybe |
| Join request | organizers |
| Someone answered for you | you |

Notifications coalesce while unread (one per event), and DM/email delivery
follows the coalesced notification, so a busy event doesn't flood anyone.

## Calendars

| Feed | Where | Contents |
|---|---|---|
| Per-event `.ics` download | local app, companion site | one VEVENT |
| Personal feed `http://party.local/cal/<token>.ics` | LAN | everything you're going/maybe/invited to |
| Email attachment | email | `METHOD:REQUEST` invite on creation, updates with `SEQUENCE`, `METHOD:CANCEL` on cancel. **This is the calendar path for family outside the house.** Calendar apps add and update it from the email |
| Public group feed `https://<site>/data/ics/g<id>.ics` | GitHub Pages | public groups' public events only |
| Personal remote feed | relay only (optional) | see [08](08-relay.md) |

Stable UIDs (`e<id>@party.local`), UTC times, RFC 5545 folding and escaping:
reuse Game Matcher's `ics.rs`. A private calendar feed reachable from outside
the house can't exist without a server, because calendar apps fetch URLs and
can't decrypt. That's why email attachments are the baseline and the relay feed
is optional.
