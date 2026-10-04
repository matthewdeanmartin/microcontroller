"""Pretend to be a few household boards pushing metrics.

    uv run writer --url http://127.0.0.1:8080 --token hm_... --backfill 6h
    uv run writer --url https://housemetrics.local --ca ../apps/housemetrics/certs/household-ca.crt \
        --token hm_... --format protobuf --live

--backfill writes that much history at once (batched); --live then keeps
sending one sample per series every --every seconds, like a real board.
--format picks the request body: influx (line protocol, the easy one for
MicroPython), json, msgpack, cbor or protobuf (a WriteBatch message).
"""
import argparse
import json
import math
import random
import ssl
import struct
import sys
import time
import urllib.request

import cbor2
import msgpack

SENSORS = [
    ("temp", "room=attic", "value", lambda t, r: 21 + 6 * math.sin(t / 43200 * math.pi) + r.gauss(0, 0.2), 1),
    ("temp", "room=cellar", "value", lambda t, r: 14 + 1.5 * math.sin(t / 43200 * math.pi) + r.gauss(0, 0.1), 1),
    ("humidity", "room=attic", "value", lambda t, r: 45 + 10 * math.cos(t / 30000) + r.gauss(0, 1), 0),
    ("power", "circuit=kitchen", "watts", lambda t, r: max(0, 120 + (900 if (t // 600) % 7 == 0 else 0) + r.gauss(0, 15)), 0),
    ("door", "name=front", "open", lambda t, r: 1.0 if (t // 900) % 11 == 0 else 0.0, 0),
]


def varint(n: int) -> bytes:
    out = bytearray()
    while n >= 0x80:
        out.append(n & 0x7F | 0x80)
        n >>= 7
    out.append(n)
    return bytes(out)


def pb_field(tag: int, wire: int, payload: bytes) -> bytes:
    return varint(tag << 3 | wire) + payload


def pb_bytes(tag: int, data: bytes) -> bytes:
    return pb_field(tag, 2, varint(len(data)) + data)


def protobuf_batch(samples: list[dict]) -> bytes:
    """WriteBatch { repeated Sample samples = 1 } by hand: protobuf needs no library."""
    body = bytearray()
    for s in samples:
        m = pb_bytes(1, s["m"].encode()) + pb_bytes(2, s["tags"].encode()) + pb_bytes(3, s["f"].encode())
        m += pb_field(4, 0, varint(s["t"])) + pb_field(5, 1, struct.pack("<d", s["v"]))
        body += pb_bytes(1, m)
    return bytes(body)


def encode(fmt: str, samples: list[dict]) -> tuple[bytes, str]:
    if fmt == "influx":
        lines = [f"{s['m']},{s['tags']} {s['f']}={s['v']} {s['t']}" for s in samples]
        return "\n".join(lines).encode(), "text/plain"
    batch = {"samples": samples}
    if fmt == "json":
        return json.dumps(batch).encode(), "application/json"
    if fmt == "msgpack":
        return msgpack.packb(batch), "application/msgpack"
    if fmt == "cbor":
        return cbor2.dumps(batch), "application/cbor"
    if fmt == "protobuf":
        return protobuf_batch(samples), "application/x-protobuf"
    raise SystemExit(f"unknown format {fmt}")


def samples_at(t_ms: int, rng: random.Random) -> list[dict]:
    out = []
    for m, tags, field, fn, decimals in SENSORS:
        v = round(fn(t_ms / 1000, rng), decimals)
        out.append({"m": m, "tags": tags, "f": field, "t": t_ms, "v": float(v)})
    return out


def post(url: str, token: str, body: bytes, content_type: str, context) -> dict:
    query = "?precision=ms" if content_type == "text/plain" else ""
    req = urllib.request.Request(
        url + "/api/v1/write" + query,
        data=body,
        method="POST",
        headers={"Authorization": f"Bearer {token}", "Content-Type": content_type, "Accept": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=15, context=context) as r:
        return json.loads(r.read())


def duration(text: str) -> int:
    units = {"s": 1, "m": 60, "h": 3600, "d": 86400}
    return int(float(text[:-1]) * units[text[-1]]) if text[-1] in units else int(text)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", default="http://127.0.0.1:8080")
    parser.add_argument("--token", required=True, help="device token (or the admin password)")
    parser.add_argument("--format", default="influx", choices=["influx", "json", "msgpack", "cbor", "protobuf"])
    parser.add_argument("--ca", help="CA certificate for https:// URLs")
    parser.add_argument("--backfill", default="0", help="history to write first, e.g. 6h")
    parser.add_argument("--every", type=float, default=10, help="seconds between samples")
    parser.add_argument("--batch", type=int, default=200, help="samples per request when backfilling")
    parser.add_argument("--live", action="store_true", help="keep sending after the backfill")
    args = parser.parse_args()
    context = ssl.create_default_context(cafile=args.ca) if args.ca else None
    rng = random.Random(7)
    now = int(time.time() * 1000)
    step = int(args.every * 1000)
    history = duration(args.backfill) * 1000
    pending: list[dict] = []
    sent = 0
    for t in range(now - history, now, step):
        pending.extend(samples_at(t, rng))
        if len(pending) >= args.batch:
            body, ct = encode(args.format, pending)
            result = post(args.url, args.token, body, ct, context)
            sent += result["accepted"]
            if result["rejected"]:
                print("rejected:", result["errors"], file=sys.stderr)
            pending = []
    if pending:
        body, ct = encode(args.format, pending)
        sent += post(args.url, args.token, body, ct, context)["accepted"]
    print(f"backfilled {sent} samples as {args.format}")
    while args.live:
        time.sleep(args.every)
        body, ct = encode(args.format, samples_at(int(time.time() * 1000), rng))
        result = post(args.url, args.token, body, ct, context)
        print(f"{time.strftime('%H:%M:%S')} accepted {result['accepted']} ({len(body)} bytes {args.format})")


if __name__ == "__main__":
    main()
