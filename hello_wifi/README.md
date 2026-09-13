# hello_wifi

"Hello world" for the **DiGiYes ESP32-S2 Mini V1.0.0** (ESP32-S2FN4R2, 4MB flash,
2MB PSRAM). The board joins your WiFi network and serves a web page on port 80.

## Endpoints

| Path      | Returns                                |
|-----------|----------------------------------------|
| `/`       | HTML page with uptime, free heap, version |
| `/health` | `ok` as plain text                     |

## One-time setup

Open **PowerShell** (not Git Bash - the IDF build system rejects MSys) and
activate the toolchain. Dot-source it so the variables stick:

```powershell
. C:\github\microcontroller\idf-env.ps1
```

Then set your WiFi credentials:

```powershell
cd C:\github\microcontroller\hello_wifi
idf.py menuconfig
```

Navigate to **Hello WiFi** -> set SSID and password. Arrow keys to move,
Enter to edit, `S` to save, `Q` to quit. Credentials land in `sdkconfig`,
which is gitignored.

> The ESP32-S2 radio is **2.4GHz only**. A 5GHz-only SSID will never connect.

## Build, flash, monitor

```powershell
idf.py build
idf.py -p COM<N> flash monitor
```

`Ctrl+]` exits the monitor. Watch for the line:

```
I (5234) hello_wifi: got IP: 192.168.1.123
```

Browse to that address.

## Putting the board in bootloader mode

The S2 has **native USB** - there is no separate USB-to-serial chip. A board
with no USB-aware firmware enumerates as nothing at all, so the first flash
needs the ROM bootloader, entered manually:

1. Hold **BOOT** (labeled `0`)
2. Tap **RESET** (labeled `RST`)
3. Release **BOOT**

A new COM port appears. Flash to it. After flashing, tap **RESET** once to
run your app.

If the port vanishes after flashing, that is expected - the port belongs to
whichever mode the chip is in, and it changes between bootloader and app.

## Layout

```
hello_wifi/
  CMakeLists.txt        project definition
  sdkconfig.defaults    board settings (USB CDC console, 4MB flash, PSRAM)
  main/
    CMakeLists.txt      component definition
    Kconfig.projbuild   adds the "Hello WiFi" menuconfig entries
    hello_wifi.c        the program
```

## Troubleshooting

| Symptom | Cause |
|---|---|
| `MSys/Mingw is not supported` | Running from Git Bash. Use PowerShell. |
| `idf.py` not found | Forgot the leading `.` when sourcing `idf-env.ps1`. |
| No COM port appears | Board not in bootloader mode, or a charge-only USB cable. |
| Connects but no web page | Client isolation / AP isolation enabled on your router. |
