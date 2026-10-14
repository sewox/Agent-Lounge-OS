# Build a minimal real NSIS setup.exe for installer CI (Windows).
# Output: <OutDir>/Agent-Lounge-OS-windows-x64-setup.exe (+ arm64 copy) + SHA256SUMS
param(
  [Parameter(Mandatory = $true)][string]$OutDir,
  [string]$Version = '0.0.0-fixture'
)

$ErrorActionPreference = 'Stop'
$Here = $PSScriptRoot
$Stage = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-nsis-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Stage, $OutDir | Out-Null

try {
  # Tiny console PE named exactly agent-lounge-os.exe (Tauri/Cargo main binary).
  # Use Framework csc.exe — pwsh/Add-Type -OutputAssembly is not available on PS 7.
  $exePath = Join-Path $Stage 'agent-lounge-os.exe'
  $csPath = Join-Path $Stage 'Program.cs'
  @"
using System;
public static class Program {
  public static int Main(string[] args) {
    if (args != null && args.Length > 0 && (args[0] == "--version" || args[0] == "-V")) {
      Console.WriteLine("Agent Lounge OS $Version");
      return 0;
    }
    Console.WriteLine("Agent Lounge OS fixture");
    return 0;
  }
}
"@ | Set-Content -Path $csPath -Encoding ASCII
  $csc = Get-ChildItem -Path "$env:WINDIR\Microsoft.NET\Framework64" -Filter csc.exe -Recurse -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $csc) {
    $csc = Get-ChildItem -Path "$env:WINDIR\Microsoft.NET\Framework" -Filter csc.exe -Recurse -ErrorAction SilentlyContinue |
      Sort-Object FullName -Descending | Select-Object -First 1
  }
  if (-not $csc) { throw 'csc.exe not found (need .NET Framework compiler for fixture exe)' }
  & $csc.FullName /nologo /target:exe /out:$exePath $csPath
  if ($LASTEXITCODE -ne 0) { throw "csc failed: $LASTEXITCODE" }
  if (-not (Test-Path -LiteralPath $exePath)) { throw "csc did not write $exePath" }
  Copy-Item -Force (Join-Path $Here 'lounge-fixture.nsi') (Join-Path $Stage 'lounge-fixture.nsi')

  # Ensure makensis is available.
  $makensis = $null
  foreach ($c in @(
      'makensis',
      "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
      "$env:ProgramFiles\NSIS\makensis.exe"
    )) {
    if ($c -eq 'makensis') {
      $cmd = Get-Command makensis -ErrorAction SilentlyContinue
      if ($cmd) { $makensis = $cmd.Source; break }
    } elseif (Test-Path -LiteralPath $c) {
      $makensis = $c; break
    }
  }
  if (-not $makensis) {
    Write-Host 'Installing NSIS via chocolatey...'
    choco install nsis -y --no-progress
    $makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
    if (-not (Test-Path -LiteralPath $makensis)) {
      $makensis = "$env:ProgramFiles\NSIS\makensis.exe"
    }
  }
  if (-not (Test-Path -LiteralPath $makensis)) {
    throw "makensis not found after NSIS install"
  }

  Push-Location $Stage
  try {
    & $makensis /V2 lounge-fixture.nsi
    if ($LASTEXITCODE -ne 0) { throw "makensis failed: $LASTEXITCODE" }
  } finally {
    Pop-Location
  }

  $built = Join-Path $Stage 'Agent-Lounge-OS-windows-x64-setup.exe'
  if (-not (Test-Path -LiteralPath $built)) { throw "NSIS did not produce $built" }
  Copy-Item -Force $built (Join-Path $OutDir 'Agent-Lounge-OS-windows-x64-setup.exe')
  Copy-Item -Force $built (Join-Path $OutDir 'Agent-Lounge-OS-windows-arm64-setup.exe')

  $lines = foreach ($name in @('Agent-Lounge-OS-windows-x64-setup.exe', 'Agent-Lounge-OS-windows-arm64-setup.exe')) {
    $p = Join-Path $OutDir $name
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash.ToLower()
    "{0}  {1}" -f $hash, $name
  }
  Set-Content -Path (Join-Path $OutDir 'SHA256SUMS') -Value $lines -Encoding Ascii
  $bad = foreach ($line in $lines) {
    $line -replace '^[0-9a-fA-F]{64}', 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef'
  }
  Set-Content -Path (Join-Path $OutDir 'SHA256SUMS.bad') -Value $bad -Encoding Ascii
  Set-Content -Path (Join-Path $OutDir 'VERSION') -Value $Version -Encoding Ascii
  Write-Host "NSIS fixture ready in $OutDir"
} finally {
  Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
}
