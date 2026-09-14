# How The Encryption Works

The goal: *Alice can write a message only Bob can read, without knowing Bob's
password.* The obvious answer is public-key crypto. This project does
something simpler that gets the same property, and the reasoning is worth
writing down because the simpler thing is not obviously sufficient until you
look at it.

## Why not public/private keys

The first instinct is RSA: give everyone a keypair, encrypt to the public key,
decrypt with the private one. Two problems.

**The board would have to do the maths.** MicroPython's `cryptolib` is
AES-only — no RSA, no elliptic curves. Implementing bignum modular
exponentiation in interpreted Python on a 240MHz chip means a single operation
takes seconds. The interpreter is roughly 100x slower than CPython on a
desktop, and this is the kind of arithmetic-heavy loop where that hurts most.

**It would not buy anything here.** The private keys have to live somewhere. If
the board holds them so it can decrypt, then anyone who reaches the board has
them, and the asymmetry has bought nothing. If the board does *not* hold
them, the decryption has to happen in the browser — a much bigger app, and the
keys still have to be derived from a password.

So: what property is actually wanted, and what is the cheapest thing that
provides it?

## The property that matters

Writing and reading need different things:

- **Writing** to Bob should need something the board can hold all the time.
- **Reading** as Bob should need something only Bob can supply.

That asymmetry is the whole of what public-key crypto was being asked for
here. It can be had with symmetric primitives and a little care about where
things are stored.

## The scheme

Each user gets a random 16-byte **user key** when the store seeds them. This is
what messages are actually encrypted to. It is never stored in the clear:

```text
mask        = derive(password, salt)        slow hash, 16 bytes
wrapped_key = user_key XOR mask             this is what is stored
```

Signing in re-derives the mask from the password typed into the form, XORs it
back out, and puts the recovered user key **in the session** — not in the user
record, not on disk, nowhere else. Signing out drops it.

Each message gets its own random key:

```text
body_key   = 16 random bytes, fresh for this message
ciphertext = AES-CTR(body_key, text)

for each recipient:
    keys[them] = body_key XOR user_key[them]
```

And there is the asymmetry. Wrapping the body key for Bob needs **Bob's user
key**, which the board holds in RAM — so Alice can write to him freely.
Unwrapping it needs **the session key**, which only exists after Bob has typed
his password.

## Why XOR is not a cop-out

XOR-as-encryption is usually a red flag, because reusing a key stream across
messages breaks it completely. It is sound here for a specific reason: each
mask is used to wrap **exactly one** 16-byte secret, and never again. That is
a one-time pad over a single block, which is not merely adequate but
information-theoretically perfect for the thing it is doing.

The message *bodies* are a different matter — they are variable length and
could be long, so they get real AES in counter mode, with a fresh random nonce
per message. The rule that CTR must never reuse a `(key, nonce)` pair is
satisfied twice over: both the key and the nonce are fresh per message.

## The slow hash

`derive()` iterates SHA-256 twenty thousand times. The only purpose is to make
guessing expensive: each attempt costs an attacker the full loop.

Twenty thousand is a compromise, and the board is the reason. The server
answers one request at a time, so a slow sign-in does not merely make one
person wait — it stalls everyone. Measured on a desktop, 50,000 rounds took
27ms; the board is perhaps sixty times slower, so that would be about 1.6
seconds per call.

And a sign-in needs the derived mask twice: once to check the password is
right, once to unwrap the key. Done naively that is over three seconds of dead
board on every sign-in.

So `User.unwrap_key` derives **once** and uses the result for both jobs:

```python
mask = crypto.derive(password, self.salt)

if crypto.verifier(mask) != self.pw_hash:
    return None

return crypto.unwrap(self.wrapped_key, mask)
```

The stored verifier is `sha256(b"verify" + mask)` rather than the mask itself,
so the thing kept on the board reveals nothing about the thing that unwraps
keys — even though both come from one derivation.

That brings a sign-in to roughly 0.6 seconds on the board. The page says
"Unlocking…" during it, because a tap that does nothing for half a second
otherwise reads as a tap that did not register.

## What this protects, and what it does not

**Protected:**

- A user reading messages not addressed to them. They are absent from the
  list, not merely hidden — the page never learns that they exist.
- Someone dumping the board's memory. Restricted message bodies are
  ciphertext, and the keys that open them are wrapped.
- Someone reading a session after sign-out. The key is gone.

**Not protected:**

- **Anyone watching the network.** This is plain HTTP. Passwords cross the LAN
  in the clear, and so do the decrypted messages on their way to the browser.

That last one is not fixable on this hardware. TLS needs a certificate, a
certificate needs a public DNS name, and the board has neither. It is worth
stating plainly rather than letting the word "encrypted" imply more than it
should: **treat every password here as public knowledge on that network.**

The seeded credentials are in source and deliberately weak, because the brief
asked for two known logins that survive a reboot. On a box with no public
address that is a convenience. Do not carry the pattern anywhere else.

## Verifying it

`test_app.py` checks the claims rather than the plumbing — that a third user
cannot see a restricted message, that the stored bytes are not plaintext, that
the wrong key does not decrypt, and that signing out revokes:

```powershell
python test_app.py
```

The AES implementation is checked against the known-answer vector from
FIPS-197 appendix C.1, so it is confirmed to be real AES rather than merely
self-consistent — a round-trip test alone would pass happily on a broken
cipher.
