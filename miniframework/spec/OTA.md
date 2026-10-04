# Over-the-air updates

Status: **design**, revised October 4, 2026. Nothing here is implemented.
Related: [NanaCoin migration](../../../nanacoin/nanacoin_rs/spec/MINIFRAMEWORK_MIGRATION.md).

## Why

Every update today is a USB runbook: find the COM port (it moves on every S2
reset), prove the MAC, read back the partition table, write the app slot,
probe. `tools/boardsafe` now does all of that in one command, but it still
needs a cable, a laptop and a person who knows to hold BOOT. For a board that
lives on someone's fridge, the update mechanism has to be "press a button in
the admin page", with USB only for the first install and for recovery.

## Ground rules (owner decisions, October 4)

1. **The first flash is never OTA.** A new board is installed over USB
   (`make flash` / NanaCoin's `provision`), which is also when it gets the
   OTA-capable partition table and bootloader.
2. **OTA is off where it does not fit.** A board whose flash cannot hold two
   app slots beside its data just has no OTA. Detected at runtime from the
   partition table, not configured: no `otadata` + two app slots, no OTA
   routes, and the UI says so.
3. **Big boards for OTA.** A household that wants OTA uses a board with the
   flash for it (the ESP32-S3 N16R8 class, 16 MiB). A 4 MiB S2 holding data
   stays USB-updated.
4. **Images are built per household.** There is no plan for one public
   image: Wi-Fi settings, the TLS key and the household CA stay compiled in,
   as today. (If boards are ever sold pre-built, revisit: that is the point
   at which settings must move to NVS and images must become generic.)

## Requirements

1. **Never lose data.** The update path writes the spare app slot only.
   Data partitions (a ledger, NVS settings, certificates) are never written.
2. **A bad update undoes itself.** A crash, watchdog reset, power cut or
   failed self-test during the first boot returns to the previous image with
   nobody involved.
3. **Only images from this household's build machine install.** Signed
   images; the board checks the signature and that the image is for this
   app and this board (the same marker `boardsafe` checks over USB).
4. **The household decides when.** The admin starts it; no silent updates
   for a money app.
5. **Same code on the desktop**, so the UI and the state machine are tested
   with `make run`, like everything else in the framework.

## Where the boards stand

| Board | Flash | App now | Data | OTA? |
|---|---|---|---|---|
| NanaCoin S3 (`ac:a7:04:2c:2c:04`) | 16 MiB | 3.67 MB | `ledger` 8 MiB @ `0x410000` | Yes, after one USB re-layout |
| NanaCoin S2 (`80:65:99:f0:1c:9c`) | 4 MiB | 2.22 MB | `ledger` 1.56 MiB | **No**: two 2.2 MB slots exceed the chip |
| housemetrics S2 (`80:65:99:f0:7b:68`) | 4 MiB | 1.57 MB | RAM + `coredump` | Yes, after one USB re-layout |
| The new big board (arriving Oct 4) | TBD | — | — | Install OTA-capable from day one |

All current tables are `factory`-only and no bootloader has rollback, so
each OTA-capable board needs one supervised USB flash: bootloader + new
table + app. After that the bootloader and table are frozen (OTA never
rewrites them), so the layouts leave headroom now.

### Layouts (data partitions keep their offsets)

**NanaCoin S3**: ledger untouched.

```
nvs       data nvs       0x9000    0x6000     unchanged
phy_init  data phy       0xf000    0x1000     unchanged
ota_0     app  ota_0     0x10000   0x400000   was "factory", same offset
ledger    data nvs       0x410000  0x800000   unchanged, never written by OTA
ota_1     app  ota_1     0xC10000  0x3D0000   3,997,696 B
otadata   data ota       0xFE0000  0x2000
coredump  data coredump  0xFF0000  0x10000
```

The smaller slot caps the image: today's 3.67 MB leaves ~325 KiB. Serving
the S3's bundle gzip-only (as the S2 does: 0.54 MB vs 1.97 MB) brings the
image to ~2.3 MB. `boardsafe` already sizes against the *smallest* app slot.

**housemetrics S2**:

```
nvs       0x9000    0x6000    unchanged
phy_init  0xf000    0x1000    unchanged
ota_0     0x10000   0x1E0000  1,966,080 B
ota_1     0x1F0000  0x1E0000
otadata   0x3D0000  0x2000
coredump  0x3F0000  0x10000   unchanged offset
```

**NanaCoin S2**: no OTA (rule 2). `make deploy BOARD=s2` stays its update.

**New boards** start from an OTA layout template in `tools/` so they never
need the re-layout flash.

## Update lifecycle

```
idle ─▶ receiving ─▶ verified ─▶ preflight ─▶ switched ─▶ reboot
                                                            │
                                     trial boot (PENDING_VERIFY, writes paused)
                                              │                    │
                                        health ok            fails / crash /
                                              │              watchdog / power cut
                                          committed             rolled back
```

1. **Receive.** Stream into the spare slot with
   `esp_ota_begin(..., OTA_WITH_SEQUENTIAL_WRITES)`, erasing a sector at a
   time; erasing a whole slot up front freezes the serving loop for seconds.
   Writes happen on the serving task (internal stack: a PSRAM stack cannot
   run while flash writes disable the cache).
2. **Verify** before switching anything: signature, image marker
   (`<APP>-BOARD:<id>:<host>;`), chip ID, size, SHA-256 of what streamed,
   `esp_ota_end` validation.
3. **Preflight** (app hook). NanaCoin: refuse if storage has failed, write a
   checkpoint, hold the ledger lock so no payment is half-committed.
4. **Switch and reboot.**
5. **Trial boot.** With `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE` the new image
   starts `PENDING_VERIFY`; a crash, watchdog or power cut before commit
   returns to the old image. During the trial the app **writes no durable
   data** (NanaCoin answers money writes 503 "updating, back in a minute"),
   so the old image never meets data written by the new one.
6. **Health gate** (app hook, framework default): storage opened and
   replayed, Wi-Fi up, the serving loop turning, stable ~60 s. Then
   `esp_ota_mark_app_valid_cancel_rollback`, else
   `esp_ota_mark_app_invalid_rollback_and_reboot`.
7. **Revert** (admin button): boot the other slot if it holds a valid image
   whose storage schema matches.

### Storage schema and rollback

New firmware that rewrites the ledger in a new format, then a Revert, loses
data. Each image carries `storage_schema` (manifest and
`esp_app_desc_t.version`, e.g. `0.5.0+s7`, readable from the other slot with
`esp_ota_get_partition_description`); Revert is offered only when they
match. Once a household runs OTA, NanaCoin's
`spec/FORWARD_COMPATIBLE_DATA_CHANGES.md` is a requirement, and the root
"development data is disposable" policy no longer holds for that board.

## Trust

Images are built per household (rule 4), so the signing key is the
household's own, made once on the build machine (`make ota-key`, kept under
`.local/`, never in a repo, never printed; like the CA key). Its public half
is compiled in. That still matters: without it, anyone with an admin session
on the LAN could install arbitrary firmware.

- Ed25519 (`ed25519-compact`, small, `no_std`), up to two public keys per
  image so a key can be rotated by an update signed with the old one.
- Bundle (`.mfw`): magic, manifest length, manifest JSON, 64-byte signature
  over the manifest, image. Manifest:
  `{app, board, chip, version, storage_schema, size, sha256}`. The board
  checks the signature before writing a byte, then hashes as it streams.
- Downgrades need an explicit "allow older" and pass the schema rule.
- No Secure Boot or flash encryption: eFuses are irreversible, and physical
  access means the owner.
- Who may install: `Service::ota_authorized(&req)`. NanaCoin: an active Nana
  session (not an API key). housemetrics: the admin token.

## Delivery

1. **From the build machine**: `make ota BOARD=s3` signs and pushes over
   Wi-Fi (`POST /api/v1/ota`). This replaces the USB runbook for every
   update after the first; `boardsafe` over USB remains for recovery and for
   boards without OTA.
2. **From the admin page**: choose a `.mfw` (sent by whoever builds for the
   household) and upload. This is the consumer path while images are built
   per household.
3. A board pulling its own updates is out of scope until images are generic.

## Framework work

- `ota` module: bundle parser, signature + marker check, the state machine,
  an `Ota` platform trait with `EspOta` (`esp_ota_*`) and `DesktopOta` (two
  slot files and a state file; "reboot" re-execs) so it is tested on the PC.
- **Streaming request bodies** in `Mux`: a route may take a body sink
  instead of the buffered body, with backpressure, so a 2 MB upload needs a
  4 KiB buffer. (The lingering close added for refused bodies is the first
  piece of this.)
- Routes: `GET /api/v1/ota` (available or why not, running version, both
  slots, state, last result), `POST /api/v1/ota`, `POST /api/v1/ota/revert`.
- `Service` hooks: `ota_authorized`, `ota_preflight`, `trial_healthy`,
  `writes_paused`.
- Light: an "updating" pattern; trial and rollback in the incident events;
  the last result survives the reboot (RTC memory, like the log).
- sdkconfig fragment with `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y`.
- NVS opened with `take_with(false)` (done October 4): an update to a newer
  ESP-IDF must never be able to erase NVS.
- `boardsafe`: a `relayout` command for the one-time USB move to an OTA
  table (verifies the *old* layout exactly, writes bootloader + table +
  blank `otadata` + app, never a data partition) and the OTA layout
  templates.

## Sprints

1. **OTA-1, desktop.** Bundle format, signing key tool, verify, `DesktopOta`,
   streaming bodies, routes, state machine, runtime "OTA unavailable"
   detection. All under `make check`.
2. **OTA-2, the new big board.** Install it OTA-capable over USB (ask
   first), then prove on hardware: a good update; one that panics at boot
   (rolls back); one that never commits (rolls back); power pulled
   mid-upload (old image keeps running); power pulled in the trial (rolls
   back); wrong board / unsigned / truncated (refused, nothing written).
3. **OTA-3, housemetrics S2.** `boardsafe relayout` (ask first), then OTA.
4. **OTA-4, NanaCoin S3.** Gzip-only bundle, ledger preflight, paused writes
   during the trial, schema-gated revert, one re-layout flash (ask first).

## Open decisions

1. Whether a ~60 s "writes paused" window after each NanaCoin update is
   acceptable (it is what makes rollback safe).
2. What the new board is (chip, flash, PSRAM), which decides its profile
   and layout.
