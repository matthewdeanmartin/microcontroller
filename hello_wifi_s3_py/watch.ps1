# Polls the board and logs every result, so an intermittent fault becomes a
# number instead of a feeling.
#
#   .\watch.ps1                        # default: esp32.local, every 5s
#   .\watch.ps1 -Target 192.168.1.157  # skip mDNS, test the IP directly
#   .\watch.ps1 -IntervalSeconds 2 -LogFile watch.csv
#
# Leave it running, move the board, change the channel - then compare the
# success rate before and after. Ctrl+C to stop; it prints a summary.

param(
    [string]$Target = "esp32.local",
    [int]$IntervalSeconds = 5,
    [string]$LogFile = ""
)

$url = "http://$Target/health"
$ok = 0
$fail = 0
$rssiValues = @()

Write-Host "polling $url every ${IntervalSeconds}s"
Write-Host "Ctrl+C to stop and see a summary`n"

if ($LogFile) {
    "timestamp,result,ms,rssi,detail" | Set-Content $LogFile -Encoding utf8
    Write-Host "logging to $LogFile`n"
}

try {
    while ($true) {
        $stamp = Get-Date -Format 'HH:mm:ss'
        $sw = [System.Diagnostics.Stopwatch]::StartNew()

        # curl rather than Invoke-WebRequest: it is a thinner client, and this
        # script exists to measure the board, not .NET's HTTP stack.
        $body = curl.exe -s --max-time 10 $url 2>&1
        $sw.Stop()
        $ms = [int]$sw.ElapsedMilliseconds

        if ($LASTEXITCODE -eq 0 -and $body -match 'ok') {
            $ok++
            # /health returns e.g. "ok rssi=-60"
            $r = if ($body -match 'rssi=(-?\d+)') { [int]$Matches[1] } else { $null }
            if ($null -ne $r) { $rssiValues += $r }

            $rssiText = if ($null -ne $r) { "$r dBm" } else { "" }
            Write-Host ("{0}  OK    {1,5} ms  {2}" -f $stamp, $ms, $rssiText) -ForegroundColor Green
            if ($LogFile) { "$stamp,ok,$ms,$r," | Add-Content $LogFile }
        }
        else {
            $fail++
            $detail = ($body | Out-String).Trim() -replace ',', ';' -replace "`r?`n", ' '
            Write-Host ("{0}  FAIL  {1,5} ms  {2}" -f $stamp, $ms, $detail) -ForegroundColor Red
            if ($LogFile) { "$stamp,fail,$ms,,$detail" | Add-Content $LogFile }
        }

        Start-Sleep -Seconds $IntervalSeconds
    }
}
finally {
    $total = $ok + $fail
    if ($total -gt 0) {
        $rate = [math]::Round(100 * $ok / $total, 1)
        Write-Host "`n--- summary ---"
        Write-Host "  attempts : $total"
        Write-Host "  ok       : $ok ($rate%)"
        Write-Host "  failed   : $fail"
        if ($rssiValues.Count -gt 0) {
            $stats = $rssiValues | Measure-Object -Average -Minimum -Maximum
            Write-Host ("  rssi     : avg {0} dBm, range {1}..{2}" -f `
                [math]::Round($stats.Average, 1), $stats.Minimum, $stats.Maximum)
        }
        if ($LogFile) { Write-Host "  log      : $LogFile" }
    }
}
