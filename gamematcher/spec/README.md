# Game Matcher specification

Status: **design**, written October 17, 2026. Nothing here is implemented.

Game Matcher is a household app for deciding *what to play and when*. Someone
posts a plan ("Catan or Factorio, Saturday afternoon, sometime 2–6"), the
others tick the games they would play and the times they can do, and the
group sorts out the rest by talking. The board keeps the plans, the
catalog, the comments and a calendar feed, and shows what's coming on its
own screen.

It is a [miniframework](../../miniframework/) app: Rust API in `api/`, Angular
UI in `ui/` built on `miniframework-ng`, served from the board at
`gamematcher.local` over HTTP, HTTPS and HTTP/2.

This replaces the ChatGPT draft (v0.1, same date). The draft assumed internet-facing
features, email delivery, per-user time zones and a matching
service. Those were cut or deferred. The reasons are in each file.

## Files

| File | Contents |
|---|---|
| [01-overview.md](01-overview.md) | Goals, non-goals, risk model, hardware target |
| [02-accounts-auth.md](02-accounts-auth.md) | Email + PIN accounts, first-run admin, Mastodon sign-in, sessions, HTTP vs HTTPS |
| [03-domain.md](03-domain.md) | Games, interests, plans, responses, comments, notifications, calendar |
| [04-api.md](04-api.md) | REST endpoints and conventions |
| [05-storage-limits.md](05-storage-limits.md) | Persistence, bounds, flash and RAM budgets |
| [06-ui.md](06-ui.md) | Angular screens, bundle budget |
| [07-display.md](07-display.md) | The board's LCD, BOOT button and LED |
| [08-mastodon.md](08-mastodon.md) | Optional outbound Mastodon: bot announcements and DMs |
| [sprints.md](sprints.md) | Sprints 0–9 with exit criteria |
| [sprint-10-storage.md](sprint-10-storage.md) | Sprint 10: per-record storage, replacing whole-state commits |
| [roadmap.md](roadmap.md) | After v1: more boards, import/export for migration, OTA, email |
| [open-questions.md](open-questions.md) | Assumptions made while writing this, to confirm |

The framework side of sign-in (a provider-neutral OAuth client and browser
sessions) is specified in the framework itself:
[miniframework/spec/OAUTH.md](../../miniframework/spec/OAUTH.md).

## Owner decisions (October 17, 2026)

1. **Local only, forever.** The board is never reachable from the internet.
   Outbound connections (Mastodon) are allowed, inbound from the internet is not. Even small
   amounts of crawler traffic would take a board down. A public-facing app
   would need a different design.
2. **The risk model is the household:** two parents, two kids, and
   occasionally guests who have the Wi-Fi password. No defense against the
   LAN beyond that.
3. **HTTP is a first-class transport.** Guests don't have the household CA,
   so everything works over plain HTTP. HTTPS and HTTP/2 are there for the
   household devices that trust the CA.
4. **"One-and-a-half sign-on".** Local accounts use an email address as the
   login name plus a numeric PIN of at least 4 digits. They never require HTTPS
   or Mastodon. Mastodon sign-in is an optional second way in.
5. **The first user to sign in becomes admin**, and the admin also gets a PIN.
6. **OAuth goes into the framework**, optional (a Cargo feature) and
   provider-neutral. The framework must not depend on mastomini or NanaCoin:
   mastomini is just one Mastodon instance a household might configure.
   mastodon.social must work too.
7. **Single type of event.** A "session" is a plan with one game, a
   "party" is a plan with several.
8. **Voting is ticking.** Each person ticks every game they're willing to
   play. No ranked choice and no winner calculation. The host decides.
9. **No matching algorithm.** People post time windows and others say "I can
   do that". The negotiation happens between people, not in an algorithm.
10. **One household time zone.** Play is in person and in the house.
11. **Everyone can add games to the catalog.**
12. **Comments are stored on the board** (not a Mastodon thread).
13. **Calendar export and feed are in v1.**
14. **Backup/restore is roadmap**, framed as board migration: what people
    actually want moved is profiles and the game catalog.
15. **Hardware:** the S2 is out. The Waveshare ESP32-C6-LCD-1.47 now running
    minicloud gets repaved for Game Matcher (minicloud moves to a larger
    board; its data was always ephemeral and is not kept). Its screen is the
    onboard display. See
    [01-overview.md § Hardware](01-overview.md#hardware) for the feasibility gate that
    decides whether the C6 can do HTTPS + OAuth, and the S3 fallback.
16. **Open sign-up is on by default.** Admin can switch it off, but in a
    household that's rarely needed.
17. **Monorepo.** Game Matcher lives in `microcontroller/gamematcher/` until it
    outgrows it.
