# Windows installer smoke against a local fixture release (no network).
# install → verify → --version → --uninstall + checksum-mismatch rejection.
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$FixtureDir = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-fixture-" + [guid]::NewGuid())
$Prefix = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-prefix-" + [guid]::NewGuid())

try {
  & bash "$Root/scripts/install/build-fixture-release.sh" "$FixtureDir"
  if ($LASTEXITCODE -ne 0) { throw "build-fixture-release.sh failed: $LASTEXITCODE" }

  $env:LOUNGE_DOWNLOAD_DIR = $FixtureDir
  $env:LOUNGE_FIXTURE_SKIP_NSIS = '1'
  $env:LOUNGE_NO_MODIFY_PATH = '1'

  Write-Host "== install =="
  & "$Root/install.ps1" -Prefix $Prefix -Version '0.0.0-fixture'
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
