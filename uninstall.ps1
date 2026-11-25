param([switch] $Purge, [switch] $Yes, [switch] $Help)

$ErrorActionPreference = "Stop"

function Show-Usage {
    Write-Output "Usage: uninstall.ps1 [-Purge] [-Yes]"
    Write-Output "Remove Krabb software: its service and the krabb and krabb-daemon binaries."
    Write-Output "KRABB_HOME stays with plugins, settings, credentials, and conversations."
    Write-Output "-Purge  Also remove KRABB_HOME, its credential entry, and the stores Krabb created."
    Write-Output "-Yes    Skip the confirmation that -Purge asks for."
    Write-Output "Environment: KRABB_HOME when no service is installed, and KRABB_KEEP_PATH=1."
}

function Stop-Uninstall([string] $Message) {
    [Console]::Error.WriteLine("[ERROR] $Message")

    # Under `irm | iex` the script runs in the user's own session, where exit would close it.
    if ($PSCommandPath) { exit 2 }
    throw "Krabb was not uninstalled."
}

function Write-Uninstall([string] $Message) {
    Write-Output "[INFO] $Message"
}

# Native tools report failures through exit codes; a missing or broken one must not stop the
# uninstall, so each call is isolated and only its exit code counts.
function Invoke-Tool([string] $Program, [string[]] $Arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"

    try {
        $output = & $Program @Arguments 2>&1
        return [pscustomobject]@{ Ok = ($LASTEXITCODE -eq 0); Output = ($output | Out-String) }
    } catch {
        return [pscustomobject]@{ Ok = $false; Output = "" }
    } finally {
        $ErrorActionPreference = $previous
    }
}

if ($Help) {
    Show-Usage
    return
}

$profileRoot = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
if (-not $profileRoot -or -not $env:LOCALAPPDATA) { Stop-Uninstall "USERPROFILE and LOCALAPPDATA must be set." }

$bin = Join-Path $env:LOCALAPPDATA "Krabb\bin"
$krabb = Join-Path $bin "krabb.exe"
$daemon = Join-Path $bin "krabb-daemon.exe"
$definition = Join-Path $profileRoot "krabb-service.xml"
$label = "it.airscript.krabb"
$removed = [Collections.Generic.List[string]]::new()
$kept = [Collections.Generic.List[string]]::new()

# Only a service that runs this installation's daemon belongs to this installation; the Krabb
# service is machine-wide, so another user's or another build's service stays.
$service = Get-Service -Name Krabb -ErrorAction SilentlyContinue
$owned = $false
$environment = @{}

if ($service) {
    $query = Invoke-Tool "sc.exe" @("qc", "Krabb")
    $runs = if ($query.Output -match 'BINARY_PATH_NAME\s*:\s*"([^"]+)"') { $Matches[1] } else { "" }

    if ($runs -and [string]::Equals($runs, $daemon, [StringComparison]::OrdinalIgnoreCase)) {
        $owned = $true
        $key = "HKLM:\SYSTEM\CurrentControlSet\Services\Krabb"
        $values = (Get-ItemProperty -Path $key -Name Environment -ErrorAction SilentlyContinue).Environment

        foreach ($entry in @($values)) {
            if ($entry -match '^(KRABB_[A-Z_]+)=(.*)$') { $environment[$Matches[1]] = $Matches[2] }
        }
    } else {
        $kept.Add("the Krabb service, which runs $(if ($runs) { $runs } else { 'another installation' })")
    }
}

# The installed service names the home and stores this installation really uses.
function Get-KrabbPath([string] $Name) {
    if ($environment.ContainsKey($Name)) { return $environment[$Name] }
    return [Environment]::GetEnvironmentVariable($Name)
}

$configRoot = if ($env:XDG_CONFIG_HOME) { $env:XDG_CONFIG_HOME } else { Join-Path $profileRoot ".config" }
$krabbHome = Get-KrabbPath "KRABB_HOME"
if (-not $krabbHome) { $krabbHome = Join-Path $configRoot "krabb" }

# Deleting a folder that is not a Krabb home, such as the profile itself, would destroy user files.
$isHome = @("plugins.lock", "config.toml", "schema.json") |
    Where-Object { Test-Path -LiteralPath (Join-Path $krabbHome $_) -PathType Leaf }

if ($Purge -and -not $Yes) {
    if ([Console]::IsInputRedirected) { Stop-Uninstall "Confirm -Purge with -Yes when no terminal is available." }
    Write-Output "Purging deletes $krabbHome with its plugins, settings, credentials, and"
    Write-Output "conversations, its credential entry, and the stores Krabb created elsewhere."
    $answer = Read-Host "Purge Krabb? [y/N]"
    if ($answer -notmatch '^(y|yes)$') { Stop-Uninstall "Nothing was removed." }
}

function Remove-Krabb([string] $Path) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return }
    Remove-Item -LiteralPath $Path -Recurse -Force
    $removed.Add($Path)
}

# A file Krabb saves atomically, with its lock and the sidecars of an interrupted save.
function Remove-Store([string] $Path) {
    if (-not $Path) { return }
    if (-not [IO.Path]::IsPathRooted($Path)) {
        $kept.Add("$Path, because it is not an absolute path")
        return
    }

    $directory = Split-Path $Path
    $name = Split-Path $Path -Leaf
    Remove-Krabb $Path

    foreach ($sidecar in "$Path-wal", "$Path-shm", "$Path-journal", (Join-Path $directory ".$name.lock")) {
        Remove-Krabb $sidecar
    }

    # The memory plugin locks its file with the extension replaced, as memory.lock.
    if ([IO.Path]::HasExtension($name)) {
        Remove-Krabb (Join-Path $directory ([IO.Path]::ChangeExtension($name, "lock")))
    }

    foreach ($pattern in "tmp", "backup", "txn", "live") {
        Get-ChildItem -LiteralPath $directory -Force -Filter ".$name.$pattern-*" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Krabb $_.FullName }
    }
}

# Read before the home goes: the exact credential entry this home's credentials use.
$credentials = Get-KrabbPath "KRABB_CREDENTIALS"
if (-not $credentials) { $credentials = Join-Path $krabbHome "credentials.json" }
$entry = $null

if (Test-Path -LiteralPath $credentials -PathType Leaf) {
    try {
        $envelope = Get-Content -Raw -LiteralPath $credentials | ConvertFrom-Json
        if ($envelope.backend -eq "keyring" -and "$($envelope.key_id)" -match '^[0-9a-fA-F]{64}$') {
            $entry = "credentials:$($envelope.key_id)"
        }
    } catch {
        $entry = $null
    }
}

if ($owned) {
    if (Test-Path -LiteralPath $krabb) { Invoke-Tool $krabb @("service", "uninstall", "--yes") | Out-Null }

    if (Get-Service -Name Krabb -ErrorAction SilentlyContinue) {
        Invoke-Tool "sc.exe" @("stop", "Krabb") | Out-Null
        if (-not (Invoke-Tool "sc.exe" @("delete", "Krabb")).Ok) {
            Stop-Uninstall "Could not remove the Krabb service; run this again as an administrator."
        }
    }

    $removed.Add("the Krabb service")
    Remove-Krabb $definition
    Remove-Store "$definition.credentials.json"
    Remove-Store "$definition.credentials.json.new"
}

# A daemon started by hand keeps running, so stop the ones started from this installation only.
Get-Process -Name krabb-daemon -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and [string]::Equals($_.Path, $daemon, [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object { Stop-Process -Id $_.Id -Force; $_.WaitForExit(10000) | Out-Null }

Remove-Krabb $krabb
Remove-Krabb $daemon

foreach ($folder in $bin, (Split-Path $bin)) {
    if ((Test-Path -LiteralPath $folder) -and -not (Get-ChildItem -LiteralPath $folder -Force)) {
        Remove-Krabb $folder
    }
}

if (-not $env:KRABB_KEEP_PATH) {
    # Read and write the raw value, so entries such as %USERPROFILE%\bin stay unexpanded.
    $variables = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        $path = [string]$variables.GetValue("Path", "", "DoNotExpandEnvironmentNames")
        $entries = $path -split ";"

        if ($entries -contains $bin) {
            $variables.SetValue("Path", (($entries | Where-Object { $_ -and $_ -ne $bin }) -join ";"), "ExpandString")
            [Environment]::SetEnvironmentVariable("KRABB_PATH_REFRESH", "1", "User")
            [Environment]::SetEnvironmentVariable("KRABB_PATH_REFRESH", $null, "User")
            $removed.Add("$bin from your user PATH")
        }
    } finally {
        $variables.Close()
    }
}

if ($isHome) {
    Remove-Krabb (Join-Path $krabbHome "cache")
    Get-ChildItem -LiteralPath (Join-Path $krabbHome "updates") -Directory -Filter "v*" -ErrorAction SilentlyContinue |
        ForEach-Object { Remove-Krabb $_.FullName }
} elseif (Test-Path -LiteralPath $krabbHome) {
    $kept.Add("$krabbHome, because it does not look like a Krabb home")
}

if ($Purge) {
    # Stores at custom paths are files Krabb created; folders such as KRABB_ROOT are not.
    foreach ($name in "KRABB_DB", "KRABB_MEMORY", "KRABB_CREDENTIALS", "KRABB_CREDENTIALS_KEY", "KRABB_TIMER") {
        Remove-Store (Get-KrabbPath $name)
    }

    if ($isHome) { Remove-Krabb $krabbHome }

    if ($entry) {
        if ((Invoke-Tool "cmdkey.exe" @("/delete:$entry.$label")).Ok) {
            $removed.Add("the $entry credential entry")
        }
    }
}

foreach ($name in "KRABB_ROOT", "KRABB_MEDIA", "KRABB_CODEX_HOME") {
    $value = Get-KrabbPath $name
    if ($value -and (Test-Path -LiteralPath $value)) { $kept.Add("$value from $name") }
}

if ($removed.Count -gt 0) {
    Write-Uninstall "Removed:"
    $removed | ForEach-Object { Write-Output "  $_" }
} else {
    Write-Uninstall "Krabb was not found, so nothing was removed."
}

if ($kept.Count -gt 0) {
    Write-Uninstall "Kept:"
    $kept | ForEach-Object { Write-Output "  $_" }
}

if (-not $Purge -and $isHome) {
    Write-Uninstall "Kept $krabbHome with your plugins, settings, credentials, and conversations."
    Write-Uninstall "Remove them too with ``uninstall.ps1 -Purge``."
}

Write-Uninstall "Remove KRABB_* secrets set in your environment, and revoke keys where they were issued."
