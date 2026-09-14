# Mastodon, as a doorbell

Mastodon is the notification system. Not a publishing channel — a way to tell
somebody that a message is waiting for them, without running a mail server.

This is the second design. The first one was wrong in a way worth recording,
because the mistake is an easy one to make.

## The version that was wrong

The board held an API token and posted the message publicly when you ticked a
box.

Two problems, and the first is the interesting one:

**It published the thing it had just encrypted.** The app goes to some trouble
to make a restricted message unreadable by anyone but its recipients — random
body key, wrapped per person, unreadable even from the REPL — and then offered
to toot the plaintext to the entire internet. If a message is worth encrypting
it is not worth publishing. The two features were arguing with each other.

**It needed a long-lived token on the board.** Written into `config.py`, on a
device with no disk encryption, no TLS, and a REPL that anyone on the network
can reach. A token that can post as you, sitting on the least defensible
machine in the house.

## What it does now

A message is posted, stays on the board, and the **link** goes out as a
**direct message** to each recipient:

```text
@flyscifiguy@mstdn.social a message is waiting for you on the fridge:
"Bins" http://secrets.local/?m=7

(only opens from the home wifi)
```

Following the link opens the app at that message. The reader still signs in,
and still has to be a recipient — the link is a pointer, not a key. Somebody
who intercepts the DM gets a URL that does not resolve unless they are in the
house, and would not open the message even then.

The joke, which is also the security model: the notification travels the whole
internet to arrive at a link that only works from the sofa.

## No keys, anywhere

The board never talks to Mastodon and never holds a token. All of it happens
in the browser, which is possible because instances are CORS-friendly —
verified against the real thing:

```console
$ curl -s -i -X OPTIONS https://mastodon.social/api/v1/apps \
    -H "Origin: http://secrets.local" \
    -H "Access-Control-Request-Method: POST"
HTTP/1.1 200 OK
access-control-allow-origin: *
access-control-allow-methods: POST, PUT, DELETE, GET, PATCH, OPTIONS
```

`Access-Control-Allow-Origin: *` on both `/api/v1/apps` and `/oauth/token`, so
a page served from `secrets.local` can run the entire OAuth flow itself.

The sequence, in `notify_js.py`:

1. **Register an app.** `POST /api/v1/apps` with a redirect URI of this page.
   No pre-shared credentials — the app is created on the spot and thrown away
   after. This is why there is nothing to put in `config.py`.
2. **Redirect to the instance** with a PKCE challenge, where the user signs in
   and approves. The instance handles this in their browser, which is why it
   works even though `secrets.local` means nothing to the outside world.
3. **Exchange the code** for a token, proving possession of the verifier.
4. **Send the DM**, `visibility=direct`.

The token lands in `sessionStorage` — this tab only, gone when it closes. The
scope requested is `write:statuses` and nothing else, so the token cannot read
anything, follow anyone, or change the account.

## What PKCE is for

A browser cannot keep a secret. Anything shipped to it is readable, so an
authorization code alone is not enough: anyone who intercepted the redirect
could redeem it for a token.

PKCE (RFC 7636) fixes that with a value that never leaves the browser:

```text
verifier  = 64 random bytes, base64url        stays here
challenge = base64url(sha256(verifier))       travels to the instance
```

The instance remembers the challenge and hands back a code. Redeeming the code
requires presenting the verifier, and only the browser that started the flow
has it. Intercepting the code gains nothing.

The separate `state` value does a different job: it binds the callback to the
flow *this* browser started, so an attacker cannot hand you a code minted for
their account and quietly sign you into it.

The implementation is a plain-JavaScript port of
[mawkingbird](https://github.com/matthewdeanmartin/mawkingbird)'s
`ui/src/app/pkce.ts` — same reasoning, no framework. Both use
`crypto.getRandomValues`; neither ever falls back to `Math.random`.

## Testing it

The Python tests can only check that `notify.js` *mentions* the right things.
That is not much of a guarantee, so the OAuth flow has its own test that
actually runs it:

```powershell
node test_notify.mjs
```

It stands up a mock instance that verifies the challenge the way a real one
does, then drives the real `notify.js` through the whole flow in a `vm`
context with a minimal fake browser. It checks, by execution rather than by
pattern:

- the challenge really is `base64url(sha256(verifier))`
- the verifier never appears in the outbound URL
- the token exchange succeeds against a server enforcing PKCE
- the DM is `direct`, mentions the recipient, and carries the link and
  subject — **but not the message body**
- a mismatched `state` is rejected
- an unsolicited callback, with no flow in this browser, is refused

Needs Node 18+ for built-in `fetch` and `webcrypto`. Nothing is installed and
nothing reaches the network.

!!! note "One subtlety in that test"

    It asks Python for the JavaScript rather than reading `notify_js.py` as
    text. `NOTIFY_JS` is a normal (non-raw) Python string, so `"\\/"` in the
    source is `"\/"` by the time it is served. Reading the file directly hands
    JavaScript the unescaped form, whose regex literals do not parse — which
    is exactly the bug the first version of the test hit.

## Handles

Each user has one, used only as an address. While this is in beta they are
baked into `store.py` as real accounts:

```python
self.add_user("alice", "wonderland", "Alice", "@mistersql@mastodon.social")
self.add_user("bob", "builder", "Bob", "@flyscifiguy@mstdn.social")
```

Both belong to the author, and deliberately sit on **different instances** —
so sending from one to the other exercises real federated delivery rather than
a self-DM that would work even if federation were broken. `test_app.py` checks
they stay on separate hosts, because pointing them at the same server would
quietly lose that coverage.

These are addresses, not credentials: the board never authenticates as anyone.
They live in source only until accounts can be created on the board, at which
point handles become per-account data and the seeding goes back to being
examples.

Changing one without a redeploy:

```python
>>> import app
>>> app.STORE.users["bob"].handle = "@someone@their.instance"
```

A user with no handle is simply skipped, and the page says who could not be
told rather than failing the whole send.

### Which account sends

The DM comes from whoever signs in during the Mastodon step — not from the
app, which has no account of its own. Connect as `mistersql`, write a message
to Bob, and `@flyscifiguy@mstdn.social` receives it from `@mistersql`.

The instance box prefills from the signed-in user's own handle, since the two
seeded accounts are on different servers and a fixed default would be wrong
half the time.

## When it fails

The message is stored **first**, always. Notifying is a separate, best-effort
step that happens after the board has already confirmed the write, so a dead
instance, a revoked token or a flaky uplink can never cost you what you wrote.

The page distinguishes the cases:

- `Sent.` — no notification requested
- `Sent. Not signed in to Mastodon, so nobody was told.`
- `Sent, and told 2 people on Mastodon.`
- `Sent. Could not notify: bob (no handle)`

A `401` from the instance clears the stored token, so the next attempt offers
to reconnect rather than failing the same way forever.
