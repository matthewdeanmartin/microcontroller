# Secret Messages

A pastebin for a house, running on the board. Sign in, read what was written
for you, write something back — and have Mastodon tell the recipient it is
waiting, since the site itself cannot be loaded from outside the network.

This is the third project in the repo, after the C status page and the
MicroPython one. It reuses that second project's whole workflow — same
firmware, same `deploy.ps1`, same three-second edit cycle — and spends the
saved effort on doing something that is actually a little interesting.

## What it does

Messages are either **public** (any signed-in user can read them) or
**restricted** to particular people. A restricted message is encrypted, and
the key is stored separately for each recipient, wrapped under something only
their password can produce.

The result is stronger than it sounds. A message restricted to Bob is not
merely hidden from Alice's page — it is not in her copy of the list at all,
and the stored bytes are unreadable without a password the board does not
keep. Dumping the board's memory over the REPL gets you ciphertext.

Everything lives in RAM. Reboot and the messages are gone; `alice` and `bob`
always come back.

## The notification trick

There is no mail server here, and no push notifications. Telling somebody a
message is waiting uses Mastodon instead — as a **direct message carrying a
link**, never as a public post:

```text
@flyscifiguy@mstdn.social a message is waiting for you on the fridge:
"Bins" http://secrets.local/?m=7
```

The message itself never leaves the board. The DM travels the whole internet
to deliver a link that only resolves from the sofa, and even then the reader
has to sign in and be a recipient.

No API key is involved. The board never talks to Mastodon at all — the
browser does, via OAuth with PKCE, and the token lives in that tab and nowhere
else. [The details](mastodon.md), including the first design and why it was
wrong.

## Contents

1. [How The Encryption Works](encryption.md) — key wrapping, and why this is
   not public-key crypto
2. [Running It](running.md) — preview, test, deploy
3. [The Board Tab](diagnostics.md) — signal, memory, uptime
4. [Mastodon](mastodon.md) — notifications, without a key on the board
5. [Publishing The Front End](publishing.md) — the plan to serve the pretty
   part from elsewhere, and the one thing that makes it hard

## The short version

No board needed:

```powershell
cd C:\github\microcontroller\secret_messages
python dev_server.py
```

Open <http://localhost:8000>, sign in as `alice` / `wonderland`. Then sign in
as `bob` / `builder` and notice which messages change.

On the board:

```powershell
.\deploy.ps1 -Port COM5
```

Then <http://secrets.local>. A different mDNS name from the `hello_wifi`
projects, so both boards can be on the network at once — see
[Finding It On Your Network](../micropython/finding_it.md) for how that works.

## What it is not

It is plain HTTP. Passwords cross the network in the clear, and a board with
no public DNS name cannot get a TLS certificate to fix that. The encryption
protects messages *at rest on the board* and *from other users* — not from
somebody watching the wire.

That is a reasonable place to land for something with no public IP address,
but it is worth being precise about, which is why
[the encryption page](encryption.md) spells out both halves.
