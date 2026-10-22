# 05 — Companion site, publication and encryption

The companion site is a static Angular app on GitHub Pages. It shows what the
board last published, decrypts what the viewer's keys open, and sends commands as
DMs through the viewer's own Mastodon account. It has no backend.

## Two repos, one owner

| Repo | Contents | Who writes it |
|---|---|---|
| `party-site` | the built companion app (and the demo, under `/demo/`) | a GitHub Action, on release |
| `party-data` | `data/` only: manifest, public JSON, sealed envelopes, keyrings, public `.ics` | **the board**, through the GitHub API |

Both publish through Pages. GitHub Pages sends `Access-Control-Allow-Origin: *`,
so the site can fetch the data repo's files from another path.

Why two repos: the board's token can then only touch data (a fine-grained token
with Contents read/write on `party-data` alone), site deploys and data pushes never
race, and the data repo can drop its history freely.

### Own the origin

Every project site of one GitHub account shares the origin
`https://<account>.github.io`, so any other Pages project of that account can
read the companion site's `localStorage`, which holds personal keys and the
browser's Mastodon token. **Install both repos under a dedicated free GitHub
organization** (for example `ourparty`), so the origin `https://ourparty.github.io`
belongs to party alone. The install guide makes this step one. A custom domain
also works, but costs money, so it isn't the default.

The site sends a strict CSP (meta tag): scripts from its own origin only,
`connect-src` limited to its own origin, the data path and Mastodon instances.
No analytics, no third-party scripts.

## Data classes

Every field in the domain has a class. The publisher enforces it, and a test
walks every published type to check that no field above `public` shows up in
plaintext.

| Class | Published as | Examples |
|---|---|---|
| `public` | plaintext | public groups' names, about, public events' title/time/area, counts of going, discoverable profiles' display names |
| `sealed` | encrypted for a scope (group or event) | unlisted/private groups and their events, who's going, notes, bring list, rides, shifts |
| `personal` | encrypted for one person, in their keyring | your answers, your invitations, your households' answers, keys for your scopes |
| `direct` | never published. DM, email or the local app only | addresses, diet answers, emails, handles of non-public people, invitation tokens, PINs, tokens |

Addresses default to `direct`: the bot DMs the address to people who are going
and asked for it, and it's in the email to people who are going. An organizer
can make an event's address `sealed` ("the usual place").

## Encryption

- **Cipher:** AES-256-GCM, because browsers have it natively (WebCrypto) and the
  board has it in mbedTLS. Random 96-bit nonce per file, never reused. The
  plaintext is gzipped JSON (the browser inflates with `DecompressionStream`).
- **Envelope** (JSON): `{"v":1,"scope":"g12","kid":3,"nonce":"…","ct":"…"}`,
  with the scope and kid as associated data, so files can't be swapped between
  scopes.
- **Scope keys:** one 256-bit key per sealed group and per invite-only event,
  with a key id `kid`. Rotated when someone leaves or is removed, or on
  demand. The new key goes into remaining members' keyrings, so nobody has to
  resend anything.
- **What rotation doesn't do:** someone who left keeps whatever they already
  downloaded. Rotation protects what's published after they left. The UI says this.

### Personal links and keyrings

Each person has a `personal_key`. Their **personal link** is

```
https://ourparty.github.io/party-site/#/k/<person id>.<personal key, base64url>
```

The key is in the fragment, so it never reaches GitHub's servers. The site
stores it in `localStorage` (with "forget this device") and removes it from the
address bar.

For each person the board publishes `data/k/<h>.json.enc`, where
`h = HMAC(board secret, person id)` truncated to 128 bits (the file name
reveals nothing), encrypted with their personal key. It contains:

- the current `(scope, kid, key)` for every sealed scope they belong to,
- their `personal` data: answers, pending invitations, events answered for them,
- their display name and the bot's handle.

Rotating a person's personal key (`newlink`, or the local app) re-encrypts
their keyring under the new key and sends the new link by DM or email. The old
link now opens nothing.

Personal links are delivered by DM (on redeem, `link`, `newlink`) or email
(organizer-created people), or shown as a QR code in the local app ("open this
on your phone").

## Publication

The board publishes to `party-data` with the Git Data API in one commit:
blobs for changed files, a tree on the previous base tree, **a commit with no
parent**, then a forced ref update. The branch is always one commit deep, so
it never grows and doesn't keep old ciphertext around in its history.
(GitHub may keep unreachable objects for a while. Treat everything ever
published as having been public for the class it had.)

- **When:** at most once per 10 minutes (GitHub Pages builds are soft-limited
  to about 10 an hour), within 10 minutes of a change, plus an admin "publish
  now" (rate-limited). Nothing changed → nothing pushed.
- **What:** only changed files, plus the manifest. A file's content hash is
  remembered so unchanged keyrings aren't rewritten.
- **Freshness:** Pages caches files for about 10 minutes, so remote viewers can
  be about 20 minutes behind. The manifest carries `generation` and `created`.
  The site shows "updated 14 min ago", and older than a day is shown as a
  warning.
- **Failure** never affects canonical data. The previous publication stays up,
  and the admin page shows the error and the token's expiry
  (GitHub returns it in a response header). It warns 14 days before expiry.

### Layout

```
data/manifest.json        { v, generation, created, instance name, bot handle,
                            public: [files], sealed: [{scope, kid, file}] }
data/public/groups.json   public groups
data/public/events.json   upcoming public events (60 days)
data/public/people.json   people who chose discoverable = public
data/s/<scope>.json.enc   one per sealed scope (opaque ids, no titles)
data/k/<h>.json.enc       one keyring per person with a personal key
data/ics/g<id>.ics        public groups only
```

Sizes are design numbers until measured: a family instance publishes tens of
KB. At 2,000 people the keyrings dominate (~1 KB each, 2 MB total, mostly
untouched between publishes).

## Writing from the companion site

The site never writes to GitHub or the board. A button like "Going +1" does this:

1. If the browser has no Mastodon token, sign in (PKCE, scopes `profile write:statuses`).
2. Post a `direct` status to the bot containing exactly the DSL line, shown to
   the person first: `@party@mastodon.social going E12 +1`.
3. Record it in the browser as **pending** with the status id and time.
4. When a later publication shows the answer in the person's keyring with
   `rev`/time ≥ the pending one, it becomes **confirmed**. The bot's DM reply
   arrives sooner, in the person's normal Mastodon app.

The UI always distinguishes "sent, waiting for the board" from "confirmed" (the
draft's requirement), and shows how long sends usually take (bot poll interval +
publish delay + Pages cache).

The site never uses a Mastodon `share?text=` link, because Mastodon would
make that post public by default. Without OAuth, the site shows the DSL line
with "copy, then send it as a private mention to @party". Email-only people see
"answer at the house, or tell the host" (or the relay form, if configured).

The draft's companion site also showed "authorized Mastodon notifications". With
`profile write:statuses` only, it can't, and shouldn't: people read the bot's
DMs in their own Mastodon app. The site shows public posts only: the bot's
public announcements for public groups and a group's hashtag, through Mastodon's
public API.

## Demo site

`/demo/` is the same build with a `DemoDataSource` and a `DemoBot`:

- Bundled fictional instance: 3 households, ~20 people, a club with a weekly
  series, a potluck with a bring list, an outing with rides, a dinner with a
  waitlist, a time poll, sealed and public groups.
- A persona picker instead of personal links ("view as Grandma, email-only").
- Commands are "sent" to `DemoBot`, which runs the **same DSL parser** (compiled
  to TypeScript test vectors, see [09](09-ui.md)) against in-memory data and
  answers after a short delay, so pending → confirmed is visible.
- A reset button. No network calls except the static files.
