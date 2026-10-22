# 08 — The optional Cloudflare relay

**Optional, never required, built late.** The baseline (Pages + a mastodon.social
bot + Brevo) has to stay something a non-professional can install. The relay is
for people who can do "click Deploy to Cloudflare, paste two values".

## What it adds

| Without the relay | With the relay |
|---|---|
| Remote answers need a Mastodon account | Anyone with a personal link can answer from the companion site, **including email-only people** |
| Remote calendar = email attachments | A private, auto-updating calendar URL per person |
| Pending → confirmed takes poll + publish + cache time | The relay can return the board's ack as soon as the board posts it |

## Shape

A single Worker plus a SQLite-backed Durable Object (or D1), on Cloudflare's
free plan. Its whole state is a mailbox and a few blobs. It holds no canonical data.

```
companion browser ── POST /cmd ──► relay ◄── GET /inbox?after=  (board polls, outbound)
                    GET /ack/<id> ◄──     ◄── POST /ack, PUT /cal/<h> (board pushes)
calendar app ─────── GET /cal/<h>.ics ──►
```

- **Commands are authenticated by the personal key.** The browser sends
  `{person h, dsl line, time, nonce, mac = HMAC(personal key, h|time|nonce|line)}`.
  The relay checks only shape and size. **The board verifies the MAC** and
  rejects replays (nonce ring, ±1 day clock window). The relay can't forge
  commands, because it never has personal keys.
- **The board polls** the relay like it polls Mastodon: same inbox worker,
  same dedupe, same service layer. The relay is just another channel.
- **Calendars:** the board pushes each opted-in person's `.ics` to
  `PUT /cal/<random feed id>`. Calendar apps can't decrypt, so **this is
  plaintext at Cloudflare.** It's off by default, per person, with that
  explanation.
- **Limits:** per-person command rate (20/hour), body ≤ 1 KB, mailbox ≤ 1,000
  unread (then 429), entries deleted once the board acks. It stays well inside
  the free tier at family scale.
- **The board's token** (`Authorization: Bearer`, set at deploy, pasted into
  the board) protects `/inbox`, `/ack`, `/cal` writes.

## Install

A template repo (`party-relay`) with a "Deploy to Cloudflare" button. The
deploy asks for one secret (the board token, generated on the board's admin page).
The person pastes the Worker URL back into the board. The board checks it
(`GET /health` with the token) and lists the relay as a channel. The companion
site learns the relay URL from the manifest.

Removing the relay: clear it on the board. The next manifest drops it and the
site falls back to Mastodon DMs.
