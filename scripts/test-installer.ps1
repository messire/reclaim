<#
.SYNOPSIS
  End-to-end check of the Reclaim installer (used by CI; safe to run by hand in a VM).

.DESCRIPTION
  Installs silently into a temporary folder with start-with-Windows on, upgrades in place
  with a higher version and start-with-Windows off, upgrades again into another folder while
  Reclaim is running, then uninstalls. After each step it checks the files, the per-user Run
  value and that exactly one installation is registered.

  WARNING: it uninstalls any Reclaim already installed for the current user.
#>
param(
    [Parameter(Mandatory)] [string]$Setup,
    [Parameter(Mandatory)] [string]$NextSetup
)

$ErrorActionPreference = "Stop"
$guid = "{09543C3B-C0B8-43FA-BC08-E11A6FB50CD4}_is1"
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$guid"
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$base = Join-Path $env:TEMP "reclaim-installer-test"
$dirA = Join-Path $base "A"
$dirB = Join-Path $base "B"
$step = 0

function Check($ok, $what) {
    if (-not $ok) { throw "FAILED: $what" }
    Write-Host "  ok: $what"
}

# $tasks: "/TASKS=autostart" (on) or "/MERGETASKS=!autostart" (off).
function Install($exe, $dir, $tasks) {
    $script:step++
    $log = Join-Path $PWD "installer-test-$($script:step).log"
    Write-Host "==> $(Split-Path $exe -Leaf) into $dir ($tasks)"
    $p = Start-Process -FilePath $exe -Wait -PassThru -ArgumentList @(
        "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/DIR=`"$dir`"", $tasks, "/LOG=`"$log`"")
    Check ($p.ExitCode -eq 0) "setup exit code 0 (got $($p.ExitCode))"
}

function RunValue {
    (Get-ItemProperty -Path $runKey -Name Reclaim -ErrorAction SilentlyContinue).Reclaim
}

function Registered {
    @(Get-ChildItem "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall" |
        Where-Object { (Get-ItemProperty $_.PSPath).DisplayName -eq "Reclaim" })
}

if (Test-Path $base) { Remove-Item -Recurse -Force $base }

# 1. Fresh install, start with Windows on.
Install $Setup $dirA "/TASKS=autostart"
Check (Test-Path "$dirA\reclaim.exe") "reclaim.exe installed in A"
Check (Test-Path "$dirA\reclaim-cli.exe") "reclaim-cli.exe installed in A"
Check ((RunValue) -eq "`"$dirA\reclaim.exe`" --tray") "Run value points to A (got '$(RunValue)')"
Check ((Registered).Count -eq 1) "one installation registered"
$v1 = (Get-ItemProperty $uninstallKey).DisplayVersion
Write-Host "  installed version $v1"

# 2. Upgrade in place, start with Windows off.
Install $NextSetup $dirA "/MERGETASKS=!autostart"
Check ((Registered).Count -eq 1) "still one installation after upgrade"
Check ((Get-ItemProperty $uninstallKey).DisplayVersion -eq "99.0.0") "registered version is 99.0.0"
Check ($null -eq (RunValue)) "Run value removed when unticked"

# 3. Upgrade into another folder while Reclaim runs.
$proc = Start-Process -FilePath "$dirA\reclaim.exe" -ArgumentList "--tray" -PassThru
Start-Sleep -Seconds 3
Install $NextSetup $dirB "/TASKS=autostart"
Check ($proc.HasExited) "running Reclaim was closed by setup"
Check (-not (Test-Path "$dirA\reclaim.exe")) "old copy in A removed"
Check (Test-Path "$dirB\reclaim.exe") "reclaim.exe installed in B"
Check ((Registered).Count -eq 1) "one installation after moving"
Check ((Get-ItemProperty $uninstallKey).InstallLocation.TrimEnd('\') -eq $dirB) "registered location is B"
Check ((RunValue) -eq "`"$dirB\reclaim.exe`" --tray") "Run value points to B"

# 4. Uninstall.
$uninstaller = (Get-ItemProperty $uninstallKey).UninstallString.Trim('"')
Write-Host "==> uninstall"
$p = Start-Process -FilePath $uninstaller -Wait -PassThru -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"
Check ($p.ExitCode -eq 0) "uninstaller exit code 0"
# The uninstaller finishes from a copy in %TEMP%; wait for the folder to go.
for ($i = 0; $i -lt 20 -and (Test-Path "$dirB\reclaim.exe"); $i++) { Start-Sleep -Milliseconds 500 }
Check (-not (Test-Path "$dirB\reclaim.exe")) "files removed"
Check ($null -eq (RunValue)) "Run value removed"
Check ((Registered).Count -eq 0) "no installation registered"

Write-Host "installer test: all checks passed" -ForegroundColor Green
