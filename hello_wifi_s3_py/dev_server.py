"""Local preview server. Runs app.py on your PC, no board required.

    python dev_server.py

Then open http://localhost:8000

Deliberately uses raw sockets rather than http.server, for the same reason
compat.format_response exists: the local server should exercise the same code
path the board does, so bugs show up here instead of after a flash.

Auto-reloads app.py on every request, so editing and refreshing is the whole
loop - no restart.
"""

import importlib
import socket
import sys
import traceback

import app
import compat

HOST = "127.0.0.1"
PORT = 8123


def handle(conn):
    """Answer one request using the current on-disk app.py."""
    request_line = conn.readline()
    if not request_line:
        return

    parts = request_line.split()
    path = parts[1].decode() if len(parts) > 1 else "/"

    while True:
        line = conn.readline()
        if not line or line == b"\r\n":
            break

    # Pick up edits without a restart. Costs a few ms per request and only
    # happens locally, which is a good trade for a tight edit loop.
    try:
        importlib.reload(app)
    except Exception:
        # A syntax error in app.py should show in the browser, not kill the
        # server and make you re-run it.
        tb = traceback.format_exc()
        print(tb, file=sys.stderr)
        body = f"<pre>app.py failed to load:\n\n{tb}</pre>"
        conn.write(compat.format_response(500, "text/html", body))
        return

    status, ctype, body = app.route(path)
    print(f"  {status}  {path}")
    conn.write(compat.format_response(status, ctype, body))


def main():
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((HOST, PORT))
    s.listen(5)

    print(f"serving app.py at http://{HOST}:{PORT}")
    print("edit app.py and refresh - no restart needed")
    print("Ctrl+C to stop\n")

    while True:
        conn = None
        try:
            conn, _ = s.accept()
            # makefile gives us readline(), matching MicroPython's socket API.
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
