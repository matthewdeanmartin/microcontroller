# 02 — Accounts, sign-in and sessions

"One-and-a-half sign-on": everyone can use **email + PIN** on plain HTTP. Some
people can also use **Sign in with Mastodon**. Both lead to the same local account,
and the board issues its own session either way.

## Accounts

| Field | Notes |
|---|---|
| `id` | u16 slot, never reused while the account exists |
| `email` | Login name. Lower-cased, trimmed, unique, never verified; it only needs to look like `x@y`. Never shown to other members. Once email is set up ([08](08-mastodon.md#email)), the board mails it plan news (each person can turn that off) and "forgot PIN?" codes. |
| `display_name` | 1–32 chars, shown everywhere |
| `pin_hash` | PBKDF2-HMAC-SHA256, 16-byte salt, round count stored with the hash. 1,000 rounds = 99 ms on the C6 (measured October 17) |
| `role` | `admin` or `member` |
| `status` | `active` or `disabled` |
| `mastodon` | 0–1 linked identity: `(instance host, account id, @handle)` |
| `interests` | bitmap over game slots ([03](03-domain.md#interests)) |
| `notify` | in-app is always on. Optional Mastodon DM ([08](08-mastodon.md)) |
| `feed_token` | 128-bit random, for the calendar feed URL. Stored hashed, rotatable |
| `created`, `last_seen` | UTC seconds |

No avatars in v1, so no media storage. The UI draws initials in a colour derived
from the id.

### Sign-up

Anyone on the LAN can create an account: email, display name, PIN twice.
This is how guests join. **Open sign-up is on by default.** Admin can turn it
off in settings so that only the admin creates accounts, but in a household that's rarely needed.

### First-run admin

While no account exists, every page shows **"Set up Game Matcher"**. Whoever submits
it first gets an `admin` account with the email and PIN they chose. The claim is
atomic under the state lock, so a second simultaneous submit gets 409.

A build-time setup code (`GAMEMATCHER_SETUP_CODE`) can optionally gate this, using the
same pattern as the framework's `wifi-setup`. It is off by default because of the risk model.

### Admin powers

Disable/enable accounts, reset someone's PIN (the admin sets a temporary PIN and tells
them), promote/demote admin (there must always be ≥ 1 active admin), delete
any comment, cancel any plan, edit or merge games, toggle open sign-up, configure
Mastodon providers and the bot, sign out all sessions of a user.

## PINs

- Digits only, 4–8 long. A PIN is enough for a house.
- **Rate limits are mandatory** because 4 digits is only 10,000 guesses:
  - per account: 5 failures, then a lockout of 1 min, doubling up to 1 h. Reset on success.
  - board-wide: 30 failures per 10 min across all accounts, then every PIN
    sign-in waits. The framework doesn't expose the client's IP
    address, so there is no per-IP limit. Mastodon sign-in is unaffected.
  - Implemented with the framework's `rate-limit` feature. Counters live in RAM
    (a reboot clears them, which is acceptable).
- Hash verification runs on the request path. About 100 ms is fine at household rates.
- Changing your PIN requires the current one. Admin reset doesn't.

## Sign in with Mastodon

Uses the framework's `oauth` feature ([miniframework/spec/OAUTH.md](../../miniframework/spec/OAUTH.md)),
with its Mastodon preset. Game Matcher knows nothing about mastomini
specifically. A household configures instances by URL.

### Providers (admin settings)

| Setting | Meaning |
|---|---|
| Instance URL | `https://mastomini.local`, `https://mastodon.social`, … |
| Trust | `household` or `invited` |
| Trust anchor | `household-ca` (the build's CA) or `public` (ESP-IDF certificate bundle) |
| Client registration | automatic via `POST /api/v1/apps` on first use, then stored in kv |

**Trust levels:**

- `household`: any account on that instance may sign in. A first sign-in
  creates a local account (email left empty, or asked once). Meant for an
  instance the household runs, like mastomini.
- `invited`: only handles on the instance's invite list (`@bob@mastodon.social`) can
  sign in, plus anyone who has already linked that identity to an existing local account.
  This stops a stranger on mastodon.social from signing in, which matters only if they're also on the Wi-Fi,
  but costs nothing.

Scopes requested at sign-in: **`profile`** only (Mastodon 4.3+), falling back to
`read:accounts` on instances that don't list `profile` in their discovery
document. The board calls `verify_credentials` once, takes `(host, id, acct,
display_name)` and **discards the access token**. Posting as yourself is a
separate, later grant ([roadmap](roadmap.md)).

### How it runs (as built)

1. The UI `POST`s `/api/v1/auth/oauth/start {provider, link}`. The board answers at
   once with a ticket (and an `mf_oauth` cookie binding it to this browser). Its
   worker registers with the instance if needed (`POST /api/v1/apps`, both http
   and https callback URIs) and builds the authorize URL.
2. The UI polls `GET /api/v1/auth/oauth/{ticket}` until `redirect`, then sends the
   browser to the instance.
3. The instance returns to `/auth/callback`. The board checks state against the
   cookie, queues the code exchange, and redirects to `/signin/finishing?t=…`.
4. That page polls the ticket until `done` (the session cookie is set right then)
   or `failed`.

Accounts made by Mastodon sign-in have no email and no PIN until the person sets
one. A closed sign-up also stops new accounts from a `household` instance (only
invited handles still get in).

### Linking

- Signed in by PIN → Profile → "Link Mastodon account" → OAuth → identity stored on
  the account. Any account with a PIN can unlink.
- Signed in by Mastodon with no PIN set → "Set a PIN" is offered but optional.
  The account can't be locked out because Mastodon sign-in still works.
- One Mastodon identity links to at most one local account (409 otherwise).
- Admin can unlink anyone's identity.

### Identity rules

The key is `(instance host, account id)`, not the handle: usernames can be renamed,
and ids can't. The handle is display only and is refreshed on each sign-in.

## Sessions

Framework `session` module ([OAUTH.md § Sessions](../../miniframework/spec/OAUTH.md#sessions)):

- Cookie `gm_session`: 128-bit random token, `HttpOnly; SameSite=Lax; Path=/`,
  plus `Secure` when issued over HTTPS. Stored as SHA-256 hash.
- 30-day idle expiry, 180-day absolute. At most 4 sessions per account, 64
  total; the oldest is evicted.
- Sessions are persisted, so a reboot doesn't sign everyone out. `last_seen`
  is written at most hourly per session (flash wear).
- "Sign out everywhere" on profile; admin can do it for anyone.
- **CSRF:** unsafe methods require `Origin` (or `Referer`) to be one of the board's own
  origins (`http(s)://gamematcher.local[:port]`, or the board's IP). This plus `SameSite=Lax`
  is enough here. There are no cross-site forms to support.

## HTTP and HTTPS

| | HTTP :80 | HTTPS :443 |
|---|---|---|
| Who | guests, anyone | household devices with the CA installed |
| UI and API | everything | everything, plus h2 |
| PIN sign-in | yes | yes |
| Mastodon sign-in | yes. Redirect URI `http://gamematcher.local/auth/callback` | yes. Redirect URI `https://…/auth/callback` |
| Session cookie | not `Secure` | `Secure` |
| Redirect HTTP→HTTPS | **never** (guests would hit a certificate warning) | — |

Both redirect URIs are registered with every instance (Mastodon accepts a
newline-separated list). The redirect URI used is the one matching the origin the browser
used, as mastomini does with `Call::origin`. The owner has signed in to
mastodon.social from `http://localhost:4200`, and Mastodon's redirect-URI
validation is permissive. `http://gamematcher.local/…` is expected to work
and gets confirmed in Sprint 6 rather than designed around.

A cookie set over HTTPS with `Secure` isn't sent over HTTP. Someone switching
schemes signs in again, which is acceptable.

The `/trust` and `/ca` pages (framework built-ins) are linked from the
profile page so household devices can install the CA when they want HTTPS.
