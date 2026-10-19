# /// script
# requires-python = ">=3.12"
# ///
"""Sprint 0 hardware gate (spec/01-overview.md#hardware).

Loads a Game Matcher board the way a game night would, while the board's
own gate worker makes outbound HTTPS requests:

- HTTP_CLIENTS keep-alive HTTP/1.1 clients polling /api/v1/status and the UI;
- one HTTPS client (household CA, SNI gamematcher.local) that reconnects
  every few requests, so TLS handshakes keep happening;
- the board's outbound gate (POST /api/v1/gate) for the same duration;
- /api/v1/sys sampled every few seconds for heap and TLS counters.

Pass: no reboot (uptime never goes backwards), no TLS allocation failure
in the board log, outbound fetches succeed, and the largest free internal
block stays >= 24 KiB. Standard library only.

    uv run tools/gate.py --url http://192.168.1.163 --ca api/certs/household-ca.crt
"""
import argparse
import http.client
import json
import socket
import ssl
import threading
import time
from urllib.parse import urlparse

MIN_LARGEST = 24 * 1024


class Counter:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.ok = 0
        self.failed = 0
        self.errors: dict[str, int] = {}
        self.max_ms = 0.0

    def record(self, ok: bool, ms: float, error: str = "") -> None:
        with self.lock:
            if ok:
                self.ok += 1
            else:
                self.failed += 1
                self.errors[error] = self.errors.get(error, 0) + 1
            self.max_ms = max(self.max_ms, ms)

    def summary(self) -> dict:
        return dict(ok=self.ok, failed=self.failed, max_ms=round(self.max_ms), errors=self.errors)


def get_json(host: str, port: int, path: str, method: str = "GET") -> dict:
    conn = http.client.HTTPConnection(host, port, timeout=10)
    headers = {"Origin": f"http://{host}"} if method != "GET" else {}
    conn.request(method, path, headers=headers)
    body = conn.getresponse().read()
    conn.close()
    return json.loads(body)


def http_client(host: str, port: int, stop: threading.Event, counter: Counter) -> None:
    paths = ["/api/v1/status?fmt=json", "/", "/api/v1/status?fmt=json"]
    conn = None
    i = 0
    while not stop.is_set():
        path = paths[i % len(paths)]
        i += 1
        started = time.perf_counter()
        try:
            if conn is None:
                conn = http.client.HTTPConnection(host, port, timeout=10)
            # Like a browser: the UI is stored gzip-only (identity gets 406).
            conn.request("GET", path, headers={"Accept": "text/html,*/*", "Accept-Encoding": "gzip"})
            response = conn.getresponse()
            response.read()
            counter.record(response.status == 200, (time.perf_counter() - started) * 1000, str(response.status))
        except Exception as e:  # noqa: BLE001 - every failure is a data point
            counter.record(False, (time.perf_counter() - started) * 1000, type(e).__name__)
            if conn is not None:
                conn.close()
            conn = None
            time.sleep(1)
        time.sleep(0.5)


class BoardHTTPS(http.client.HTTPSConnection):
    """Connects to the board's address but verifies it as gamematcher.local."""

    def __init__(self, host: str, context: ssl.SSLContext) -> None:
        super().__init__(host, 443, timeout=15, context=context)
        self.tls_context = context

    def connect(self) -> None:
        raw = socket.create_connection((self.host, self.port), self.timeout)
        self.sock = self.tls_context.wrap_socket(raw, server_hostname="gamematcher.local")


def https_client(host: str, ca: str, stop: threading.Event, counter: Counter, reconnect_every: int) -> None:
    context = ssl.create_default_context(cafile=ca)
    conn = None
    sent = 0
    while not stop.is_set():
        started = time.perf_counter()
        try:
            if conn is None or sent % reconnect_every == 0:
                if conn is not None:
                    conn.close()
                conn = BoardHTTPS(host, context)
            conn.request("GET", "/api/v1/status?fmt=json", headers={"Host": "gamematcher.local"})
            response = conn.getresponse()
            response.read()
            sent += 1
            counter.record(response.status == 200, (time.perf_counter() - started) * 1000, str(response.status))
        except Exception as e:  # noqa: BLE001
            counter.record(False, (time.perf_counter() - started) * 1000, type(e).__name__)
            if conn is not None:
                conn.close()
            conn = None
            sent = 0
            time.sleep(1)
        time.sleep(1)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", required=True, help="http://<board ip>")
    parser.add_argument("--ca", required=True, help="household CA (PEM)")
    parser.add_argument("--secs", type=int, default=300)
    parser.add_argument("--http-clients", type=int, default=3)
    parser.add_argument("--no-https", action="store_true", help="plain-HTTP firmware")
    parser.add_argument("--reconnect-every", type=int, default=5)
    parser.add_argument("--out", help="write the JSON result here too")
    args = parser.parse_args()
    parsed = urlparse(args.url)
    host, port = parsed.hostname or "", parsed.port or 80

    before = get_json(host, port, "/api/v1/sys?fmt=json")
    print(f"Board: {before['chip']} {before['build']} uptime {before['uptime_ms'] / 1000:.0f} s, reset {before['reset_reason']}")
    print(f"Heap before: free {before['heap']['internal_free']:,} largest {before['heap']['internal_largest']:,} min {before['heap']['internal_min']:,}")
    get_json(host, port, f"/api/v1/gate?fmt=json&secs={args.secs}", method="POST")

    stop = threading.Event()
    http_counter, https_counter = Counter(), Counter()
    threads = [threading.Thread(target=http_client, args=(host, port, stop, http_counter)) for _ in range(args.http_clients)]
    if not args.no_https:
        threads.append(threading.Thread(target=https_client, args=(host, args.ca, stop, https_counter, args.reconnect_every)))
    for t in threads:
        t.start()

    samples = []
    rebooted = False
    last_uptime = before["uptime_ms"]
    deadline = time.monotonic() + args.secs
    while time.monotonic() < deadline:
        time.sleep(10)
        try:
            sys_info = get_json(host, port, "/api/v1/sys?fmt=json")
        except Exception as e:  # noqa: BLE001
            print(f"  sys sample failed: {type(e).__name__}")
            continue
        if sys_info["uptime_ms"] < last_uptime:
            rebooted = True
            print(f"  REBOOT detected: reset {sys_info['reset_reason']}")
        last_uptime = sys_info["uptime_ms"]
        heap, net = sys_info["heap"], sys_info["net"]
        samples.append(dict(t=round(sys_info["uptime_ms"] / 1000), free=heap["internal_free"], largest=heap["internal_largest"],
                            min=heap["internal_min"], tls_handshakes=net["tls_handshakes"], tls_failures=net["tls_failures"],
                            tls_open=net["tls_open"], http_open=net["http_open"], rejected=net["rejected"]))
        s = samples[-1]
        print(f"  t={s['t']}s free={s['free']:,} largest={s['largest']:,} min={s['min']:,} tls={s['tls_handshakes']}/{s['tls_failures']}fail http_open={s['http_open']}")
    stop.set()
    for t in threads:
        t.join(timeout=20)

    gate = get_json(host, port, "/api/v1/gate?fmt=json")
    after = get_json(host, port, "/api/v1/sys?fmt=json")
    log = http.client.HTTPConnection(host, port, timeout=10)
    log.request("GET", "/api/v1/log.txt")
    log_text = log.getresponse().read().decode("utf-8", "replace")
    alloc_failures = [line for line in log_text.splitlines() if "alloc" in line.lower() and ("fail" in line.lower() or "err" in line.lower())]

    smallest_largest = min((s["largest"] for s in samples), default=0)
    outbound_ok = all(t["ok"] > 0 for t in gate["targets"])
    outbound_failed = sum(t["failed"] for t in gate["targets"])
    result = dict(
        board=dict(build=after["build"], chip=after["chip"], reset_reason=after["reset_reason"]),
        secs=args.secs,
        http=http_counter.summary(),
        https=None if args.no_https else https_counter.summary(),
        outbound=gate["targets"],
        heap_min_ever=after["heap"]["internal_min"],
        smallest_largest_block=smallest_largest,
        rebooted=rebooted,
        alloc_failure_lines=alloc_failures[-10:],
        samples=samples,
    )
    passed = not rebooted and not alloc_failures and outbound_ok and smallest_largest >= MIN_LARGEST
    result["passed"] = passed
    print(json.dumps({k: v for k, v in result.items() if k != "samples"}, indent=2))
    print(f"Outbound failures: {outbound_failed}")
    print("GATE PASSED" if passed else "GATE FAILED")
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(result, f, indent=2)


if __name__ == "__main__":
    main()
