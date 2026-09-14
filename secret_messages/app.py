"""Routes. THIS is the file you edit.

Runs unchanged under CPython (via dev_server.py) and MicroPython (via main.py),
exactly like hello_wifi_py - same contract, one step further: a route here
takes the method, path and body, and still returns (status, content_type, body).

THE SHAPE OF THIS APP
Three static routes serve the page, and everything else is a JSON API under
/api/. That split is deliberate: the stated plan is to move the static files
to a public host and leave the API on the board. When that happens, stop
serving the first three routes and change one constant in app.js. The CORS
headers that make it work are already sent by compat.format_response.
"""

import compat
import notify_js
import store as store_module
import ui

# One store for the life of the process. Rebooting the board wipes it and
# re-seeds alice and bob - which is the documented behaviour, not an accident.
STORE = store_module.Store()


# ------------------------------------------------------------------ JSON

# MicroPython has `json`, but building these small documents by hand avoids a
# surprise: ujson.dumps on some ports does not escape every control character
# the way the browser expects. Being explicit is cheaper than debugging that
# over a serial cable.

def _json_escape(text):
    out = []
    for ch in text:
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        elif ord(ch) < 0x20:
            out.append("\\u{:04x}".format(ord(ch)))
        else:
            out.append(ch)
    return "".join(out)


def _dump(value):
    """Serialise the small subset of types this app actually returns."""
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, str):
        return '"' + _json_escape(value) + '"'
    if isinstance(value, (list, tuple)):
        return "[" + ",".join(_dump(v) for v in value) + "]"
    if isinstance(value, dict):
        return "{" + ",".join(
            '"' + _json_escape(str(k)) + '":' + _dump(v) for k, v in value.items()
        ) + "}"
    return '"' + _json_escape(str(value)) + '"'


def _parse_json(text):
    """Parse a request body. Returns {} on anything unparseable.

    Uses the runtime's own parser - the risk noted above is in *writing* JSON,
    not reading it, and the input here is whatever a browser sent.
    """
    if not text:
        return {}
    try:
        import json

        value = json.loads(text)
        return value if isinstance(value, dict) else {}
    except (ValueError, TypeError):
        return {}


def _ok(payload):
    return 200, "application/json", _dump(payload)


def _err(status, message):
    return status, "application/json", _dump({"error": message})


# -------------------------------------------------------------- API routes

def api_login(body, _token, _host):
    data = _parse_json(body)
    username = str(data.get("username", "")).strip().lower()
    password = str(data.get("password", ""))

    if not username or not password:
        return _err(400, "Username and password are both needed.")

    token = STORE.login(username, password)
    if token is None:
        # One message for both failure modes, so this cannot be used to
        # enumerate which accounts exist.
        return _err(401, "That username and password do not match.")

    user = STORE.users[username]
    return _ok({
        "token": token,
        "user": user.name,
        "display": user.display,
        # Sent at login so the compose form can list recipients without a
        # second round trip - the board answers one request at a time, so
        # round trips are the thing worth saving.
        #
        # The handle goes out too, because the browser - not the board - is
        # what sends the notification DM.
        "users": [
            {"name": u.name, "display": u.display, "handle": u.handle}
            for u in STORE.users.values()
        ],
    })


def api_logout(_body, token, _host):
    STORE.logout(token)
    return _ok({"ok": True})


def api_diagnostics(_body, token, _host):
    """Board vitals - signal, memory, uptime, addresses.

    Behind a session like everything else. The figures are harmless, but the
    MAC and SSID are house details and there is no reason to hand them to an
    unauthenticated caller.
    """
    sess = STORE.session(token)
    if sess is None:
        return _err(401, "Your session has expired. Sign in again.")

    return _ok(compat.diagnostics())


def api_messages(_body, token, _host):
    sess = STORE.session(token)
    if sess is None:
        return _err(401, "Your session has expired. Sign in again.")

    messages = []
    for msg, body_key in STORE.readable(sess):
        messages.append({
            "id": msg.id,
            "sender": msg.sender,
            "subject": msg.subject,
            "body": STORE.decrypt(msg, body_key),
            "visibility": msg.visibility,
            "recipients": msg.recipients,
        })

    return _ok({
        "messages": messages,
        "stats": STORE.stats(),
        "free": compat.free_memory_text(),
    })


def api_post(body, token, host):
    sess = STORE.session(token)
    if sess is None:
        return _err(401, "Your session has expired. Sign in again.")

    data = _parse_json(body)
    subject = str(data.get("subject", "")).strip()
    text = str(data.get("body", "")).strip()
    visibility = data.get("visibility", store_module.PUBLIC)
    recipients = data.get("recipients", [])

    if not subject or not text:
        return _err(400, "A subject and a message are both needed.")

    if visibility not in (store_module.PUBLIC, store_module.RESTRICTED):
        return _err(400, "Unknown visibility.")

    # Keep only recipients that exist, so a stale or hand-crafted client
    # cannot create a message addressed to nobody and silently unreadable.
    recipients = [r for r in recipients if r in STORE.users]
    if visibility == store_module.RESTRICTED and not recipients:
        return _err(400, "Choose at least one recipient.")

    if len(subject) > 80:
        subject = subject[:80]

    msg = STORE.post(
        sender=sess["user"],
        visibility=visibility,
        recipients=recipients,
        subject=subject,
        body=text,
    )

    # A link back to this exact message, built from the Host header so it uses
    # whichever name the sender reached the board by - secrets.local for most,
    # a raw IP where mDNS does not work. The board cannot know which of its
    # names a given phone can resolve, so it echoes the one that demonstrably
    # worked.
    link = "http://{}/?m={}".format(host or "secrets.local", msg.id)

    return _ok({"ok": True, "id": msg.id, "link": link})


API = {
    "/api/login": api_login,
    "/api/logout": api_logout,
    "/api/messages": api_messages,
    "/api/post": api_post,
    "/api/diagnostics": api_diagnostics,
}


# ------------------------------------------------------------------ route

def route(method, path, body=b"", token=None, host=None):
    """Map a request to a response. Returns (status, content_type, body)."""

    # A browser sends OPTIONS before a cross-origin POST. Answering it now
    # means the day the page moves to a public host, the API needs no change.
    if method == "OPTIONS":
        return 204, "text/plain", ""

    if path in API:
        if method not in ("GET", "POST"):
            return _err(405, "Method not allowed.")
        try:
            return API[path](body, token, host)
        except Exception as e:  # noqa: BLE001
            # A crash in one handler should return an error the page can show,
            # not drop the connection and leave the board looking dead.
            return _err(500, "Server error: {}".format(e))

    if path == "/" or path == "/index.html":
        return 200, "text/html", ui.PAGE

    if path == "/style.css":
        return 200, "text/css", ui.STYLE

    if path == "/app.js":
        return 200, "application/javascript", ui.APP_JS

    if path == "/notify.js":
        # The Mastodon OAuth client. Served separately from app.js because it
        # is self-contained and cacheable, and because keeping it apart makes
        # it obvious that no credential passes through the board.
        return 200, "application/javascript", notify_js.NOTIFY_JS

    if path == "/health":
        # Plain text and no auth, so watch.ps1 from the other projects works
        # against this board unchanged.
        return 200, "text/plain", "ok messages={}\n".format(len(STORE.messages))

    if path == "/favicon.ico":
        return 204, "text/plain", ""

    return 404, "text/plain", "not found\n"
