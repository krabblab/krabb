# Steps of the Install and Update drills on Windows. Each step reads and extends the job
# environment through GITHUB_ENV and GITHUB_PATH, so the workflow lists them as named steps.
param([Parameter(Mandatory = $true)][string] $Step)

$ErrorActionPreference = "Stop"

function Invoke-Krabb {
  $output = krabb @args
  if ($LASTEXITCODE -ne 0) { throw "krabb $args failed: $output" }
  return ($output | Out-String | ConvertFrom-Json)
}

function Invoke-Native([string] $Failure, [scriptblock] $Command) {
  & $Command
  if ($LASTEXITCODE -ne 0) { throw $Failure }
}

function Show-Logs {
  Get-ChildItem (Join-Path $env:KRABB_HOME "logs") -Filter *.log -ErrorAction SilentlyContinue |
    ForEach-Object { Get-Content $_.FullName -Tail 40 }
}

function Wait-Daemon([string] $State, [string[]] $Logs = @()) {
  for ($i = 0; $i -lt 60; $i++) {
    if ((Invoke-Krabb --json status).daemon -eq $State) { return }
    Start-Sleep -Seconds 1
  }

  if ($Logs) { Get-Content $Logs -ErrorAction SilentlyContinue } else { Show-Logs }
  throw "The daemon did not become $State."
}

function Get-Digest([string] $Path) {
  return (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Sessions {
  return (Invoke-Krabb --json session list).items.id
}

function Get-State {
  $state = foreach ($file in "config.toml", "plugins.lock") {
    (Get-FileHash (Join-Path $env:KRABB_HOME $file) -Algorithm SHA256).Hash
  }

  return ($state -join ",")
}

function Get-Ready {
  return @((Invoke-Krabb --json status).plugin_states | Where-Object { $_.state -eq "ready" } |
    ForEach-Object { $_.id })
}

function Set-Home([bool] $Served) {
  $user = Join-Path $env:RUNNER_TEMP "user"
  New-Item -ItemType Directory -Force -Path $user | Out-Null
  "USERPROFILE=$user" >> $env:GITHUB_ENV
  "HOME=$user" >> $env:GITHUB_ENV
  "LOCALAPPDATA=$user\AppData\Local" >> $env:GITHUB_ENV
  "KRABB_HOME=$user\.krabb" >> $env:GITHUB_ENV
  "$user\AppData\Local\Krabb\bin" >> $env:GITHUB_PATH

  if ($Served) {
    $release = Join-Path $env:RUNNER_TEMP "release"
    "KRABB_RELEASE_BASE=$(([Uri]$release).AbsoluteUri.TrimEnd('/'))" >> $env:GITHUB_ENV
  }
}

function Install-Krabb {
  New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA | Out-Null

  # The installer runs no native command, so only its own `exit` sets this.
  $global:LASTEXITCODE = 0
  & (Join-Path $env:RUNNER_TEMP "installers\install.ps1")
  if ($LASTEXITCODE -ne 0) { throw "install.ps1 failed." }
}

function Install-Plugins {
  $plugins = $env:KRABB_PLUGINS | ConvertFrom-Json
  $failed = @()

  foreach ($id in $plugins) {
    krabb plugin install $id --yes
    if ($LASTEXITCODE -ne 0) { $failed += $id }
  }

  $installed = (Invoke-Krabb --json plugin list).items.id

  foreach ($id in $plugins) {
    if ($installed -notcontains $id) { $failed += "$id (not listed)" }
  }

  if ($failed.Count -gt 0) { throw "Plugins that did not install: $($failed -join ', ')" }
}

function Test-Health {
  Invoke-Native "krabb doctor failed after installing plugins." { krabb --json doctor | Out-Null }
  $healthy = @("needs setup", "configured", "ready")
  $items = (Invoke-Krabb --json plugin list).items
  $unhealthy = @($items | Where-Object { $_.health -notin $healthy })

  if ($unhealthy.Count -gt 0) {
    $names = $unhealthy | ForEach-Object { "$($_.id) ($($_.health))" }
    throw "Plugins that are not healthy: $($names -join ', ')"
  }
}

function Test-Daemon {
  $out = Join-Path $env:RUNNER_TEMP "daemon.out"
  $err = Join-Path $env:RUNNER_TEMP "daemon.err"

  function Start-Daemon {
    Start-Process krabb-daemon -PassThru -NoNewWindow -RedirectStandardOutput $out `
      -RedirectStandardError $err
  }

  $daemon = Start-Daemon
  Wait-Daemon "running" @($out, $err)
  Invoke-Krabb --json session new install-flow | Out-Null

  Stop-Process -Id $daemon.Id
  $daemon.WaitForExit()
  Wait-Daemon "stopped" @($out, $err)

  $daemon = Start-Daemon
  Wait-Daemon "running" @($out, $err)
  if ((Get-Sessions) -notcontains "install-flow") {
    throw "The session did not survive the restart."
  }

  Stop-Process -Id $daemon.Id
  $daemon.WaitForExit()
}

function Test-Service {
  $installed = Invoke-Krabb --json service install

  try {
    if (!(Test-Path $installed.path)) {
      throw "Service definition is missing at $($installed.path)."
    }

    $status = Invoke-Krabb --json service status
    if ($status.status -ne "installed") { throw "Service status reported '$($status.status)'." }

    Invoke-Native "krabb service start failed." { krabb service start }
    Wait-Daemon "running"
    Invoke-Krabb --json session new service-flow | Out-Null

    Invoke-Native "krabb service restart failed." { krabb service restart }
    Wait-Daemon "running"

    if ((Get-Sessions) -notcontains "service-flow") {
      throw "The session did not survive the service restart."
    }

    Invoke-Native "krabb service stop failed." { krabb service stop }
    Wait-Daemon "stopped"
  } catch {
    # Name the owner of every state file, since access to them is what fails here.
    Get-ChildItem $env:KRABB_HOME -Recurse -File -ErrorAction SilentlyContinue |
      Select-Object -First 60 |
      ForEach-Object { "{0}  {1}" -f (Get-Acl $_.FullName).Owner, $_.FullName }
    throw
  } finally {
    Invoke-Native "krabb service uninstall failed." { krabb service uninstall --yes }
  }

  if (Test-Path $installed.path) { throw "Service definition remains at $($installed.path)." }
  if (Get-Service -Name Krabb -ErrorAction SilentlyContinue) {
    throw "Windows still has the Krabb service."
  }
}

function Test-Uninstall {
  $bin = Join-Path $env:LOCALAPPDATA "Krabb\bin"

  Invoke-Krabb --json service install | Out-Null
  $uninstaller = Join-Path $env:RUNNER_TEMP "installers\uninstall.ps1"
  Invoke-Native "uninstall.ps1 failed." { pwsh -NoProfile -File $uninstaller }
  if (Get-Service -Name Krabb -ErrorAction SilentlyContinue) {
    throw "Uninstall left the Krabb service."
  }

  foreach ($path in "$bin\krabb.exe", "$bin\krabb-daemon.exe") {
    if (Test-Path $path) { throw "Uninstall left $path." }
  }

  if (!(Test-Path "$env:KRABB_HOME\config.toml")) { throw "Uninstall removed the configuration." }
  if (!(Test-Path "$env:KRABB_HOME\plugins")) { throw "Uninstall removed the plugins." }
  $path = [Environment]::GetEnvironmentVariable("Path", "User")
  if (($path -split ";") -contains $bin) { throw "Uninstall left $bin in the user PATH." }

  Invoke-Native "uninstall.ps1 -Purge failed." { pwsh -NoProfile -File $uninstaller -Purge -Yes }

  $left = @(Get-ChildItem $env:USERPROFILE -Recurse -Force -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like "*krabb*" })
  if ($left.Count -gt 0) { throw "Purge left: $($left.FullName -join ', ')" }
  if (Get-Service -Name Krabb -ErrorAction SilentlyContinue) {
    throw "Purge left the Krabb service."
  }
}

function New-Broken {
  $work = Join-Path $env:RUNNER_TEMP "work"
  $broken = Join-Path $env:RUNNER_TEMP "broken"
  New-Item -ItemType Directory -Force -Path $broken | Out-Null

  foreach ($release in "previous", "candidate") {
    $version = if ($release -eq "previous") { $env:KRABB_PREVIOUS } else { $env:KRABB_CANDIDATE }
    $name = "krabb-core-v$version-$env:KRABB_TARGET"
    $archive = Join-Path $env:RUNNER_TEMP "$release\$name.zip"
    Expand-Archive -Path $archive -DestinationPath (Join-Path $work $release) -Force
    $daemon = Join-Path $work "$release\$name\bin\krabb-daemon.exe"
    "KRABB_$($release.ToUpperInvariant())_DAEMON=$(Get-Digest $daemon)" >> $env:GITHUB_ENV
  }

  # whoami.exe never reports to the service manager, so the service cannot start.
  $name = "krabb-core-v$env:KRABB_CANDIDATE-$env:KRABB_TARGET"
  $package = Join-Path $work "candidate\$name"
  Copy-Item "$env:SystemRoot\System32\whoami.exe" "$package\bin\krabb-daemon.exe" -Force
  Compress-Archive -Path $package -DestinationPath (Join-Path $broken "$name.zip") -Force
  $hash = Get-Digest (Join-Path $broken "$name.zip")
  "$hash  $name.zip" | Set-Content -Encoding ascii (Join-Path $broken "SHA256SUMS")
}

function Install-Previous {
  $release = Join-Path $env:RUNNER_TEMP "previous"
  $env:KRABB_RELEASE_BASE = ([Uri]$release).AbsoluteUri.TrimEnd('/')
  Install-Krabb

  $reported = krabb version
  if ($reported -ne "krabb $env:KRABB_PREVIOUS") { throw "Installed CLI reports '$reported'." }
}

function Set-Up {
  Invoke-Krabb --json init | Out-Null
  $flags = @("--yes")
  if ($env:KRABB_UNSTABLE -eq "true") { $flags += "--unstable" }

  foreach ($id in ($env:KRABB_PLUGINS | ConvertFrom-Json)) {
    Invoke-Native "krabb plugin install $id failed." { krabb plugin install $id @flags }
  }

  Invoke-Krabb --json service install | Out-Null
  Invoke-Native "krabb service start failed." { krabb service start }
  Wait-Daemon "running"
  Invoke-Krabb --json session new update-drill | Out-Null
  $ids = @((Invoke-Krabb --json plugin list).items.id | Sort-Object) -join ","
  Set-Content -Path (Join-Path $env:RUNNER_TEMP "plugins.ids") -Value $ids
  Set-Content -Path (Join-Path $env:RUNNER_TEMP "state") -Value (Get-State)
}

function Test-Restore {
  $env:KRABB_RELEASE_BASE = $env:KRABB_BROKEN_BASE

  $output = & {
    $ErrorActionPreference = "Continue"
    krabb --json update $env:KRABB_CANDIDATE --yes 2>&1
  } | Out-String

  $code = $LASTEXITCODE
  Remove-Item Env:KRABB_RELEASE_BASE
  Write-Output $output

  if ($code -eq 0) { throw "The update succeeded although the new daemon cannot start." }
  if (!$output.Contains("so Krabb $env:KRABB_PREVIOUS was restored")) {
    throw "The update did not restore."
  }

  $reported = krabb version
  if ($reported -ne "krabb $env:KRABB_PREVIOUS") { throw "The CLI was not restored: '$reported'." }

  $daemon = Join-Path $env:LOCALAPPDATA "Krabb\bin\krabb-daemon.exe"
  if ((Get-Digest $daemon) -ne $env:KRABB_PREVIOUS_DAEMON) {
    throw "The daemon binary was not restored."
  }

  Wait-Daemon "running"
  if ($null -ne (Invoke-Krabb --json status).recovery) { throw "The recovery point remains." }

  $saved = (Get-Content (Join-Path $env:RUNNER_TEMP "state") -Raw).Trim()
  if ((Get-State) -ne $saved) { throw "The configuration or plugin lock was not restored." }

  $ids = @((Invoke-Krabb --json plugin list).items.id | Sort-Object) -join ","
  $saved = (Get-Content (Join-Path $env:RUNNER_TEMP "plugins.ids") -Raw).Trim()
  if ($ids -ne $saved) { throw "The plugins changed: $ids." }

  if ((Get-Sessions) -notcontains "update-drill") {
    throw "The session did not survive the restore."
  }
}

function Update-Candidate {
  $before = Get-Ready

  $detached = (Invoke-Krabb --json plugin list).items |
    Where-Object {
      $before -contains $_.id -and @($_.capabilities | Where-Object { $_ -ne "client" }).Count -gt 0
    } |
    Select-Object -First 1 -ExpandProperty id

  if (!$detached) { throw "No plugin is loaded, so none can stay detached." }
  Invoke-Krabb --json plugin detach $detached | Out-Null
  Write-Output "Detached $detached; ready before the update: $($before -join ', ')."

  $env:KRABB_RELEASE_BASE = $env:KRABB_CANDIDATE_BASE
  $result = Invoke-Krabb --json update $env:KRABB_CANDIDATE --yes
  Remove-Item Env:KRABB_RELEASE_BASE
  $result | ConvertTo-Json -Depth 8

  if ($result.status -ne "updated" -or $result.version -ne $env:KRABB_CANDIDATE) {
    throw "The update result is not an update to $env:KRABB_CANDIDATE."
  }

  if (@($result.unloaded).Where({ $_ }).Count -gt 0) {
    throw "Plugins were not loaded: $($result.unloaded)."
  }

  $reported = krabb version
  if ($reported -ne "krabb $env:KRABB_CANDIDATE") { throw "The CLI was not replaced: '$reported'." }

  $daemon = Join-Path $env:LOCALAPPDATA "Krabb\bin\krabb-daemon.exe"
  if ((Get-Digest $daemon) -ne $env:KRABB_CANDIDATE_DAEMON) {
    throw "The daemon binary was not replaced."
  }

  $status = Invoke-Krabb --json status
  if ($status.daemon -ne "running") { throw "The service is not running." }

  if ($status.recovery.accepted -ne $true -or $status.recovery.from -ne $env:KRABB_PREVIOUS) {
    throw "The recovery point was not kept."
  }

  $after = Get-Ready
  Write-Output "Ready after the update: $($after -join ', ')."

  $lost = @($before | Where-Object { $_ -ne $detached -and $after -notcontains $_ })
  if ($lost.Count -gt 0) {
    throw "Plugins loaded before the update are not loaded: $($lost -join ', ')."
  }
  if ($after -contains $detached) {
    throw "The detached plugin $detached was attached by the update."
  }
  if ((Get-Sessions) -notcontains "update-drill") {
    throw "The session did not survive the update."
  }

  Invoke-Krabb --json plugin attach $detached | Out-Null
  if ((Get-Ready) -notcontains $detached) {
    throw "The detached plugin $detached did not attach again."
  }
}

function Test-Rollback {
  $daemon = Join-Path $env:LOCALAPPDATA "Krabb\bin\krabb-daemon.exe"

  Invoke-Native "krabb service stop failed." { krabb service stop }
  Wait-Daemon "stopped"

  for ($i = 0; $i -lt 30; $i++) {
    try {
      Copy-Item "$env:SystemRoot\System32\whoami.exe" $daemon -Force
      break
    } catch {
      if ($i -eq 29) { throw }
      Start-Sleep -Seconds 1
    }
  }

  krabb service start
  Start-Sleep -Seconds 15
  if ((Invoke-Krabb --json status).daemon -ne "stopped") {
    throw "The daemon started although it cannot."
  }

  $rollback = Join-Path $env:KRABB_HOME "updates\recovery\bin\0\krabb.exe"
  if (!(Test-Path $rollback)) { throw "The recovery point has no krabb binary." }
  $output = & $rollback --json update --rollback --yes
  if ($LASTEXITCODE -ne 0) { throw "Rollback failed: $output" }
  $result = $output | Out-String | ConvertFrom-Json
  $result | ConvertTo-Json

  if ($result.status -ne "restored" -or $result.version -ne $env:KRABB_PREVIOUS) {
    throw "The rollback result is not a restore of $env:KRABB_PREVIOUS."
  }

  $reported = krabb version
  if ($reported -ne "krabb $env:KRABB_PREVIOUS") { throw "The CLI was not restored: '$reported'." }
  if ((Get-Digest $daemon) -ne $env:KRABB_PREVIOUS_DAEMON) {
    throw "The daemon binary was not restored."
  }
  Wait-Daemon "running"

  if ((Get-Sessions) -notcontains "update-drill") {
    throw "The session did not survive the rollback."
  }

  if ($null -ne (Invoke-Krabb --json status).recovery) {
    Get-ChildItem (Join-Path $env:KRABB_HOME "updates\recovery") -Recurse -Force |
      Select-Object -First 60 | ForEach-Object { $_.FullName }
    throw "The recovery point remains after the rollback."
  }
}

switch ($Step) {
  "install-home" { Set-Home $true }
  "install" { Install-Krabb }

  "version" {
    $reported = krabb version
    if ($reported -ne "krabb $env:KRABB_EXPECTED") { throw "Installed CLI reports '$reported'." }
  }

  "init" { Invoke-Native "krabb init failed." { krabb --json init } }
  "doctor" { Invoke-Native "krabb doctor failed." { krabb --json doctor } }
  "status" { Invoke-Native "krabb status failed." { krabb --json status } }
  "plugins" { Install-Plugins }
  "health" { Test-Health }
  "daemon" { Test-Daemon }
  "service" { Test-Service }
  "uninstall" { Test-Uninstall }
  "broken" { New-Broken }
  "update-home" { Set-Home $false }
  "previous" { Install-Previous }
  "setup" { Set-Up }
  "restore" { Test-Restore }
  "update" { Update-Candidate }
  "rollback" { Test-Rollback }

  "remove" {
    if (Get-Command krabb -ErrorAction SilentlyContinue) { krabb service uninstall --yes }
    $global:LASTEXITCODE = 0
  }

  default { throw "Unknown drill step '$Step'." }
}
