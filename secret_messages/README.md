# secret_messages

A pastebin for a house. Sign in, read the messages written for you, write one
back — and have Mastodon tell the recipient it is waiting, since the site
itself cannot be loaded from outside the network.

Third project in this repo, after `hello_wifi` (C) and `hello_wifi_py`
(MicroPython). Same deploy story as the latter: the firmware is flashed once,
after that only `.py` files move.

## The idea

Messages are either **public** (anyone signed in can read them) or
**restricted** to particular people. A restricted message is encrypted, and
the key to it is stored separately for each recipient, wrapped under something
only their password can produce. So it is not merely hidden from other
users — it is not readable at all without the password, even to someone
poking at the board's memory over the REPL.

Everything lives in RAM. Reboot the board and the messages are gone; `alice`
and `bob` always come back.

## Try it without a board

```powershell
cd C:\github\microcontroller\secret_messages
python dev_server.py
```

Open <http://localhost:8000> and sign in as `alice` / `wonderland` or
`bob` / `builder`. Alice has already left Bob a restricted message; sign in as
each in turn to see it appear and disappear.

```powershell
python test_app.py      # 117 checks, no pytest, no dependencies
node test_notify.mjs    # 24 more: the OAuth flow, executed against a mock
```

## Put it on the board

`config.py` is gitignored; copy the template if this is a fresh checkout.

```powershell
copy config_example.py config.py   # then add your WiFi details
.\deploy.ps1 -Port COM5
```

Then <http://secrets.local> — a different mDNS name from the `hello_wifi`
projects, so both boards can be on the network at once.

If the board still has C firmware on it, flash MicroPython first with
`.\flash_micropython.ps1`; `deploy.ps1` says so and prints the steps. The
scripts, the firmware and the COM-port advice are all shared with
`hello_wifi_py` — see [the MicroPython docs](../docs/micropython/index.md).

## Files

```text
app.py           routes + JSON API      <- start here
ui.py            HTML, CSS, JavaScript  <- the look
notify_js.py     the Mastodon OAuth client (served as /notify.js)
store.py         users, sessions, the message ring
crypto.py        the scheme: key wrapping, message encryption
compat.py        CPython/MicroPython differences, board diagnostics
http_parse.py    reading a request (method, body, session header, host)
aes_soft.py      pure-Python AES — dev server only, never copied to the board
test_app.py      run with: python test_app.py
test_notify.mjs  run with: node test_notify.mjs
main.py          device entry point
dev_server.py    local preview
```

`app.py` runs unchanged in both places, exactly as in `hello_wifi_py`. The
difference is that a route now takes `(method, path, body, token, host)` —
this app needs POSTs, a session header, and the Host header to build working
notification links — and that `store.py` keeps state between requests.

Note `dev_server.py` does **not** reload modules by default, unlike the one in
`hello_wifi_py`. That app was stateless so reloading was free; here a reload
would wipe every message and sign you out on each click. Pass `--reload` while
working on the page, and leave it off while using the app.

## How the encryption works

Each user gets a random 16-byte **user key** when the store seeds. It is never
stored in the clear:

```text
stored:  wrapped_key = user_key XOR derive(password, salt)
```

Signing in re-derives the mask and unwraps the key into the session, and
nowhere else. Each message then gets its own random key:

```text
body_key    = 16 random bytes, fresh per message
ciphertext  = AES-CTR(body_key, text)
per person  = body_key XOR user_key[them]
```

This is the part worth understanding: **writing to Alice needs only her user
key, which the board holds; reading needs her password, which it does not.**
That asymmetry is what makes "send a message only she can read" work without
the sender knowing her password — the same property public-key crypto would
give you, without asking a 240MHz microcontroller to do bignum arithmetic in
an interpreter.

Signing out drops the session key, so it genuinely revokes the ability to read
rather than merely hiding the messages.

### What this does not protect against

**Anyone listening to the network.** It is plain HTTP — passwords cross the
LAN in the clear. A board with no public DNS name cannot get a TLS
certificate, so this is a limit of where it runs, not an oversight. Treat
every password here as public knowledge on that network.

The seeded credentials are in source and deliberately weak, because the brief
asked for two known logins that survive a reboot. Do not reuse this pattern
anywhere reachable from the internet.

## Mastodon, as a doorbell

There is no mail server here and no push notifications, so telling someone a
message is waiting uses Mastodon — as a **direct message carrying a link**,
never a public post:

```text
@flyscifiguy@mstdn.social a message is waiting for you on the fridge:
"Bins" http://secrets.local/?m=7

(only opens from the home wifi)
```

The message itself never leaves the board. The DM crosses the whole internet
to deliver a link that only resolves from inside the house, and the reader
still signs in and still has to be a recipient — the link is a pointer, not a
key.

**There is nothing to configure.** No token, no `config.py` entry, no key on
the board — the board never talks to Mastodon at all. The browser does:

1. `POST /api/v1/apps` registers a throwaway app on the spot
2. redirect to the instance with a PKCE challenge, user approves
3. exchange the code for a token, proving possession of the verifier
4. send the DM with `visibility=direct`

Instances send `Access-Control-Allow-Origin: *` on both endpoints, which is
what makes this possible from a page served at `secrets.local`. The token asks
only for `write:statuses` and lives in `sessionStorage` — that tab, and
nowhere else.

The PKCE logic is a plain-JS port of
[mawkingbird](https://github.com/matthewdeanmartin/mawkingbird)'s
`ui/src/app/pkce.ts`. `node test_notify.mjs` executes the whole flow against a
mock instance that verifies the challenge the way a real one does.

Each user has a `handle` used only as an address. While this is in beta the
two seeded accounts carry real handles, deliberately on **different
instances**, so a notification between them exercises federated delivery:

```python
self.add_user("alice", "wonderland", "Alice", "@mistersql@mastodon.social")
self.add_user("bob", "builder", "Bob", "@flyscifiguy@mstdn.social")
```

The DM comes from whoever signs in during the Mastodon step, so connecting as
`mistersql` and writing to Bob delivers to `@flyscifiguy@mstdn.social`.
Override one without redeploying:

```python
>>> app.STORE.users['bob'].handle = '@someone@their.instance'
```

The message is stored **first** and notifying happens after, so a dead
instance or a revoked token can never cost you what you wrote.

> An earlier version had the board holding an API token and tooting the
> message publicly. That was wrong twice: it published the plaintext it had
> just bothered to encrypt, and it parked a long-lived credential on the least
> defensible machine in the house. The docs keep the post-mortem.

## The board tab

A third tab shows what the hardware is doing — RSSI with a quality bar, free
heap, uptime, IP, MAC and SSID. The same figures `hello_wifi` put on its
status page, from `compat.diagnostics()`.

Fetched when you open the tab, not polled: the board answers one request at a
time, and it should spend that time on messages rather than on questions about
itself. For monitoring over time, `watch.ps1` still works unchanged against
the no-auth `/health` route.

## Publishing the front end elsewhere

The eventual plan is to serve the HTML, CSS and JS from a public host and
leave only the API on the board, so the pretty part is not shipped over USB.
The code is already arranged for it — static files are separate routes, the
API is JSON-only, and CORS headers go out on every response.

One thing to know before trying it: **the public page must be served over
plain `http://`, not `https://`.** A secure page may not call a plain-HTTP
API, and no CORS header overrides that — the board has no certificate, so it
cannot be the other half of an HTTPS page. Browsers are getting stricter about
plain HTTP, so serving everything from the board stays the more durable
option.

## Limits

| | |
|---|---|
| Messages held | 50, oldest dropped |
| Message length | 1000 characters |
| Session | 1 hour, sliding |
| Sign-in cost | ~0.6s on the board — the key derivation is slow on purpose |
| Concurrency | one request at a time |

That last two interact: while someone is signing in, the board is answering
nobody else. It is a house, not a service, and 20,000 rounds of hashing is
the compromise — see the note in `crypto.py` if you want to move it.
