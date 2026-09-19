# Board Skill — ESP32-S2 Mini (S2FNR2 / S2FN4R2)

Working notes for the small cheap S2, the companion to
[`BOARD_SKILL_ESP32_S3_N16R8.md`](BOARD_SKILL_ESP32_S3_N16R8.md). Everything
here was **measured on the hardware**.

The long-form tutorial for this board is `docs/basic_setup/` — read
[the board](docs/basic_setup/the_board.md) for the full explanation of native
USB and bootloader mode. This file is the quick reference and, more usefully,
**the part that says which of the two boards a job belongs on**.

---

## Identity

Ask the chip; do not trust the listing or the label Windows puts on it.

```powershell
python -m esptool --port COM4 chip-id
```

```text
Chip type:          ESP32-S2FNR2 (revision v1.0)
Features:           Wi-Fi, Single Core, 240MHz, Embedded Flash 4MB,
                    Embedded PSRAM 2MB
Crystal frequency:  40MHz
USB mode:           USB-OTG
MAC:                80:65:99:f0:7b:68
```

| Property | Value |
|---|---|
| Chip | ESP32-S2FNR2 rev v1.0 |
| Core | **Single** Xtensa LX7 @ 240MHz |
| Flash | **4MB embedded** |
| PSRAM | **2MB embedded** |
| Radio | WiFi 2.4GHz b/g/n — **no Bluetooth** |
| USB | Native USB-OTG only — **no bridge chip** |
| MAC (this unit) | `80:65:99:f0:7b:68` |

`FNR2` and `FN4R2` both appear on these boards; the numbers that matter —
4MB flash, 2MB PSRAM, single core — are the same.

---

## Which board for which job

The two boards in this project are not interchangeable, and the differences are
large enough to decide the architecture rather than just the build flags.

| | **S2 Mini** | **S3-N16R8** |
|---|---|---|
| Cores | 1 | 2 (+ LP) |
| Flash | 4MB | 16MB |
| PSRAM | 2MB | 8MB octal |
| Measured free heap | ~2MB | 8.29MB |
| Filesystem after firmware | ~2MB | 14.7MB |
| Bluetooth | none | BT 5 (LE) |
| USB ports | 1 (native) | 2 (native + CH343 bridge) |
| Port stability | moves on every reset | bridge port is stable |
| mDNS under MicroPython | yes | yes |

**Put static file serving on the S2.** Files stream off flash; it needs space
and almost no compute, which is what this board has. The NanaCoin Angular
bundle is ~350KB against ~2MB of filesystem — comfortable, though gzipping the
assets is worth doing here rather than treated as an optimisation.

**Put anything with a working set on the S3.** NanaCoin's ledger, its bounded
arrays and its four request workers were sized against 8MB of PSRAM, and its
history window and log ring are RAM budgets, not disk ones. None of that fits
in 2MB.

**Anything Bluetooth is the S3 by elimination.** The S2 has no radio for it,
and S3 tutorials compile happily here and then fail at runtime.

---

## The USB port moves, every single time

The S2 has **no bridge chip**. USB goes straight into the S2, so the port is
presented by whatever firmware is running — and it disappears and re-enumerates
on every reset, every flash, and every crash.

This is the single biggest day-to-day difference from the S3, where the CH343
bridge holds a stable COM8 no matter what the chip is doing.

Consequences worth internalising:

- **Never hard-code the port.** It was COM4 in one session and COM5 in another,
  on the same machine and the same board.
- **A port that vanishes mid-session is normal.** `esptool` finishes with
  "Hard resetting with a watchdog", the port drops, and the next command fails
  with "could not open port". Nothing is wrong; the board is re-enumerating.
- **An empty or non-USB-aware firmware enumerates as nothing at all** — no COM
  port, no unknown device, no evidence in Device Manager. That is the documented
  normal state for a blank S2, not a fault.

Find it, rather than assuming:

```powershell
[System.IO.Ports.SerialPort]::GetPortNames()
```

COM3 on this machine is the motherboard's Intel AMT SOL port and is always
present. Ignore it. The port that *appears* is the board.

To see it with its identity attached:

```powershell
Get-PnpDevice -PresentOnly |
  Where-Object { $_.InstanceId -match 'VID_303A' } |
  Select-Object FriendlyName, Status, InstanceId | Format-Table -AutoSize
```

`-PresentOnly` matters. Without it Windows lists every ESP32 ever plugged into
this machine, all with status `Unknown`, which reads exactly like a board that
is connected and misbehaving. Several minutes were lost to that.

---

## mpremote hangs, and the port is then unusable

The worst failure mode on this board, because it looks like a dead port and is
not.

**A board running its own `main.py` does not answer the REPL.** A serve loop
sits in `accept()` forever, so `mpremote` waits for a prompt that never comes.
It does not time out on its own.

What makes it costly is the second half: that `mpremote` process is blocked in
a Windows serial read, and **`taskkill /F /T` will not end it** — Windows does
not terminate a process while a driver call is pending. It keeps the port open.
Every retry starts another one, so attempting the same command again makes the
situation strictly worse:

```text
could not open port 'COM12': PermissionError(13, 'Access is denied.')
```

Recovery is **physical: unplug the board, wait a few seconds, plug it back
in.** Nothing on the PC side clears it. A reboot works too, and is the answer
if the handle survives a replug.

Two habits avoid it entirely:

- **Put a timeout on every `mpremote` call.** `timeout 25 python -m mpremote …`
  turns a wedged machine into an error message. `deploy.sh` wraps every call
  this way.
- **BOOT + RST before deploying**, so the board is at a REPL rather than in its
  serve loop.

And one rule learned the hard way: **when a serial command hangs, stop.** The
second attempt does not succeed where the first failed, and each one holds the
port against the next.

## Deploys must not destroy before they can write

Related, and the same incident: an early `deploy.sh` wiped the board's `/www`
and *then* copied the new files. The copy failed on a wedged port, and the
board served 404s until it could be reached again — which required the physical
replug above.

Stage, then swap. Copy into `/www.new`, and replace `/www` only once every byte
has landed. A failed deploy then leaves the running site untouched, and the
destructive step is a few milliseconds at the end rather than the whole
transfer.

## Bootloader mode

The ROM bootloader lives in silicon and always works, whatever the flash holds:

1. **Hold** BOOT (marked `0`)
2. **Tap** RESET (marked `RST`)
3. **Release** BOOT

A port appears within a second or two. This is the answer whenever the board is
invisible and you have ruled out the cable.

---

## Vendor and product IDs, and a trap

| ID | What it is |
|---|---|
| `VID_303A&PID_0002` | **S2** native USB |
| `VID_303A&PID_1001` | **S3** native USB |
| `VID_1A86&PID_55D3` | CH343 bridge — **S3 board only** |

Windows labels `PID_0002` as "ESP32-S2", which is right, but it labels it that
way from a driver INF rather than by asking the chip. Do not use the Windows
name to tell the boards apart when it matters — run `chip-id`.

**A sibling device showing `Error` is usually fine.** The S2 presents a
composite device: the serial interface (`MI_00`) plus a JTAG/debug interface
(`MI_02`). Windows often has no driver for `MI_02` and reports problem code 28
("no driver installed") against a device named ESP32-S2. Meanwhile `MI_00` is
`OK` and serving a working COM port. Read the interface, not the error.

---

## When the board is invisible

In the order worth trying, because the cheap checks rule out the common causes:

1. **The cable.** A charge-only USB-C cable powers the board and carries no
   data. The board lights up and the PC sees nothing at all — identical
   symptoms to dead firmware. Swapping to a known-good data cable is the first
   move, not the last.
2. **BOOT + RST** into the ROM bootloader.
3. **The other port**, on a board that has two. (The S2 Mini does not; the S3
   does, and its bridge port is the one to use.)
4. **Power / hub.**

The reasoning that ranks these: on the S3, the CH343 enumerates on power alone
regardless of firmware state, so "nothing at all appeared" points at power or
cable rather than software. On the S2 there is no such independent witness —
firmware state *can* silence the port completely — so the bootloader button
sequence is genuinely diagnostic here in a way it is not on the S3.

---

## Flashing

Unlike the S3, **the S2 flashes at the default offset**. The `0x0` rule in the
S3 notes is an S3 bootloader fact and does not apply here.

MicroPython for this board is the plain `ESP32_GENERIC_S2` build — not an
S3 image, and not a SPIRAM_OCT variant, both of which will flash "successfully"
and leave a board that never boots.

```powershell
python -m esptool --port COM4 --chip esp32s2 erase-flash
python -m esptool --port COM4 --chip esp32s2 write-flash 0x1000 ESP32_GENERIC_S2-*.bin
```

The port will have moved between those two commands. Re-check it.

---

## Quick reference

```powershell
# What ports exist right now (COM3 is the motherboard, ignore it)
[System.IO.Ports.SerialPort]::GetPortNames()

# What is actually plugged in
Get-PnpDevice -PresentOnly |
  Where-Object { $_.InstanceId -match 'VID_(1A86|303A)' } |
  Select-Object FriendlyName, Status | Format-Table -AutoSize

# Which chip is it really
python -m esptool --port COM4 chip-id

# Bootloader: hold BOOT(0), tap RST, release BOOT
```

| Symptom | Cause |
|---|---|
| No port at all, ever | charge-only cable, or blank/non-USB firmware |
| mpremote hangs forever | board is in its serve loop; BOOT+RST first |
| "Access is denied" on the port | a wedged mpremote still holds it; replug |
| Port vanished after a command | normal; native USB re-enumerates on reset |
| Port number changed | normal; never hard-code it |
| "ESP32-S2" with a yellow bang | the JTAG interface, not the serial one |
| Every ESP32 listed as `Unknown` | missing `-PresentOnly` |
| Flashed fine, never boots | wrong MicroPython variant for the chip |
