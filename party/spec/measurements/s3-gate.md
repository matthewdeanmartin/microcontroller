# Sprint 0 gate: ESP32-S3 measurements

Status: **not measured yet.** The board is assigned now (`ac:a7:04:2c:29:9c`,
see DEPLOY.md). The gate firmware must be rebuilt with Wi-Fi (`make firmware GATE=1`).
The October 21 532 KB image had none, so the app had been compiled away. Fill
this in from
`GET /api/v1/gate` and `/api/v1/sys` on the board (see
[DEPLOY.md](../../DEPLOY.md#sprint-0-gate)).

The gate decides whether [07](../07-storage-limits.md) holds as designed:

| Measurement | Pass | Result |
|---|---|---|
| Write 20,000 × 200 B records into `partydata` | no write failures | — |
| Read all 20,000 back (what boot does) | < 10 s | — |
| Mean / worst single read | — | — |
| gzip + AES-256-GCM of 256 KiB | — (record it) | — |
| Outbound HTTPS to mastodon.social, api.github.com, api.brevo.com for 5 min while serving 4 HTTP + 1 HTTPS clients | no TLS allocation failures | — |
| Largest free internal block during all of the above | ≥ 32 KiB | — (idle after first boot: 31 KiB, already under the line) |
| PIN hash, 1,000 rounds (boot log) | ~100 ms | — |

## Desktop reference (October 21, 2026)

Not a substitute for the board, but a floor:

- `boot_with_2000_people_and_writes_stay_small` (release build, in-memory
  store): reopening a board with 2,000 people takes 3.6 ms. With 2,000
  people, joining a group writes 128 bytes and a profile edit 187 bytes.
- Adding a person also rewrites the people index (`ip`), which grows by about
  3 bytes per person (~6 KB at 2,000). That is the one write that grows with
  the board, and it happens only on sign-up.
