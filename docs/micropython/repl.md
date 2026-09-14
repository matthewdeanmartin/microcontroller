# The REPL

A live Python prompt **on the running board**. This is the thing C cannot do at
all, and it is the biggest practical difference.

```powershell
python -m mpremote connect COM6 repl
```

```text
Connected to MicroPython at COM6
Use Ctrl-] or Ctrl-x to exit this shell
>>>
```

That `>>>` is the board. Type Python; it runs there, on the hardware, right
now.

`Ctrl+]` exits.

## Why it matters

In C, answering "how much memory is free?" means editing the source, rebuilding,
reflashing, and reading a log. Here:

```python
>>> import gc
>>> gc.mem_free()
2054160
```

No rebuild. No flash. The board is already running and you are talking to it.

## Useful things to type

**What is my IP?**

```python
>>> import network
>>> network.WLAN(network.STA_IF).ifconfig()
('192.168.1.157', '255.255.255.0', '192.168.1.1', '192.168.1.1')
```

That tuple is (IP, netmask, gateway, DNS).

**How strong is the WiFi signal?**

```python
>>> network.WLAN(network.STA_IF).status('rssi')
-60
```

In dBm, always negative, closer to zero is stronger. Better than -70 is
comfortable.

**What networks can it see?**

```python
>>> w = network.WLAN(network.STA_IF)
>>> for ssid, bssid, ch, rssi, sec, hidden in w.scan():
...     print(ssid.decode(), ch, rssi)
```

Genuinely useful when a connection is failing: it answers "can the board even
hear the router?" without guessing.

**What files are on the board?**

```python
>>> import os
>>> os.listdir()
['boot.py', 'compat.py', 'app.py', 'config.py', 'main.py']
```

**Test a route without a browser:**

```python
>>> import app
>>> app.route('/health')
(200, 'text/plain', 'ok\n')
```

## Reading crashes

When board code fails, the REPL prints an ordinary Python traceback:

```text
Traceback (most recent call last):
  File "main.py", line 87, in <module>
  File "main.py", line 74, in main
  File "app.py", line 42, in route
NameError: name 'pgae_index' isn't defined
```

A filename, a line number, and a typo you can see. Compare the C equivalent,
which points at the FreeRTOS scheduler and leaves you to work out which of your
functions had a large local variable.

## Stopping the running program

`main.py` runs a `while True` loop, so the REPL may open into a running server
rather than a prompt. **`Ctrl+C`** interrupts it and drops you to `>>>`.

From there:

```python
>>> import machine
>>> machine.reset()          # reboot, runs main.py again
```

## Other mpremote commands

The REPL is interactive; these are one-shot, useful in scripts:

```powershell
# Run one expression and print the result
python -m mpremote connect COM6 eval "2+2"

# Run several statements
python -m mpremote connect COM6 exec "import gc; print(gc.mem_free())"

# List files on the board
python -m mpremote connect COM6 fs ls

# Copy a file to the board
python -m mpremote connect COM6 fs cp app.py :app.py

# Copy a file back off it
python -m mpremote connect COM6 fs cp :app.py recovered.py

# Reboot
python -m mpremote connect COM6 reset
```

`deploy.ps1` is a wrapper around `fs cp` and `reset`.

!!! note "One program per port"

    A serial port can only be open once. If the REPL will not connect, close
    any other window holding it — another REPL, a monitor, or a stray
    `deploy.ps1`.
