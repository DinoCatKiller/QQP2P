$self = Get-CimInstance Win32_Process -Filter "ProcessId = $PID" | Select-Object -ExpandProperty CommandLine
$matched = Get-CimInstance Win32_Process | Where-Object {
    ($_.CommandLine -match 'NapCat' -or $_.CommandLine -match 'napcat') -and
    ($_.CommandLine -notmatch 'stop_napcat')
}
if ($matched) {
    Write-Host 'Found napcat processes:'
    $matched | ForEach-Object {
        Write-Host ("  PID {0} | {1} | {2}" -f $_.ProcessId, $_.Name, $_.CommandLine)
    }
    $matched | ForEach-Object {
        Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        Write-Host ("Stopped PID {0} ({1})" -f $_.ProcessId, $_.Name)
    }
    Start-Sleep -Seconds 1
    $remaining = Get-CimInstance Win32_Process | Where-Object {
        ($_.CommandLine -match 'NapCat' -or $_.CommandLine -match 'napcat') -and
        ($_.CommandLine -notmatch 'stop_napcat')
    }
    if ($remaining) {
        Write-Host 'Some processes still running:'
        $remaining | ForEach-Object { Write-Host ("  STILL RUNNING PID {0} | {1}" -f $_.ProcessId, $_.Name) }
    } else {
        Write-Host 'All napcat processes stopped.'
    }
} else {
    Write-Host 'No napcat processes found.'
}