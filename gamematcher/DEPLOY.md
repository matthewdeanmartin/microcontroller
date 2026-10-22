# Deploy Game Matcher to the Waveshare C6

## The board

| Item | Value |
|---|---|
| Model | Waveshare ESP32-C6-LCD-1.47, **non-Touch** ([board notes](../BOARD_SKILL_ESP32_C6_LCD_1_47.md)) |
| Chip | ESP32-C6FH8, one RISC-V core, 512 KiB SRAM, no PSRAM |
| Flash | 8 MiB (probed) |
| Base MAC | `ac:eb:e6:1e:13:40` (formerly minicloud) |
| USB | VID:PID `303A:1001`, native USB Serial/JTAG; COM17 when this was written |
| Hostname | `gamematcher.local` |

Find the port by VID:PID, never by habit; exclude COM3 (Intel AMT). The
deploy script refuses any board whose MAC or flash size differs.

## Flash layout

| Partition | Offset | Size | Kept by `make update` |
|---|---|---|---|
| nvs | `0x9000` | 24 KiB | yes |
| phy_init | `0xf000` | 4 KiB | yes |
| factory (app + UI) | `0x10000` | 2.5 MiB | rewritten |
| coredump | `0x290000` | 64 KiB | yes |
| gmstate (accounts, sessions) | `0x2A0000` | 512 KiB | yes |

## Rules

1. **`make flash` erases everything.** Use it for the first install (the
   repave from minicloud, authorized by the owner on October 17, 2026) and
   never again unless the partition table changes.
2. **`make update` for every later change.** It reads the board's partition
   table first and refuses unless it is exactly Game Matcher's.
3. **Agents ask before flashing.** The owner said yes to the Sprint 0 repave;
   each later flash is its own question.
4. **Claim admin immediately after a first install.** Whoever submits the
   setup form first becomes admin. Open http://gamematcher.local/ yourself
   right after flashing (or build with `GAMEMATCHER_SETUP_CODE`).

## Steps

```sh
cd gamematcher
make certs                         # once
make firmware                      # TRANSPORT=http2 default; GATE=1 only for the gate
make flash PORT=COM17              # first install
make update PORT=COM17             # afterwards
```

Then check, by IP if `.local` doesn't resolve:

```sh
curl -s http://gamematcher.local/api/v1/status?fmt=json
curl -s --ssl-no-revoke --cacert api/certs/household-ca.crt https://gamematcher.local/api/v1/status?fmt=json
curl -s http://gamematcher.local/api/v1/log.txt | tail -20
curl -s http://gamematcher.local/.well-known/ops/health
```

Windows curl (schannel) needs `--ssl-no-revoke`: the household CA publishes
no revocation list, so without it the check fails with
`CERT_TRUST_REVOCATION_STATUS_UNKNOWN` and prints nothing. It still verifies
the chain and hostname; never substitute `-k`. By IP, add
`--resolve gamematcher.local:443:<ip>`.

## Diagnostics

- `/api/v1/sys?fmt=json`: heap (free, minimum ever, largest block), TLS
  handshakes/failures, open connections, reset reason.
- `/api/v1/log.txt`: the board's own log ring, including the previous
  boot's crash report from the core dump. Prefer it to serial captures.
- `/metrics`: the same as Influx lines, plus `gm_accounts`, `gm_sessions`,
  `gm_commits`, `gm_last_commit_bytes`, `gm_max_commit_bytes` (housemetrics can scrape it).
- `/.well-known/ops/health`: miniframework `ops` checks plus Game Matcher's
  `store.slot`, `store.accounts`, `store.games` (read by mastomini-bots' `sre`).

## Deployments

### 2026-10-19: ops health endpoint (SRE sprint 0)

- Owner attached the board for this flash. COM17, composite and esptool base
  MAC `ac:eb:e6:1e:13:40`, 8 MB flash. `make check` passed.
- `make update`: app 2,408,576 / 2,621,440 bytes (91.9%), written at
  `0x10000` only, hash verified. No backup (owner's choice; slow USB).
- Before: build `4b10110+`, setup done, `clock_synced: false`. After: build
  `6f335bf`, setup done, clock synced, HTTP and strict HTTPS (household CA,
  `--ssl-no-revoke`) status 200 at `192.168.1.163`.
- Health all `ok`: 88 KiB internal free (lowest 56 KiB), largest block
  55 KiB, −66 dBm, snapshot slot current, 2/32 accounts, 6/256 games.
- Logs: `.local/ops-deploy-*.log`.
