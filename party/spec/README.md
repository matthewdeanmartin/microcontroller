# party specification

Status: **design**, written October 21, 2026. Nothing here is implemented.

party is a small Meetup: clubs and parties for a household and the people
around it. Someone starts a group ("Sunday dinners", "Book club"), schedules
events, invites people, and they answer: going, maybe, no, plus one, bringing
the salad, need a ride. The canonical data lives on one ESP32-S3 in the house.
People outside the house see it through a static companion site on GitHub
Pages and talk to it through a Mastodon bot that the board polls.

It is a [miniframework](../../miniframework/) app: Rust API in `api/`,
Angular workspace in `ui/` (the local app, the companion site and its demo
build), served from the board at `party.local`. If it outgrows the
microcontroller, it moves to its own repo.

This replaces the "Gather32" draft (v0.1). That draft's settled points are kept;
its open decisions are answered below and in each file.

## Files

| File | Contents |
|---|---|
| [01-overview.md](01-overview.md) | Audiences, architecture, risk model, hardware |
| [02-identity-auth.md](02-identity-auth.md) | People, identities, PIN, Mastodon sign-in, invitations, households, roles |
| [03-domain.md](03-domain.md) | Groups, events, event kinds and modules, RSVPs, plus-ones, recurrence, notifications, calendars |
| [04-channels.md](04-channels.md) | The Mastodon bot as a message queue, the command DSL, the optional LLM, email |
| [05-companion-site.md](05-companion-site.md) | GitHub Pages, publication, data classes, encryption, keyrings, remote RSVP, demo site |
| [06-api.md](06-api.md) | REST endpoints and conventions |
| [07-storage-limits.md](07-storage-limits.md) | Record store, bounds, flash layout, RAM |
| [08-relay.md](08-relay.md) | The optional Cloudflare relay |
| [09-ui.md](09-ui.md) | Angular workspace, local app screens, companion screens |
| [sprints.md](sprints.md) | Sprints 0–13 with exit criteria |
| [roadmap.md](roadmap.md) | Afterwards: anyone can join, virtual events, backup, federation, Bluesky |
| [open-questions.md](open-questions.md) | Assumptions made while writing this, to confirm |
| [NEXT_POST_INITIAL_TESTING.md](NEXT_POST_INITIAL_TESTING.md) | What's left after Sprints 0–8: remaining sprints, carried-over gaps, owner steps and decisions |

## Owner decisions (October 21, 2026)

1. **Name.** The app is `party`. Its mDNS name is `party.local`. It lives in
   `microcontroller/party/`.
2. **Audiences, in order:** the house (a few people), the family in other
   houses (an adult son, a mother-in-law), and as a stretch goal the open
   internet.
3. **The board is never on the internet.** It only makes outbound connections.
   Crawler traffic alone would knock it over.
4. **GitHub Pages is public.** Plaintext publication is limited to
   non-sensitive data. Sensitive data goes by Mastodon DM (later perhaps
   Bluesky), or is **encrypted** before it is published.
5. **Some people only get email.** They get limited functionality: they read
   the email, view what their link decrypts, and use the full app when they
   visit the house.
6. **Mastodon is the message queue.** The bot runs on the board. RSVPs from
   the companion site are DMs to the bot, picked up when the board polls. If
   that fails in practice, other transports get considered.
7. **Commands are a DSL**, with an optional Gemma model through OpenRouter
   to turn free text into the DSL.
8. **party is general, Game Matcher is specific.** Game Matcher keeps growing
   home-game features. party is for clubs and parties and is not tied to one
   domain.
9. **The suite integrates, optionally.** NanaCoin, Game Matcher, mastomini and
   party may later talk to each other (PKCE, API keys). No app may require
   another.
10. **Hardware: ESP32-S3 with PSRAM.** Not the S2. Not the P4 as a requirement:
    it's too expensive to ask people to buy one.
11. **Backup is roadmap**, done last, as a non-binary export. The data is
    fairly ephemeral, so losing it is not a disaster.
12. **Federation is unsolved.** Ideas go in the roadmap. No ActivityPub.
13. **A rich domain, not Meetup's anemic one.** Several event kinds and RSVP
    shapes, each family with a generic option.
14. **Plus-ones** are allowed unless the organizer forbids them or caps them.
15. **Recurring events** and **calendar feeds** are in.
16. **An optional Cloudflare component** is allowed, but never required. The
    baseline must be installable by someone who can set up a mastodon.social
    bot account and nothing harder.
17. **Tech:** the Rust framework in this repo, Angular, and the record-store
    style already used by mastomini (see [07](07-storage-limits.md)).
