# MicroPython

Running Python on the board instead of C, with a local preview so you can see
your web page in a browser before it ever touches the hardware.

## Why bother

The C workflow is: edit, build, hold a button, find a COM port, flash, find the
port again, watch the serial log. A minute or so, every single change.

With MicroPython the firmware is flashed **once**. After that your code is just
files copied over USB:

| | C (ESP-IDF) | MicroPython |
|---|---|---|
| See a change | rebuild + reflash, ~1 min | refresh the browser |
| Put it on the board | ~1 min | ~3 seconds |
| Preview without the board | no | yes |
| Poke at a running board | no | yes, via the REPL |

The cost is speed and memory — see [Trade-offs](trade_offs.md). For a web page,
neither matters.

## Contents

1. [How It Works](how_it_works.md) — what MicroPython actually is, and the
   three-layer layout that lets one file run in two places
2. [Setup](setup.md) — flashing the firmware, once
3. [The Workflow](workflow.md) — edit, preview, deploy
4. [Finding It On Your Network](finding_it.md) — the `esp32.local` trick
5. [Signal and Placement](signal_and_placement.md) — where you put it matters
   more than anything in the code
6. [The REPL](repl.md) — a live Python prompt on a running board
7. [Trade-offs](trade_offs.md) — when to reach for C instead

## The short version

Preview on your PC, no board needed:

```powershell
cd C:\github\microcontroller\hello_wifi_py
python dev_server.py
```

Open <http://localhost:8000>, edit `app.py`, refresh.

Happy with it? Put it on the board:

```powershell
.\deploy.ps1 -Port COM6
```

Then visit **<http://esp32.local>** from any device in the house.

