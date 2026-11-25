param([switch] $Purge, [switch] $Yes, [switch] $Help)

$ErrorActionPreference = "Stop"

function Show-Usage {
    Write-Output "Usage: uninstall.ps1 [-Purge] [-Yes]"
    Write-Output "Remove Krabb software: the service, the krabb and krabb-daemon binaries, and plugin code."
    Write-Output "Settings, credentials, and conversations in KRABB_HOME stay unless you pass -Purge."
    Write-Output "-Purge  Also remove KRABB_HOME, saved credential entries, and stores Krabb created."
    Write-Output "-Yes    Skip the confirmation that -Purge asks for."
    Write-Output "Environment: KRABB_HOME, KRABB_DB, KRABB_MEMORY, and KRABB_KEEP_PATH=1 to leave PATH."
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

if ($Help) {
    Show-Usage
    return
}

$profileRoot = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
if (-not $profileRoot -or -not $env:LOCALAPPDATA) { Stop-Uninstall "USERPROFILE and LOCALAPPDATA must be set." }

$bin = Join-Path $env:LOCALAPPDATA "Krabb\bin"
$configRoot = if ($env:XDG_CONFIG_HOME) { $env:XDG_CONFIG_HOME } else { Join-Path $profileRoot ".config" }
$krabbHome = if ($env:KRABB_HOME) { $env:KRABB_HOME } else { Join-Path $configRoot "krabb" }
$definition = Join-Path $profileRoot "krabb-service.xml"
$krabb = Join-Path $bin "krabb.exe"
$label = "it.airscript.krabb"

if ($Purge -and -not $Yes) {
    if ([Console]::IsInputRedirected) { Stop-Uninstall "Confirm -Purge with -Yes when no terminal is available." }
    $answer = Read-Host "Delete $krabbHome with its settings, credentials, and conversations? [y/N]"
    if ($answer -notmatch '^(y|yes)$') { Stop-Uninstall "Nothing was removed." }
}

$removed = [Collections.Generic.List[string]]::new()

function Remove-Krabb([string] $Path) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return }
    Remove-Item -LiteralPath $Path -Recurse -Force
    $removed.Add($Path)
}

# Krabb removes its own service when it can run; the service is deleted directly otherwise.
$service = Get-Service -Name Krabb -ErrorAction SilentlyContinue
if ($service -or (Test-Path -LiteralPath $definition)) {
    $done = $false
    if (Test-Path -LiteralPath $krabb) {
        & $krabb service uninstall --yes *> $null
        $done = $LASTEXITCODE -eq 0
    }

    if (-not $done -and $service) {
        & sc.exe stop Krabb *> $null
        & sc.exe delete Krabb *> $null
        if ($LASTEXITCODE -ne 0) { Stop-Uninstall "Could not remove the Krabb service; run this again as an administrator." }
    }

    $removed.Add("the Krabb service")
    Remove-Krabb $definition
}

Remove-Krabb "$definition.credentials.json"

# A daemon started by hand keeps running, so stop the ones started from this installation only.
$daemon = Join-Path $bin "krabb-daemon.exe"
Get-Process -Name krabb-daemon -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $daemon } |
    ForEach-Object { Stop-Process -Id $_.Id -Force; $_.WaitForExit(10000) | Out-Null }

Remove-Krabb $krabb
Remove-Krabb $daemon
Remove-Krabb (Join-Path $krabbHome "plugins")
Remove-Krabb (Join-Path $krabbHome "cache")
Get-ChildItem -LiteralPath (Join-Path $krabbHome "updates") -Directory -Filter "v*" -ErrorAction SilentlyContinue |
    ForEach-Object { Remove-Krabb $_.FullName }

foreach ($folder in $bin, (Split-Path $bin)) {
    if ((Test-Path -LiteralPath $folder) -and -not (Get-ChildItem -LiteralPath $folder -Force)) {
        Remove-Krabb $folder
    }
}

$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $env:KRABB_KEEP_PATH -and $path -and ($path -split ";") -contains $bin) {
    $entries = ($path -split ";") | Where-Object { $_ -and $_ -ne $bin }
    [Environment]::SetEnvironmentVariable("Path", ($entries -join ";"), "User")
    $removed.Add("$bin from your user PATH")
}

if ($Purge) {
    Remove-Krabb $krabbHome

    # Stores at custom paths are files Krabb created; folders such as KRABB_ROOT are not.
    Remove-Krabb $env:KRABB_DB
    if ($env:KRABB_MEMORY) {
        Remove-Krabb $env:KRABB_MEMORY
        Remove-Krabb "$env:KRABB_MEMORY.lock"
    }

    $targets = & cmdkey.exe /list 2>$null |
        ForEach-Object { if ($_ -match 'target=(\S*\.' + [regex]::Escape($label) + ')\s*$') { $Matches[1] } }
    foreach ($target in $targets) {
        & cmdkey.exe "/delete:$target" *> $null
        if ($LASTEXITCODE -eq 0) { $removed.Add("the $target credential entry") }
    }
}

if ($removed.Count -gt 0) {
    Write-Uninstall "Removed:"
    $removed | ForEach-Object { Write-Output "  $_" }
} else {
    Write-Uninstall "Krabb was not found, so nothing was removed."
}

if (-not $Purge -and (Test-Path -LiteralPath $krabbHome)) {
    Write-Uninstall "Kept $krabbHome with your settings, credentials, and conversations."
    Write-Uninstall "Remove them too with ``uninstall.ps1 -Purge``."
}

foreach ($variable in "KRABB_ROOT", "KRABB_MEDIA") {
    $value = [Environment]::GetEnvironmentVariable($variable)
    if ($value -and (Test-Path -LiteralPath $value)) {
        Write-Uninstall "Kept $value from $variable; delete it yourself if Krabb was its only user."
    }
}

Write-Uninstall "Remove KRABB_* secrets set in your environment, and revoke keys where they were issued."
