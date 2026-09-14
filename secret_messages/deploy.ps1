# Copies the app onto the board over USB. No re-flash, no rebuild.
#
#   .\deploy.ps1 -Port COM5
#
# Takes a couple of seconds, versus the C project's build/flash/reset cycle.
# This is the payoff of MicroPython: the firmware stays put and only your
# Python files change.

param(
    [Parameter(Mandatory = $true)]
    [string]$Port,

    # Skip the reset, e.g. when copying files to inspect from the REPL.
    [switch]$NoReset
)

$ErrorActionPreference = 'Stop'

# Invoke tools as "<python> -m mpremote" rather than relying on a bare
# `uv` or `mpremote` being on PATH. PATH here varies: an ESP-IDF-activated
# shell puts the IDF virtualenv first, a fresh shell has system Python, and
# neither necessarily exposes uv's shims. Resolving an interpreter explicitly
# sidesteps all of it.
function Find-Python {
    # Prefer whatever `python` currently resolves to, so the script follows
    # the shell the user is actually in.
    $candidates = @()
    $current = (Get-Command python -ErrorAction SilentlyContinue).Source
    if ($current) { $candidates += $current }
    $candidates += 'C:\Users\matth\AppData\Local\Programs\Python\Python312\python.exe'
    $candidates += 'C:\Espressif\python_env\idf5.5_py3.11_env\Scripts\python.exe'

    foreach ($py in $candidates) {
        if (-not (Test-Path $py)) { continue }
        & $py -m mpremote --version *> $null
        if ($LASTEXITCODE -eq 0) { return $py }
    }
    return $null
}

$PY = Find-Python
if (-not $PY) {
    Write-Host "Could not find a Python with mpremote installed." -ForegroundColor Red
    Write-Host "  Install it with:  python -m pip install mpremote esptool"
    exit 1
}

# Every module the board imports, in dependency order. Note what is NOT
# here: aes_soft.py, the pure-Python AES used only by the dev server.
# MicroPython has cryptolib in C, so shipping 6KB of unused interpreted
# crypto to the machine least able to afford it would be careless.
$files = @(
    'compat.py',
    'crypto.py',
    'http_parse.py',
    'store.py',
    'notify_js.py',
    'ui.py',
    'app.py',
    'config.py',
    'main.py'
)

foreach ($f in $files) {
    if (-not (Test-Path $f)) {
        if ($f -eq 'config.py') {
            Write-Host "config.py not found." -ForegroundColor Red
            Write-Host "  Copy config_example.py to config.py and add your WiFi details."
            exit 1
        }
        Write-Host "missing: $f" -ForegroundColor Red
        exit 1
    }
}

# Confirm MicroPython is actually on the board before copying anything.
# Without this, a board still running C firmware just times out, which reads
# like a cable or port problem rather than "wrong firmware".
Write-Host "checking for MicroPython on $Port ..."
& $PY -m mpremote connect $Port eval "1+1" *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "`nNo MicroPython REPL on $Port." -ForegroundColor Red
    Write-Host ""
    Write-Host "  If this board still has the C firmware, flash MicroPython first:"
    Write-Host "    1. Hold BOOT (0), tap RESET (RST), release BOOT"
    Write-Host "    2. [System.IO.Ports.SerialPort]::GetPortNames()"
    Write-Host "    3. .\flash_micropython.ps1 -Port COM4"
    Write-Host "    4. Tap RESET, re-check the port, then run this script again."
    Write-Host ""
    Write-Host "  Otherwise: wrong port, or something else is holding it open"
    Write-Host "  (a REPL or monitor in another window)."
    exit 1
}

foreach ($f in $files) {
    Write-Host "  -> $f"
    & $PY -m mpremote connect $Port fs cp $f ":$f"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "failed copying $f" -ForegroundColor Red
        exit 1
    }
}

if (-not $NoReset) {
    Write-Host "`nresetting ..."
    & $PY -m mpremote connect $Port reset
}

Write-Host "`nDone. Watch it boot with:" -ForegroundColor Green
Write-Host "  $PY -m mpremote connect $Port repl"
Write-Host "(Ctrl+] to exit the REPL)"
