# Initial execution — 2026-09-18

## Physical board: setup itself became unresponsive

Target: `http://192.168.1.158`. No firmware edits, flash, or reset were made.
The board initially reported an unprovisioned household and 512 ledger slots.

| Request, in order | Result | Header free heap | GC count |
|---|---:|---:|---:|
| Status | 200 | 9,696 | 5 |
| Provision Nana | 201 | 9,232 | 6 |
| Authorize Nana | 200 | 7,184 | 30 |
| Exchange Nana token | 200 | 5,904 | 58 |
| List users | 200 | 5,312 | 59 |
| Create first member | 201 | 5,152 | 60 |
| Authorize member | ConnectionError after 19.2 seconds | unavailable | unavailable |

The six successful requests took about two seconds together. Setup was
sequential: this did not require concurrent users. Later diagnostic connection
attempts timed out too, including the readiness check for a read-only Locust
run. That run did not start sending load. The board had already been running
before this experiment; this is not a fresh-boot benchmark.

The heap decline and rapid collection count increase make memory pressure a
plausible explanation. Without serial evidence or a surviving crash record,
this does **not** establish OOM, a leak, fragmentation, or a thermal cause.
No ESP32 serial port was attached on this PC (only COM3 was visible).
The user was asked to reboot before a read-only baseline.

Evidence: `reports/20260918-130858-prepare-a9cf/` and
`reports/20260918-131555-status-54fa/`. These are local, gitignored artifacts;
this note retains the result if reports are moved or removed.

## Harness validation, separate desktop target

Against an isolated desktop build at `http://127.0.0.1:18080`:

- All nine end-to-end assertions passed: balance changes, reversal, purchase
  retry, four concurrent issuance retries, authorization, history and invariants.
- All seven Locust workload profiles completed short two-user runs without
  request failures. A separate 1→2-user browsing ramp completed 120 requests
  without failures. These are harness checks, not ESP32 performance claims.
- A deliberate desktop-server shutdown verified three failed diagnostic
  probes stop the shape, preserve Locust HTML/CSV and raw evidence, and perform
  bounded recovery probes. It exposed a double-shutdown race in the initial
  harness; shutdown is now owned solely by the load shape, and the repeated
  fault test completed without that exception.
- The HTML overview was rendered and visually inspected in headless Edge.

Next board runs after reboot: public status baseline first; then fixture setup,
authenticated reads, browser bursts, writes, and authentication churn separately.
Keep fresh-boot and aged-board results separate. Do not assume the throughput
ceiling is I/O or thermals until the memory evidence supports that conclusion.

## Authentication allocation work: attached board, weak Wi-Fi (2026-09-18)

The owner confirms the attached board is four floors above the router. These
runs measure network plus application resilience; repeat near the router for
an application-focused comparison. Native USB logs are now available on COM9.

The first new firmware reserved the entire prior auth ceilings up front:
64 sessions / 16 codes / 64 failure counters, 9,088 bytes of slot storage.
This left about 4.2 KiB at startup. During the first status profile the serial
heap samples fell through 3,920, 2,400, 1,088, 1,376 and 1,248 bytes. Serial then
printed `fatal error: out of memory` and `abort called`. This is confirmed OOM,
not an inference from network timeouts. Five Locust requests were recorded;
the five serial connections also include the readiness/diagnostic traffic.

Evidence: `reports/20260918-135329-status-64fc/` and
`reports/20260918-auth-memory-hardware/serial.log`.

The revised board profile reserves 32 sessions / 8 codes / 16 failure counters
(3,776 bytes). Full pools refuse new logins; they never replace live sessions.
This leaves the HTTP workers, TCP pool and ledger capacities unchanged. The
second flash is a revised intended build, not an availability rollback.

The second profile passed 27 status requests (0 failures, median about 67 ms)
but then OOMed during provisioning after that warm-up:
`reports/20260918-135733-status-a051/` and `serial-v2.log` in the hardware folder.
The third image used fixed SHA compression scratch and got through provisioning,
Nana authorization and token exchange. It OOMed on the first member creation;
`reports/20260918-140713-prepare-1eae/` and `serial-v3.log` retain that evidence.
Nana authorize/token no longer caused the earlier 52-GC jump, but fixed headroom
was still insufficient. A fourth intended build saves 3,840 bytes by batching
the ledger invariant scratch calculation; transaction capacity remains 512.

## Final auth image and isolated verification

Final image size: 935,472 bytes. COM8 disappeared; flashing and hash verification
succeeded through the same board's native USB Serial/JTAG COM9. No older firmware
was restored. Boot reports 20,928 free bytes and the auth self-check reports:
`hash allocations 3 verify allocations 0 verify bytes 0`.

- Setup passed: `reports/20260918-142144-prepare-1981/`.
- E2E: `reports/20260918-142226-e2e-b57c/`. Transfer debit, reversal restoration,
  purchase retry identity and seller paid once all passed (four checks). The
  following four concurrent same-key `/admin/issue` requests produced two 201s
  and two connection failures/timeouts. Serial confirmed OOM. The last successful
  issuance header sampled only 3,168 free bytes. Remaining E2E checks did not run.
  Serial: `reports/20260918-auth-memory-hardware/serial-v5.log`.
- The same image was reset once for an **isolated authentication experiment**,
  not restored for availability. Setup passed again in
  `reports/20260918-142610-prepare-ebc7/`.
- Auth ramp: `reports/20260918-142612-auth-c8b6/`, 1 -> 2 -> 4 users, 20 seconds
  per stage, default 0.3-1 second journey waits, diagnostics every five seconds.
  100 authorization + 100 token + 100 logout requests, **zero failures**. Overall
  4.90 requests/s; p50 201 ms, p95 653 ms, p99 1,210 ms. Lowest sampled free heap
  1,440 bytes. No monitor anomalies or serial OOM in this isolated run. Serial
  ended around 8,864 free bytes after connection 330. These are different sample
  points than response headers, not competing measurements of the same instant.
  Serial: `reports/20260918-auth-memory-hardware/serial-auth-isolated.log`.

All runs were with the board attached four floors above the router. Repeat near
the router before treating latency or connection failures as application-only
limits. Authentication now has a measured zero-allocation verifier and a passing
short churn test; the server still has a reproducible concurrent-write OOM.
Next experiment: isolate four-request idempotent issuance, capture allocation
size/peak request ownership, and compare cold versus warm receipt caches.

The captured ROM boot log also printed an image SHA comparison warning before
entering the application. Esptool's flash-content verification succeeded and the
app booted; this warning is separate from the application's SHA implementation
(which has not started at that point). Its cause was not investigated here.

### Unexpected restart after the auth run

After the successful auth run ended at approximately 14:27:15 EDT, a later status
check found an unprovisioned household. Diagnostic uptime places the new boot at
approximately 14:27:38. The owner confirmed they had not touched the device.
This is an unexpected restart, with no captured cause; do not classify the auth
experiment as an endurance pass or attribute this restart to OOM without evidence.

The estimated boot time coincides with the 180-second serial watcher finishing.
However, an isolated native USB open/close check did not reproduce a reset:
uptime advanced from 228 to 229 to 232 seconds across open and close, then to 417
seconds after that Python process had exited. Evidence is in
`reports/20260918-auth-memory-hardware/serial-control-test.json`. The timing
correlation alone does not establish that the watcher caused the restart.

This firmware has no surviving crash record: `crashed:false` and `boots:0` after
a reboot cannot rule out a crash. A follow-up experiment must keep serial capture
and uptime polling running beyond workload completion, recording monitor shutdown
times separately, to capture either a panic or the next ROM reset banner. Keep
this unexplained restart separate from the serial-confirmed concurrent-write OOM.

## Near-router repeat (2026-09-18, 14:40 EDT)

The owner subsequently clarified that they had picked up the board during the
earlier session, then unplugged it from the development machine and moved it
four floors downstairs, near the router. Movement could interrupt the weak
office link; it does not by itself establish the cause of the earlier uptime
reset. This repeat uses independent power and HTTP telemetry only, with no USB
serial capture. Firmware was not changed or flashed for this repeat.

Preparation succeeded in `reports/20260918-144018-prepare-db9e/`. E2E in
`reports/20260918-144041-e2e-8984/` passed the same four transfer/reversal/purchase
checks, then became unresponsive at four concurrent same-key issuance requests.
All four requests failed: one ConnectTimeout and three ConnectionErrors. The
last successful response sampled 7,712 free bytes. The last diagnostic before
the burst reported uptime 71 seconds. Subsequent diagnostic probes also failed.

This reproduces the failing workload near the router, making weak office Wi-Fi
an insufficient explanation for the repeated failure. Unlike the earlier USB
run, this run cannot confirm OOM or a restart. No responses from the concurrent
writes means their commit status is unknown; remaining E2E checks did not run.
Independent before/after probes are retained in
`reports/20260918-near-router-observation.jsonl`. Auth/read stress awaits a
user-operated power cycle so it can be measured separately.

## Near-router auth and read sequence after power cycle (14:42–14:49 EDT)

The owner power-cycled the downstairs board. Preparation succeeded in
`reports/20260918-144245-prepare-6971/`. No firmware changes, USB connection or
further resets were made. Workloads below ran sequentially on this same boot,
with default 0.3–1 second journey waits and diagnostic polling every five seconds.

| Workload | Schedule | Requests / failures | p50 / p95 | Minimum sampled free heap |
|---|---|---|---|---|
| Auth | 1, 2, 4 users; 20 s each | 330 / 0 | 182 / 543 ms | 1,184 bytes |
| Status | 1, 2, 4, 8 users; 20 s each | 398 / 0 | 87 / 210 ms | 192 bytes |
| Browse | planned 1, 2, 4 users; stopped early | 85 / 35 | failures dominate later latency | 48 bytes |

Auth completed 110 full authorization/token/logout cycles. Seven post-auth
snapshots over 60 seconds showed uptime 131–191 seconds and free_now consistently
8,032 bytes. Status also passed, followed by seven idle snapshots over 60 seconds;
uptime continued to 356 seconds without a reset. These short, paced passes do not
establish endurance or maximum throughput. Reports:
`reports/20260918-144254-auth-5ecb/` and
`reports/20260918-144519-status-22b1/` include the idle snapshots.

Browse launches five overlapping reads per user (me, users, listings, status,
history). It completed 50 requests, then all five requests in the next burst
timed out. Last successful load response was 10.34 seconds into the run; the
first failed requests began around 11.31 seconds, still in the **one-user stage**.
The first diagnostic failure completed at 18.41 seconds, before the two-user
stage began at 20.41 seconds. Do not attribute the initial failure to two users.
The harness subsequently recorded 35 failures and stopped after three failed
diagnostic probes. Three recovery probes after load stopped also failed, the last
about 18 seconds after subprocess exit. Report:
`reports/20260918-144756-browse-bdb7/`.

The 48-byte free-heap sample strongly suggests memory pressure but cannot confirm
OOM without serial evidence. This is a cumulative-workload reproduction: browsing
started at uptime 357 seconds after auth/status traffic, with 5,200 free_now bytes.
Next diagnostic target is concurrent request memory and retained state after
earlier traffic. Compare a fresh-boot browse-only run, then isolate each of the
five read endpoints and their overlap. User count alone is insufficient: one
page-load user already makes five simultaneous requests, plus diagnostic traffic.

### Fresh-boot browse-only reproduction (14:52 EDT)

After the owner's next reboot, the first preparation probe timed out; a second
preparation succeeded (`reports/20260918-145239-prepare-8c22/`). Browse alone then
ran with one virtual user, five overlapping reads per journey, a planned 60-second
duration and five-second diagnostics. No auth/status load profiles preceded it.

`reports/20260918-145249-browse-a666/` recorded 65 successful requests followed by
20 failures. The last successful request completed 12.38 seconds into the run.
Minimum sampled free heap was 448 bytes. Before load, uptime was 89 seconds and
free_now was 13,536 bytes; the last responding monitor at uptime 100 seconds
reported 4,752 bytes. Three failed diagnostic probes stopped load, and all three
post-run recovery probes failed. Earlier auth/status stress is therefore not
required to reproduce the browse failure.

Practical diagnosis: concurrent request handling exhausts available RAM. The
earlier serial-confirmed write OOM and effectively zero heap during read bursts
support that conclusion. The remaining engineering question is attribution and
budgeting, not whether 48 bytes constitutes useful headroom. Code inspection
finds that boardhttp preallocates body/output chunks but buildRequest still
creates an http.Request, parsed URL, header map, copied strings and header-value
slices; responseWriter and its header map are also constructed per request.
Concurrent workers keep several such object graphs live together. Reusing these
objects with explicit reset/ownership rules is a concrete next target; their
individual TinyGo peak costs and any retained references still need measurement.

## HTTP reuse image: attached-board repeat, 15:29–15:33 EDT

Flashed the intended 937,152-byte image through COM8, verified by esptool. Device
is attached upstairs again; weak-link latency is not directly comparable with
the preceding downstairs runs. No concurrency or ledger capacity reduction.

- Preparation: `reports/20260918-152921-prepare-45e3/`, passed.
- Browse: `reports/20260918-152930-browse-4b5b/`, one user/five overlapping reads,
  60 seconds, usual waits and five-second diagnostics. **245 requests, 0 failures**,
  p50 400 ms, p95 700 ms, minimum sampled free heap 976 bytes. This exceeds the old
  fresh-boot browse failure after 65 successes; it is a short pass, not endurance.
- E2E on that same boot: `reports/20260918-153041-e2e-8bf0/`, failed on the first
  transfer. Last successful response reported 4,544 bytes free. HTTP remained
  unavailable; serial capture does not include the terminal cause for this run.
  Serial: `reports/20260918-http-reuse-hardware/serial-run.log`.
- Isolated same-image reset: COM8 reset left board silent/unreachable, recorded
  by failed preparations `153148-prepare-0ae6` and `153200-prepare-4c32`. COM9 reset
  restored execution. No additional flash or rollback. Preparation then passed:
  `reports/20260918-153244-prepare-966a/`.
- Fresh E2E: `reports/20260918-153253-e2e-de79/`, transfer debit and reversal passed,
  listing creation passed, then purchase failed. Serial explicitly reports
  **fatal error: out of memory / abort called**. Last successful response sampled
  7,504 bytes free. Concurrent same-key issuance was not reached.
  Serial: `reports/20260918-http-reuse-hardware/serial-isolated-native.log`.

Startup free heap inferred from serial baseline deltas is 15,056 bytes, about
5,872 below the previous image's 20,928. Reusable HTTP state improves the browse
case but spends fixed RAM that writes also need. The next target is write-path
peak memory and the startup budget; the current image is not OOM-free. The board
was left in its observed failed state after preserving evidence, with no recovery
flash or extra reset for availability.

## Reduced history capacities (15:39 EDT)

At the owner's request, reduced ledger capacity 512 -> 365 and event log 64 -> 58.
No other capacities changed. Internal race tests passed with modulo wrap tests.
Flashed the 937,216-byte image through native COM9; esptool verified flash data.
Startup free heap inferred from serial baseline is 24,128 bytes, up 9,072 bytes
from the HTTP-reuse build. Preparation passed in
`reports/20260918-153909-prepare-f001/`. E2E passed in
`reports/20260918-153919-e2e-2239/`, including purchase, four overlapping retries
returning one transaction ID, exactly-once credit and final ledger invariants.
Serial and status evidence: `reports/20260918-capacity-365/`.
Board remains attached upstairs; this is a short correctness pass, not endurance.


## 2026-09-18: bounded write memory and listing text recovery

See [WRITE_MEMORY.md](../nanacoin/WRITE_MEMORY.md) for implementation, full
experiment history, retry/commit explanations, and verified CPU-core usage.
Fixed response/retry/parser/transaction storage and direct typed commit
application are flashed. Discard-only journaling no longer encodes/decode-copies
bytes with no consumer. Diagnostic HTTP events remain readable.

An intermediate build completed 1,008 issuances at 1/2/4 users without failure,
then 328 same-key retry requests without duplicate-ID anomalies. Marketplace
stress exposed truncated listing IDs, then false capacity refusal. Atomic text
replacement, early closed-offer recycling, and pointer-owned placement scratch
resolve those exercised failures. These are bounded app-storage issues, not
proof of a network stack leak.

Final firmware: E2E 9/9; market 502 requests in 60s at two users, 501 successes,
one transport failure, no 404/507 responses. Median 199ms, p95 457ms. Board
remained responsive; ledger_balanced=true. Post-GC free heap 5,904 bytes, but
transient minimum 128 bytes: memory is still tight. No serial OOM/reset.
Final report: reports/20260918-164339-market-9b4c/index.html.
Serial/build/frame evidence: reports/20260918-write-reuse/.

These runs were upstairs with the device attached. Transport failures remain
unattributed; a strong-signal comparison is needed to isolate Wi-Fi effects.
Application allocations remain (owned strings, IDs, snapshots and some typed
objects), as do network/runtime allocations. Do not describe this as zero-alloc
or a proven endurance limit. No per-request flash persistence was introduced.
