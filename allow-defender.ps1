# Allow this project's binaries through Windows Defender (false positive Bearfoos).
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$ErrorActionPreference = "Continue"
Add-MpPreference -ExclusionPath $root
Add-MpPreference -ExclusionProcess "ioscpy.exe"
Write-Host "Exclusion added: $root"
try {
    Get-MpThreat -ErrorAction SilentlyContinue | ForEach-Object {
        $text = ($_.Resources | Out-String)
        if ($text -match "ioscpy") {
            Remove-MpThreat -ThreatID $_.ThreatID -ErrorAction SilentlyContinue
        }
    }
} catch {}
