"""The web app. THIS is the file you edit.

It runs unchanged in two places:

  * locally, under CPython, via `python dev_server.py`
  * on the board, under MicroPython, via main.py

Nothing here touches sockets or WiFi. Routes are plain functions that take a
path and return (status, content_type, body). That is what makes the same code
work in both places - see compat.py for the machinery.
"""

import compat


def page_index():
    """The status page. Mirrors the C version's `/` endpoint."""
    uptime = compat.uptime_seconds()
    free = compat.free_memory_text()
    impl = compat.implementation_name()
    net = compat.network_text()
    signal = compat.signal_text()

    # Say plainly which machine answered, so a local preview is never mistaken
    # for the real board.
    if compat.IS_MICROPYTHON:
        heading = "Hello from the ESP32-S2!"
        subtitle = "Served by your DiGiYes S2 Mini over WiFi."
    else:
        heading = "Hello from your PC"
        subtitle = "Local preview. Flash to the board to see the real thing."

    # Kept as one f-string rather than a template engine: MicroPython has no
    # jinja2, and pulling one in would break the "same file both places" rule.
    return f"""<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>ESP32-S2</title>
<style>
  body {{ font-family: system-ui, sans-serif; max-width: 32rem;
         margin: 3rem auto; padding: 0 1rem; line-height: 1.5; }}
  h1 {{ font-size: 1.5rem; }}
  dt {{ font-weight: 600; margin-top: .75rem; }}
  dd {{ margin: 0; font-family: ui-monospace, monospace; }}
  .env {{ display: inline-block; padding: .15rem .5rem; border-radius: .25rem;
          background: #eef; font-size: .8rem; font-family: ui-monospace, monospace; }}
</style>
</head>
<body>
  <h1>{heading}</h1>
  <p>{subtitle} <span class="env">{impl}</span></p>
  <dl>
    <dt>Uptime</dt><dd>{uptime} seconds</dd>
    <dt>Free memory</dt><dd>{free}</dd>
    <dt>Address</dt><dd>{net}</dd>
    <dt>Signal</dt><dd>{signal}</dd>
  </dl>
  <p><a href="/health">/health</a></p>
</body>
</html>
"""


def route(path):
    """Map a URL path to a response.

    Returns (status_code, content_type, body). Deliberately dumb - a dict
    lookup would be tidier but this is easier to follow and to extend.
    """
    if path == "/":
        return 200, "text/html", page_index()

    if path == "/health":
        # Includes the signal so a monitoring script can correlate failures
        # with link quality without scraping the HTML page.
        return 200, "text/plain", f"ok rssi={compat.rssi()}\n"

    if path == "/favicon.ico":
        # Browsers ask for this on every page load. Answering 204 keeps it out
        # of the logs, same as the C version does.
        return 204, "text/plain", ""

    return 404, "text/plain", "not found\n"
