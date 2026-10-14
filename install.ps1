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
#   -Uninstall               remove files this installer placed under the prefix
#   -Version <v>             install a specific release
#   -Prefix <dir>            install root
#
# Downloads the Windows setup/portable asset, verifies SHA256 against SHA256SUMS
# (mandatory), and installs per-user (no admin). Roadmap: Minisign; update.

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
$CliName = 'agent-lounge-os.cmd'
$Manifest = Join-Path $ShareDir 'install-manifest.txt'

# PROCESSOR_ARCHITEW6432 is set when this shell is an x64 process on ARM.
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

function Uninstall-Lounge {
  if (Test-Path -LiteralPath $Manifest) {
    Get-Content -LiteralPath $Manifest | ForEach-Object {
      $p = $_.Trim()
      if (-not $p) { return }
      if ($p -eq $Manifest) { return }
      if (Test-Path -LiteralPath $p) {
        Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue
      }
    }
    Remove-Item -LiteralPath $Manifest -Force -ErrorAction SilentlyContinue
  } else {
    Remove-Item -LiteralPath (Join-Path $BinDir $CliName) -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $ShareDir -Recurse -Force -ErrorAction SilentlyContinue
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
  $SumLines = Get-Content $Sums
  if (-not $SumLines -or $SumLines.Count -eq 0) {
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

  $Target = Join-Path $ShareDir $Asset
  if (Test-Path -LiteralPath $Target) {
    $Aside = "$Target.old"
    Remove-Item -Force $Aside -ErrorAction SilentlyContinue
    Move-Item -Force $Target $Aside
  }
  Move-Item -Force $Exe $Target

  # Per-user silent NSIS when asset is a setup.exe; fixtures may be a stub .exe.
  $IsNsis = $Asset -like '*-setup.exe'
  $manifestLines = [System.Collections.Generic.List[string]]::new()
  [void]$manifestLines.Add($Target)
  if ($IsNsis -and -not $env:LOUNGE_FIXTURE_SKIP_NSIS) {
    $NsisDir = Join-Path $Prefix 'app'
    New-Item -ItemType Directory -Force -Path $NsisDir | Out-Null
    $p = Start-Process -FilePath $Target -ArgumentList @('/S', "/D=$NsisDir") -Wait -PassThru
    if ($p.ExitCode -ne 0) {
      throw "NSIS installer exited $($p.ExitCode)"
    }
    $Launch = Get-ChildItem -Path $NsisDir -Recurse -Filter 'agent-lounge-os.exe' -ErrorAction SilentlyContinue |
      Select-Object -First 1
    if (-not $Launch) {
      $Launch = Get-ChildItem -Path $NsisDir -Recurse -Filter '*.exe' -ErrorAction SilentlyContinue |
        Select-Object -First 1
    }
    if ($Launch) {
      $CmdTarget = $Launch.FullName
    } else {
      $CmdTarget = $Target
    }
    [void]$manifestLines.Add($NsisDir)
  } else {
    # Fixture / portable: treat the downloaded file as the payload.
    # Fixtures may ship a batch stub under a .exe name — copy to .cmd so the shim can run it.
    $PayloadCmd = Join-Path $ShareDir 'agent-lounge-os-payload.cmd'
    Copy-Item -Force -LiteralPath $Target -Destination $PayloadCmd
    $CmdTarget = $PayloadCmd
    [void]$manifestLines.Add($PayloadCmd)
  }

  $CliPath = Join-Path $BinDir $CliName
  Set-Content -Path $CliPath -Value "@`"$CmdTarget`" %*" -Encoding Ascii
  [void]$manifestLines.Add($CliPath)
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
  Write-Host "Installed Agent Lounge OS → $Target"
  Write-Host "CLI shim → $CliPath"
  Write-Host "Verify: $CliName --version"
  Write-Host "Uninstall: irm .../install.ps1 | iex  with -Uninstall  (or: .\install.ps1 -Uninstall -Prefix '$Prefix')"
  Write-Host "Roadmap: Minisign signing; update subcommand (not in this installer)."
} finally {
  Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
