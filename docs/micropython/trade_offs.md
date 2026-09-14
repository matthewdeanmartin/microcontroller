# Trade-offs

MicroPython is not strictly better than C. It is a different set of
compromises, and worth knowing which way they cut.

## Side by side

| | C (ESP-IDF) | MicroPython |
|---|---|---|
| Edit → running on board | ~1 min (rebuild + reflash) | ~3 s (file copy) |
| Preview without hardware | no | yes |
| Live inspection | no | yes, the [REPL](repl.md) |
| Crash diagnosis | backtrace of hex addresses | Python traceback with line numbers |
| Raw speed | full | roughly 10–100× slower |
| Memory for your code | ~2MB | ~1MB (the interpreter takes the rest) |
| Concurrent requests | yes, threaded | one at a time |
| Precise timing | microseconds | milliseconds, and not guaranteed |
| Library ecosystem | all of ESP-IDF | a smaller set of MicroPython modules |

## What the numbers mean in practice

**"10–100× slower" sounds fatal and usually is not.** Serving a web page is
almost entirely waiting — for the network, for the client, for the next
request. The board spends its time idle either way. You would notice the
difference generating a page a thousand times a second; you will not notice it
serving your browser.

**"~1MB of RAM" is still a lot.** The status page uses a few kilobytes. This
board reports over 2MB free with PSRAM, so there is plenty of headroom.

**"One request at a time" is the real limitation.** The simple server here
handles one connection, finishes it, then accepts the next. Fine for a handful
of viewers; wrong for anything busy. MicroPython has `asyncio` if that becomes
a problem.

## Reach for C when

- **Timing must be precise.** Driving addressable LEDs, generating waveforms,
  bit-banging a protocol. Garbage collection can pause Python for milliseconds
  at unpredictable moments, which ruins tight timing.
- **Speed genuinely matters.** Signal processing, tight loops, anything
  measured in operations per second.
- **You need a specific ESP-IDF feature.** Bluetooth, low-power sleep modes,
  ULP coprocessor, some peripheral drivers. MicroPython exposes a subset.
- **Memory is tight.** A large data buffer plus a ~1MB interpreter may not fit,
  though PSRAM makes this unlikely here.

## Reach for MicroPython when

- **You are still figuring out what to build.** The fast loop matters more than
  the fast runtime, and this is most projects most of the time.
- **It is mostly I/O.** Web pages, reading sensors every few seconds, posting
  to an API. The board waits either way.
- **You want to poke at it.** The REPL turns "rebuild and hope" into "ask it".
- **Someone else needs to change it later.** A `.py` file they can edit beats a
  toolchain they have to install.

## Both, actually

They coexist. Flashing one replaces the other, but swapping back takes a
minute:

```powershell
# back to C
cd ..\hello_wifi
. C:\github\microcontroller\idf-env.ps1
idf.py -p COM4 flash monitor

# back to MicroPython
cd ..\hello_wifi_py
.\flash_micropython.ps1 -Port COM4
.\deploy.ps1 -Port COM6
```

A reasonable pattern is to prototype in MicroPython — where trying things is
cheap — and port to C only if you hit a wall that C actually solves.

## For this project

The status page is a web server on an idle board. Every MicroPython downside is
irrelevant and every upside is immediate: preview in a browser, deploy in three
seconds, inspect a running board.

The C version in [`hello_wifi`](../basic_setup/index.md) remains worth reading.
It shows what the interpreter is doing on your behalf — event handlers, task
stacks, explicit memory — and that is the layer you will need if a project ever
outgrows Python.
