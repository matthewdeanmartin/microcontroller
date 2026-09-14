"""The encryption scheme, identical on the board and on your PC.

WHAT THIS PROTECTS AGAINST, AND WHAT IT DOES NOT

It protects against: someone who logs in as Bob reading Alice's restricted
messages, and someone who dumps the message store (over the REPL, say) reading
anything restricted at all.

It does NOT protect against: a network eavesdropper. Traffic is plain HTTP -
passwords cross the LAN in the clear. That is a deliberate limit of a board
with no public DNS name and therefore no TLS certificate. This is a home
network toy; treat every password here as public knowledge on that LAN.

THE SCHEME

Each user has a random 16-byte "user key" generated at seed time. That key is
what messages are actually encrypted to. It is never stored in the clear:

    stored:  wrapped_key = user_key XOR derive(password, salt)

So the store holds the user key only in a form that requires the password to
undo. Logging in re-derives the same mask and unwraps the key into the session
- nowhere else. The password itself is stored only as a salted hash, and never
in a form that can be reversed.

A restricted message is then:

    body_key    = 16 random bytes, fresh per message
    ciphertext  = AES-CTR(body_key, plaintext)
    per recipient: body_key XOR user_key[recipient]

This is why the sender does not need the recipient's password. Writing to
Alice needs only Alice's *user key*, which the app can hold in RAM - while
reading needs the password, which it cannot. That asymmetry is the whole
point, and it is what you asked for.

The XOR wrapping is a one-time-pad over a single block: sound here precisely
because each mask is used for exactly one 16-byte secret and is never reused.
"""

import compat

# Iterating the hash slows password guessing. This is the single most
# expensive thing the board does, and it blocks: the server answers one
# request at a time, so a slow login stalls everyone.
#
# Measured ~27ms for 50k rounds under CPython on a desktop. The S2 runs this
# loop perhaps 60x slower, so 50k would be ~1.6s per call - and a login needs
# two calls (verify, then unwrap), which is over three seconds of dead board.
#
# 20k keeps a login near a second while still costing an attacker the full
# loop per guess. On a LAN-only box with no public address, that is the right
# trade. Raise it if you ever expose this beyond the house.
KDF_ROUNDS = 20_000


def derive(password, salt, rounds=KDF_ROUNDS):
    """Turn a password into 16 key bytes. Deliberately slow.

    A plain iterated SHA-256 rather than PBKDF2-HMAC: MicroPython's hashlib
    has no HMAC and no pbkdf2, and the construction below gives the property
    that matters here - each guess costs the attacker the full loop.
    """
    if isinstance(password, str):
        password = password.encode("utf-8")
    if isinstance(salt, str):
        salt = salt.encode("utf-8")

    h = compat.sha256(salt + password)
    for _ in range(rounds):
        # Feeding the salt back in each round prevents a precomputed chain
        # from being shared between two users who chose the same password.
        h = compat.sha256(h + salt)
    return h[:16]


def verifier(mask):
    """Turn an already-derived mask into the value stored for login checks.

    Split from hash_password so a login can derive once and use the result for
    both jobs - see User.unwrap_key. The KDF is slow on purpose, so running it
    twice per login is a cost worth avoiding.

    Note the b"verify" prefix: it means this value and the mask itself are
    outputs of different inputs, so the stored hash reveals nothing about the
    mask that wraps the user key.
    """
    return compat.to_hex(compat.sha256(b"verify" + mask))


def hash_password(password, salt):
    """A verifier for login, from a password. Used when creating a user."""
    return verifier(derive(password, salt))


def wrap(secret, mask):
    """XOR a 16-byte secret with a 16-byte mask."""
    if len(secret) != len(mask):
        raise ValueError("wrap needs equal lengths")
    return bytes(secret[i] ^ mask[i] for i in range(len(secret)))


# unwrap is the same operation - XOR is its own inverse. Named separately so
# calling code reads as what it intends.
unwrap = wrap


def encrypt_body(plaintext, body_key):
    """AES-CTR a message body. Returns (nonce, ciphertext)."""
    nonce = compat.random_bytes(8)
    return nonce, compat.aes_ctr(body_key, nonce, plaintext.encode("utf-8"))


def decrypt_body(ciphertext, body_key, nonce):
    """Reverse encrypt_body. CTR is symmetric, so this is the same call."""
    plain = compat.aes_ctr(body_key, nonce, ciphertext)
    try:
        return plain.decode("utf-8")
    except UnicodeError:
        # Wrong key. Returning a marker rather than raising keeps one corrupt
        # message from blanking the whole inbox page.
        return "[could not decrypt]"
