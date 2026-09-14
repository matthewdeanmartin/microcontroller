"""Reading an HTTP request, shared by the board and the dev server.

hello_wifi_py only needed the path, so it read the request line and threw the
headers away. This app needs the method, a POST body, and the X-Session
header, so the parsing is a little more involved - and therefore worth having
in one place, where a bug shows up locally rather than only after a flash.

Both callers hand in a file-like object with readline() and read(): a socket
via makefile() under CPython, and a raw socket under MicroPython, whose socket
objects already have both methods.
"""

# Refuse anything larger, so a bad or hostile client cannot make the board
# allocate its way into a reset. Comfortably above a maximum-length message.
MAX_BODY_BYTES = 4096


def read_request(stream):
    """Parse one request. Returns (method, path, body_text, session_token, host).

    `host` is the Host header as the client sent it - "secrets.local" or a raw
    IP, depending on how they reached the board. It is echoed back into
    notification links so the link works from wherever the sender was; the
    board cannot know which of its names a given phone can resolve.

    Returns all-None if the connection closed before sending anything, which
    happens routinely - browsers open speculative connections.
    """
    request_line = stream.readline()
    if not request_line:
        return None, None, None, None, None

    parts = request_line.split()
    if len(parts) < 2:
        return None, None, None, None, None

    method = parts[0].decode()
    path = parts[1].decode()

    # Strip any query string; this app routes on the path alone.
    if "?" in path:
        path = path.split("?")[0]

    content_length = 0
    token = None
    host = None

    while True:
        line = stream.readline()
        if not line or line == b"\r\n":
            break

        # Header names are case-insensitive, and browsers differ on the casing
        # of custom headers - so compare in lower case.
        lowered = line.lower()

        if lowered.startswith(b"content-length:"):
            try:
                content_length = int(line.split(b":", 1)[1].strip())
            except ValueError:
                content_length = 0

        elif lowered.startswith(b"x-session:"):
            token = line.split(b":", 1)[1].strip().decode()

        elif lowered.startswith(b"host:"):
            # Split on the FIRST colon only: the value may itself carry a port
            # ("secrets.local:8000"), which must be kept or the link breaks.
            try:
                host = line.split(b":", 1)[1].strip().decode()
            except UnicodeError:
                host = None

    body = ""
    if content_length > 0:
        if content_length > MAX_BODY_BYTES:
            # Read and discard, so the client still gets a clean response
            # rather than a reset connection.
            _drain(stream, content_length)
            return method, path, "", token, host

        raw = stream.read(content_length)
        if raw:
            try:
                body = raw.decode("utf-8")
            except UnicodeError:
                body = ""

    return method, path, body, token, host


def _drain(stream, count):
    """Read and throw away `count` bytes, in chunks, without allocating them all."""
    remaining = count
    while remaining > 0:
        chunk = stream.read(min(512, remaining))
        if not chunk:
            break
        remaining -= len(chunk)
