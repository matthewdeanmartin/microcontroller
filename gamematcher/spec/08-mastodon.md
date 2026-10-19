# 08 — Optional Mastodon bots and email

Everything here is **off until an admin configures it**, and the app works
fully without it. Sign-in is in [02](02-accounts-auth.md). This file covers
outbound delivery and incoming RSVP replies.

Generic Mastodon REST only (`POST /api/v1/statuses`) for posting. The one mastomini
extra is optional: creating the bot account through mastomini's admin API
(`/api/mastomini/v1/admin/bots`), the way mastomini-bots enrols bots. mastomini, mastodon.social or any instance with the same API works.

## Bot account

Admin settings: instance (one of the configured providers), an access token
for a bot account with `write:statuses` (pasted in. Created on the instance's
own "Development" page, or on mastomini by its admin), and a default
visibility.

| Post | When | Default visibility |
|---|---|---|
| "New plan: Catan or Wingspan, Sat afternoon. Answer at gamematcher.local" | plan created, if the host ticked "announce" | `unlisted` on a `household` instance, `private` elsewhere |
| "Locked in: Catan, Sat 2pm" | lock-in, if announced | same |
| "Cancelled: …" | cancel, if announced | same |

Rules:

- **Never** post emails, PINs, feed tokens, comment text, or anything with a
  session or token in a URL. Links are only ever to `http://gamematcher.local/plans/{id}`,
  which is useless outside the house and harmless.
- Announcing is opt-in **per plan** (host's checkbox), default off unless the
  admin sets "announce by default".
- `public` visibility is never chosen automatically. The admin must pick it explicitly
  as the default.

## Direct-message notifications

Locked plans queue a reminder 15 minutes before their chosen start, once per
scheduled time, for the host and everyone who answered `in`. Each reachable linked
Mastodon identity receives its own `direct` status. Rescheduling and other plan
edits also send direct updates to engaged participants (host and `in`/`maybe`,
except the editor). These deliveries do not depend on the public-announcement
checkbox or email preferences.

A local bot reaches identities on its own instance; a federating bot can also
reach public instances. Email reminders use the configured Brevo sender and honor
the person's plan-email preference. The board checks due reminders once a minute,
requires a synced clock, and does not send reminders for already-started events.

## Two bot accounts and incoming RSVPs

The household bot retains its existing bot/outbox KV keys and /api/v1/admin/bot.
The public bot uses independent pub_* keys and /api/v1/admin/public-bot.
Each has its own token, queue, digest settings, notification cursor, and inbox.
Public digests include only plans with announce_public enabled. Plan posts require
separate host consent for each network.

Incoming RSVPs are opt-in via the replies setting. The worker polls mentions
every minute, using min_id to page forward in bounded batches. The first poll
establishes the newest cursor without executing historical commands. Pending
mentions and cursor persist together. State commits before inbox acknowledgement;
per-account processed IDs prevent duplicate changes after a crash.

Accept exact in/maybe/out replies to known bot plan posts, or an explicit plan
number (in 42). Native (instance, account-ID) matching avoids trusting display
names, handles, or a remote instance's unrelated numeric IDs. Disabled/unlinked
accounts cannot RSVP. Public users may answer an explicitly shared plan or one
they already participate in. Existing game choices and notes are retained;
confirmed-time answers track the RSVP. Accepted commands get direct confirmation.

Protocol: [Mastodon notifications API](https://docs.joinmastodon.org/methods/notifications/).
Tokens need read:notifications, read:accounts, and write:statuses. This uses
outbound HTTPS; opening the board to the internet is not required. Website and
calendar links still require access to the home network.

## Delivery status and retry

- Each bot has a bounded 32-entry queue in its own KV record; the household queue
  also carries email. Finished entries are evicted first.
- One outbound worker serves OAuth, both bots, and email sequentially.
- Failed attempts back off from two minutes to about an hour; jobs fail after
  12 attempts or 24 hours. Admins can retry from /status.
- GET /api/v1/admin/delivery returns both bot views and mail status. It is
  admin-only because delivery text may contain private messages and addresses.
- POST /api/v1/admin/bot/retry/{id} and /api/v1/admin/public-bot/retry/{id}
  retry failed attempts. Sent and currently sending jobs cannot be retried.
  Mastodon retries preserve the Idempotency-Key derived from job ID/creation time.
- Status shows attempts, next retry, last error, provider acceptance, and incoming
  polling errors. Provider acceptance is not a recipient read/delivery receipt.

## Email

Added October 17, 2026: "the goal is for this to be unattended and super easy". The board sends
through **Brevo's transactional API** (`POST https://api.brevo.com/v3/smtp/email`, one HTTPS
request on the same outbound worker and queue as the bot). SMTP would need a mail client on the
board. That's why it takes a Brevo **API key** (`xkeysib-…`) and refuses an SMTP key (`xsmtpsib-…`)
with a hint. Code: `api/src/mail.rs` (what's said), `api/src/outbound.rs` (queue, sending).

- **Setup:** `BREVO_API_KEY` (or `BREVO_OTHER_KEY` / `GAMEMATCHER_BREVO_API_KEY`) in any
  `.env` the build reads, including the household `C:\github\.env`, or pasted on the admin
  page. The worker checks the key with `GET /v3/senders` and sends from the first verified sender
  (or `GAMEMATCHER_MAIL_FROM`), as "Game Matcher". A key set on the admin page wins over the
  built-in one.
- **Plan news:** new plan (to people into its games), lock-in, cancellation, new times,
  reminder, "you're hosting". On by default; each person can turn it off on their profile.
  Answers and comments stay in the bell. Email follows the bell's coalescing: one email per
  unread notification, so a busy plan doesn't flood anyone.
- **Invites:** addresses typed on a plan's invite card (up to 8 at a time), from "Sam via Game
  Matcher". The share links and `mailto:` stay for people without the board's email.
- **Forgot PIN:** `POST /api/v1/auth/pin-reset {email}` emails a 6-digit code (always answers
  OK, so it can't test addresses; at most one per minute per account). `/confirm {email, code,
  pin}` sets the PIN and signs in. Codes last 15 minutes and allow 5 tries. They're kept in RAM only.
- **Limits:** 100 emails a day (Brevo's free plan allows 300). Queued email survives a reboot
  and retries with backoff like posts. Links point at `http://gamematcher.local`, so they work
  on the home Wi-Fi.
- **Deliverability:** the sender is a Gmail address verified in Brevo. Gmail-to-Gmail through a
  relay may land in spam at first; "not spam" once fixes it. A domain of our own (DKIM) is the
  real fix, if it's ever needed.

## Not in v1

Posting as yourself remains on the [roadmap](roadmap.md). Incoming RSVP replies
are implemented for linked identities native to either configured bot's instance.
