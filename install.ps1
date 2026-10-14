# Agent Lounge OS rootless installer (Windows).
#
#   irm https://raw.githubusercontent.com/sewox/Agent-Lounge-OS/main/install.ps1 | iex
#
# Environment:
#   LOUNGE_VERSION           release tag/version, e.g. 0.1.0 (default: latest)
#   LOUNGE_INSTALL_PREFIX    install root (default: %LOCALAPPDATA%\AgentLounge)
#   LOUNGE_DOWNLOAD_BASE     mirror/base URL for assets (CI fixtures)
#   LOUNGE_DOWNLOAD_DIR      local directory of release assets (skips HTTP; CI fixtures)
#   LOUNGE_REPO              GitHub owner/repo (default: sewox/Agent-Lounge-OS)
#   LOUNGE_NO_MODIFY_PATH=1  leave the user PATH untouched
#
# Parameters:
#   -Uninstall               remove via NSIS uninstaller + our shim/registry/shortcuts
#   -Version <v>             install a specific release
#   -Prefix <dir>            install root (NSIS /D= target under <prefix>\app)
#
# Tauri v2 main binary name (Cargo package): agent-lounge-os.exe
# Product / Start Menu / HKCU Uninstall DisplayName: "Agent Lounge OS"
# Never falls back to "first *.exe".

[CmdletBinding()]
param(
  [switch]$Uninstall,
  [string]$Version = $env:LOUNGE_VERSION,
  [string]$Prefix = $env:LOUNGE_INSTALL_PREFIX
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = if ($env:LOUNGE_REPO) { $env:LOUNGE_REPO } else { 'sewox/Agent-Lounge-OS' }
if (-not $Prefix) {
  $Prefix = Join-Path $env:LOCALAPPDATA 'AgentLounge'
}
$BinDir = Join-Path $Prefix 'bin'
$ShareDir = Join-Path $Prefix 'share\agent-lounge-os'
$NsisDir = Join-Path $Prefix 'app'
$CliName = 'agent-lounge-os.cmd'
$Manifest = Join-Path $ShareDir 'install-manifest.txt'
# Exact Tauri v2 / Cargo main binary name — do not guess via *.exe.
$MainExeName = 'agent-lounge-os.exe'
$ProductName = 'Agent Lounge OS'
$UninstallRegPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"
$StartMenuDir = Join-Path ([Environment]::GetFolderPath('StartMenu')) "Programs\$ProductName"
$StartMenuShortcut = Join-Path $StartMenuDir "$ProductName.lnk"

$ArchRaw = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$AssetCandidates = if ($ArchRaw -eq 'ARM64') {
  @('Agent-Lounge-OS-windows-arm64-setup.exe', 'Agent-Lounge-OS-windows-x64-setup.exe')
} else {
  @('Agent-Lounge-OS-windows-x64-setup.exe')
}

$DownloadDir = $env:LOUNGE_DOWNLOAD_DIR
$Base = if ($DownloadDir) {
  "file://$DownloadDir"
} elseif ($env:LOUNGE_DOWNLOAD_BASE) {
  $env:LOUNGE_DOWNLOAD_BASE.TrimEnd('/')
} elseif ($Version) {
  "https://github.com/$Repo/releases/download/v$($Version.TrimStart('v'))"
} else {
  "https://github.com/$Repo/releases/latest/download"
}

function Get-ReleaseAsset([string]$Name, [string]$OutFile) {
  if ($DownloadDir) {
    $src = Join-Path $DownloadDir $Name
    if (-not (Test-Path -LiteralPath $src)) { throw "local asset missing: $src" }
    Copy-Item -Force -LiteralPath $src -Destination $OutFile
    return
  }
  Invoke-WebRequest -Uri "$Base/$Name" -OutFile $OutFile -UseBasicParsing
}

function Update-UserPath([string]$Dir) {
  $Key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
  try {
    $Raw = [string]$Key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $Want = $Dir.TrimEnd('\')
    $Entries = @($Raw -split ';' | Where-Object { $_ })
    $IsDir = { param($Entry) [Environment]::ExpandEnvironmentVariables($Entry).TrimEnd('\') -eq $Want }
    if ($Entries.Count -gt 0 -and (& $IsDir $Entries[0])) { return $false }
    $Others = @($Entries | Where-Object { -not (& $IsDir $_) })
    $Key.SetValue('Path', ((@($Dir) + $Others) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
  } finally {
    $Key.Close()
  }
  if (-not ('LoungeInstall.Native' -as [type])) {
    Add-Type -Namespace LoungeInstall -Name Native -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);
'@
  }
  $Result = [UIntPtr]::Zero
  [void][LoungeInstall.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$Result)
  return $true
}

function Remove-PathLoud([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return }
  try {
    Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
  } catch {
    throw "failed to remove $Path : $($_.Exception.Message)"
  }
  if (Test-Path -LiteralPath $Path) {
    throw "still present after remove: $Path"
  }
}

function Uninstall-Lounge {
  $errors = [System.Collections.Generic.List[string]]::new()

  # Prefer NSIS uninstaller when present.
  # Note (PS 5.1 / NSIS): some uninstallers copy themselves to %TEMP% and
  # relaunch. We Start-Process -Wait the UninstallString/uninstall.exe we
  # invoke; that wait is sufficient for our fixture and Tauri NSIS layout
  # (no extra TEMP-child polling). Failures still surface via exit code and
  # the post-checks below (exe/registry/shortcuts still present).
  $Uninstaller = Join-Path $NsisDir 'uninstall.exe'
  if (Test-Path -LiteralPath $Uninstaller) {
    Write-Host "Running NSIS uninstaller: $Uninstaller /S"
    $p = Start-Process -FilePath $Uninstaller -ArgumentList @('/S') -Wait -PassThru
    if ($null -eq $p -or $p.ExitCode -ne 0) {
      $errors.Add("NSIS uninstaller exited $(if ($null -eq $p) { 'null' } else { $p.ExitCode })")
    }
  } elseif (Test-Path -LiteralPath $UninstallRegPath) {
    $regUninstall = (Get-ItemProperty -LiteralPath $UninstallRegPath -ErrorAction SilentlyContinue).UninstallString
    if ($regUninstall) {
      # UninstallString may be quoted: "C:\...\uninstall.exe"
      $exe = ($regUninstall.Trim() -replace '^"(.*)"$', '$1')
      if (Test-Path -LiteralPath $exe) {
        Write-Host "Running registry UninstallString: $exe /S"
        $p = Start-Process -FilePath $exe -ArgumentList @('/S') -Wait -PassThru
        if ($null -eq $p -or $p.ExitCode -ne 0) {
          $errors.Add("registry uninstaller exited $(if ($null -eq $p) { 'null' } else { $p.ExitCode })")
        }
      } else {
        $errors.Add("UninstallString path missing: $exe")
      }
    }
  }

  # Remove HKCU uninstall entry if still present.
  if (Test-Path -LiteralPath $UninstallRegPath) {
    try {
      Remove-Item -LiteralPath $UninstallRegPath -Recurse -Force -ErrorAction Stop
    } catch {
      $errors.Add("failed to remove HKCU uninstall key: $($_.Exception.Message)")
    }
  }
  if (Test-Path -LiteralPath $UninstallRegPath) {
    $errors.Add("HKCU uninstall key still present: $UninstallRegPath")
  }

  # Start Menu shortcuts / folder.
  if (Test-Path -LiteralPath $StartMenuShortcut) {
    try { Remove-PathLoud $StartMenuShortcut } catch { $errors.Add("$_") }
  }
  if (Test-Path -LiteralPath $StartMenuDir) {
    try { Remove-PathLoud $StartMenuDir } catch { $errors.Add("$_") }
  }
  if (Test-Path -LiteralPath $StartMenuShortcut) {
    $errors.Add("Start Menu shortcut still present: $StartMenuShortcut")
  }
  if (Test-Path -LiteralPath $StartMenuDir) {
    $errors.Add("Start Menu folder still present: $StartMenuDir")
  }

  # Manifest-recorded paths (setup copy, CLI shim, app dir leftovers).
  if (Test-Path -LiteralPath $Manifest) {
    Get-Content -LiteralPath $Manifest | ForEach-Object {
      $p = $_.Trim()
      if (-not $p) { return }
      if ($p -eq $Manifest) { return }
      if (Test-Path -LiteralPath $p) {
        try { Remove-PathLoud $p } catch { $errors.Add("$_") }
      }
    }
    try { Remove-PathLoud $Manifest } catch { $errors.Add("$_") }
  }

  $MainExe = Join-Path $NsisDir $MainExeName
  if (Test-Path -LiteralPath $MainExe) {
    $errors.Add("main exe still present after uninstall: $MainExe")
  }
  if (Test-Path -LiteralPath (Join-Path $NsisDir 'uninstall.exe')) {
    $errors.Add("NSIS uninstall.exe still present under $NsisDir")
  }

  if ($errors.Count -gt 0) {
    throw ("uninstall incomplete:`n - " + ($errors -join "`n - "))
  }
  Write-Host "Uninstalled Agent Lounge OS from $Prefix"
}

if ($Uninstall) {
  Uninstall-Lounge
  return
}

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("lounge-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
  $Sums = Join-Path $Tmp 'SHA256SUMS'
  Write-Host "Fetching SHA256SUMS from $Base ..."
  Get-ReleaseAsset -Name 'SHA256SUMS' -OutFile $Sums
  $SumLines = @(Get-Content $Sums)
  if ($SumLines.Count -eq 0) {
    throw 'SHA256SUMS is missing or empty (checksums are mandatory)'
  }

  $Asset = $null
  $ExpectedMatches = @()
  foreach ($Candidate in $AssetCandidates) {
    $ExpectedMatches = @()
    foreach ($Line in $SumLines) {
      $Parts = $Line.Trim() -split '\s+', 2
      if ($Parts.Count -eq 2 -and $Parts[1].TrimStart('*') -eq $Candidate) {
        $ExpectedMatches += $Parts[0].ToLower()
      }
    }
    if ($ExpectedMatches.Count -gt 0) { $Asset = $Candidate; break }
  }
  if (-not $Asset) {
    throw "no release artifact for Windows/$ArchRaw (expected $($AssetCandidates -join ' or ') in SHA256SUMS)"
  }
  if ($ExpectedMatches.Count -ne 1) {
    throw "SHA256SUMS must contain exactly one entry for $Asset"
  }
  $Expected = $ExpectedMatches[0]

  Write-Host "Fetching $Asset ..."
  $Exe = Join-Path $Tmp $Asset
  Get-ReleaseAsset -Name $Asset -OutFile $Exe
  $Actual = (Get-FileHash -Algorithm SHA256 $Exe).Hash.ToLower()
  if ($Expected -ne $Actual) {
    throw "checksum mismatch for $Asset (expected $Expected, got $Actual)"
  }

  New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
  New-Item -ItemType Directory -Force -Path $ShareDir | Out-Null
  New-Item -ItemType Directory -Force -Path $NsisDir | Out-Null

  $Target = Join-Path $ShareDir $Asset
  if (Test-Path -LiteralPath $Target) {
    Remove-PathLoud $Target
  }
  Move-Item -Force $Exe $Target

  # Always run the real NSIS install path for *-setup.exe (no skip flag).
  if ($Asset -notlike '*-setup.exe') {
    throw "expected a *-setup.exe NSIS asset, got $Asset"
  }

  Write-Host "Running NSIS installer: $Target /S /D=$NsisDir"
  $p = Start-Process -FilePath $Target -ArgumentList @('/S', "/D=$NsisDir") -Wait -PassThru
  if ($p.ExitCode -ne 0) {
    throw "NSIS installer exited $($p.ExitCode)"
  }

  $MainExe = Join-Path $NsisDir $MainExeName
  if (-not (Test-Path -LiteralPath $MainExe)) {
    throw "NSIS install did not produce exact main exe '$MainExeName' at $MainExe (refusing *.exe fallback)"
  }

  $CliPath = Join-Path $BinDir $CliName
  Set-Content -Path $CliPath -Value "@`"$MainExe`" %*" -Encoding Ascii

  $manifestLines = @(
    $Target,
    $CliPath,
    $NsisDir,
    $MainExe,
    (Join-Path $NsisDir 'uninstall.exe'),
    $StartMenuShortcut,
    $StartMenuDir
  )
  Set-Content -Path $Manifest -Value ($manifestLines -join "`n") -Encoding Ascii

  if (-not $env:LOUNGE_NO_MODIFY_PATH -and (Update-UserPath -Dir $BinDir)) {
    $SessionOthers = @($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $BinDir.TrimEnd('\') })
    $env:Path = (@($BinDir) + $SessionOthers) -join ';'
    Write-Host "Put $BinDir first on your user PATH (open a new terminal to pick it up)."
  } elseif (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -eq $BinDir.TrimEnd('\') })) {
    Write-Host ""
    Write-Host "$BinDir is not on your PATH. Add it with:"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', '$BinDir;' + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
  }

  Write-Host ""
  Write-Host "Installed Agent Lounge OS → $MainExe"
  Write-Host "CLI shim → $CliPath"
  Write-Host "Verify: $CliName --version"
  Write-Host "Uninstall: .\install.ps1 -Uninstall -Prefix '$Prefix'"
  Write-Host "Roadmap: Minisign signing; update subcommand (not in this installer)."
} finally {
  if (Test-Path -LiteralPath $Tmp) {
    Remove-Item -Recurse -Force $Tmp -ErrorAction Stop
  }
}
