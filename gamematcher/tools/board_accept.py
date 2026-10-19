# /// script
# requires-python = ">=3.12"
# ///
"""Sprint 1 acceptance against a live board with NO accounts yet.

Creates test accounts (setup → admin, a member), checks PIN sign-in and
lockout, admin actions, HTTP and HTTPS cookies, and timing. With --after-reset
it only checks that a previous run's state survived a reboot. It leaves its
accounts behind: run it on a board you will wipe (`make flash`) afterwards.

    uv run tools/board_accept.py --url http://192.168.1.163 --ca api/certs/household-ca.crt
"""
import argparse
import http.client
import json
import socket
import ssl
import time
from urllib.parse import urlparse

ADMIN = {"email": "accept-admin@test", "display_name": "Accept Admin", "pin": "4321"}
MEMBER = {"email": "accept-member@test", "display_name": "Accept Member", "pin": "1111"}


class Board:
    def __init__(self, host: str, ca: str | None = None) -> None:
        self.host = host
        self.ca = ca

    def call(self, method: str, path: str, body: dict | None = None, cookie: str | None = None, secure: bool = False):
        if secure:
            context = ssl.create_default_context(cafile=self.ca)
            raw = socket.create_connection((self.host, 443), 15)
            conn = http.client.HTTPSConnection(self.host, 443, timeout=15, context=context)
            conn.sock = context.wrap_socket(raw, server_hostname="gamematcher.local")
            origin = "https://gamematcher.local"
            host = "gamematcher.local"
        else:
            conn = http.client.HTTPConnection(self.host, 80, timeout=15)
            origin = f"http://{self.host}"
            host = self.host
        headers = {"Host": host, "Origin": origin, "Content-Type": "application/json"}
        if cookie:
            headers["Cookie"] = cookie
        started = time.perf_counter()
        sep = "&" if "?" in path else "?"
        conn.request(method, f"{path}{sep}fmt=json", body=json.dumps(body) if body is not None else None, headers=headers)
        response = conn.getresponse()
        data = response.read()
        ms = (time.perf_counter() - started) * 1000
        set_cookie = response.getheader("Set-Cookie")
        conn.close()
        parsed = json.loads(data) if data else None
        return response.status, parsed, set_cookie, ms


def check(label: str, ok: bool, detail: str = "") -> bool:
    print(f"{'PASS' if ok else 'FAIL'}  {label}{'  ' + detail if detail else ''}")
    return ok


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", required=True)
    parser.add_argument("--ca", required=True)
    parser.add_argument("--after-reset", action="store_true")
    args = parser.parse_args()
    board = Board(urlparse(args.url).hostname or "", args.ca)
    results = []

    if args.after_reset:
        status, me, cookie, ms = board.call("POST", "/api/v1/auth/pin", {"email": ADMIN["email"], "pin": ADMIN["pin"]})
        results.append(check("admin signs in after reset", status == 200 and me["role"] == "admin", f"{ms:.0f} ms"))
        session = cookie.split(";")[0]
        status, users, _, _ = board.call("GET", "/api/v1/admin/users", cookie=session)
        emails = {u["email"] for u in users["users"]} if status == 200 else set()
        results.append(check("both accounts survived the reboot", {ADMIN["email"], MEMBER["email"]} <= emails))
        status, storage, _, _ = board.call("GET", "/api/v1/admin/storage", cookie=session)
        results.append(check("storage loaded cleanly", status == 200 and not storage["fallback_used"], json.dumps(storage)))
        status, settings, _, _ = board.call("GET", "/api/v1/admin/settings", cookie=session)
        results.append(check("sign-up setting survived (closed)", status == 200 and settings["signup_open"] is False))
        raise SystemExit(0 if all(results) else 1)

    status, s, _, _ = board.call("GET", "/api/v1/status")
    if not s["setup_needed"]:
        raise SystemExit("This board already has accounts; run this only on a fresh board.")
    results.append(check("fresh board: setup needed, sign-up open", s["signup_open"]))

    status, me, cookie, ms = board.call("POST", "/api/v1/setup", {**ADMIN, "setup_code": ""})
    results.append(check("first-run setup makes an admin", status == 201 and me["role"] == "admin", f"{ms:.0f} ms"))
    results.append(check("session cookie is HttpOnly, not Secure on HTTP", cookie is not None and "HttpOnly" in cookie and "Secure" not in cookie))
    admin = cookie.split(";")[0]
    status, _, _, _ = board.call("POST", "/api/v1/setup", {**MEMBER, "setup_code": ""})
    results.append(check("second setup refused", status == 409))

    status, me, cookie, ms = board.call("POST", "/api/v1/auth/signup", MEMBER)
    results.append(check("guest sign-up over HTTP", status == 201 and me["role"] == "member", f"{ms:.0f} ms"))
    member = cookie.split(";")[0]
    status, _, _, _ = board.call("GET", "/api/v1/admin/users", cookie=member)
    results.append(check("member cannot reach admin", status == 403))

    times = []
    for _ in range(5):
        status, _, _, ms = board.call("POST", "/api/v1/auth/pin", {"email": MEMBER["email"], "pin": "9999"})
        times.append(ms)
    status, _, _, _ = board.call("POST", "/api/v1/auth/pin", {"email": MEMBER["email"], "pin": MEMBER["pin"]})
    results.append(check("5 wrong PINs lock the account", status == 429, f"wrong-PIN round trips {min(times):.0f}–{max(times):.0f} ms"))
    status, _, _, _ = board.call("PATCH", "/api/v1/admin/users/2", {"pin": "2222"}, cookie=admin)
    status, _, _, _ = board.call("POST", "/api/v1/auth/pin", {"email": MEMBER["email"], "pin": "2222"})
    results.append(check("admin PIN reset clears the lock", status == 200))

    status, me, cookie, ms = board.call("POST", "/api/v1/auth/pin", {"email": ADMIN["email"], "pin": ADMIN["pin"]}, secure=True)
    results.append(check("PIN sign-in over HTTPS", status == 200, f"{ms:.0f} ms incl. handshake"))
    results.append(check("HTTPS cookie is Secure", cookie is not None and "Secure" in cookie))
    status, me, _, _ = board.call("GET", "/api/v1/me", cookie=cookie.split(";")[0], secure=True)
    results.append(check("HTTPS session works", status == 200 and me["sessions"] == 2))

    status, _, _, _ = board.call("POST", "/api/v1/auth/logout", cookie=member)
    status, _, _, _ = board.call("GET", "/api/v1/me", cookie=member)
    results.append(check("sign-out ends the session", status == 401))

    status, settings, _, _ = board.call("PATCH", "/api/v1/admin/settings", {"signup_open": False}, cookie=admin)
    results.append(check("admin closes sign-up", status == 200 and settings["signup_open"] is False))
    status, _, _, _ = board.call("POST", "/api/v1/auth/signup", {"email": "x@test", "display_name": "X", "pin": "1234"})
    results.append(check("closed sign-up refuses", status == 403))

    status, _, _, _ = board.call("POST", "/api/v1/auth/logout", body=None, cookie=admin)
    print("Now reset the board and rerun with --after-reset.")
    raise SystemExit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
