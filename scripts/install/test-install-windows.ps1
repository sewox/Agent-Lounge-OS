# Windows installer smoke against a local fixture release (no network).
# Builds fixtures in PowerShell (avoids Git Bash tar "C:" host traps).
# install → verify → --version → --uninstall + checksum-mismatch rejection.
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$FixtureDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-fixture-" + [guid]::NewGuid())
$Prefix = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-prefix-" + [guid]::NewGuid())
$Version = '0.0.0-fixture'

function New-WindowsStub([string]$Path, [string]$Ver) {
  $content = @"
@echo off
if "%~1"=="--version" (
  echo Agent Lounge OS $Ver
  exit /b 0
)
echo Agent Lounge OS fixture Windows payload
exit /b 0
"@
  Set-Content -Path $Path -Value $content -Encoding Ascii
}

function Write-Sha256Sums([string]$Dir) {
  $names = @(
    'Agent-Lounge-OS-windows-x64-setup.exe',
    'Agent-Lounge-OS-windows-arm64-setup.exe'
  )
  $lines = foreach ($name in $names) {
    $p = Join-Path $Dir $name
    if (-not (Test-Path -LiteralPath $p)) { continue }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash.ToLower()
    "{0}  {1}" -f $hash, $name
  }
  $sums = Join-Path $Dir 'SHA256SUMS'
  Set-Content -Path $sums -Value $lines -Encoding Ascii
  $bad = foreach ($line in $lines) {
    $line -replace '^[0-9a-fA-F]{64}', 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef'
  }
  Set-Content -Path (Join-Path $Dir 'SHA256SUMS.bad') -Value $bad -Encoding Ascii
}

try {
  New-Item -ItemType Directory -Force -Path $FixtureDir | Out-Null
  New-WindowsStub -Path (Join-Path $FixtureDir 'Agent-Lounge-OS-windows-x64-setup.exe') -Ver $Version
  New-WindowsStub -Path (Join-Path $FixtureDir 'Agent-Lounge-OS-windows-arm64-setup.exe') -Ver $Version
  Set-Content -Path (Join-Path $FixtureDir 'VERSION') -Value $Version -Encoding Ascii
  Write-Sha256Sums -Dir $FixtureDir

  $env:LOUNGE_DOWNLOAD_DIR = $FixtureDir
  $env:LOUNGE_FIXTURE_SKIP_NSIS = '1'
  $env:LOUNGE_NO_MODIFY_PATH = '1'

  Write-Host "== install =="
  & "$Root/install.ps1" -Prefix $Prefix -Version $Version
  $Cli = Join-Path $Prefix 'bin\agent-lounge-os.cmd'
  if (-not (Test-Path -LiteralPath $Cli)) { throw "missing CLI shim $Cli" }

  Write-Host "== --version =="
  $ver = & cmd.exe /c "`"$Cli`" --version"
  Write-Host $ver
  if ("$ver" -notmatch 'Agent Lounge OS') { throw "unexpected --version: $ver" }

  Write-Host "== checksum mismatch rejection =="
  $BadDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-bad-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Path $BadDir | Out-Null
  Copy-Item -Force (Join-Path $FixtureDir '*') $BadDir
  Copy-Item -Force (Join-Path $FixtureDir 'SHA256SUMS.bad') (Join-Path $BadDir 'SHA256SUMS')
  $BadPrefix = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-bad-prefix-" + [guid]::NewGuid())
  $env:LOUNGE_DOWNLOAD_DIR = $BadDir
  $failed = $false
  try {
    & "$Root/install.ps1" -Prefix $BadPrefix
    $failed = $true
  } catch {
    if ("$($_.Exception.Message)$($_)" -notmatch 'checksum mismatch') {
      throw "expected checksum mismatch, got: $($_.Exception.Message) $_"
    }
    Write-Host "ok: checksum mismatch rejected"
  }
  if ($failed) { throw 'install succeeded with bad SHA256SUMS' }
  Remove-Item -Recurse -Force $BadDir, $BadPrefix -ErrorAction SilentlyContinue

  Write-Host "== uninstall =="
  $env:LOUNGE_DOWNLOAD_DIR = $FixtureDir
  & "$Root/install.ps1" -Uninstall -Prefix $Prefix
  if (Test-Path -LiteralPath $Cli) { throw 'CLI still present after uninstall' }

  Write-Host 'ALL OK (windows installer fixture)'
} finally {
  Remove-Item -Recurse -Force $FixtureDir, $Prefix -ErrorAction SilentlyContinue
  Remove-Item Env:LOUNGE_DOWNLOAD_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:LOUNGE_FIXTURE_SKIP_NSIS -ErrorAction SilentlyContinue
  Remove-Item Env:LOUNGE_NO_MODIFY_PATH -ErrorAction SilentlyContinue
}
