"""Device entry point. MicroPython runs this automatically at boot.

Connects to WiFi, then serves app.py's routes forever. Edit app.py (and ui.py)
for content; this file is the plumbing.

Deliberately close to hello_wifi_py/main.py - the WiFi join, the hostname
dance and the mDNS reasoning are all the same problem, already solved there.
The differences are that requests now carry a method and a body, and that the
store lives across requests.
"""

import gc
import socket
import sys
import time

import network

import app
import compat
import http_parse

try:
    from config import WIFI_SSID, WIFI_PASSWORD
except ImportError:
    print("!! config.py missing - copy config_example.py to config.py")
    print("!! and put your WiFi credentials in it.")
    sys.exit(1)

PORT = 80

# Advertised over mDNS, so the board is reachable at http://secrets.local
# regardless of what IP DHCP hands out. A different name from the hello_wifi
# projects, so both boards can be on the network at once.
HOSTNAME = "secrets"

CONNECT_TIMEOUT_S = 30


def connect_wifi():
    """Join the network. Returns the IP address, or None on failure."""
    wlan = network.WLAN(network.STA_IF)
    wlan.active(True)

    # Must be set BEFORE connect(): the ESP32 port advertises this over mDNS,
    # and once the announcement has gone out it is too late. See
    # docs/micropython/finding_it.md.
    try:
        wlan.config(hostname=HOSTNAME)
    except (OSError, ValueError) as e:
        print("could not set hostname:", e)

    if wlan.isconnected():
        return wlan.ifconfig()[0]

    print("connecting to SSID {!r} ...".format(WIFI_SSID))
    wlan.connect(WIFI_SSID, WIFI_PASSWORD)

    deadline = time.time() + CONNECT_TIMEOUT_S
    while not wlan.isconnected():
        if time.time() > deadline:
            print("failed to connect (status {})".format(wlan.status()))
            print("check SSID/password in config.py, and that the network is 2.4GHz")
            return None
        time.sleep(0.5)

    ip = wlan.ifconfig()[0]
    print("got IP:", ip)
    print("MAC:   ", _mac_string(wlan), "  <- use this for a DHCP reservation")
    return ip


def _mac_string(wlan):
    return ":".join("{:02x}".format(b) for b in wlan.config("mac"))


def serve(ip):
    """Accept connections forever and answer them from app.route()."""
    addr = socket.getaddrinfo("0.0.0.0", PORT)[0][-1]

    s = socket.socket()
    # Without SO_REUSEADDR a crash-and-restart hits "address in use" while the
    # old socket lingers.
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(addr)
    s.listen(5)

    print("")
    print("  reachable at:")
    print("    http://{}.local     <- survives an IP change".format(HOSTNAME))
    print("    http://{}".format(ip))
    print("")
    print("  sign in as alice/wonderland or bob/builder")
    print("")

    while True:
        conn = None
        try:
            conn, _ = s.accept()

            # A phone that has locked or wandered off can leave a connection
            # half-open; without a timeout the board waits on it forever and
            # stops answering everyone else.
            try:
                conn.settimeout(10)
            except (AttributeError, OSError):
                pass

            method, path, body, token, host = http_parse.read_request(conn)
            if method is None:
                continue

            status, ctype, response = app.route(method, path, body, token, host)
            conn.write(compat.format_response(status, ctype, response))

        except OSError as e:
            # One bad client should never take the server down.
            print("connection error:", e)

        except Exception as e:  # noqa: BLE001
            # Nor should one bad request. The store is in RAM - a crash here
            # would lose every message on the board.
            print("request error:", e)

        finally:
            if conn:
                conn.close()
            # Each request allocates a response body and some short-lived
            # crypto buffers. Collecting per request keeps the heap from
            # fragmenting over a long uptime.
            gc.collect()


def main():
    ip = connect_wifi()
    if ip is None:
        return
    serve(ip)


main()
