$ErrorActionPreference = "Stop"

function Show-Usage {
    Write-Output "Usage: install.ps1 [PLUGIN[@VERSION]]"
    Write-Output "Install Krabb core from a release, or one official plugin from the plugin index."
    Write-Output "Environment: KRABB_VERSION and KRABB_RELEASE_BASE (core), KRABB_BIN."
}

function Stop-Install([string] $Message) {
    Write-Error "[ERROR] $Message"
    exit 2
}

function Write-Install([string] $Message) {
    Write-Output "[INFO] $Message"
}

if ($args.Count -eq 1 -and ($args[0] -eq "--help" -or $args[0] -eq "-h")) {
    Show-Usage
    exit 0
}
if ($args.Count -gt 1) {
    Show-Usage
    exit 2
}

$plugin = if ($args.Count -eq 1) { $args[0] } else { "core" }
if ($plugin -cnotmatch '^[a-z0-9][a-z0-9-]*(@[0-9A-Za-z.-]+)?\z') { Stop-Install "Plugin ID '$plugin' is not valid." }

if ($plugin -like "core@*") { Stop-Install "Set KRABB_VERSION to choose a Krabb version." }

if ($plugin -ne "core") {
    $id = $plugin.Split("@")[0]
    if ($env:KRABB_VERSION) {
        Stop-Install "KRABB_VERSION applies to Krabb itself; install a plugin version with $id@VERSION."
    }
    if ($env:KRABB_RELEASE_BASE) {
        Stop-Install "KRABB_RELEASE_BASE applies to Krabb itself; plugins come from the plugin index."
    }
    $krabb = if ($env:KRABB_BIN) {
        $env:KRABB_BIN
    } else {
        Join-Path $env:LOCALAPPDATA "Krabb/bin/krabb.exe"
    }
    if (-not (Test-Path -LiteralPath $krabb -PathType Leaf)) {
        $command = Get-Command krabb.exe -ErrorAction SilentlyContinue
        if ($command) { $krabb = $command.Source }
    }
    if (-not (Test-Path -LiteralPath $krabb -PathType Leaf)) {
        Stop-Install "Install the Krabb core before installing an official plugin."
    }
    & $krabb plugin install $plugin --yes
    if ($LASTEXITCODE -ne 0) { Stop-Install "The Krabb plugin installation failed." }
    Write-Install "Installed plugin $plugin."
    exit 0
}

$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    "X64" { "x86_64" }
    "Arm64" { "aarch64" }
    default { Stop-Install "Unsupported architecture." }
}

if ($env:KRABB_VERSION) {
    $version = $env:KRABB_VERSION
} else {
    $version = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 120 https://raw.githubusercontent.com/krabblab/krabb/main/VERSION).Content.Trim()
}
if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$') {
    Stop-Install "Version '$version' is not valid."
}

$name = "krabb"
$archive = "krabb-core-v$version-$arch-pc-windows-msvc.zip"
$base = if ($env:KRABB_RELEASE_BASE) {
    $env:KRABB_RELEASE_BASE.TrimEnd('/')
} else {
    "https://github.com/krabblab/krabb/releases/download/v$version"
}
if ($base -notmatch '^(https://|file://)') {
    Stop-Install "KRABB_RELEASE_BASE must use https:// or file://."
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $archivePath = Join-Path $tmp $archive
    $checksumsPath = Join-Path $tmp "SHA256SUMS"
    Write-Install "Downloading $archive."
    $archiveUri = "$base/$archive"
    $checksumsUri = "$base/SHA256SUMS"
    if ($base.StartsWith("file://")) {
        Copy-Item ([Uri]$archiveUri).LocalPath $archivePath
        Copy-Item ([Uri]$checksumsUri).LocalPath $checksumsPath
    } else {
        Invoke-WebRequest -UseBasicParsing -TimeoutSec 120 $archiveUri -OutFile $archivePath
        Invoke-WebRequest -UseBasicParsing -TimeoutSec 120 $checksumsUri -OutFile $checksumsPath
    }

    $line = Get-Content $checksumsPath | Where-Object {
        $parts = $_ -split '\s+', 3
        $parts.Count -ge 2 -and $parts[1].TrimStart('*') -eq $archive
    } | Select-Object -First 1
    if (-not $line) { Stop-Install "Checksum is missing for '$archive'." }
    $expected = (($line -split '\s+', 3)[0]).ToLowerInvariant()
    $actual = (Get-FileHash $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { Stop-Install "Checksum verification failed for '$archive'." }

    Expand-Archive $archivePath -DestinationPath $tmp
    $file = Get-ChildItem $tmp -Recurse -File -Filter "$name.exe" | Select-Object -First 1
    if (-not $file) { Stop-Install "The archive does not contain '$name.exe'." }
    $daemon = Get-ChildItem $tmp -Recurse -File -Filter "krabb-daemon.exe" | Select-Object -First 1
    if (-not $daemon) { Stop-Install "The archive does not contain 'krabb-daemon.exe'." }
    $dest = Join-Path $env:LOCALAPPDATA "Krabb/bin"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    $temporary = Join-Path $dest ".$name.exe.tmp"
    Copy-Item $file.FullName $temporary
    Move-Item -Force $temporary (Join-Path $dest "$name.exe")
    $daemonTemporary = Join-Path $dest ".krabb-daemon.exe.tmp"
    Copy-Item $daemon.FullName $daemonTemporary
    Move-Item -Force $daemonTemporary (Join-Path $dest "krabb-daemon.exe")
    Write-Install "Installed $name and krabb-daemon in $dest."
} finally {
    if (Test-Path -LiteralPath $tmp) { Remove-Item -Recurse -Force $tmp }
}
