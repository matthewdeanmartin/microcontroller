# Activates ESP-IDF 5.5.3 in the current PowerShell session.
#
#   Usage:  . .\idf-env.ps1      <-- note the leading dot (dot-source it,
#                                    so the env vars persist in your shell)
#
# Works around two gotchas on this machine:
#   1. export.ps1 picks up system Python 3.12, but the venv is py3.11.
#      Putting the bundled Python first on PATH fixes the detection.
#   2. idf_tools.py aborts if MSYSTEM is set (i.e. launched from Git Bash).

# Catch the most common mistake: running the script instead of dot-sourcing it.
# Without the leading dot this runs in a child process, prints a cheerful
# "Done!", and leaves the calling shell completely unchanged.
if ($MyInvocation.InvocationName -ne '.') {
    Write-Host ""
    Write-Host "  This script must be DOT-SOURCED to have any effect." -ForegroundColor Yellow
    Write-Host "  Note the leading dot and the space:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "      . $($MyInvocation.MyCommand.Path)" -ForegroundColor Green
    Write-Host ""
    exit 1
}

$IdfPath   = 'C:\Espressif\frameworks\esp-idf-v5.5.3'
$ToolsPath = 'C:\Espressif'
$PyEnv     = 'C:\Espressif\python_env\idf5.5_py3.11_env'
$PyHome    = 'C:\Espressif\tools\idf-python\3.11.2'

# MSYSTEM is inherited by any shell launched from (or alongside) Git Bash, and
# ESP-IDF refuses to run when it is set. Clearing it in this session is enough;
# it is not persisted in the registry, so nothing else is affected.
if ($env:MSYSTEM) {
    Write-Host "  Clearing inherited MSYSTEM=$($env:MSYSTEM) (ESP-IDF rejects it)." -ForegroundColor DarkGray
    Remove-Item Env:MSYSTEM -ErrorAction SilentlyContinue
}

# Drop Git Bash's unix tools from PATH; they confuse the IDF build.
# -notlike, not -notmatch: these are wildcard patterns, so the backslashes
# are literal and need no regex escaping.
$clean = ($env:PATH -split ';' | Where-Object {
              $_ -notlike '*\Git\usr\bin*' -and
              $_ -notlike '*\Git\mingw*'   -and
              $_ -notlike '*msys*'
          }) -join ';'

$env:PATH               = "$PyHome;$clean"
$env:IDF_PATH           = $IdfPath
$env:IDF_TOOLS_PATH     = $ToolsPath
$env:IDF_PYTHON_ENV_PATH= $PyEnv

& "$IdfPath\export.ps1"

# export.ps1 has now put the virtualenv's Scripts dir on PATH. $PyHome must come
# back off the front: it is the bare bundled interpreter with no packages
# installed, so leaving it ahead of the venv makes idf.py run without click.
# It was only needed so export.ps1 would detect Python 3.11 instead of the
# system-wide 3.12.
$env:PATH = ($env:PATH -split ';' | Where-Object { $_ -ne $PyHome }) -join ';'

# Fail loudly here rather than letting idf.py die later with a confusing
# "No module named 'click'".
$resolved = (Get-Command python -ErrorAction SilentlyContinue).Source
if ($resolved -notlike "$PyEnv*") {
    Write-Host ""
    Write-Host "  WARNING: 'python' resolves to" -ForegroundColor Red
    Write-Host "      $resolved" -ForegroundColor Red
    Write-Host "  but should be inside" -ForegroundColor Red
    Write-Host "      $PyEnv" -ForegroundColor Red
    Write-Host "  idf.py will fail. Something else is prepending to PATH." -ForegroundColor Red
    Write-Host ""
}
