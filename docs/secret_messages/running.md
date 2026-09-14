# Running It

The workflow is the one from
[the MicroPython project](../micropython/workflow.md) — the firmware is
flashed once, and after that only `.py` files move. This page covers what is
different here.

## Preview, no board

```powershell
cd C:\github\microcontroller\secret_messages
python dev_server.py
```

Open <http://localhost:8000>. Sign in as `alice` / `wonderland` or
`bob` / `builder`.

The interesting thing to try first: sign in as Alice and read the message she
restricted to Bob, then sign out, sign in as Bob, and see the same message
from the other side. Then add a third user in the REPL and watch it vanish
from their list entirely.

### One difference from `hello_wifi_py`

That project's dev server reloaded `app.py` on every request, so edits landed
on the next refresh. It could do that because the app had no state.

This one **does not reload by default**. The users, sessions and messages live
in module state, so a reload on every request would sign you out and wipe the
messages on every click.

```powershell
python dev_server.py --reload   # picks up edits; also resets everything
python dev_server.py            # state persists; restart to pick up edits
```

Use `--reload` while working on the page's appearance, and leave it off while
actually using the app.

## The tests

```powershell
python test_app.py       # 117 checks
node test_notify.mjs     # 24 more, for the OAuth flow
```

No pytest and no dependencies — deliberately, to match the rest of the repo:
you should be able to check your work before spending a flash cycle on it.

The second command is separate because the Mastodon client runs in the
browser, not in Python. It drives the real `notify.js` against a mock instance
that enforces PKCE the way a live one does — see [Mastodon](mastodon.md). It
needs Node 18+; if Node is not installed, the Python tests still cover
everything the board itself does.

They test the claims rather than the plumbing. That a third user cannot see a
restricted message, that the stored bytes are not plaintext, that the ring
buffer holds at fifty, that signing out revokes. Those would be embarrassing
to get wrong and none of them is obvious from reading the code.

## On the board

`config.py` is gitignored. On a fresh checkout:

```powershell
copy config_example.py config.py
```

…then put your WiFi details in it. Then:

```powershell
.\deploy.ps1 -Port COM5
```

It copies nine files and resets the board. It does **not** copy `aes_soft.py` —
that is the pure-Python AES used only by the dev server, because CPython has no
AES in its standard library and MicroPython does. Sending 6KB of unused code to
the board that can least afford it would be careless.

If the board still has the C firmware, `deploy.ps1` notices and prints the
steps — the short version is hold **BOOT**, tap **RESET**, release **BOOT**,
then `.\flash_micropython.ps1 -Port COM4`. See
[Setup](../micropython/setup.md), and
[the COM port shuffle](../basic_setup/the_board.md#the-com-port-moves) for why
the port number changes underneath you.

## Finding it

```text
http://secrets.local
```

A different mDNS hostname from the `hello_wifi` projects (`esp32.local`), so
both boards can be on the network at once. The name is set *before*
`connect()` in `main.py`, which matters —
[Finding It On Your Network](../micropython/finding_it.md) explains why
setting it afterwards silently does nothing.

## Watching it

`watch.ps1` is copied from `hello_wifi_py` and works unchanged, because this
app keeps a `/health` route that needs no sign-in and answers in plain text:

```powershell
.\watch.ps1 -Target secrets.local
```

```text
ok messages=3
```

Handy for leaving running while you move the board around — see
[Signal and Placement](../micropython/signal_and_placement.md).

## The REPL

The board is still a live Python prompt, which is the best part of this
whole approach:

```powershell
python -m mpremote connect COM5 repl
```

The store is a module-level object, so it can be poked at while the site is
serving:

```python
>>> import app
>>> app.STORE.stats()
{'messages': 3, 'capacity': 50, 'users': 2, 'sessions': 1}
>>> app.STORE.add_user('carol', 'hunter2', 'Carol', '@carol@mastodon.social')
>>> [m.subject for m in app.STORE.messages]
['Welcome', 'Just for Bob', 'Can it build?']
```

Adding a user this way is the intended route — there is no sign-up page, on
purpose. This is a house, and the guest list is short.

The fourth argument is the Mastodon handle used to notify them. The two
seeded accounts already carry real handles on two different instances — see
[Mastodon](mastodon.md) — so a notification sent between them actually
arrives. Override one without redeploying:

```python
>>> app.STORE.users['bob'].handle = '@someone@their.instance'
```

Note what you *cannot* do from here, which is the point of
[the encryption](encryption.md):

```python
>>> m = app.STORE.messages[1]
>>> m.ciphertext[:16]
b'\x8a\x1f\xd3...'          # not the message
>>> m.keys
{'alice': b'...', 'bob': b'...'}   # wrapped, not usable
```

The restricted message is right there in memory and still unreadable, because
the keys that open it are wrapped under passwords the board never stored.

`Ctrl+]` exits.

## Limits

| | |
|---|---|
| Messages held | 50, oldest dropped |
| Message length | 1000 characters |
| Session | 1 hour, sliding |
| Sign-in | ~0.6s on the board |
| Concurrency | one request at a time |

The last two interact: while someone is signing in, the board answers nobody
else. Twenty thousand rounds of hashing is the compromise between that and
making password guessing expensive. If you want to move it, the constant and
the reasoning are both at the top of `crypto.py`.

The **Board** tab shows all of this live — signal, free memory, uptime,
addresses. See [The Board Tab](diagnostics.md).
