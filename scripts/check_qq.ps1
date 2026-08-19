$results = @()
$keys = @('HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall', 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
foreach ($k in $keys) {
    if (Test-Path $k) {
        Get-ChildItem $k -ErrorAction SilentlyContinue | ForEach-Object {
            $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
            if ($p.DisplayName -match 'QQ|Tencent') {
                $results += [PSCustomObject]@{ Name = $p.DisplayName; Ver = $p.DisplayVersion; Loc = $p.InstallLocation }
            }
        }
    }
}
if ($results) { $results | Format-Table -AutoSize | Out-String -Width 300 } else { Write-Host 'No QQ uninstall entries found' }