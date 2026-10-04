# Is serialization worth thinking about?

The question behind miniframework: NanaCoin and mastomini send JSON. Is that
a mistake on a 2 MiB board, or is the cost of a request dominated by
everything else (Wi-Fi round trips, TLS handshakes) so the format is a
rounding error?

Short answer, from measurements on these boards and this framework:

1. **Connection setup dominates everything for small responses.** A full
   TLS handshake on an ESP32 costs about **1 second** (mastomini measured
   0.9–1.5 s on an S3 at 240 MHz; it is almost all ECDHE). A resumed
   handshake costs 13–50 ms. Encoding a typical response costs **0.01–1 ms**.
   For anything under a few KiB, the format is noise next to whether the
   client reused its connection.
2. **The layout of the data matters more than the codec.** The same 360
   time-series points: JSON as rows is 10.9 KB, JSON as columns (`t0` +
   deltas + values) is 4.5 KB, smaller than *any* binary format as rows
   (protobuf rows 6.5 KB). Pick columns for pages of uniform data, then
   pick a codec.
3. **Bytes start to matter past the TCP send window.** Up to ~1.4 KB fits
   one segment, up to ~5.7 KB fits the board's send buffer
   (`CONFIG_LWIP_TCP_SND_BUF_DEFAULT=5760`); beyond that every extra window
   waits for an ACK, a Wi-Fi round trip of several ms each. That is where a
   2× smaller format can be worth a few to tens of ms.
4. **Dynamic gzip is the most expensive option on the board,** not for CPU
   alone: a deflate compressor holds ~300 KiB while it runs. It gives the
   smallest bytes for large repetitive payloads (rows → ~6× smaller) but
   costs 20+ ms of server time per 100 KB even on a desktop, and RAM a
   2 MiB board would rather spend on connections. Pre-compressed static
   files cost nothing and the framework always uses them.
5. **What actually limited NanaCoin to ~8 users was memory, not encoding
   speed:** concurrent requests each holding a JSON object graph plus a
   buffer. miniframework encodes every format by streaming straight into
   one reusable buffer, so the format choice no longer changes peak memory
   much; the response size limit does.

So: for a single request on a new connection, the format is not where the
time goes. **Connection reuse** (keep-alive + session tickets) comes first,
then **shape** (columns), then the format. But on this FPU-less board with a
warm connection, JSON is measurably slower from ~100 rows (2–3× at 100 rows,
+230 ms at 1000), from both float formatting and bytes. CBOR with integer
keys or protobuf for large pages; JSON for small objects and debugging.

## How the formats compare (same data, same streaming encoder design)

All five encoders are hand-written streaming writers behind one `Writer`
trait, so differences come from the formats, not from library quality.

| Format | Self-describing | Floats | Typical size vs JSON (rows) | Notes |
|---|---|---|---|---|
| JSON | yes | shortest decimal text | 1× | Native `JSON.parse` in browsers is very fast |
| MessagePack | yes | f32 when exact, else f64 | ~0.65× | Field names still sent per row |
| CBOR | yes | f32 when exact, else f64 | ~0.65× | Same as MessagePack in practice |
| CBOR int keys | no (needs schema) | f32 when exact, else f64 | ~0.45× | Keys become 1-byte integers |
| Protobuf | no (needs schema) | always f64 for `double` | ~0.4× | Omits default values; packed arrays |

Columns change the picture: field names vanish from every format, so
the self-describing formats catch up, and protobuf can lose (its `double` is
always 8 bytes where CBOR/MessagePack use 4 bytes when the value is an exact
float32, e.g. 21.5).

## Measurements

### Desktop (Windows, localhost, debug build) — Format Lab, 2026-10-02

Not representative of a board's absolute numbers; shows the relative
costs. Medians, ms.

| Payload | Format | Wire bytes | Server encode | Fetch | Decode (browser) |
|---|---|---|---|---|---|
| System info | JSON | 677 | 0.02 | 1.4 | 0.00 |
| System info | Protobuf | 130 | 0.02 | 2.6 | 0.00 |
| 100 rows | JSON | 9.6 KiB | 0.52 | 2.2 | 0.0 |
| 100 rows | MessagePack | 6.6 KiB | 0.10 | 1.2 | 0.0 |
| 100 rows | JSON + gzip | 1.7 KiB | 2.33 | 5.2 | 0.0 |
| 1000 rows | JSON | 96 KiB | 4.34 | 11.4 | 0.4 |
| 1000 rows | CBOR int keys | 45 KiB | 1.27 | 4.7 | 0.9 |
| 1000 rows | Protobuf | 40 KiB | 1.09 | 4.8 | 0.4 |
| 1000 rows | JSON + gzip | 15 KiB | 21.65 | 23.6 | 0.3 |

### Raw time series (360 regular points, desktop), bytes

| | rows | columns | rows + gzip | columns + gzip |
|---|---|---|---|---|
| JSON | 10,913 | 4,450 | 1,357 | 202 |
| MessagePack | 6,923 | 2,974 | 1,772 | 200 |
| CBOR | 6,923 | 2,974 | 1,770 | 192 |
| CBOR int keys | 6,172 | 2,944 | 1,722 | 171 |
| Protobuf | 6,524 | 3,656 | 1,660 | 176 |

The store itself keeps those 360 points in 390 bytes (Gorilla
compression); realistic noisy sensor data averages 2.7 bytes per point.

### ESP32-S2 board (housemetrics, 2026-10-02)

ESP32-S2 Mini at 240 MHz (single core, **no floating-point unit**), Wi-Fi
at −78 dBm, HTTPS, `make bench` (`bench/fmtbench`), medians of 3, ms.
Connection setup = TCP connect + TLS.

| Mode | Payload | Format | Wire bytes | Setup | TTFB | Body | **Total** |
|---|---|---|---|---|---|---|---|
| warm | system info | JSON | 791 | 0 | 21.7 | 0.1 | **21.8** |
| warm | system info | CBOR int | 281 | 0 | 34.0 | 0.0 | **34.0** |
| warm | system info | protobuf | 254 | 0 | 23.3 | 0.1 | **23.4** |
| warm | 100 rows | JSON | 9,785 | 0 | 41.9 | 62.7 | **104.6** |
| warm | 100 rows | CBOR int | 4,528 | 0 | 18.1 | 18.1 | **36.2** |
| warm | 100 rows | protobuf | 3,962 | 0 | 32.5 | 25.2 | **59.0** |
| warm | 1000 rows (columns) | JSON | 52,769 | 0 | 169.7 | 387.4 | **570.9** |
| warm | 1000 rows (columns) | CBOR int | 38,210 | 0 | 72.2 | 271.3 | **343.5** |
| warm | 1000 rows (columns) | protobuf | 34,540 | 0 | 73.6 | 270.2 | **343.8** |
| resumed TLS | system info | JSON | 791 | 77 | 30.6 | 0 | **119.7** |
| resumed TLS | 1000 rows (columns) | protobuf | 34,540 | 90 | 86.9 | 231.5 | **411.5** |
| fresh TLS | system info | JSON | 806 | 930 | 40.9 | 0 | **972.8** |
| fresh TLS | 100 rows | protobuf | 3,962 | 940 | 17.9 | 26.4 | **989.6** |
| fresh TLS | 1000 rows (columns) | JSON | 52,769 | 951 | 150.3 | 456.6 | **1570.9** |

Raw results: `.local/bench-s2-https.json` (all 81 measurements).

What it says:

- **A fresh TLS handshake (~930 ms) dwarfs every format difference** up to
  100 rows (spread 12–68 ms). On a fresh connection, the format is noise.
  A resumed handshake costs ~75 ms; a kept-alive connection nothing.
- **On a warm connection the format does matter from ~100 rows:** JSON is
  2–3× slower than CBOR int keys / protobuf at 100 rows, and 230 ms slower
  at 1000 rows. Two causes, both visible above:
  - *Server time (TTFB):* JSON's encode is ~100 ms slower for 1000 rows.
    The S2 has no FPU, so formatting floats as shortest decimal text is
    software double arithmetic; binary formats copy the bits.
  - *Bytes:* the board sends ~135 KB/s over TLS at this signal, so 18 KB
    more JSON costs ~120 ms.
- **For small objects it is a tie** (system info: 22–34 ms in every format).
- Protobuf and CBOR with integer keys tie for large payloads; CBOR int keys
  was fastest at 100 rows.

Throughput history: the first firmware slept a 10 ms FreeRTOS tick after
every 1 KiB written, capping the board near 57 KB/s and exaggerating every
byte's cost (and so JSON's). Fixed (1 kHz tick, 16 KiB per client per turn):
~135 KB/s.

## Reproducing

- Browser: open the board's site, **Format lab**, Run. "Download results"
  saves the raw samples as JSON.
- Connection modes (fresh TLS / resumed TLS / warm keep-alive), which a
  browser cannot control: `cd bench && uv run fmtbench --url ... --ca ...`.
