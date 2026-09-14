# hello_wifi_py

The same status page as `hello_wifi`, in MicroPython instead of C — and
runnable on your PC so you can see changes without touching the board.

## Why this exists

The C project's cycle is: edit, build (~30s), BOOT/RESET, find the COM port,
flash, hunt for the port again, monitor. Here it is:

| | |
|---|---|
| **Local** | `python dev_server.py`, edit, refresh the browser |
| **Device** | `.\deploy.ps1 -Port COM5` — a couple of seconds, no re-flash |

The firmware is flashed **once**. After that only your `.py` files move.

## Files

```text
app.py              routes + HTML         <- THE FILE YOU EDIT
compat.py           CPython/MicroPython differences
dev_server.py       local preview runner
main.py             device runner (MicroPython runs this at boot)
config.py           your WiFi credentials (gitignored)
config_example.py   template for the above
deploy.ps1          copies files to the board
firmware/           the MicroPython .bin
```

`app.py` runs **unchanged** in both places. That is the whole design: a route
is a plain function returning `(status, content_type, body)`, and neither the
local server nor the board needs it to know which one it is.

## Local preview

```powershell
python dev_server.py
```

Open <http://localhost:8000>. Edit `app.py`, refresh — no restart, the server
reloads the module per request. A syntax error shows as a traceback in the
browser rather than killing the server.

The page states plainly which machine answered, so a local preview is never
mistaken for the real board.

## One-time: flash MicroPython

This **replaces** the C firmware. Flashing ESP-IDF again later puts it back.

Put the board in bootloader mode — hold **BOOT** (`0`), tap **RESET** (`RST`),
release **BOOT** — then find the port:

```powershell
[System.IO.Ports.SerialPort]::GetPortNames()
```

Then:

```powershell
.\flash_micropython.ps1 -Port COM4
```

It confirms before erasing, finds the firmware itself, and prints the next step.

Tap **RESET**. The board now runs MicroPython and will re-enumerate on a
different COM port — see the C project's docs on
[the COM port shuffle](../docs/basic_setup/the_board.md#the-com-port-moves).

## Deploy your code

```powershell
.\deploy.ps1 -Port COM5
```

(`config.py` already holds this machine's WiFi credentials. On a fresh
checkout, copy `config_example.py` to `config.py` and fill it in — it is
gitignored, so it never leaves your machine.)

Watch it boot:

```powershell
python -m mpremote connect COM5 repl
```

`Ctrl+]` exits.

## The REPL

The real advantage over C. `mpremote ... repl` gives a live Python prompt on
the running board:

```python
>>> import gc; gc.mem_free()
2087312
>>> import network; network.WLAN(network.STA_IF).ifconfig()
('192.168.1.157', '255.255.255.0', '192.168.1.1', '192.168.1.1')
```

Inspect and change a running board with no flash cycle at all.

## Trade-offs versus the C version

| | C (ESP-IDF) | MicroPython |
|---|---|---|
| Edit → running | ~1 min, reflash | ~2 s, file copy |
| Local preview | no | yes |
| Live REPL | no | yes |
| Speed | fast | ~100x slower |
| RAM for your code | ~2MB | ~1MB (interpreter takes the rest) |
| Concurrent requests | yes (threaded) | one at a time |

For a status page, none of the downsides bite. They start to matter with
precise timing, high request rates, or tight memory.

