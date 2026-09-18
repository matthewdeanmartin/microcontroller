"""Device entry point. MicroPython runs this automatically at boot.

Connects to WiFi, then serves app.py's routes forever. The on-board RGB LED
reports which of those stages we are in, so a board that fails to join the
network says so without a serial cable - see led.py.

Edit app.py for page content; this file is the plumbing.
"""

import gc
import socket
import sys
import time

import network

import app
import boottime
import clock
import compat
import led

try:
    from config import WIFI_SSID, WIFI_PASSWORD
except ImportError:
    print("!! config.py missing - copy config_example.py to config.py")
    print("!! and put your WiFi credentials in it.")
    sys.exit(1)

# Hostname is per-board, so it lives in config.py alongside the credentials -
# two boards on one network must not both answer to the same name.
try:
    from config import HOSTNAME
except ImportError:
    HOSTNAME = "esp32s3"

PORT = 80

CONNECT_TIMEOUT_S = 30


def connect_wifi():
    """Join the network. Returns the IP address, or None on failure."""
    # ticks_ms has been counting since reset, so this first mark captures
    # everything before it: interpreter startup, imports, compiling main.py.
    boottime.mark("interpreter+imports")

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

    print("connecting to SSID {!r} ...".format(WIFI_SSID))
    wlan.connect(WIFI_SSID, WIFI_PASSWORD)

    deadline = time.time() + CONNECT_TIMEOUT_S
    step = 0
    while not wlan.isconnected():
        if time.time() > deadline:
            # status() returns a negative code on failure; see the MicroPython
            # network docs. Printing it beats a bare "failed".
            print("failed to connect (status {})".format(wlan.status()))
            print("check SSID/password in config.py, and that the network is 2.4GHz")
            led.error()
            return None
        led.connecting(step)
        step += 1
        time.sleep(0.5)

    boottime.mark("wifi join")

    ip = wlan.ifconfig()[0]
    print("got IP: {}".format(ip))
    print("MAC:    {}   <- use this for a DHCP reservation".format(_mac_string(wlan)))

    # The ESP32 has no battery-backed RTC and boots at 2000-01-01 every time,
    # so every timestamp is fiction until this runs. Done here, immediately
    # after the link comes up, because it needs the network and nothing else
    # should produce a timestamp before it.
    if clock.sync():
        print("clock:  {} (NTP)".format(clock.iso()))
    else:
        print("clock:  NOT synced - timestamps unavailable")
    boottime.mark("ntp sync")

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
    print("    http://{}.local     <- survives an IP change".format(HOSTNAME))
    print("    http://{}".format(ip))

    boottime.ready()
    bt = boottime.status()
    print("  ready in {} ms:".format(bt["total_ms"]))
    for ph in bt["phases"]:
        print("    {:<22} {:>6} ms".format(ph["name"], ph["duration_ms"]))
    print("")

    step = 0
    while True:
        conn = None
        try:
            led.serving(step)
            step += 1

            conn, remote = s.accept()
            led.request()

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

            # Time the route so the dashboard can show how long the board
            # takes to answer - the one latency figure the browser cannot
            # measure for itself, since its own timing includes the network.
            t0 = time.ticks_us()
            status, ctype, body = app.route(path)
            elapsed = time.ticks_diff(time.ticks_us(), t0)
            app.record_request(path, status, elapsed, len(body))

            conn.write(compat.format_response(status, ctype, body))

        except OSError as e:
            # One bad client should never take the server down.
            print("connection error:", e)

        finally:
            if conn:
                conn.close()
            # 8MB of PSRAM is a lot of headroom, but the dashboard polls every
            # two seconds forever, so collecting per-request keeps the heap
            # flat rather than sawtoothing up to a pause.
            gc.collect()


def main():
    led.booting()
    ip = connect_wifi()
    if ip is None:
        led.error()
        return
    serve(ip)


main()
