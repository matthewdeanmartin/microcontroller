# Roadmap (after v1)

Ordered roughly by value to the household. None of this is committed.

## Board migration: export and import

Owner framing: when Game Matcher moves to another board, people want their
**profiles and the game catalog** to come along. Plan history is nice to have.

- `GET /api/v1/admin/export` → one versioned file (CBOR or JSON, the client
  picks): accounts (email, display name, role, PIN *hash* + rounds, linked
  identities, interests, notify settings), games, and optionally plans +
  comments. **Excludes** sessions, feed tokens, OAuth client registrations, the
  bot token, and the outbound queue. Those are per-board secrets or ephemeral.
  Users sign in again and re-copy calendar URLs.
- `POST /api/v1/admin/import` on a fresh board (setup screen offers "Restore
  from another board"): validates the schema version, remaps ids, refuses
  if any bound would be exceeded, and is all-or-nothing.
- Because PIN hashes move, PINs keep working. Different hardware may want
  more rounds: the stored count lets the new board rehash on next sign-in.
- The export is plain data with PIN hashes in it. Say so in the UI.

## More boards

- **ESP32-S3 N16R8 profile.** More connections, 2+ TLS sessions, PSRAM-resident
  comments, room for OTA. This is also the fallback if the Sprint 0 gate fails.
- **ESP32-P4** (mastomini already targets one) if a household has one free.
- **Board profiles as data:** bounds from [05](05-storage-limits.md) become a
  per-board table chosen at build time (`GAMEMATCHER_BOARD=c6|s3|…`), so the same
  code ships everywhere.
- **No S2 web server** (owner decision): S2 boards are better used as
  non-HTTP workers. A possible fit here is the remote display below.
- **Remote display device:** a second board polls `/api/v1/display` every 60 s
  and draws the same views. The ST7789 renderer moves into the framework as an optional
  `display-st7789` feature at that point (the C component plus its Rust wrapper).

## OTA

Follows the framework design ([miniframework/spec/OTA.md](../../miniframework/spec/OTA.md)).
On the C6 the 8 MiB flash leaves about 4.8 MiB free after the v1 layout, enough for a
second 2.5 MiB app slot. The partition table must change, so that's one more USB
flash with an export/import around it.

## Sign-in and identity

- **OIDC in the framework OAuth client:** verify `id_token` (JWKS, ES256/RS256)
  for providers that offer it. That turns household SSO into real SSO with any
  OIDC provider (Authelia, Keycloak, Pocket ID).
- **Household SSO across apps:** NanaCoin, housemetrics etc. can opt into the
  same framework `oauth` feature. Each app configures its own providers. There's
  no shared dependency between apps.
- **Guest passes:** a QR code on the LCD that signs a guest into one plan for a
  day with no account.

## Mastodon

- **Post as yourself:** a separate `write:statuses` grant per user, token stored
  in kv, revocable. Only after the C6/S3 decision, because tokens at rest on
  unencrypted flash are a new class of secret.
- **Reply to RSVP:** implemented for household and public bots with durable
  polling and linked native identities; see [08](08-mastodon.md).

## Email

- Optional SMTP relay (S3 only, or the C6 if the gate leaves room): invite a
  guest by email with a link to `http://gamematcher.local/…` and LAN-only
  instructions; optional email verification. Still no inbound mail.

## Features

- Recurring plans ("every Friday"): implemented as finite weekly/fortnightly/monthly series.
- Who owns which game (tabletop copies, Steam licences).
- Game metadata import from a JSON file (not a third-party API).
- Plan history stats: most-played, "haven't played Wingspan in 3 months".
- Private plans (invite-only visibility) if the household turns out to want them.
