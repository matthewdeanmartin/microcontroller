# One-time: erase the board and install MicroPython.
#
#   .\flash_micropython.ps1 -Port COM8
#
# Use the CH343 BRIDGE port (COM8), not the native USB one. The bridge drives
# BOOT and RESET over RTS/DTR, so no buttons need pressing - unlike the S2,
# where the BOOT/RESET dance is mandatory and the port moves afterwards.
#
# Two things differ from the S2 and both are silent failures if got wrong:
#   * offset 0x0, not 0x1000 - the S3 bootloader lives at zero
#   * the SPIRAM_OCT build - N16R8 is octal PSRAM; the plain build leaves
#     the 8MB switched off and reports ~167KB of heap instead of 8.3MB
#
# This ERASES whatever is on the board, including any app files.

param(
    [Parameter(Mandatory = $true)]
    [string]$Port
)

$ErrorActionPreference = 'Stop'

function Find-Python {
    $candidates = @()
    $current = (Get-Command python -ErrorAction SilentlyContinue).Source
    if ($current) { $candidates += $current }
    $candidates += 'C:\Users\matth\AppData\Local\Programs\Python\Python312\python.exe'
    $candidates += 'C:\Espressif\python_env\idf5.5_py3.11_env\Scripts\python.exe'

    foreach ($py in $candidates) {
        if (-not (Test-Path $py)) { continue }
        & $py -m esptool version *> $null
        if ($LASTEXITCODE -eq 0) { return $py }
    }
    return $null
}

$PY = Find-Python
if (-not $PY) {
    Write-Host "Could not find a Python with esptool installed." -ForegroundColor Red
    Write-Host "  Install it with:  python -m pip install mpremote esptool"
    exit 1
}

$firmware = Get-ChildItem "firmware\ESP32_GENERIC_S3-SPIRAM_OCT-*.bin" -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending | Select-Object -First 1

if (-not $firmware) {
    Write-Host "No firmware found in firmware\" -ForegroundColor Red
    Write-Host "  Download from https://micropython.org/download/ESP32_GENERIC_S3/"
    exit 1
}

Write-Host "firmware: $($firmware.Name)"
Write-Host "port:     $Port"
Write-Host ""
Write-Host "This ERASES the board, including the C firmware." -ForegroundColor Yellow
$reply = Read-Host "Continue? (y/N)"
if ($reply -ne 'y') {
    Write-Host "cancelled."
    exit 0
}

Write-Host "`nerasing ..."
& $PY -m esptool --chip esp32s3 --port $Port erase-flash
if ($LASTEXITCODE -ne 0) {
    Write-Host "`nErase failed." -ForegroundColor Red
    Write-Host "  Is $Port the CH343 bridge port? Check with:"
    Write-Host "    Get-CimInstance Win32_PnPEntity | ? { `$_.Name -match 'COM\d+' }"
    Write-Host "  The bridge shows as 'USB-Enhanced-SERIAL CH343' (VID 1A86)."
    Write-Host "  The native USB port (VID 303A) cannot auto-reset into the bootloader;"
    Write-Host "  on that one you would need the BOOT/RESET buttons."
    exit 1
}

# On the S2 the port vanished here, because native USB dies with an empty
# flash. The CH343 bridge is a separate chip and stays enumerated throughout,
# so we can go straight on to writing - no re-plug, no buttons, no re-detect.
Write-Host "`nwriting firmware to $Port ..."
& $PY -m esptool --chip esp32s3 --port $Port --baud 460800 `
    write-flash -z 0x0 $firmware.FullName
if ($LASTEXITCODE -ne 0) {
    Write-Host "`nFlash failed." -ForegroundColor Red
    Write-Host "  The flash is empty until this succeeds, but the CH343 bridge keeps"
    Write-Host "  $Port alive regardless, so this is recoverable - just run it again."
    exit 1
}

Write-Host "`nDone." -ForegroundColor Green
Write-Host "  .\deploy.ps1 -Port $Port"
Write-Host ""
Write-Host "  The bridge port does not move, so it is still $Port." -ForegroundColor DarkGray
