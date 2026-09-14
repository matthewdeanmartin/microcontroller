"""Local preview server. Runs app.py on your PC, no board required.

    python dev_server.py

Then open http://localhost:8000

Same reasoning as hello_wifi_py's dev server: raw sockets rather than
http.server, so the local run exercises the same code path the board does and
bugs surface here instead of after a flash.

One difference worth knowing. That server reloaded app.py on every request,
which was free because its app had no state. This one holds users, sessions
and messages in module state, so a reload would log you out and wipe the
board's memory on every click. Reloading is therefore opt-in:

    python dev_server.py --reload

Use it while working on layout; leave it off while testing the app.
"""

import importlib
import socket
import sys
import traceback

import app
import compat
import http_parse

HOST = "127.0.0.1"
PORT = 8000

RELOAD = "--reload" in sys.argv


def handle(stream):
    """Answer one request using app.route()."""
    method, path, body, token, host = http_parse.read_request(stream)
    if method is None:
        return

    if RELOAD:
        try:
            # Reloads the routes and the page markup - and resets the store,
            # which is why this is not the default.
            importlib.reload(app)
        except Exception:
            tb = traceback.format_exc()
            print(tb, file=sys.stderr)
            page = "<pre>app.py failed to load:\n\n{}</pre>".format(tb)
            stream.write(compat.format_response(500, "text/html", page))
            return

    try:
        status, ctype, response_body = app.route(method, path, body, token, host)
    except Exception:
        # Mirrors the board's behaviour: show the error rather than dropping
        # the connection, so a mistake is visible in the browser.
        tb = traceback.format_exc()
        print(tb, file=sys.stderr)
        status, ctype, response_body = 500, "text/plain", tb

    print("  {}  {} {}".format(status, method, path))
    stream.write(compat.format_response(status, ctype, response_body))


def main():
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((HOST, PORT))
    s.listen(5)

    print("serving app.py at http://{}:{}".format(HOST, PORT))
    if RELOAD:
        print("--reload is ON: edits apply per request, but state resets too")
    else:
        print("state persists; restart to reset. --reload to pick up edits")
    print("sign in as alice/wonderland or bob/builder")
    print("Ctrl+C to stop\n")

    while True:
        conn = None
        try:
            conn, _ = s.accept()
            # makefile gives readline()/read(), matching MicroPython's socket.
            with conn.makefile("rwb") as f:
                handle(f)
        except KeyboardInterrupt:
            print("\nbye")
            break
        except (OSError, BrokenPipeError) as e:
            print("connection error:", e)
        finally:
            if conn:
                conn.close()


if __name__ == "__main__":
    main()
