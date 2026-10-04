param([switch]$SkipConfigure)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    . "$PSScriptRoot\..\idf-env.ps1"
    if (-not $SkipConfigure) {
        python configure.py
        if ($LASTEXITCODE -ne 0) { throw 'Wi-Fi configuration failed' }
    }
    python generate_font.py
    if ($LASTEXITCODE -ne 0) { throw 'Font generation failed' }
    $env:IDF_TARGET = 'esp32c6'
    idf.py build
    if ($LASTEXITCODE -ne 0) { throw 'Firmware build failed' }
} finally { Pop-Location }
