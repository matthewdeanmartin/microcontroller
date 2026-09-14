# How It Works

## What MicroPython is

A complete Python interpreter, small enough to live on a microcontroller.

Flash it once and the board stops being "a thing you compile programs for" and
becomes "a small computer that runs Python files". You copy `.py` files onto
it, exactly like copying files to a USB stick, and it runs them.

It is real Python — functions, classes, exceptions, f-strings — with a smaller
standard library and a few extras for talking to hardware.

## What changed on the board

| | Before (C) | After (MicroPython) |
|---|---|---|
| What is in flash | your compiled program | the Python interpreter |
| Changing your code | recompile, reflash | copy a `.py` file |
| Files on the board | none, it is one binary | `main.py`, `app.py`, … |
| Poking at it live | no | yes, via the [REPL](repl.md) |

MicroPython looks for **`main.py`** at boot and runs it. That is the entire
startup convention.

## The three-layer layout

The interesting part of this project is that **one file runs in two places** —
on your PC for previewing, and on the board for real. That needs a little care,
because the two environments genuinely differ.

```text
        app.py            <- routes and HTML. YOU EDIT THIS.
           │
           ▼
       compat.py          <- hides the differences
           │
     ┌─────┴─────┐
     ▼           ▼
dev_server.py  main.py
  (your PC)    (the board)
```

### `app.py` — the part you write

Routes are plain functions. No sockets, no WiFi, no hardware:

```python
def route(path):
    if path == "/":
        return 200, "text/html", page_index()
    if path == "/health":
        return 200, "text/plain", "ok\n"
    return 404, "text/plain", "not found\n"
```

A route takes a path and returns `(status, content_type, body)`. That is the
whole interface. Because it touches nothing environment-specific, the same file
runs unchanged in both places.

### `compat.py` — the differences

A few things genuinely differ between Python-on-your-PC and
Python-on-the-board:

| Question | On the board | On your PC |
|---|---|---|
| How much memory is free? | `gc.mem_free()` | no such concept |
| What am I running on? | `micropython` | `cpython` |
| What is my IP? | ask the WiFi chip | not meaningful |

Each one lives behind a function in `compat.py`, so `app.py` can just call
`compat.free_memory_text()` and never ask where it is running.

The page says which machine answered — "Hello from your PC" locally, "Hello
from the ESP32-S2!" on the board — so a preview is never mistaken for the real
thing.

### `dev_server.py` and `main.py` — the two runners

Both do the same job: listen on a port, read a request, call `app.route()`,
write the response back.

`main.py` also connects to WiFi first. `dev_server.py` also reloads `app.py` on
every request, so editing and refreshing is the whole loop — no restart.

## Why not Flask?

Flask is the obvious way to write a Python web app, and it will not run here —
it is far too large for a microcontroller, and depends on parts of the standard
library MicroPython does not have.

Writing to a shared, tiny interface instead means the local preview is
genuinely the same code, not an approximation of it. If a page renders locally,
it renders on the board.

The cost is doing without Flask's conveniences: no routing decorators, no
template engine, no request parsing beyond the path. For a status page that is
a fair trade.
