"""A cold page load, the way a browser does it: HTTP/1.1 or HTTP/2?

    uv run pageload --url https://minicloud.local --ca ../../../mastomini/minicloud_rs/certs/household-ca.crt
    uv run pageload --url http://127.0.0.1:8090 --paths / /main-HUGMXD6Z.js /styles-HROJ5BSP.css /api/status

Fetches every path once, starting from no connections at all (no TLS
session tickets either), and reports when the last byte arrived:

  h1   up to --connections HTTP/1.1 connections at once (browsers use 6),
       each fetching paths in turn over keep-alive
  h2   one HTTP/2 connection, every path requested at once (multiplexed);
       over https it is negotiated with ALPN, over http it is cleartext
       prior knowledge (h2c), which only miniframework built with `http2`
       accepts

The HTTP/2 client is hyper-h2, an implementation independent of the board's,
so a run is also a conformance check. Without --paths, the paths are the
site's index page plus every script and stylesheet it links.
"""
import argparse
import re
import socket
import ssl
import statistics
import threading
import time
import urllib.parse

import h2.config
import h2.connection
import h2.events


ADDRESS = None  # --address: connect here, keep the URL's name for TLS and Host


def connect(url, ca, alpn):
    """A socket to the URL's host, TLS with `alpn` for https. Returns
    (socket, seconds for TCP, seconds for TLS, protocol chosen)."""
    started = time.perf_counter()
    host = ADDRESS or url.hostname
    raw = socket.create_connection((host, url.port or (443 if url.scheme == "https" else 80)), timeout=30)
    raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    tcp = time.perf_counter() - started
    if url.scheme != "https":
        return raw, tcp, 0.0, None
    context = ssl.create_default_context(cafile=ca) if ca else ssl.create_default_context()
    context.set_alpn_protocols(alpn)
    started = time.perf_counter()
    tls = context.wrap_socket(raw, server_hostname=url.hostname)
    return tls, tcp, time.perf_counter() - started, tls.selected_alpn_protocol()


def h1_fetch(sock, host, path):
    """One keep-alive GET; returns (status, body bytes)."""
    sock.sendall(f"GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: gzip\r\n\r\n".encode())
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = sock.recv(65536)
        if not chunk:
            raise ConnectionError(f"{path}: connection closed in the headers")
        data += chunk
    head, body = data.split(b"\r\n\r\n", 1)
    status = int(head.split(b" ", 2)[1])
    length = 0
    for line in head.split(b"\r\n")[1:]:
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            length = int(value.strip())
    while len(body) < length:
        chunk = sock.recv(65536)
        if not chunk:
            raise ConnectionError(f"{path}: connection closed in the body")
        body += chunk
    return status, len(body)


def run_h1(url, ca, paths, connections):
    queue = list(paths)
    lock = threading.Lock()
    setups, results, errors = [], {}, []

    def worker():
        sock = None
        try:
            while True:
                with lock:
                    if not queue:
                        return
                    path = queue.pop(0)
                if sock is None:
                    sock, tcp, tls, _ = connect(url, ca, ["http/1.1"])
                    setups.append(tcp + tls)
                started = time.perf_counter()
                status, size = h1_fetch(sock, url.hostname, path)
                results[path] = (status, size, time.perf_counter() - started)
        except Exception as error:  # noqa: BLE001 - report every failure
            errors.append(f"{error}")
        finally:
            if sock is not None:
                sock.close()

    started = time.perf_counter()
    threads = [threading.Thread(target=worker) for _ in range(min(connections, len(paths)))]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return time.perf_counter() - started, setups, results, errors


def run_h2(url, ca, paths):
    started = time.perf_counter()
    sock, tcp, tls, chosen = connect(url, ca, ["h2", "http/1.1"])
    if url.scheme == "https" and chosen != "h2":
        sock.close()
        raise SystemExit(f"the server chose {chosen!r}, not h2: built without the `http2` feature?")
    conn = h2.connection.H2Connection(h2.config.H2Configuration(client_side=True, header_encoding="utf-8"))
    conn.initiate_connection()
    streams = {}
    authority = url.netloc
    for path in paths:
        stream = conn.get_next_available_stream_id()
        conn.send_headers(
            stream,
            [(":method", "GET"), (":scheme", url.scheme), (":authority", authority), (":path", path),
             ("accept-encoding", "gzip")],
            end_stream=True,
        )
        streams[stream] = {"path": path, "status": 0, "size": 0, "done": False, "sent": time.perf_counter()}
    sock.sendall(conn.data_to_send())
    results, errors = {}, []
    while not all(s["done"] for s in streams.values()):
        data = sock.recv(65536)
        if not data:
            errors.append("connection closed")
            break
        for event in conn.receive_data(data):
            if isinstance(event, h2.events.ResponseReceived):
                streams[event.stream_id]["status"] = int(dict(event.headers)[":status"])
            elif isinstance(event, h2.events.DataReceived):
                streams[event.stream_id]["size"] += len(event.data)
                conn.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
            elif isinstance(event, h2.events.StreamEnded):
                s = streams[event.stream_id]
                s["done"] = True
                results[s["path"]] = (s["status"], s["size"], time.perf_counter() - s["sent"])
            elif isinstance(event, h2.events.StreamReset):
                s = streams[event.stream_id]
                s["done"] = True
                errors.append(f"{s['path']}: reset ({event.error_code})")
            elif isinstance(event, h2.events.ConnectionTerminated):
                errors.append(f"GOAWAY {event.error_code}")
                for s in streams.values():
                    s["done"] = True
        outgoing = conn.data_to_send()
        if outgoing:
            sock.sendall(outgoing)
    total = time.perf_counter() - started
    sock.close()
    return total, [tcp + tls], results, errors


def discover(url, ca):
    """The index page plus the scripts and stylesheets it references."""
    sock, _, _, _ = connect(url, ca, ["http/1.1"])
    try:
        sock.sendall(f"GET / HTTP/1.1\r\nHost: {url.hostname}\r\nConnection: close\r\n\r\n".encode())
        data = b""
        while chunk := sock.recv(65536):
            data += chunk
    finally:
        sock.close()
    head, _, body = data.partition(b"\r\n\r\n")
    if b"content-encoding: gzip" in head.lower():
        # A gzip-only board answers gzip even when not asked.
        import gzip

        body = gzip.decompress(body)
    html = body.decode("utf-8", "replace")
    found = re.findall(r'(?:src|href)="(/?[^"]+\.(?:js|css))"', html)
    return ["/"] + ["/" + p.lstrip("/") for p in dict.fromkeys(found)]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", required=True)
    parser.add_argument("--ca", help="household CA (PEM) for https")
    parser.add_argument("--paths", nargs="*", help="default: the index page and what it links")
    parser.add_argument("--connections", type=int, default=6, help="HTTP/1.1 connections at once (browsers: 6)")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--modes", nargs="*", default=["h1", "h2"], choices=["h1", "h2"])
    parser.add_argument("--address", help="IP to connect to (skips slow .local lookups); TLS still checks the URL's name")
    args = parser.parse_args()
    global ADDRESS
    ADDRESS = args.address
    url = urllib.parse.urlsplit(args.url)
    paths = args.paths or discover(url, args.ca)
    print(f"{len(paths)} paths from {args.url}: {' '.join(paths)}")
    for mode in args.modes:
        totals, setups, failures = [], [], []
        last = {}
        for _ in range(args.repeats):
            if mode == "h1":
                total, setup, results, errors = run_h1(url, args.ca, paths, args.connections)
            else:
                total, setup, results, errors = run_h2(url, args.ca, paths)
            totals.append(total)
            setups.extend(setup)
            failures.extend(errors)
            last = results
            time.sleep(0.5)
        if not setups:
            raise SystemExit(f"{mode}: no connection succeeded: {sorted(set(failures))[:3]}")
        statuses = sorted({r[0] for r in last.values()})
        size = sum(r[1] for r in last.values())
        print(
            f"{mode}: page in {statistics.median(totals) * 1000:7.1f} ms (median of {len(totals)}), "
            f"{len(setups) // len(totals)} connection(s) at {statistics.median(setups) * 1000:.1f} ms setup each, "
            f"{size} bytes, statuses {statuses}"
            + (f", FAILURES: {failures}" if failures else "")
        )


if __name__ == "__main__":
    main()
