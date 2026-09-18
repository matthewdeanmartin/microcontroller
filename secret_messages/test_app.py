"""Tests that run under plain CPython - no board, no pytest, no dependencies.

    python test_app.py

Deliberately dependency-free and runnable with one command, to match the rest
of this repo: the point of the dev-server layout is that you can check your
work before spending a flash cycle on it.

What is worth testing here is not the web plumbing but the two claims this app
makes: that a restricted message is unreadable by anyone else, and that the
store cannot grow without bound. Both would be embarrassing to get wrong and
neither is obvious from reading the code.
"""

import app
import compat
import crypto
import store as store_module
import ui

PASS = 0
FAIL = 0


def check(label, condition):
    global PASS, FAIL
    if condition:
        PASS += 1
        print("  ok    " + label)
    else:
        FAIL += 1
        print("  FAIL  " + label)


def section(title):
    print("\n" + title)


# ------------------------------------------------------------------- AES

section("AES")

# The known-answer test from FIPS-197, appendix C.1. If this passes, the
# block cipher is genuinely AES and not merely self-consistent.
if not compat.IS_MICROPYTHON:
    from aes_soft import AES128

    key = bytes.fromhex("000102030405060708090a0b0c0d0e0f")
    plain = bytes.fromhex("00112233445566778899aabbccddeeff")
    expect = "69c4e0d86a7b0430d8cdb78070b4c55a"

    check("FIPS-197 known-answer vector", AES128(key).encrypt_block(plain).hex() == expect)
    check("block decrypt inverts encrypt", AES128(key).decrypt_block(bytes.fromhex(expect)) == plain)

section("AES-CTR")

k = b"0123456789abcdef"
n = b"12345678"
for size in (0, 1, 15, 16, 17, 200):
    data = b"z" * size
    check(
        "round-trips at {} bytes".format(size),
        compat.aes_ctr(k, n, compat.aes_ctr(k, n, data)) == data,
    )

check("ciphertext length matches plaintext", len(compat.aes_ctr(k, n, b"abc")) == 3)
check("a different nonce gives different output",
      compat.aes_ctr(k, b"11111111", b"hello") != compat.aes_ctr(k, b"22222222", b"hello"))

section("hex")
check("hex round-trips", compat.from_hex(compat.to_hex(b"\x00\xff\x10")) == b"\x00\xff\x10")


# ----------------------------------------------------------------- crypto

section("key derivation")

salt = b"12345678"
check("derive is deterministic", crypto.derive("pw", salt) == crypto.derive("pw", salt))
check("a different password gives a different key",
      crypto.derive("pw", salt) != crypto.derive("pw2", salt))
check("a different salt gives a different key",
      crypto.derive("pw", salt) != crypto.derive("pw", b"87654321"))
check("derive returns 16 bytes", len(crypto.derive("pw", salt)) == 16)

mask = crypto.derive("pw", salt)
check("the stored verifier is not the mask itself",
      crypto.verifier(mask) != compat.to_hex(mask))

secret = b"S" * 16
check("unwrap inverts wrap", crypto.unwrap(crypto.wrap(secret, mask), mask) == secret)


# ------------------------------------------------------------------ store

section("accounts")

st = store_module.Store()
check("seeds alice and bob", sorted(st.users) == ["alice", "bob"])
check("alice can sign in", st.login("alice", "wonderland") is not None)
check("bob can sign in", st.login("bob", "builder") is not None)
check("a wrong password is refused", st.login("alice", "WRONG") is None)
check("an unknown user is refused", st.login("nobody", "x") is None)
check("the password is not stored in the clear",
      "wonderland" not in repr(vars(st.users["alice"])))

section("restricted messages")

alice = st.session(st.login("alice", "wonderland"))
bob = st.session(st.login("bob", "builder"))
st.add_user("eve", "evil", "Eve")
eve = st.session(st.login("eve", "evil"))


def subjects(sess):
    return [m.subject for m, _ in st.readable(sess)]


check("alice sees the message she restricted to bob", "Just for Bob" in subjects(alice))
check("bob sees the message restricted to him", "Just for Bob" in subjects(bob))
check("eve does not see it at all", "Just for Bob" not in subjects(eve))
check("eve still sees public messages", "Welcome" in subjects(eve))

secret_msg = [m for m in st.messages if m.subject == "Just for Bob"][0]
check("the stored body is not plaintext", b"Only you and I" not in secret_msg.ciphertext)
check("eve has no wrapped key for it", "eve" not in secret_msg.keys)
check("the body is unreadable with the wrong key",
      "Only you and I" not in crypto.decrypt_body(
          secret_msg.ciphertext, b"0" * 16, secret_msg.nonce))

for msg, body_key in st.readable(bob):
    if msg.subject == "Just for Bob":
        check("bob decrypts it correctly", "Only you and I" in st.decrypt(msg, body_key))

section("sending")

st.post("bob", store_module.RESTRICTED, ["alice"], "For Alice", "hello alice")
check("a restricted message reaches its recipient", "For Alice" in subjects(alice))
check("the sender keeps a readable copy", "For Alice" in subjects(bob))
check("a third party is excluded", "For Alice" not in subjects(eve))

long_body = "x" * 5000
msg = st.post("alice", store_module.PUBLIC, [], "long", long_body)
for m, bk in st.readable(alice):
    if m.id == msg.id:
        check("an over-long body is truncated, not rejected",
              len(st.decrypt(m, bk)) == store_module.MAX_BODY)

section("the message ring")

before = len(st.messages)
for i in range(80):
    st.post("alice", store_module.PUBLIC, [], "spam {}".format(i), "body")
check("the ring holds at its capacity", len(st.messages) == store_module.MAX_MESSAGES)
check("the newest message survives",
      st.messages[-1].subject == "spam 79")
check("the oldest were dropped",
      "Welcome" not in [m.subject for m in st.messages])

section("sessions")

token = st.login("alice", "wonderland")
check("a fresh token resolves", st.session(token) is not None)
st.logout(token)
check("logging out revokes the token", st.session(token) is None)
check("an unknown token is refused", st.session("deadbeef") is None)
check("an empty token is refused", st.session(None) is None)

expired = st.login("alice", "wonderland")
st.sessions[expired]["seen"] -= store_module.SESSION_MS + 1000
check("an old session expires", st.session(expired) is None)


# ------------------------------------------------------------------ routes

section("HTTP routes")


def call(method, path, body="", token=None, host="secrets.local"):
    return app.route(method, path, body, token, host)


status, ctype, _ = call("GET", "/")
check("GET / serves HTML", status == 200 and ctype == "text/html")
check("GET /style.css serves CSS", call("GET", "/style.css")[1] == "text/css")
check("GET /app.js serves JavaScript",
      call("GET", "/app.js")[1] == "application/javascript")
check("GET /health answers for watch.ps1", call("GET", "/health")[0] == 200)
check("an unknown path is 404", call("GET", "/nope")[0] == 404)
check("OPTIONS is answered for CORS preflight", call("OPTIONS", "/api/login")[0] == 204)

check("the API rejects a missing session", call("GET", "/api/messages")[0] == 401)
check("the API rejects a bad session",
      call("GET", "/api/messages", "", "deadbeef")[0] == 401)

status, _, body = call("POST", "/api/login", '{"username":"alice","password":"WRONG"}')
check("a bad login is 401", status == 401)
check("the error does not say which half was wrong", "password" not in body.lower()
      or "do not match" in body)

status, _, body = call("POST", "/api/login", '{"username":"alice","password":"wonderland"}')
check("a good login is 200", status == 200)
api_token = body.split('"token":"')[1].split('"')[0]

status, _, body = call("GET", "/api/messages", "", api_token)
check("messages come back for a valid session", status == 200)
check("the response carries the stats footer", '"stats"' in body)

check("posting with no subject is 400",
      call("POST", "/api/post", '{"subject":"","body":"x"}', api_token)[0] == 400)
check("restricting to nobody is 400",
      call("POST", "/api/post",
           '{"subject":"a","body":"b","visibility":"restricted","recipients":[]}',
           api_token)[0] == 400)
check("an unknown recipient is rejected",
      call("POST", "/api/post",
           '{"subject":"a","body":"b","visibility":"restricted","recipients":["ghost"]}',
           api_token)[0] == 400)
check("an unknown visibility is rejected",
      call("POST", "/api/post",
           '{"subject":"a","body":"b","visibility":"weird"}', api_token)[0] == 400)
check("malformed JSON does not crash the route",
      call("POST", "/api/post", "{not json", api_token)[0] == 400)

section("JSON encoding")

check("newlines are escaped", app._dump("a\nb") == '"a\\nb"')
check("quotes are escaped", app._dump('say "hi"') == '"say \\"hi\\""')
check("backslashes are escaped", app._dump("a\\b") == '"a\\\\b"')
check("control characters are escaped", app._dump("\x01") == '"\\u0001"')
check("nested structures encode",
      app._dump({"a": [1, True, None]}) == '{"a":[1,true,null]}')


# --------------------------------------------------------------- diagnostics

section("diagnostics")

diag = compat.diagnostics()
for field in ("implementation", "uptime", "free_memory_text", "signal_text",
              "rssi", "ip", "mac", "ssid", "signal_quality", "on_board"):
    check("reports {}".format(field), field in diag)

check("uptime is a number", isinstance(diag["uptime"], int))
check("off-board it says so", diag["on_board"] is False)
check("off-board there is no RSSI to invent", diag["rssi"] is None)

# The thresholds the page colours its bar by. Boundaries included, because
# an off-by-one here would mislabel a working signal as a failing one.
check("-50 is excellent", compat.signal_quality(-50) == "excellent")
check("-60 is excellent at the boundary", compat.signal_quality(-60) == "excellent")
check("-61 is good", compat.signal_quality(-61) == "good")
check("-70 is good at the boundary", compat.signal_quality(-70) == "good")
check("-75 is marginal", compat.signal_quality(-75) == "marginal")
check("-85 is unreliable", compat.signal_quality(-85) == "unreliable")
check("unknown RSSI is not guessed", compat.signal_quality(None) == "unknown")

check("the diagnostics route needs a session",
      call("GET", "/api/diagnostics")[0] == 401)

status, _, body = call("GET", "/api/diagnostics", "", api_token)
check("the diagnostics route answers a session", status == 200)
check("it carries the signal figure", '"signal_quality"' in body)


# ---------------------------------------------------------------- the link

section("notification links")

status, _, body = call(
    "POST", "/api/post",
    '{"subject":"Bins","body":"tomorrow","visibility":"restricted","recipients":["bob"]}',
    api_token, host="secrets.local")
check("posting returns the message id", '"id"' in body)
check("posting returns a link", '"link"' in body)
check("the link points at this board", "http://secrets.local/?m=" in body)

# The Host header is echoed so the link works from wherever the sender was -
# an IP when mDNS does not resolve, a port under the dev server.
status, _, body = call(
    "POST", "/api/post", '{"subject":"x","body":"y","visibility":"public"}',
    api_token, host="192.168.1.50:8000")
check("the link honours the Host header",
      "http://192.168.1.50:8000/?m=" in body)

status, _, body = call(
    "POST", "/api/post", '{"subject":"x","body":"y","visibility":"public"}',
    api_token, host=None)
check("a missing Host falls back to the mDNS name",
      "http://secrets.local/?m=" in body)

# Fetching one message by id must obey exactly the same rules as the list.
st2 = store_module.Store()
a2 = st2.session(st2.login("alice", "wonderland"))
b2 = st2.session(st2.login("bob", "builder"))
st2.add_user("mallory", "pw", "Mallory")
m2 = st2.session(st2.login("mallory", "pw"))

restricted = [m for m in st2.messages if m.subject == "Just for Bob"][0]
found, key = st2.by_id(restricted.id, b2)
check("a recipient can open the link", found is not None)
check("and it decrypts", "Only you and I" in st2.decrypt(found, key))

found, key = st2.by_id(restricted.id, m2)
check("a stranger following the link gets nothing", found is None)
check("and no key with it", key is None)

found, _ = st2.by_id(99999, a2)
check("an unknown id is not an error", found is None)


# ------------------------------------------------------------------ handles

section("mastodon handles")

check("seeded users have handles", st2.users["alice"].handle.startswith("@"))
check("a handle is optional", store_module.User("x", "y", "X").handle == "")

# Both seeded handles are real accounts on DIFFERENT instances, so that
# sending from one to the other tests federated delivery rather than a
# self-DM. If these are ever pointed at the same instance, that coverage is
# quietly lost - hence the check.
alice_handle = st2.users["alice"].handle
bob_handle = st2.users["bob"].handle
check("the two seeded handles differ", alice_handle != bob_handle)
check("they are on different instances",
      alice_handle.split("@")[-1] != bob_handle.split("@")[-1])
check("both are fully qualified (@user@host)",
      alice_handle.count("@") == 2 and bob_handle.count("@") == 2)

status, _, body = call("POST", "/api/login",
                       '{"username":"alice","password":"wonderland"}')
check("handles are sent to the browser", '"handle"' in body)
check("the board no longer reports a mastodon capability",
      '"mastodon"' not in body)


# ---------------------------------------------------------------- notify.js

section("the browser notifier")

import notify_js

js = notify_js.NOTIFY_JS


def js_code_only(source):
    """The JavaScript with // comments stripped.

    Needed because several checks below assert that something is *absent*, and
    this file discusses the very things it must not do ("Never Math.random",
    "sessionStorage, not localStorage"). A plain substring search cannot tell a
    rule from a violation of it, so the prose has to go first.

    Only strips a // that starts a line, because the file also contains
    "https://" inside string literals - cutting at every // would delete the
    URLs this is meant to check for.
    """
    out = []
    for line in source.split("\n"):
        if line.strip().startswith("//"):
            continue
        out.append(line)
    return "\n".join(out)


code = js_code_only(js)

check("notify.js is served", call("GET", "/notify.js")[0] == 200)
check("it is javascript",
      call("GET", "/notify.js")[1] == "application/javascript")

# The point of moving this into the browser: no credential on the board.
check("no token is stored on the board", "MASTODON_TOKEN" not in js)
check("it registers an app dynamically", "/api/v1/apps" in code)
check("it uses PKCE S256", "code_challenge_method" in code and "S256" in code)
check("the verifier is hashed, not sent", "sha256Base64Url" in code)
check("randomness is cryptographic", "getRandomValues" in code)
check("it never falls back to Math.random", "Math.random" not in code)
check("state is checked on return", "statesMatch" in code)
check("the DM is direct, never public",
      '"direct"' in code and '"public"' not in code)
check("the token lives in sessionStorage only",
      "sessionStorage" in code and "localStorage" not in code)


# -------------------------------------------------------------- the forms

section("forms cannot leak into the URL")

# A form with no method= defaults to GET against the current URL. If the JS
# submit handler ever fails to bind, the browser submits natively and puts
# every NAMED field in the query string - which is how
# "?username=alice&password=wonderland" ended up in the address bar and in
# browser history. Two independent guards, both checked here.

page = ui.PAGE

import re as _re

forms = _re.findall(r"<form[^>]*>", page)
check("every form declares method=post",
      all('method="post"' in f for f in forms))
check("there is more than one form to check", len(forms) >= 2)

# The password field must not be serialisable at all.
login_form = page[page.index('id="login-form"'):page.index("</form>")]
check("the password input has no name attribute",
      'type="password"' in login_form and
      not _re.search(r'<input[^>]*type="password"[^>]*name=', login_form))
check("the username input has no name attribute",
      not _re.search(r'<input[^>]*id="u"[^>]*name=', login_form))
check("no input anywhere is named username or password",
      'name="username"' not in page and 'name="password"' not in page)

# The radio group legitimately uses name= - that is how a radio group is
# formed at all - but it carries no secret and its form is method=post. Only
# form controls matter here, so <meta name=...> is excluded.
control_names = set(_re.findall(r'<(?:input|textarea|select)[^>]*name="([^"]+)"', page))
check("the radio group is the only named form control", control_names == {"vis"})

# A ReferenceError at the top level would stop every addEventListener below it
# from running - which is what makes a native submission possible in the first
# place. The one top-level statement that depends on notify.js must be guarded.
app_js = ui.APP_JS
guarded = app_js[app_js.index("MASTO.completeIfReturning") - 400:
                 app_js.index("MASTO.completeIfReturning")]
check("the cross-file call at top level is inside a try",
      "try {" in guarded)
check("refreshMastoUi does not name MASTO unguarded",
      'typeof MASTO !== "undefined"' in app_js)


# ------------------------------------------------------------------ result

print("\n" + "-" * 40)
print("  {} passed, {} failed".format(PASS, FAIL))
print("-" * 40)

if FAIL:
    raise SystemExit(1)
