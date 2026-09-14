# Setup

Flashing MicroPython onto the board. **Once** — after this, changes are file
copies.

!!! warning "This erases the C firmware"

    The `hello_wifi` program is wiped. The source is untouched and
    `idf.py flash` puts it back whenever you want, but the two cannot both be
    on the board at the same time.

## Tools

Two Python packages do the work:

```powershell
python -m pip install mpremote esptool
```

- **esptool** — writes firmware to the chip. Used once.
- **mpremote** — copies files to a running board, and opens the
  [REPL](repl.md). Used constantly.

!!! note "Why `python -m pip` and not `uv`"

    Whether a command is on your PATH depends on which shell you are in — an
    ESP-IDF-activated window has a different Python first than a fresh one.
    `python -m <tool>` sidesteps that by naming the interpreter explicitly.
    The scripts here do the same.

## The firmware file

Already downloaded to `hello_wifi_py/firmware/`. Newer versions come from
<https://micropython.org/download/ESP32_GENERIC_S2/>.

One binary covers boards with and without PSRAM — it detects what is attached
at boot, so there is no variant to choose.

## Flashing

**1. Bootloader mode.** Hold **BOOT** (marked `0`), tap **RESET** (marked
`RST`), release **BOOT**.

**2. Find the port:**

```powershell
[System.IO.Ports.SerialPort]::GetPortNames()
```

Ignore `COM3` — that is a motherboard serial port present on most desktops. The
one that appears when the board is plugged in is the board.

**3. Run the script:**

```powershell
cd C:\github\microcontroller\hello_wifi_py
.\flash_micropython.ps1 -Port COM4
```

It confirms before erasing, then erases and writes.

!!! warning "The port moves mid-flash"

    Erasing ends with a chip reset. With an empty flash there is no firmware to
    bring USB back up, so **the port vanishes entirely** — this is normal and
    not damage.

    The script pauses and asks you to redo BOOT/RESET before writing. If you
    run the commands by hand instead, expect to re-check the port between the
    erase and the write.

**4. Tap RESET**, then find the port again — it will have changed:

```powershell
[System.IO.Ports.SerialPort]::GetPortNames()
```

## Checking it worked

```powershell
python -m mpremote connect COM6 eval "2+2"
```

A `4` means MicroPython is running and listening.

More detail:

```powershell
python -m mpremote connect COM6 exec "import sys, gc; print(sys.implementation); print('free:', gc.mem_free())"
```

```text
(name='micropython', version=(1, 29, 0, ''), _machine='Generic ESP32S2 module with ESP32-S2', ...)
free: 2061072
```

That ~2MB free is the PSRAM being picked up automatically.

## Your WiFi credentials

```powershell
Copy-Item config_example.py config.py
```

Edit `config.py`:

```python
WIFI_SSID = "your-network-name"
WIFI_PASSWORD = "your-password"
```

`config.py` is gitignored, so credentials never reach version control.

!!! note "2.4GHz only"

    The ESP32-S2 has no 5GHz radio. If your router splits the bands into
    separate names, use the 2.4GHz one.

## Deploy

```powershell
.\deploy.ps1 -Port COM6
```

Copies four files and resets the board. About three seconds.

Then visit **<http://esp32.local>** — see [Finding It](finding_it.md) for why
that works.

## Going back to C

Nothing here is permanent:

```powershell
cd ..\hello_wifi
. C:\github\microcontroller\idf-env.ps1
idf.py -p COM4 flash monitor
```

That overwrites MicroPython with the C firmware. Swap back and forth as often
as you like.
