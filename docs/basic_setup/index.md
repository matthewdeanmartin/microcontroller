# Basic Setup

Getting a **DiGiYes ESP32-S2 Mini V1.0.0** from freshly-unboxed to serving a web
page over WiFi, on Windows, using ESP-IDF.

This is written as a walkthrough rather than a reference. It includes the things
that go wrong, because on this platform most of the elapsed time goes into those
rather than into writing code.

## What you need

- The board, and a USB-C cable that carries **data** (charge-only cables are
  physically identical and fail silently)
- [ESP-IDF 5.5.x](https://docs.espressif.com/projects/esp-idf/en/stable/esp32s2/get-started/windows-setup.html)
  installed via the Windows installer
- A **2.4GHz** WiFi network

## Contents

1. [The Board](the_board.md) — what makes the ESP32-S2 different, and why it
   sometimes appears to be dead
2. [Toolchain Setup](toolchain.md) — activating ESP-IDF, and the environment
   problems that stop it working
3. [The Project](the_project.md) — anatomy of an ESP-IDF project, walked through
   file by file
4. [Build, Flash, Monitor](build_flash_monitor.md) — the actual cycle, including
   the COM port shuffle
5. [Troubleshooting](troubleshooting.md) — every error hit while writing this,
   with causes

## The short version

Once everything is set up, the cycle is:

```powershell
. C:\github\microcontroller\idf-env.ps1   # once per terminal window
cd C:\github\microcontroller\hello_wifi
idf.py menuconfig                          # set WiFi credentials
idf.py -p COM4 flash monitor               # BOOT/RESET on the board first
```

Success looks like this:

```text
I (5756) hello_wifi: got IP: 192.168.1.157
I (5756) hello_wifi: HTTP server listening on port 80
```

Then open that IP in a browser.
