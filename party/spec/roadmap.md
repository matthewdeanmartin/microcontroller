# Roadmap (after v0.1)

Directions, not commitments. Roughly ordered by how soon they'll be needed.

## Accepting anyone

v0.1 is invitation-only outside the house. Opening up means:

- **Open join by DM:** anyone can DM `join G3` for an `open` public group. Their
  Mastodon identity becomes a new person with no PIN.
- **Abuse controls before that ships:** per-sender command rate limits that
  persist across reboots, a block list (accounts and whole instances),
  organizer approval queues, account age/follower heuristics (configurable, off
  by default), and a "pause all public joins" switch.
- **Moderation:** report an event or person (DM `report …`), an admin queue,
  hiding without deleting, an audit trail.
- **Cost of strangers:** every stranger costs bot replies (the 2,400/day
  ceiling) and keyring files. Open groups should default to fewer DMs and rely
  on the public site.

## Scale

What breaks first as an instance grows, and what to do:

1. **Bot replies** (≈ 300 statuses / 3 h per account): coalesce more, notify by
   email, spread over several bot accounts (one per group or round-robin, the
   board polls all of them), or route via the relay.
2. **Publication size:** keyrings split by "changed recently", public data
   paginated by month, per-group public files.
3. **Board RAM/flash:** an SD-card backend for the record store (the S3 Touch LCD
   boards have slots), or move canonical state to a bigger machine. That's the
   point where party gets its own repo.
4. **Latency:** the owner's framing is right. A message-queue design can serve
   tens of thousands of people if they accept answers that take minutes to hours.
   Make the expected delay visible rather than fighting it.

## Federation

No ActivityPub. The pieces party already has suggest **static federation**:

- **Read side, pull-based:** each board already publishes a manifest. A board
  can *follow* another board's public (or sealed, given a scope key) data by
  polling its Pages URL, like RSS. "Grandma's board shows our family's events"
  needs no new transport.
- **Write side, signed envelopes over existing queues:** each board gets an
  Ed25519 board key. A cross-board command (my member RSVPs to your event) is a
  signed envelope sent as a DM from my bot to your bot (or through your relay).
  Your board verifies my board's key, which an admin pasted in when the boards
  were paired (out of band, like exchanging phone numbers).
- **Identity across boards:** the same Mastodon `(instance, account id)` is the
  same human. Each board still keeps its own person record. Boards never share
  PINs or personal keys.
- **Open problems:** who owns an event two boards care about (answer: the host
  board, and everyone else is a mirror), ordering and conflicts (last writer by
  host-board receipt order), pairing UX, key revocation.
- **Suite-wide:** the envelope (board key, nonce, timestamp, idempotency id,
  payload type) should be a framework feature, because NanaCoin board-to-board
  payments need the same thing plus two-phase holds. Design it once, in
  `miniframework/spec/`, when the second consumer is real.

## Suite integration (optional, never required)

- Game Matcher: a `game night` event kind that links a Game Matcher plan (by URL,
  or by API key/PKCE between `party.local` and `gamematcher.local`).
- NanaCoin: `cost` module shows "pay in NanaCoin", and a bank API key lets an
  organizer request payment. Payments stay in NanaCoin.
- mastomini: the household instance as a second bot or a sign-in provider for
  the house. Already works through generic Mastodon.
- Household SSO across apps via framework OAuth/OIDC (Game Matcher roadmap).

## Virtual and hybrid events

- `link` module grows: a meeting link (Jitsi needs no account), `sealed` or `direct`
  class, sent by DM/email to people who are going shortly before start.
- **Per-event time zones:** needed once guests are in other zones. Store an
  IANA zone name per event and a compact zone table on the board, or the series
  rule in UTC offset plus DST rule.
- Hybrid: separate capacities for in-person and online.

## Inbound email

Poll an IMAP mailbox (a free Gmail/Fastmail address dedicated to the instance)
the way the bot polls Mastodon: sender = verified `From` + DKIM pass, DSL in the
body, or replies to invitation emails carrying a per-person reply token in the
address (`party+<token>@…`). This turns email-only people into full remote
users without the relay. Risk: spoofing, so it needs DKIM checking and the reply
token, and admin opt-in.

## Bluesky

A second DM channel: app password with DM access, polling `chat.bsky.convo`,
sender = DID, linking like Mastodon identities. Bluesky DMs are not end-to-end
encrypted either. Same data rules.

## Backup, restore and migration

Owner: last, as a non-binary export, because the schema keeps changing and the
data is fairly ephemeral.

- `GET /admin/export` → one versioned JSON document: people (PIN hashes and
  rounds, identities, households), groups, memberships, series, upcoming events,
  answers. Excludes sessions, tokens, keys, queues.
- `POST /admin/import` on a fresh board, all-or-nothing, with id remapping.
- New personal keys are issued after a restore (old links stop working), and the
  new links go out by DM/email.
- Optionally, an encrypted export pushed to the data repo as an off-board backup.

## OTA

Partition `app1` is reserved from Sprint 0. Follow
[miniframework/spec/OTA.md](../../miniframework/spec/OTA.md) when the framework
builds it. Until then, flashing is USB, app-only.

## Smaller things

- Avatars (resized in the browser, small WebP, stored as records).
- Comments per event, if people want them (Game Matcher's design).
- Flash encryption for secrets at rest.
- A P4 board profile.
- "Who's free?" availability polls across a group without an event.
- Photo links after an event (links only, no media storage).
