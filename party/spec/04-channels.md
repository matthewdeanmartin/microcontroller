# 04 — Channels: the bot, the DSL, the LLM, email

Every input (local REST, DM, relay, organizer recording an answer for someone)
turns into the same `Command` and goes through the same service layer and
permission check. This file covers the ones that aren't the local web app.

## The Mastodon bot as a message queue

The bot is an ordinary account on a public instance (mastodon.social to
start), with the "this is a bot" flag set. The board holds its access token.

- **Setup** is the whole skill the baseline asks for: create the account,
  create an application on its Development page with scopes
  `read:notifications read:statuses write:statuses`, paste the token into
  the board's admin page. The board verifies it (`verify_credentials`) and shows the handle.
- **Inbound:** poll `GET /api/v1/notifications?types[]=mention` every 60 s
  (configurable 30 s–10 min), paging forward with `min_id` in bounded batches.
  Only `direct`-visibility statuses are commands. Public mentions get one
  polite DM reply asking to DM instead, at most once per account per day.
- **First poll** records the newest id and executes nothing historical (Game
  Matcher's rule).
- **Sender identity** is the status's account `(instance host, account id)`.
  Never the display name or handle text.
- **Outbound:** replies and notifications are `direct` statuses mentioning only
  the recipient, with an `Idempotency-Key` per job.
- **Offline is normal.** The queue is Mastodon itself: commands wait there
  while the board is off, and are processed in order when it comes back.

### Durable processing

1. Fetch a batch. For each status, skip it if its id is in the processed set.
2. Parse and execute it. The state change and "processed id + reply job" commit
   together, so a crash replays nothing twice and loses no replies.
3. Advance the cursor only after the batch is committed.

The processed set is a ring of the last 4,096 status ids plus the cursor, enough
to cover any overlap between polls.

### Throughput ceiling

Mastodon's default limits are about 300 API requests per 5 minutes and 300
posted statuses per 3 hours per account. Polling uses a tiny share of the
first. **The second caps the bot at roughly 2,400 replies a day.** At family
scale that's irrelevant. For the internet stretch goal it's the real limit, so:

- Replies coalesce: one DM can confirm several commands from the same sender
  in the same batch.
- Notifications follow people's settings, and email takes some of the load.
- A second bot account, or the relay, is the scaling lever. Note it, don't build
  it yet ([roadmap](roadmap.md#scale)).

## The command DSL

Plain words, case-insensitive, one command per line (up to 5 per DM). The bot's
mention at the start is ignored. The companion site generates exactly these lines,
so people see what they send.

```
help [topic]
me                                   your groups, upcoming answers, settings
events | upcoming [group]            next events you can see
event E12                            one event's details (only what you may see)
groups | find <words>                groups you're in / discoverable groups
join <group>  |  leave <group>       group = G3 or a name
going E12 [+N] [note text]           also: maybe E12, no E12
rsvp E12 going|maybe|no [+N] [kids K] [note text]
bring E12 <slot or text>             claim a bring-list slot
unbring E12 <slot>
ride E12 need N | offer N | own
when E12 1,3                         time-poll choices
shift E12 <name>
answer E12 <question no> <answer>
regular S4 on|off                    standing "going" for a series
redeem <token>                       accept an invitation
link                                 send me my personal link again
newlink                              reset my personal link (old one stops working)
stop [group]  |  start [group]       DM notifications off/on
undo                                 revert your last change (within 1 h)
confirm <code>  |  cancel            for actions that need confirming
```

Organizer commands come later ([sprints](sprints.md#sprint-11-llm-and-organizer-commands)):
`new event`, `cancel E12`, `invite @handle G3`, `remind E12`.

Resolution rules:

- `E12`, `G3`, `S4` are exact. Names are matched against groups and events
  **the sender can see**, normalized. More than one match, or a match with low
  confidence → a clarification reply listing up to 5 choices, with ids.
- Relative words are accepted where unambiguous: `going saturday` means the
  sender's only event on the next Saturday, otherwise a clarification.
- Unknown command → a short `help` reply (rate-limited per sender).

### Replies

Short, human, with a machine-readable last line the companion site and tests can
match:

```
You're going to Sunday dinner (Sun Oct 26, 5pm) with +1. Bringing: salad.
⟨party ack 109274455 E12 going+1 r7⟩
```

The bot never DMs member lists, addresses (unless the event's address class
allows it and the person is going), emails, PINs, or other people's answers
beyond counts.

## The optional LLM

When a DM doesn't parse, and the admin has configured an OpenRouter key, and
the sender's `llm_ok` is on:

1. The board sends OpenRouter: the DSL grammar, the sender's visible upcoming
   events and groups (ids, titles, dates only), today's date, and the message
   text. Model: a Gemma model, set by id in admin settings.
2. It asks for **exactly one DSL line or `unclear: <question>`**.
3. The board parses the answer with the same parser. Anything that doesn't parse
   counts as `unclear`. The LLM never touches state and never sees anything the
   sender couldn't see.
4. Member-level commands (RSVP, bring, ride) run and the reply echoes the
   interpreted command with an `undo` hint. Organizer and admin commands always
   need `confirm <code>`.
5. Budget: at most 200 LLM calls a day and 20 per sender per day. Over budget
   → `help` reply. A timeout (15 s) → "I didn't understand, try `help`".

The admin page and `help` both say plainly that free-text messages go to
OpenRouter when the LLM is on.

## Email

Same mechanism as Game Matcher ([gamematcher 08 § Email](../../gamematcher/spec/08-mastodon.md#email)):
Brevo's transactional API, an API key (not an SMTP key), the outbound worker,
100 a day by default (Brevo's free plan allows 300).

- **Outbound only.** No inbound mail in v1, so email is the "limited
  functionality" channel the owner described.
- What email carries: invitations, event updates and reminders with `.ics`
  (`REQUEST`/`CANCEL`), the person's personal link (at creation and on
  `newlink` from the local app), forgot-PIN codes.
- **Reply-To is the organizer's email** if they have one and allow it, so
  "reply to this email" reaches a human who can answer for them.
- Inbound email (poll an IMAP mailbox like the bot polls Mastodon) is
  [roadmap](roadmap.md#inbound-email): same queue design, different transport.

## Outbound queue

One worker serves OAuth steps, DMs, email, LLM calls and publication, in
priority order: OAuth (a person is waiting) > command replies > notifications >
LLM > publication. 64 entries in PSRAM, persisted, backoff from 2 minutes to 1
hour, give up after 24 h, admin retry. Copied from Game Matcher's `outbound.rs`
and widened for the new job kinds.

## Bluesky (later)

The channel trait (`poll() -> Vec<Inbound>`, `send(Outbound)`) is written so a
Bluesky DM channel can be added: an app password with DM access, polling the
chat API, the sender identified by DID. [Roadmap](roadmap.md#bluesky).
