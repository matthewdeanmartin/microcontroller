# /// script
# requires-python = ">=3.12"
# ///
"""A tiny stand-in Mastodon instance for trying "Sign in with Mastodon"
without a real one (desktop or board; plain HTTP on the LAN).

It does what Game Matcher needs and nothing else: OAuth discovery, app
registration, an authorize page that approves at once (add `?deny=1` to the
instance URL in Admin to have it refuse), the token exchange with PKCE
checked, verify_credentials and revoke. One account: @sam (id 42).

    uv run tools/mock_mastodon.py --port 8099 [--host 0.0.0.0]

Then in Admin add an instance with address http://<this machine>:8099.
"""
import argparse
import base64
import hashlib
import json
import secrets
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlparse

apps: dict[str, dict] = {}
codes: dict[str, dict] = {}
tokens: set[str] = set()
log: list[str] = []


class Handler(BaseHTTPRequestHandler):
    def base(self) -> str:
        return f"http://{self.headers.get('Host')}"

    def send_json(self, status: int, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def form(self) -> dict[str, str]:
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length).decode()
        return {k: v[0] for k, v in parse_qs(raw).items()}

    def do_GET(self) -> None:
        url = urlparse(self.path)
        log.append(f"GET {url.path}")
        q = {k: v[0] for k, v in parse_qs(url.query).items()}
        if url.path == "/.well-known/oauth-authorization-server":
            b = self.base()
            return self.send_json(200, {
                "issuer": b + "/",
                "authorization_endpoint": b + "/oauth/authorize",
                "token_endpoint": b + "/oauth/token",
                "revocation_endpoint": b + "/oauth/revoke",
                "scopes_supported": ["read", "write", "profile"],
                "code_challenge_methods_supported": ["S256"],
            })
        if url.path == "/oauth/authorize":
            app = apps.get(q.get("client_id", ""))
            if not app or q.get("redirect_uri") not in app["redirect_uris"]:
                return self.send_json(400, {"error": "invalid_request"})
            back = q["redirect_uri"]
            if "deny" in q or app.get("deny"):
                target = back + "?" + urlencode({"error": "access_denied", "error_description": "The user denied", "state": q.get("state", "")})
            else:
                code = secrets.token_urlsafe(16)
                codes[code] = {"client_id": q["client_id"], "challenge": q.get("code_challenge", ""), "redirect_uri": back}
                target = back + "?" + urlencode({"code": code, "state": q.get("state", "")})
            self.send_response(302)
            self.send_header("Location", target)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if url.path == "/api/v1/accounts/verify_credentials":
            auth = self.headers.get("Authorization", "")
            if auth.removeprefix("Bearer ") not in tokens:
                return self.send_json(401, {"error": "The access token is invalid"})
            return self.send_json(200, {"id": "42", "username": "sam", "acct": "sam", "display_name": "Sam (mock)",
                                        "avatar": "", "emojis": [], "fields": [], "source": {"privacy": "public"}})
        return self.send_json(404, {"error": "Record not found"})

    def do_POST(self) -> None:
        url = urlparse(self.path)
        log.append(f"POST {url.path}")
        f = self.form()
        if url.path == "/api/v1/apps":
            cid, secret = secrets.token_urlsafe(12), secrets.token_urlsafe(24)
            apps[cid] = {"secret": secret, "redirect_uris": f.get("redirect_uris", "").split("\n"), "deny": False}
            return self.send_json(200, {"id": str(len(apps)), "name": f.get("client_name"), "client_id": cid, "client_secret": secret})
        if url.path == "/oauth/token":
            grant = codes.pop(f.get("code", ""), None)
            app = apps.get(f.get("client_id", ""))
            verifier = f.get("code_verifier", "")
            challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
            ok = (grant and app and app["secret"] == f.get("client_secret") and grant["client_id"] == f.get("client_id")
                  and grant["redirect_uri"] == f.get("redirect_uri") and grant["challenge"] == challenge)
            if not ok:
                return self.send_json(400, {"error": "invalid_grant"})
            token = secrets.token_urlsafe(24)
            tokens.add(token)
            return self.send_json(200, {"access_token": token, "token_type": "Bearer", "scope": "profile", "created_at": 0})
        if url.path == "/oauth/revoke":
            tokens.discard(f.get("token", ""))
            return self.send_json(200, {})
        return self.send_json(404, {"error": "Record not found"})

    def log_message(self, fmt: str, *args) -> None:  # quiet
        pass


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=int, default=8099)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()
    print(f"mock Mastodon on http://{args.host}:{args.port}", flush=True)
    ThreadingHTTPServer((args.host, args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
