"""In-memory data store: users, sessions and the message ring.

Everything here dies on reboot, by design. The board has no database, and a
JSON file on flash would buy persistence at the cost of write cycles and a
whole class of corruption bugs. Rebooting wipes the board; the two seeded
accounts always come back.

MEMORY IS THE REAL CONSTRAINT. The S2 has roughly 1MB of heap once MicroPython
has loaded, and fragmentation bites long before exhaustion does. Hence a fixed
ring: the store cannot grow without bound no matter how long the board runs,
so it cannot fail in the small hours because someone got chatty.
"""

import compat
import crypto

# Oldest messages fall off the end once this many are held. Sized so that even
# 50 maximum-length messages is a small fraction of free heap.
MAX_MESSAGES = 50

# Longest message body accepted, in characters. Bounded so one paste cannot
# consume the heap; the ring size guarantee is only as good as this limit.
MAX_BODY = 1000

# How long a login lasts. Short enough that a forgotten phone on the kitchen
# table stops being a key, long enough not to nag during actual use.
SESSION_MS = 60 * 60 * 1000  # one hour

# Visibility values a message can carry.
PUBLIC = "public"          # anyone logged in can read it
RESTRICTED = "restricted"  # only the named recipients


class User:
    """One account.

    Note what is NOT here: the password, in any recoverable form. pw_hash
    verifies a login attempt, and wrapped_key yields the user key only when
    XORed with a mask that requires the password to compute.
    """

    def __init__(self, name, password, display, handle=""):
        self.name = name
        self.display = display

        # Mastodon handle, e.g. "@you@mastodon.social". Only used to address a
        # notification DM; the board never authenticates as anyone, so this is
        # an address and not a credential.
        self.handle = handle

        self.salt = compat.random_bytes(8)
        self.pw_hash = crypto.hash_password(password, self.salt)

        # The key messages are actually encrypted to. Random, not derived from
        # the password - so a user could change their password later without
        # every message addressed to them becoming unreadable.
        user_key = compat.random_bytes(16)

        # Held in RAM so the app can encrypt TO this user without their
        # password. This is the deliberate asymmetry: writing needs only this,
        # reading needs the password.
        self.user_key = user_key

        self.wrapped_key = crypto.wrap(user_key, crypto.derive(password, self.salt))

    def unwrap_key(self, password):
        """Verify a password and recover the user key. Returns None if wrong.

        Verification and unwrapping are done together, from a SINGLE call to
        derive(). Doing them separately would be tidier to read but would run
        the deliberately-slow KDF twice, and on the board that is the
        difference between a login taking about a second and taking three -
        during which it answers no other request.
        """
        mask = crypto.derive(password, self.salt)

        if crypto.verifier(mask) != self.pw_hash:
            return None

        return crypto.unwrap(self.wrapped_key, mask)


class Message:
    """One stored message. The body is always ciphertext, never plaintext."""

    def __init__(self, sender, visibility, recipients, subject, nonce, ciphertext, keys):
        self.id = None  # assigned by the Store
        self.sender = sender
        self.visibility = visibility
        self.recipients = recipients  # list of usernames
        self.subject = subject        # kept clear, so an inbox can list it
        self.nonce = nonce
        self.ciphertext = ciphertext

        # username -> body_key wrapped under that user key.
        self.keys = keys

        self.created = compat.uptime_seconds()


class Store:
    """Holds everything. One instance, created at import by app.py."""

    def __init__(self):
        self.users = {}
        self.messages = []
        self.sessions = {}  # token -> {"user": name, "key": bytes, "seen": ticks}
        self._next_id = 1
        self._seed()

    # ------------------------------------------------------------- seeding

    def _seed(self):
        """Create the two known accounts and a few starting messages.

        These credentials are intentionally in source and intentionally weak.
        The brief calls for the board to always come up with the same two
        known logins, and on a network with no public address that is a
        convenience rather than a hole. Do not reuse this pattern anywhere
        reachable from the internet.
        """
        # Real handles, baked in while this is in beta - both belong to the
        # author, deliberately on DIFFERENT instances so that sending from one
        # account to the other exercises real federated delivery rather than a
        # self-DM that would work even if federation were broken.
        #
        # Note which account sends: the DM comes from whoever signs in during
        # the Mastodon step, not from the app. Connect as mistersql, write to
        # Bob, and flyscifiguy@mstdn.social gets it.
        #
        # These are addresses, not credentials - the board never authenticates
        # as anyone. They are in source only until accounts can be created on
        # the board; at that point handles become per-account data and this
        # seeding goes back to being examples.
        self.add_user("alice", "wonderland", "Alice", "@mistersql@mastodon.social")
        self.add_user("bob", "builder", "Bob", "@flyscifiguy@mstdn.social")

        self.post(
            sender="alice",
            visibility=PUBLIC,
            recipients=[],
            subject="Welcome",
            body=(
                "This board is holding every message in RAM. Reboot it and "
                "everything here vanishes except Alice and Bob.\n\n"
                "Public messages are readable by anyone who can log in. "
                "Restricted ones are encrypted to particular people.\n\n"
                "Nothing here is ever posted publicly. Mastodon is only used "
                "to send you a direct message saying something is waiting - "
                "the link it carries only works from inside the house."
            ),
        )

        self.post(
            sender="alice",
            visibility=RESTRICTED,
            recipients=["bob"],
            subject="Just for Bob",
            body=(
                "Only you and I can read this one. It is encrypted with a "
                "random key, and that key is stored wrapped under each of our "
                "passwords. Log in as alice and you will see it too - log in "
                "as anyone else and it is not even listed."
            ),
        )

        self.post(
            sender="bob",
            visibility=PUBLIC,
            recipients=[],
            subject="Can it build?",
            body="Yes it can.",
        )

    def add_user(self, name, password, display, handle=""):
        user = User(name, password, display, handle)
        self.users[name] = user
        return user

    def by_id(self, message_id, sess):
        """One message by id, if this session may read it. Returns (msg, key).

        Used by the deep link in a notification. Returns (None, None) rather
        than raising when the id is unknown, expired out of the ring, or
        simply not readable by this user - the page treats all three the same
        way, and distinguishing them would leak that a message exists.
        """
        for msg, body_key in self.readable(sess):
            if msg.id == message_id:
                return msg, body_key
        return None, None

    # ------------------------------------------------------------ sessions

    def login(self, name, password):
        """Verify a password and open a session. Returns a token, or None.

        The unwrapped user key is held in the session and nowhere else. That
        is what makes logging out (or expiring) actually revoke the ability to
        read restricted messages, rather than merely hiding them.
        """
        user = self.users.get(name)
        if user is None:
            # Still run the KDF for a missing user, so a wrong username does
            # not answer noticeably faster than a wrong password and thereby
            # reveal which accounts exist.
            crypto.derive(password, b"decoy_salt")
            return None

        key = user.unwrap_key(password)
        if key is None:
            return None

        token = compat.to_hex(compat.random_bytes(16))
        self.sessions[token] = {
            "user": name,
            "key": key,
            "seen": compat.ticks_ms(),
        }
        self._reap_sessions()
        return token

    def logout(self, token):
        self.sessions.pop(token, None)

    def session(self, token):
        """Return the live session for a token, or None if absent or expired."""
        if not token:
            return None

        sess = self.sessions.get(token)
        if sess is None:
            return None

        if compat.ticks_diff(compat.ticks_ms(), sess["seen"]) > SESSION_MS:
            del self.sessions[token]
            return None

        # Sliding expiry: active use keeps a session alive.
        sess["seen"] = compat.ticks_ms()
        return sess

    def _reap_sessions(self):
        """Drop expired sessions. Called on login, which is rare enough."""
        now = compat.ticks_ms()
        dead = [
            t for t, s in self.sessions.items()
            if compat.ticks_diff(now, s["seen"]) > SESSION_MS
        ]
        for t in dead:
            del self.sessions[t]

    # ------------------------------------------------------------ messages

    def post(self, sender, visibility, recipients, subject, body):
        """Encrypt and store a message. Returns the new Message."""
        if len(body) > MAX_BODY:
            body = body[:MAX_BODY]

        body_key = compat.random_bytes(16)
        nonce, ciphertext = crypto.encrypt_body(body, body_key)

        keys = {}
        if visibility == RESTRICTED:
            # The sender is always added, or they could not re-read their own
            # message - a surprise that would read as a bug.
            for name in set(list(recipients) + [sender]):
                user = self.users.get(name)
                if user is not None:
                    keys[name] = crypto.wrap(body_key, user.user_key)
        else:
            # A public message still gets encrypted, but its key is stored
            # unwrapped under "*". Given that public means "any logged-in
            # user", wrapping it per account would add work without adding
            # secrecy - and this keeps a single code path for reading.
            keys["*"] = body_key

        msg = Message(
            sender, visibility, list(recipients), subject, nonce, ciphertext, keys
        )
        msg.id = self._next_id
        self._next_id += 1

        self.messages.append(msg)

        # The ring. Trimming here, at the single point of insertion, is what
        # makes the memory bound real.
        while len(self.messages) > MAX_MESSAGES:
            self.messages.pop(0)

        return msg

    def readable(self, sess):
        """Messages this session may read, newest first.

        A message the user cannot decrypt is not merely hidden from the body -
        it is absent from the list entirely, so the inbox does not advertise
        the existence of secrets.
        """
        out = []
        for msg in reversed(self.messages):
            key = self._body_key_for(msg, sess)
            if key is None:
                continue
            out.append((msg, key))
        return out

    def _body_key_for(self, msg, sess):
        """The body key this session can recover for a message, or None."""
        if "*" in msg.keys:
            return msg.keys["*"]

        wrapped = msg.keys.get(sess["user"])
        if wrapped is None:
            return None

        # The session key came from the password at login. No password, no
        # session key, no plaintext.
        return crypto.unwrap(wrapped, sess["key"])

    def decrypt(self, msg, body_key):
        return crypto.decrypt_body(msg.ciphertext, body_key, msg.nonce)

    def stats(self):
        return {
            "messages": len(self.messages),
            "capacity": MAX_MESSAGES,
            "users": len(self.users),
            "sessions": len(self.sessions),
        }
