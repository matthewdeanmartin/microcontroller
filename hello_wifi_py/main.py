"""Device entry point. MicroPython runs this automatically at boot.

Connects to WiFi, then serves app.py's routes forever.

Edit app.py for page content; this file is the plumbing.
"""

import gc
import socket
import sys
import time

import network

import app
import compat

try:
    from config import WIFI_SSID, WIFI_PASSWORD
except ImportError:
    print("!! config.py missing - copy config_example.py to config.py")
    print("!! and put your WiFi credentials in it.")
    sys.exit(1)

PORT = 80

# Advertised over mDNS, so the board is reachable at http://esp32.local
# regardless of what IP DHCP hands out. Lower-case letters, digits and
# hyphens only - no dots, no underscores.
HOSTNAME = "esp32"

# Roughly matches the C version's startup behaviour: try for a while, then
# report rather than hanging silently forever.
CONNECT_TIMEOUT_S = 30


def connect_wifi():
    """Join the network. Returns the IP address, or None on failure."""
    wlan = network.WLAN(network.STA_IF)
    wlan.active(True)

    # Set the hostname BEFORE connecting. The ESP32 port advertises this over
    # mDNS, which is what makes http://<HOSTNAME>.local work - so the board is
    # reachable by name even when DHCP gives it a different IP after a reboot.
    # Setting it after connect() is too late; the announcement has gone out.
    try:
        wlan.config(hostname=HOSTNAME)
    except (OSError, ValueError) as e:
        # Not fatal - the IP still works, you just lose the friendly name.
        print("could not set hostname:", e)

    if wlan.isconnected():
        return wlan.ifconfig()[0]

    print(f"connecting to SSID {WIFI_SSID!r} ...")
    wlan.connect(WIFI_SSID, WIFI_PASSWORD)

    deadline = time.time() + CONNECT_TIMEOUT_S
    while not wlan.isconnected():
        if time.time() > deadline:
            # status() returns a negative code on failure; see the MicroPython
            # network docs. Printing it beats a bare "failed".
            print(f"failed to connect (status {wlan.status()})")
            print("check SSID/password in config.py, and that the network is 2.4GHz")
            return None
        time.sleep(0.5)

    ip = wlan.ifconfig()[0]
    print(f"got IP: {ip}")
    print(f"MAC:    {_mac_string(wlan)}   <- use this for a DHCP reservation")
    return ip


def _mac_string(wlan):
    """The board's MAC as aa:bb:cc:dd:ee:ff, for router config."""
    mac = wlan.config("mac")
    return ":".join("{:02x}".format(b) for b in mac)


def serve(ip):
    """Accept connections forever and answer them from app.route()."""
    addr = socket.getaddrinfo("0.0.0.0", PORT)[0][-1]

    s = socket.socket()
    # Without SO_REUSEADDR a crash-and-restart hits "address in use" for a
    # minute or so while the old socket lingers.
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(addr)
    s.listen(5)

    print("")
    print("  reachable at:")
    print(f"    http://{HOSTNAME}.local     <- survives an IP change")
    print(f"    http://{ip}")
    print("")

    while True:
        conn = None
        try:
            conn, remote = s.accept()

            # Read only the request line - enough for the path, and avoids
            # buffering a whole request we do not use.
            request_line = conn.readline()
            if not request_line:
                continue

            # b"GET /health HTTP/1.1" -> "/health"
            parts = request_line.split()
            path = parts[1].decode() if len(parts) > 1 else "/"

            # Drain the headers, or the client may see a reset instead of the
            # response.
            while True:
                line = conn.readline()
                if not line or line == b"\r\n":
                    break

            status, ctype, body = app.route(path)
            conn.write(compat.format_response(status, ctype, body))

        except OSError as e:
            # One bad client should never take the server down.
            print("connection error:", e)

        finally:
            if conn:
                conn.close()
            # The board has ~2MB with PSRAM but fragments without help.
            gc.collect()


def main():
    ip = connect_wifi()
    if ip is None:
        return
    serve(ip)


main()
