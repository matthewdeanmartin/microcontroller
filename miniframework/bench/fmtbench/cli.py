"""Where does a request's time go: connection setup or serialization?

    uv run fmtbench --url https://housemetrics.local --ca ../apps/housemetrics/certs/household-ca.crt
    uv run fmtbench --url http://127.0.0.1:8080 --repeats 20

For every connection mode, payload, format and gzip setting it times:

  connect  TCP connect
  tls      TLS handshake (full, or resumed with a session ticket)
  ttfb     request sent -> first response byte (server work + one round trip)
  body     first byte -> last byte (bytes on the wire)
  total    all of the above

Connection modes (a browser cannot choose these, which is why this tool exists):

  warm     one kept-alive connection for every request
  resumed  a new connection per request, TLS resumed from a session ticket
  fresh    a new connection per request, full TLS handshake

Plain http:// URLs run warm and fresh (no TLS). Results print as a table of
medians and are saved as JSON (--out).
"""
import argparse
import json
import socket
import ssl
import statistics
import sys
import time
import urllib.parse
import zlib
from dataclasses import asdict, dataclass

FORMATS = ["json", "msgpack", "cbor", "cbor-int", "protobuf"]
PAYLOADS = {
    "sys": "/api/v1/sys",
    "rows10": "/api/v1/bench/rows?n=10",
    "rows100": "/api/v1/bench/rows?n=100",
    "rows1000": "/api/v1/bench/rows?n=1000",
    "cols1000": "/api/v1/bench/rows?n=1000&shape=columns",
    "raw": "/api/v1/query?ids={id}&from=0&raw=1&limit=2000",
    "rawcols": "/api/v1/query?ids={id}&from=0&raw=1&limit=2000&shape=columns",
}


@dataclass
class Timing:
    mode: str
    payload: str
    format: str
    gzip: bool
    connect: float
    tls: float
    ttfb: float
    body: float
    total: float
    wire_bytes: int
    raw_bytes: int
    server_enc: float
    resumed: bool


class Connection:
    """One HTTP/1.1 connection on a raw socket, so each phase can be timed."""

    def __init__(self, host: str, port: int, context: ssl.SSLContext | None, session=None):
        self.connect_ms = 0.0
        self.tls_ms = 0.0
        self.resumed = False
        t0 = time.perf_counter()
        raw = socket.create_connection((host, port), timeout=20)
        raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        t1 = time.perf_counter()
        self.connect_ms = (t1 - t0) * 1000
        if context:
            sock = context.wrap_socket(raw, server_hostname=host, session=session, do_handshake_on_connect=False)
            sock.do_handshake()
            self.tls_ms = (time.perf_counter() - t1) * 1000
            self.resumed = sock.session_reused
            self.sock = sock
        else:
            self.sock = raw
        self.host = host
        self.buffer = b""

    @property
    def session(self):
        return getattr(self.sock, "session", None)

    def request(self, path: str, gzip: bool) -> tuple[float, float, dict, bytes]:
        headers = (
            f"GET {path} HTTP/1.1\r\nHost: {self.host}\r\nAccept-Encoding: {'gzip' if gzip else 'identity'}\r\n"
            "Connection: keep-alive\r\n\r\n"
        )
        t0 = time.perf_counter()
        self.sock.sendall(headers.encode())
        first = None
        while b"\r\n\r\n" not in self.buffer:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("server closed the connection")
            first = first or time.perf_counter()
            self.buffer += chunk
        head, self.buffer = self.buffer.split(b"\r\n\r\n", 1)
        lines = head.decode("latin-1").split("\r\n")
        status = int(lines[0].split()[1])
        hdrs = {k.lower(): v.strip() for k, v in (l.split(":", 1) for l in lines[1:] if ":" in l)}
        length = int(hdrs.get("content-length", "0"))
        while len(self.buffer) < length:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("server closed the connection mid-body")
            self.buffer += chunk
        body, self.buffer = self.buffer[:length], self.buffer[length:]
        t2 = time.perf_counter()
        if status != 200:
            raise RuntimeError(f"{path}: HTTP {status}: {body[:200]!r}")
        return (first - t0) * 1000, (t2 - first) * 1000, hdrs, body

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


def server_enc(header: str) -> float:
    for part in header.split(","):
        name, *params = part.strip().split(";")
        if name == "enc":
            for p in params:
                if p.strip().startswith("dur="):
                    return float(p.strip()[4:])
    return 0.0


def pick_series(host: str, port: int, context) -> int | None:
    conn = Connection(host, port, context)
    try:
        _, _, _, body = conn.request("/api/v1/series?fmt=json", False)
    finally:
        conn.close()
    series = json.loads(body)["series"]
    if not series:
        return None
    return max(series, key=lambda s: s["raw_points"])["id"]


def run(args) -> list[Timing]:
    url = urllib.parse.urlparse(args.url)
    secure = url.scheme == "https"
    host = url.hostname
    port = url.port or (443 if secure else 80)
    context = None
    if secure:
        context = ssl.create_default_context(cafile=args.ca) if args.ca else ssl.create_default_context()
        if args.insecure:
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
    payloads = [p for p in args.payloads.split(",") if p]
    formats = FORMATS if args.formats == "all" else args.formats.split(",")
    gzips = {"off": [False], "on": [True], "both": [False, True]}[args.gzip]
    modes = ["warm", "resumed", "fresh"] if secure else ["warm", "fresh"]
    if args.modes:
        modes = [m for m in modes if m in args.modes.split(",")]
    series = pick_series(host, port, context) if any("{id}" in PAYLOADS[p] for p in payloads) else None
    results: list[Timing] = []
    for mode in modes:
        warm = Connection(host, port, context) if mode == "warm" else None
        session = None
        if mode == "resumed":
            first = Connection(host, port, context)
            first.request("/api/v1/sys", False)  # TLS 1.3 tickets arrive after the handshake
            session = first.session
            first.close()
        for round_ in range(args.repeats + 1):
            for payload in payloads:
                template = PAYLOADS[payload]
                if "{id}" in template and series is None:
                    continue
                for fmt in formats:
                    for gz in gzips:
                        path = template.format(id=series)
                        path += ("&" if "?" in path else "?") + f"fmt={fmt}" + ("&gz=1" if gz else "")
                        conn = warm or Connection(host, port, context, session=session)
                        try:
                            ttfb, body_ms, hdrs, body = conn.request(path, gz)
                        except (ConnectionError, OSError):
                            if warm:  # the server closed an idle connection: reconnect once
                                warm = conn = Connection(host, port, context)
                                ttfb, body_ms, hdrs, body = conn.request(path, gz)
                            else:
                                raise
                        if mode == "resumed" and conn.session is not None:
                            session = conn.session
                        raw = len(zlib.decompress(body, 31)) if hdrs.get("content-encoding") == "gzip" else len(body)
                        timing = Timing(
                            mode=mode,
                            payload=payload,
                            format=fmt,
                            gzip=gz,
                            connect=0.0 if warm else conn.connect_ms,
                            tls=0.0 if warm else conn.tls_ms,
                            ttfb=ttfb,
                            body=body_ms,
                            total=(0.0 if warm else conn.connect_ms + conn.tls_ms) + ttfb + body_ms,
                            wire_bytes=len(body),
                            raw_bytes=raw,
                            server_enc=server_enc(hdrs.get("server-timing", "")),
                            resumed=conn.resumed,
                        )
                        if round_ > 0:  # round 0 warms caches and the server
                            results.append(timing)
                        if not warm:
                            conn.close()
                        print(".", end="", flush=True, file=sys.stderr)
        if warm:
            warm.close()
    print(file=sys.stderr)
    return results


def report(results: list[Timing]) -> None:
    groups: dict[tuple, list[Timing]] = {}
    for r in results:
        groups.setdefault((r.mode, r.payload, r.format, r.gzip), []).append(r)
    med = lambda rs, f: statistics.median(getattr(r, f) for r in rs)  # noqa: E731
    print(f"{'mode':8} {'payload':9} {'format':12} {'wire':>8} {'connect':>8} {'tls':>8} {'ttfb':>8} {'body':>8} {'total':>8}  (ms, medians)")
    for (mode, payload, fmt, gz), rs in groups.items():
        name = fmt + ("+gz" if gz else "")
        resumed = sum(r.resumed for r in rs)
        note = f"  resumed {resumed}/{len(rs)}" if mode == "resumed" else ""
        print(
            f"{mode:8} {payload:9} {name:12} {int(med(rs, 'wire_bytes')):>8} {med(rs, 'connect'):8.1f} {med(rs, 'tls'):8.1f} "
            f"{med(rs, 'ttfb'):8.1f} {med(rs, 'body'):8.1f} {med(rs, 'total'):8.1f}{note}"
        )
    print()
    by_mode: dict[str, list[Timing]] = {}
    for r in results:
        by_mode.setdefault(r.mode, []).append(r)
    for mode, rs in by_mode.items():
        setup = statistics.median(r.connect + r.tls for r in rs)
        print(f"{mode:8} median connection setup per request: {setup:7.1f} ms")
    payloads = sorted({r.payload for r in results})
    for p in payloads:
        warm = [rs for (m, pl, _, _), rs in groups.items() if m == "warm" and pl == p]
        if not warm:
            continue
        totals = [statistics.median(r.total for r in rs) for rs in warm]
        print(f"{p:9} spread between fastest and slowest format (warm): {max(totals) - min(totals):7.1f} ms")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", required=True)
    parser.add_argument("--ca", help="CA certificate (household-ca.crt) for https")
    parser.add_argument("--insecure", action="store_true", help="skip certificate checks")
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--payloads", default="sys,rows100,rows1000,cols1000,raw,rawcols")
    parser.add_argument("--formats", default="all")
    parser.add_argument("--gzip", choices=["off", "on", "both"], default="both")
    parser.add_argument("--modes", default="", help="subset of warm,resumed,fresh")
    parser.add_argument("--out", default="fmtbench-results.json")
    args = parser.parse_args()
    results = run(args)
    report(results)
    with open(args.out, "w") as f:
        json.dump({"url": args.url, "when": time.strftime("%Y-%m-%dT%H:%M:%S"), "results": [asdict(r) for r in results]}, f, indent=1)
    print(f"\nSaved {len(results)} measurements to {args.out}")


if __name__ == "__main__":
    main()
