# The Workflow

Edit, preview, deploy.

## 1. Preview on your PC

```powershell
cd C:\github\microcontroller\hello_wifi_py
python dev_server.py
```

```text
serving app.py at http://127.0.0.1:8000
edit app.py and refresh - no restart needed
Ctrl+C to stop
```

Open <http://localhost:8000>. Edit `app.py`. Refresh the browser. That is the
loop — no build, no board, no cable.

The server reloads `app.py` on every request, so a save is enough. A syntax
error appears as a traceback **in the browser** rather than killing the server:

```text
app.py failed to load:

  File "app.py", line 42
    return 200 "text/html", body
               ^^^^^^^^^^^
SyntaxError: invalid syntax
```

Fix it, refresh, carry on.

## 2. Deploy to the board

```powershell
.\deploy.ps1 -Port COM6
```

```text
checking for MicroPython on COM6 ...
  -> compat.py
  -> app.py
  -> config.py
  -> main.py

resetting ...
```

About three seconds. No rebuild, no button-holding — the interpreter is already
on the board and only your files change.

## 3. Look at it

**<http://esp32.local>**

Or watch it boot over USB:

```powershell
python -m mpremote connect COM6 repl
```

```text
connecting to SSID 'Fios-Martin' ...
got IP: 192.168.1.157
MAC:    80:65:99:f0:1c:9c   <- use this for a DHCP reservation

  reachable at:
    http://esp32.local     <- survives an IP change
    http://192.168.1.157
```

`Ctrl+]` exits the REPL.

## What to edit

**`app.py`, almost always.** It holds the routes and the HTML, and it is the
file that runs in both places.

Adding a page means adding to `route()`:

```python
def route(path):
    if path == "/":
        return 200, "text/html", page_index()

    if path == "/hello":                      # <- new
        return 200, "text/plain", "hi there\n"

    if path == "/health":
        return 200, "text/plain", "ok\n"

    return 404, "text/plain", "not found\n"
```

Save, refresh <http://localhost:8000/hello>, then deploy when happy.

The other files rarely change:

| File | When you would touch it |
|---|---|
| `app.py` | constantly — pages and routes |
| `compat.py` | new PC/board difference to hide |
| `main.py` | WiFi behaviour, hostname, port |
| `dev_server.py` | almost never |
| `config.py` | changing networks |

## Checking without a browser

```powershell
Invoke-WebRequest http://esp32.local/health -UseBasicParsing | Select-Object -ExpandProperty Content
```

```text
ok
```

That is what `/health` is for — a plain-text endpoint that scripts can read
without parsing HTML.

## When something is wrong

**Board not responding after a deploy?** Watch it boot — the REPL shows the
error:

```powershell
python -m mpremote connect COM6 repl
```

Then tap RESET with the REPL open and read the traceback. Unlike C, a crash
prints a normal Python traceback with a line number.

**Deploy says "No MicroPython REPL"?** Either the port is wrong (check
`GetPortNames()`), or something else is holding it open — a REPL or monitor in
another window. Only one program can use a serial port at a time.

**Page loads but looks stale?** The board caches nothing, but browsers do.
Hard-refresh with `Ctrl+F5`.
