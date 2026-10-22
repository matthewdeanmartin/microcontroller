# What's next, after initial testing

Written October 21, 2026, after Sprints 0–8 were built and Sprints 3–8 were
deployed to the board (`ac:a7:04:2c:29:9c`, see [../DEPLOY.md](../DEPLOY.md)).
This lists what's left besides the owner's own testing.

## Remaining sprints

Details and exit criteria are in [sprints.md](sprints.md).

| Sprint | What it adds |
|---|---|
| **9: Private (encrypted) data** | Encrypted files on GitHub Pages: a key per private group or event, and a key file per person that their personal link opens. Keys change when someone is removed. The companion site decrypts in the browser, and the demo gets a "view as" picker. |
| **10: Answering from the companion site** | Mastodon sign-in in the browser (send-only access, `profile write:statuses`), the site writing the bot command for you, and "sent" vs "confirmed". Also accepting an invitation from outside the house, and the site's command builder tested against [dsl-vectors.json](dsl-vectors.json). |
| **11: AI help and organizer commands** | Optional Gemma through OpenRouter to turn free text into commands, with daily limits and an opt-out per person. Organizers can create, cancel, invite and remind by DM, each needing a `confirm`. |
| **12: Optional Cloudflare relay** | Email-only people can answer from their personal link without Mastodon, plus private calendar feeds that work outside the house. It ships as a template repo with a "Deploy to Cloudflare" button. |
| **13: Hardening and v0.1** | Load test on the board, 20 power cuts during writes, a flash-wear estimate, an accessibility pass, a complete deployment runbook, and the 12-step acceptance run in [sprints.md](sprints.md#v01-acceptance). |

## Left over from earlier sprints

- **The Sprint 0 measurements were never run**: 20,000 stored records, encryption
  speed, outbound HTTPS while serving ([measurements/s3-gate.md](measurements/s3-gate.md)).
  Sprint 9's encryption and the new outbound worker both depend on those numbers.
  The largest free internal memory block is already 31 KB, under the 32 KB line
  Sprint 0 set. After the Sprints 3–8 deploy, internal free RAM went from 78.9 KB to
  67.6 KB (the outbound worker's stack).
- **HTTPS:** the board serves plain HTTP only. `make certs` and an HTTPS build
  haven't been done.
- **Small gaps** (recorded in [sprints.md](sprints.md)):
  - no QR code for the personal link
  - `stop` turns off all bot messages rather than per group
  - `undo` is forgotten on reboot
  - the companion site is copied into its repo by hand, not built by its own Action
  - no warning before the GitHub token expires, because the framework's HTTP client
    doesn't return response headers (a framework change)
- **Game Matcher:** the storage fix (its Sprint 10) isn't flashed to the kitchen
  board, and its outbox still rewrites the whole queue on every change.

## Owner steps for Sprints 6–8

These need real accounts, so they weren't done:

- A real Mastodon sign-in against mastodon.social, from the board.
- Opening an emailed calendar invitation in a real calendar app.
- One live run with a test bot account.
- Creating the GitHub organization and repos, then a live publication
  ([../INSTALL.md](../INSTALL.md)).

## Decisions only the owner can make

From [open-questions.md](open-questions.md). These five block later sprints:

1. The GitHub organization's name (`ourparty` is a placeholder).
2. The bot's handle, and whether mastomini should also host a bot for people in the house.
3. Whether event addresses should be published, encrypted, to the family. Today
   they never are; Sprint 9 decides this.
4. Whether the AI help is on for everyone once set up, or opt-in per person.
5. Whether the son's and mother-in-law's houses are in the same time zone.

Smaller ones:

- Whether events need comments.
- Whether co-organizers who aren't hosts can answer for people.
- What scenario the demo should show.

## Not started on purpose

Everything in [roadmap.md](roadmap.md), such as letting anyone join, federation
between boards, backup and restore, OTA updates, Bluesky and online events.

## Suggested order

1. The Sprint 0 measurements on the board (`make firmware GATE=1`, then the
   steps in [../DEPLOY.md](../DEPLOY.md#sprint-0-gate)), since Sprint 9 builds on them.
2. Sprint 9, then Sprint 10.
