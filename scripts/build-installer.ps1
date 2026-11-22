<#
.SYNOPSIS
  Builds the Reclaim installer (and the portable zip) from a fresh clone.

.DESCRIPTION
  1. Builds reclaim.exe and reclaim-cli.exe (cargo, release profile).
  2. Writes THIRD_PARTY_NOTICES.txt (scripts/third_party_notices.py).
  3. Compiles installer/reclaim.iss with Inno Setup 6.
  4. Packs the same files into the portable zip.

  Missing Inno Setup 6 or Python 3 are installed with winget (per user, no admin) unless
  -NoInstallTools is given. Rust (rustup) and the MSVC build tools must already be there,
  as for any build of Reclaim.

  Output: dist\reclaim-<version>-windows-x64-setup.exe and dist\reclaim-<version>-windows-x64.zip

.EXAMPLE
  scripts\build-installer.cmd
.EXAMPLE
  scripts\build-installer.cmd -Run          # build, then start the installer
#>
[CmdletBinding()]
param(
    # Where the installer and zip go (relative to the repository root).
    [string]$OutDir = "dist",
    # Reuse binaries already in target\release.
    [switch]$SkipBuild,
    # Do not install missing tools; fail instead.
    [switch]$NoInstallTools,
    # Start the installer when it is built.
    [switch]$Run
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }

function Install-WithWinget($id, $what) {
    if ($NoInstallTools) { throw "$what is not installed (and -NoInstallTools was given)." }
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw "$what is not installed and winget is not available. Install $what by hand and run again."
    }
    Step "Installing $what with winget ($id)"
    & winget install --id $id --exact --scope user --silent --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE -ne 0) {
        # Some packages have no per-user installer: try the default scope.
        & winget install --id $id --exact --silent --accept-package-agreements --accept-source-agreements
        if ($LASTEXITCODE -ne 0) { throw "winget could not install $what (exit code $LASTEXITCODE)." }
    }
}

function Find-Iscc {
    $cmd = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe")
    )
    foreach ($c in $candidates) { if ($c -and (Test-Path $c)) { return $c } }
    return $null
}

function Find-Python {
    foreach ($name in @("python", "py")) {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        # Skip the Microsoft Store alias, which only opens the Store.
        if ($cmd -and $cmd.Source -notmatch "WindowsApps") {
            if ($name -eq "py") { return @($cmd.Source, "-3") }
            return $cmd.Source
        }
    }
    $local = Get-ChildItem (Join-Path $env:LOCALAPPDATA "Programs\Python") -Filter python.exe -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($local) { return $local.FullName }
    # No output: the caller's @(...) is then empty.
}

# --- Tools -----------------------------------------------------------------------------
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is not installed. Install Rust from https://rustup.rs (with the MSVC build tools) and run again."
}
$iscc = Find-Iscc
if (-not $iscc) {
    Install-WithWinget "JRSoftware.InnoSetup" "Inno Setup 6"
    $iscc = Find-Iscc
    if (-not $iscc) { throw "Inno Setup 6 was installed but ISCC.exe was not found." }
}
# @(...): a single path must stay an array (exe, then optional arguments such as "-3").
$python = @(Find-Python)
if ($python.Count -eq 0) {
    Install-WithWinget "Python.Python.3.12" "Python 3"
    $python = @(Find-Python)
    if ($python.Count -eq 0) { throw "Python 3 was installed but python.exe was not found. Open a new terminal and run again." }
}

# --- Build -----------------------------------------------------------------------------
$version = ((& cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).packages |
    Where-Object { $_.name -eq "reclaim-app" }).version
if (-not $version) { throw "Could not read the version of reclaim-app." }
$name = "reclaim-$version-windows-x64"
Step "Reclaim $version"

if (-not $SkipBuild) {
    # A Reclaim started from target\ (a dev build, or autostart pointing there) locks the exe
    # and cargo cannot replace it ("Access is denied"). Stop only those copies.
    $targetDir = (Join-Path $root "target") + "\"
    $running = @(Get-Process -Name reclaim, reclaim-cli -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($targetDir, [StringComparison]::OrdinalIgnoreCase) })
    foreach ($p in $running) {
        Step "Stopping $($p.Path) (pid $($p.Id)) so it can be rebuilt"
        Stop-Process -Id $p.Id -Force
        $p.WaitForExit(5000) | Out-Null
    }
    Step "Building reclaim.exe and reclaim-cli.exe (release)"
    & cargo build --release --locked -p reclaim-app -p reclaim-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed." }
}

# Only this version's folder: CI caches restore old ones, and later steps pick the only one.
$stageRoot = Join-Path $root "target\installer"
if (Test-Path $stageRoot) { Remove-Item -Recurse -Force $stageRoot }
$stage = Join-Path $stageRoot $name
New-Item -ItemType Directory -Force $stage | Out-Null

Step "Writing third-party notices"
$pyExe = $python[0]
$pyArgs = @($python | Select-Object -Skip 1) + @("scripts/third_party_notices.py", "--out", (Join-Path $stage "THIRD_PARTY_NOTICES.txt"))
& $pyExe @pyArgs
if ($LASTEXITCODE -ne 0) { throw "third_party_notices.py failed." }

Copy-Item target\release\reclaim.exe, target\release\reclaim-cli.exe, LICENSE, README.md $stage

# --- Installer and zip -----------------------------------------------------------------
$out = Join-Path $root $OutDir
New-Item -ItemType Directory -Force $out | Out-Null

Step "Compiling the installer ($iscc)"
& $iscc /Q "/DAppVersion=$version" "/DStageDir=$stage" "/DOutputDir=$out" installer\reclaim.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed (exit code $LASTEXITCODE)." }
$setup = Join-Path $out "$name-setup.exe"

Step "Packing the portable zip"
$zip = Join-Path $out "$name.zip"
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -Force

Write-Host ""
Write-Host "Installer: $setup" -ForegroundColor Green
Write-Host "Portable:  $zip" -ForegroundColor Green

if ($Run) {
    Step "Starting the installer"
    Start-Process -FilePath $setup
}
