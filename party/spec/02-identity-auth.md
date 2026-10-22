# 02 — People, identity and sign-in

The draft's first architectural rule stands: **one internal person, many ways
to prove it's them.** Signing in never creates a second person by accident.

## Person

| Field | Notes |
|---|---|
| `id` | u32, assigned by the board, never reused |
| `display_name` | 1–40 chars |
| `bio` | ≤ 280 chars, optional |
| `avatar` | none in v1: initials in a colour from the id, like Game Matcher |
| `household` | optional household id (below) |
| `email` | optional, unique, lower-cased. Delivery address only: never a login name for remote use, never published, never shown to other members |
| `identities` | up to 3 linked external identities: `(kind, instance host, account id, @handle)`, each unique across the board. `kind` = `mastodon` in v1, `bluesky` later |
| `pin_hash` | optional. PBKDF2-HMAC-SHA256 with salt and stored round count, as Game Matcher. Rounds re-measured on the S3 (target ~100 ms) |
| `personal_key` | 256-bit random, never leaves the board except inside the person's personal link ([05](05-companion-site.md#personal-links-and-keyrings)). Rotatable |
| `role` | `admin` or `member` (instance level). Group roles are per membership |
| `status` | `invited`, `active`, `disabled` |
| `discoverable` | `nobody` / `members` (people who share a group) / `public`. Default `members` |
| `notify` | per channel (in-app, DM, email) on/off, plus a reminder lead time |
| `llm_ok` | may free-text DMs from this person go to the LLM? Default yes once the admin enables the LLM; the person can turn it off |
| `created`, `last_seen` | UTC seconds |

A person with no PIN and no identity can still exist: an email-only relative
created by an organizer. They're `invited` until they sign in somewhere, and
the organizer can answer for them ([03](03-domain.md#answering-for-someone)).

## Households

A household is a named set of people ("The Martins", "Grandma's"). It exists
because family invitations are by household: invite a household and everyone
in it is invited, and a household member can answer for the others.

| Field | Notes |
|---|---|
| `id`, `name` | |
| `members` | people ids |
| `home` | the *instance* household (the one the board sits in) is flagged. Only it gets LAN self-registration powers, see below |

Any admin can create households and move people between them. A person is in
at most one household.

## Sign-in methods

| Method | Where | Creates an account? |
|---|---|---|
| PIN | `party.local` only (the board is only on the LAN) | Self sign-up on the LAN, if open sign-up is on (default on) |
| Mastodon OAuth, board-side | `party.local` | No. Links to an existing person, or redeems an invitation |
| Mastodon OAuth, browser-side | companion site | No. Authorizes the *browser* to send DMs as the person. The board never sees that token |
| Personal link | companion site | No. Decrypts the person's keyring. A read credential, not a sign-in |
| DM from a linked identity | bot | No, except `redeem <token>` |

### PIN (local)

Copy Game Matcher's rules exactly: email (or display name, see
[open questions](open-questions.md)) + 4–8 digit PIN, per-account lockout
doubling to 1 h, board-wide 30 failures per 10 minutes, first visitor becomes
admin, optional build-time setup code, ≥ 1 active admin invariant, forgot-PIN
codes by email when email is configured.

Draft open decision 5 ("how is local registration restricted to actual
household access?"): **it isn't, beyond being on the Wi-Fi.** That's the
Game Matcher risk model and the owner's. Open sign-up creates an ordinary
`member` with no groups. Admins can switch open sign-up off.

### Mastodon OAuth on the board

The framework `oauth` feature, as in Game Matcher: PKCE, dynamic registration,
every network step on the outbound worker, the token used once (to read the
account) and revoked. Redirect URI `http://party.local/auth/callback`. Works
from any instance. Used to link an identity to a signed-in person, to sign in
as a person who has one linked, or to redeem an invitation while visiting.

### Mastodon OAuth in the companion browser

The companion site does its own PKCE flow against the person's instance
(Mastodon allows CORS on `/api/v1/apps` and `/oauth/token`):

- Scopes: **`profile write:statuses`** and nothing else. `profile` tells the
  site which account it is, `write:statuses` lets it post a direct DM to the bot.
  It cannot read the person's DMs or timeline.
- Redirect URI: the companion site's own `/auth/mastodon` page.
- The token stays in that browser (`localStorage`), with a "forget this
  device" button. The board never receives it.

This answers the draft's open decision 4 (OAuth callbacks while the board isn't
reachable): the companion site never needs the board to finish OAuth.

## Invitations

An invitation has a target (group, event, or the instance), an issuer, an
optional recipient hint (email, Mastodon handle, household), a role to grant,
an expiry (default 14 days), a use count (default 1) and a status
(`open`, `used`, `revoked`, `expired`). The token is 128 bits, stored hashed.

Delivery: by DM to a handle, by email, or as a link/QR the organizer copies.
The link is `https://<site>/#/invite/<token>`. The token sits in the URL
fragment, so GitHub never sees it.

Redemption paths:

1. **At the house:** open the link on the Wi-Fi (the site offers "open on
   party.local"), sign up with a PIN or with Mastodon, done.
2. **Remotely, with Mastodon:** the companion site signs the browser in with
   Mastodon and posts `redeem <token>` as a direct DM to the bot. When the board
   next polls, it binds the sender's `(instance, account id)` to a new or
   existing person, activates them, and DMs back their personal link.
   If the board is offline, the DM waits in Mastodon. **Asynchronous redemption
   falls out of the queue for free** (draft open decision 2).
3. **Remotely, email only:** the email carries the person's personal link
   directly (the organizer created them, so the board already has a person
   to make a key for). Reading works at once. Answering needs a visit, someone
   to answer for them, or the relay.

Draft open decision ("are household and invited accounts different?"): **no.**
Once active they're the same kind of person. What differs is group membership,
which the invitation grants.

## Roles

Unchanged from the draft: visitor, member, organizer, co-organizer,
administrator. Every check happens in the service layer, whatever the input
channel. Group roles live on the membership ([03](03-domain.md#groups)).
Co-organizers can do everything an organizer can except transfer ownership,
delete the group, or demote the last organizer. Admin actions go into a
bounded audit log.

## Sessions

Framework `session` feature: HttpOnly cookie, stored hashed, 30-day sliding
expiry, at most 6 per person. Same-origin guard on unsafe methods. The local
app is served over HTTP for guests and HTTPS/HTTP2 for household devices
that trust the household CA, as in Game Matcher.
