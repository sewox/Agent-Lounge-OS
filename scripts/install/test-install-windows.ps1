# Windows installer smoke: real NSIS path (no LOUNGE_FIXTURE_SKIP_NSIS).
# install → exact agent-lounge-os.exe → --version → registry/shortcuts →
# uninstall (NSIS) → assert cleanup + checksum-mismatch rejection.
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$FixtureDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-fixture-" + [guid]::NewGuid())
$Prefix = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-prefix-" + [guid]::NewGuid())
$Version = '0.0.0-fixture'
$ProductName = 'Agent Lounge OS'
$MainExeName = 'agent-lounge-os.exe'
$UninstallRegPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"
$StartMenuDir = Join-Path ([Environment]::GetFolderPath('StartMenu')) "Programs\$ProductName"
$StartMenuShortcut = Join-Path $StartMenuDir "$ProductName.lnk"

try {
  & "$Root\scripts\install\fixture-nsis\build-fixture-nsis.ps1" -OutDir $FixtureDir -Version $Version

  # Must NOT set LOUNGE_FIXTURE_SKIP_NSIS — exercise the real NSIS path.
  $env:LOUNGE_DOWNLOAD_DIR = $FixtureDir
  $env:LOUNGE_NO_MODIFY_PATH = '1'
  Remove-Item Env:LOUNGE_FIXTURE_SKIP_NSIS -ErrorAction SilentlyContinue

  Write-Host "== install (real NSIS) =="
  & "$Root\install.ps1" -Prefix $Prefix -Version $Version

  $MainExe = Join-Path $Prefix "app\$MainExeName"
  if (-not (Test-Path -LiteralPath $MainExe)) {
    throw "missing exact main exe $MainExe"
  }
  # Guard against accidental *.exe fallback: only the named binary counts.
  $foreign = Get-ChildItem -Path (Join-Path $Prefix 'app') -Filter '*.exe' |
    Where-Object { $_.Name -ne $MainExeName -and $_.Name -ne 'uninstall.exe' }
  if ($foreign) {
    throw ("unexpected extra exe(s) (installer must not rely on first *.exe): " + ($foreign.Name -join ', '))
  }

  if (-not (Test-Path -LiteralPath $UninstallRegPath)) {
    throw "HKCU uninstall entry missing: $UninstallRegPath"
  }
  $display = (Get-ItemProperty -LiteralPath $UninstallRegPath).DisplayName
  if ($display -ne $ProductName) {
    throw "DisplayName mismatch: $display"
  }
  if (-not (Test-Path -LiteralPath $StartMenuShortcut)) {
    throw "Start Menu shortcut missing: $StartMenuShortcut"
  }

  $Cli = Join-Path $Prefix 'bin\agent-lounge-os.cmd'
  if (-not (Test-Path -LiteralPath $Cli)) { throw "missing CLI shim $Cli" }

  Write-Host "== --version =="
  $ver = & cmd.exe /c "`"$Cli`" --version"
  Write-Host $ver
  if ("$ver" -notmatch 'Agent Lounge OS') { throw "unexpected --version: $ver" }

  $InstalledPaths = @(
    $MainExe,
    (Join-Path $Prefix 'app\uninstall.exe'),
    $Cli,
    (Join-Path $Prefix "share\agent-lounge-os\Agent-Lounge-OS-windows-x64-setup.exe"),
    $StartMenuShortcut
  )

  Write-Host "== checksum mismatch rejection =="
  $BadDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-bad-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Path $BadDir | Out-Null
  Copy-Item -Force (Join-Path $FixtureDir '*') $BadDir
  Copy-Item -Force (Join-Path $FixtureDir 'SHA256SUMS.bad') (Join-Path $BadDir 'SHA256SUMS')
  $BadPrefix = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-bad-prefix-" + [guid]::NewGuid())
  $env:LOUNGE_DOWNLOAD_DIR = $BadDir
  $failed = $false
  try {
    & "$Root\install.ps1" -Prefix $BadPrefix
    $failed = $true
  } catch {
    if ("$($_.Exception.Message)$($_)" -notmatch 'checksum mismatch') {
      throw "expected checksum mismatch, got: $($_.Exception.Message) $_"
    }
    Write-Host "ok: checksum mismatch rejected"
  }
  if ($failed) { throw 'install succeeded with bad SHA256SUMS' }
  Remove-Item -Recurse -Force $BadDir, $BadPrefix -ErrorAction Stop

  Write-Host "== uninstall (NSIS) =="
  $env:LOUNGE_DOWNLOAD_DIR = $FixtureDir
  & "$Root\install.ps1" -Uninstall -Prefix $Prefix

  foreach ($p in $InstalledPaths) {
    if (Test-Path -LiteralPath $p) {
      throw "still present after uninstall: $p"
    }
  }
  if (Test-Path -LiteralPath $UninstallRegPath) {
    throw "HKCU uninstall key still present after uninstall"
  }
  if (Test-Path -LiteralPath $StartMenuDir) {
    throw "Start Menu folder still present after uninstall"
  }

  Write-Host 'ALL OK (windows real NSIS installer fixture)'
} finally {
  Remove-Item -Recurse -Force $FixtureDir, $Prefix -ErrorAction SilentlyContinue
  Remove-Item Env:LOUNGE_DOWNLOAD_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:LOUNGE_NO_MODIFY_PATH -ErrorAction SilentlyContinue
  Remove-Item Env:LOUNGE_FIXTURE_SKIP_NSIS -ErrorAction SilentlyContinue
}
