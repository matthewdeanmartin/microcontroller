param([string]$Port)
$ErrorActionPreference = 'Stop'
if (-not $Port) {
    $ports = @(python -c "import serial.tools.list_ports; print('\n'.join(p.device for p in serial.tools.list_ports.comports() if p.vid == 0x303a))")
    $ports = @($ports | Where-Object { $_ })
    if ($ports.Count -ne 1) { throw 'Connect the C6 using a USB data cable, then run .\deploy.ps1 -Port COMn.' }
    $Port = $ports[0]
}
& "$PSScriptRoot\build.ps1"
Push-Location $PSScriptRoot
try {
    # IDF/esptool checks the connected chip against the esp32c6 target before flashing.
    idf.py -p $Port flash
    if ($LASTEXITCODE -ne 0) { throw 'Firmware flashing failed' }
    Write-Host "Flashed $Port. Run idf.py -p $Port monitor to read the IP and POST log."
} finally { Pop-Location }
