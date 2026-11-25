$ErrorActionPreference = "Stop"

function Show-Usage {
    Write-Output "Usage: install.ps1 [PLUGIN[@VERSION]]"
    Write-Output "Install Krabb core from a release, or one official plugin from the plugin index."
    Write-Output "Environment: KRABB_VERSION and KRABB_RELEASE_BASE (core), KRABB_BIN, and"
    Write-Output "KRABB_KEEP_PATH=1 to leave the user PATH unchanged."
}

function Stop-Install([string] $Message) {
    [Console]::Error.WriteLine("[ERROR] $Message")

    # Under `irm | iex` the script runs in the user's own session, where exit would close it.
    if ($PSCommandPath) { exit 2 }
    throw "Krabb was not installed."
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

$pattern = '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$'
$releases = "https://github.com/krabblab/krabb/releases"
$script = "https://raw.githubusercontent.com/krabblab/krabb/main/install.ps1"
$target = "$arch-pc-windows-msvc"
$invalid = "is not valid. Use a version such as 0.1.0 or 0.1.0-rc.1."
if ($env:KRABB_VERSION) {
    $version = $env:KRABB_VERSION -replace '^v', ''
    if ($version -notmatch $pattern) { Stop-Install "Version '$version' $invalid" }
    $base = "$releases/download/v$version"
    $missing = "Cannot reach Krabb $version; check the version and the connection."
} else {
    $version = $null
    $base = "$releases/latest/download"
    $missing = "Cannot reach the latest Krabb release; check the connection or set KRABB_VERSION."
}
if ($env:KRABB_RELEASE_BASE) {
    $base = $env:KRABB_RELEASE_BASE
    $missing = "Cannot download SHA256SUMS from $base."
}
$base = $base.TrimEnd('/')
if ($base -notmatch '^(https://|file://)') {
    Stop-Install "KRABB_RELEASE_BASE must use https:// or file://."
}

function Get-Release([string]$Uri, [string]$Path) {
    if ($Uri.StartsWith("file://")) {
        Copy-Item ([Uri]$Uri).LocalPath $Path
    } else {
        Invoke-WebRequest -UseBasicParsing -TimeoutSec 120 $Uri -OutFile $Path
    }
}

# Stable releases are what releases/latest serves; before the first one, name the newest
# pre-release so the user can pin it.
function Stop-Prerelease([string]$Directory) {
    $list = if ($env:KRABB_RELEASE_LIST) { $env:KRABB_RELEASE_LIST } else {
        "https://api.github.com/repos/krabblab/krabb/releases?per_page=1"
    }

    $path = Join-Path $Directory "releases.json"
    try { Get-Release $list $path } catch { return }
    try { $release = @(Get-Content -Raw $path | ConvertFrom-Json)[0] } catch { return }

    # A stable newest release means the download failed for another reason.
    if ($release.prerelease -ne $true) { return }

    # The tag goes into a command the user copies, so it must be a plain version.
    $newest = "$($release.tag_name)" -replace '^v', ''
    if ($newest -notmatch $pattern) { return }

    Stop-Install ("No stable Krabb release is published yet.`n" +
        "The newest pre-release is $newest; install it with this command:`n" +
        "  `$env:KRABB_VERSION = '$newest'; irm $script | iex`n" +
        "Set KRABB_VERSION first in the same window so the installer receives it.")
}

$name = "krabb"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $checksumsPath = Join-Path $tmp "SHA256SUMS"
    try { Get-Release "$base/SHA256SUMS" $checksumsPath } catch {
        if (-not $version -and (-not $env:KRABB_RELEASE_BASE -or $env:KRABB_RELEASE_LIST)) {
            Stop-Prerelease $tmp
        }
        Stop-Install $missing
    }
    $checksums = @{}
    foreach ($entry in Get-Content $checksumsPath) {
        $parts = $entry -split '\s+', 3
        if ($parts.Count -ge 2) {
            $checksums[$parts[1].TrimStart('*')] = $parts[0].ToLowerInvariant()
        }
    }

    if (-not $version) {
        $suffix = "-$target.zip"
        $latest = $checksums.Keys |
            Where-Object { $_.StartsWith("krabb-core-v") -and $_.EndsWith($suffix) } |
            Select-Object -First 1
        if (-not $latest) { Stop-Install "The latest Krabb release has no build for $target." }
        $version = $latest -replace '^krabb-core-v', '' -replace "$([regex]::Escape($suffix))$", ''
        if ($version -notmatch $pattern) { Stop-Install "Version '$version' $invalid" }
        if (-not $env:KRABB_RELEASE_BASE) { $base = "$releases/download/v$version" }
    }

    $archive = "krabb-core-v$version-$target.zip"
    $archivePath = Join-Path $tmp $archive
    Write-Install "Downloading $archive."
    try {
        Get-Release "$base/$archive" $archivePath
    } catch {
        Stop-Install "Cannot download $archive."
    }

    $expected = $checksums[$archive]
    if (-not $expected) { Stop-Install "Checksum is missing for '$archive'." }
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

    # Read and write the raw value, so entries such as %USERPROFILE%\bin stay unexpanded.
    $environment = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        $path = [string]$environment.GetValue("Path", "", "DoNotExpandEnvironmentNames")
        if (-not $env:KRABB_KEEP_PATH -and ($path -split ";") -notcontains $dest) {
            $entries = @($path, $dest) | Where-Object { $_ }
            $environment.SetValue("Path", ($entries -join ";"), "ExpandString")
            Write-Install "Added $dest to your user PATH for new terminals."
        }
    } finally {
        $environment.Close()
    }

    if (($env:Path -split ";") -notcontains $dest) { $env:Path = "$env:Path;$dest" }
    Write-Install "Set up Krabb with ``krabb init``."
} finally {
    if (Test-Path -LiteralPath $tmp) { Remove-Item -Recurse -Force $tmp }
}
