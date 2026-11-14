# End-to-end check of the plugin's Windows build hook (herdr/install.ps1), run
# the way `herdr plugin install` runs it, each case on a fresh copy of the
# checkout. CI runs it on windows-latest (VS 2026, no VS 2022: #67).
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File tests/install-windows.ps1 [-RequirePrebuilt]
# -RequirePrebuilt (the release job): only the first case, and the version's
# Windows binary must be on GitHub, so a missing or misnamed asset fails
# instead of skipping. CI already ran the source-build cases on this commit.
param([switch]$RequirePrebuilt)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$repo = Split-Path -Parent $PSScriptRoot
$failures = 0

function Assert([string]$name, [bool]$ok) {
    if ($ok) { Write-Host "  ok    $name" } else { Write-Host "  FAIL  $name"; $script:failures++ }
}

# The checkout's tracked files, uncommitted edits included.
function New-Checkout([string]$version) {
    $dir = Join-Path ([IO.Path]::GetTempPath()) ("herdr-nvim-install-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Item -ItemType Directory -Path $dir | Out-Null
    $tree = git -C $repo stash create
    if (-not $tree) { $tree = "HEAD" }
    git -C $repo archive --format=zip -o "$dir.zip" $tree
    Expand-Archive -LiteralPath "$dir.zip" -DestinationPath $dir
    Remove-Item -LiteralPath "$dir.zip"
    if ($version) {
        $manifest = Join-Path $dir "herdr-plugin.toml"
        $text = [IO.File]::ReadAllText($manifest) -replace '(?m)^version = ".*"', "version = `"$version`""
        [IO.File]::WriteAllText($manifest, $text)
    }
    $dir
}

# Captures stderr without Windows PowerShell 5.1 turning it into errors.
function Invoke-Captured([string]$file, [string]$arguments, [string]$dir, [bool]$withCargo) {
    $info = New-Object System.Diagnostics.ProcessStartInfo -ArgumentList $file, $arguments
    $info.UseShellExecute = $false
    $info.WorkingDirectory = $dir
    $info.RedirectStandardError = $true
    if (-not $withCargo) {
        $info.EnvironmentVariables["PATH"] = ($env:PATH -split ";" |
            Where-Object { $_ -and -not (Test-Path -LiteralPath (Join-Path $_ "cargo.exe")) }) -join ";"
    }
    $process = [System.Diagnostics.Process]::Start($info)
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    $stderr -split "`r?`n" | Where-Object { $_ } | ForEach-Object { Write-Host "    | $_" }
    Write-Host "    | exit $($process.ExitCode)"
    [pscustomobject]@{ Exit = $process.ExitCode; Stderr = $stderr }
}

# The exact command herdr-plugin.toml gives herdr for Windows.
function Invoke-Hook([string]$dir, [bool]$withCargo) {
    Invoke-Captured "powershell.exe" "-NoProfile -ExecutionPolicy Bypass -File herdr/install.ps1" $dir $withCargo
}

function Assert-Installed([string]$dir) {
    $exe = Join-Path $dir "bin\herdr-nvim.exe"
    Assert "bin\herdr-nvim.exe is installed" (Test-Path -LiteralPath $exe)
    if (Test-Path -LiteralPath $exe) {
        Assert "bin\herdr-nvim.exe runs (no-arg exit 2)" ((Invoke-Captured $exe "" $dir $true).Exit -eq 2)
    }
}

function Assert-NoDownloadLeft([string]$dir) {
    Assert "no staged exe left in bin\" (-not (Test-Path -LiteralPath (Join-Path $dir "bin\herdr-nvim.new.exe")))
}

function Get-Aside([string]$dir) {
    @(Get-ChildItem -LiteralPath (Join-Path $dir "bin") -Filter "herdr-nvim.old-*.exe")
}

function Finish {
    if ($failures) { Write-Host "FAILED: $failures check(s)"; exit 1 }
    Write-Host "OK"
    exit 0
}

$builtFromSource = "target\release\herdr-nvim.exe"
$version = (Select-String -LiteralPath (Join-Path $repo "herdr-plugin.toml") -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value

Write-Host "== release v$version has the Windows binary: install.ps1 downloads it, no cargo needed"
$url = "https://github.com/ChmaraX/herdr-nvim/releases/download/v$version/herdr-nvim-x86_64-pc-windows-msvc.exe"
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$published = try { Invoke-WebRequest -UseBasicParsing -Method Head -Uri $url | Out-Null; $true } catch { $false }
if ($published) {
    $dir = New-Checkout $null
    $r = Invoke-Hook $dir $false
    Assert "exit 0 without cargo" ($r.Exit -eq 0)
    Assert-Installed $dir
    Assert "no source build" (-not (Test-Path -LiteralPath (Join-Path $dir $builtFromSource)))
    Assert-NoDownloadLeft $dir
}
elseif ($RequirePrebuilt) {
    Assert "v$version has the Windows binary on GitHub ($url)" $false
}
else {
    Write-Host "  skip  v$version has no Windows binary on GitHub (released before it existed, or not released yet)"
}
if ($RequirePrebuilt) { Finish }

Write-Host "== no Windows binary for this version (404): builds from source with the MSVC cargo finds"
$built = New-Checkout "0.0.0"
$r = Invoke-Hook $built $true
Assert "exit 0" ($r.Exit -eq 0)
Assert-Installed $built
Assert "built from source" (Test-Path -LiteralPath (Join-Path $built $builtFromSource))
Assert-NoDownloadLeft $built

# A sidebar keeps bin\herdr-nvim.exe running, and Windows can't overwrite a
# running exe. A copy of ping.exe stands in for it there, running, so the lock
# is the real image lock. Reinstalls reuse the build above (cargo has nothing to do).
Write-Host "== reinstall twice while the installed exe runs, then once after it ends"
$exe = Join-Path $built "bin\herdr-nvim.exe"
Copy-Item -LiteralPath (Join-Path $env:SystemRoot "System32\PING.EXE") -Destination $exe -Force
$running = Start-Process -FilePath $exe -ArgumentList "-n 600 127.0.0.1" -WindowStyle Hidden -PassThru
try {
    $r = Invoke-Hook $built $true
    Assert "first reinstall: exit 0" ($r.Exit -eq 0)
    Assert-Installed $built
    Assert "first reinstall: the running exe is set aside" ((Get-Aside $built).Count -eq 1)
    $r = Invoke-Hook $built $true
    Assert "second reinstall, the first still running: exit 0" ($r.Exit -eq 0)
    Assert-Installed $built
    Assert "second reinstall: the still-running exe stays set aside" ((Get-Aside $built).Count -eq 1)
    Assert-NoDownloadLeft $built
}
finally {
    Stop-Process -Id $running.Id -Force -ErrorAction SilentlyContinue
    $running.WaitForExit()
}
$r = Invoke-Hook $built $true
Assert "after it ends: exit 0" ($r.Exit -eq 0)
Assert-Installed $built
Assert "after it ends: no set-aside exe left" ((Get-Aside $built).Count -eq 0)

Write-Host "== no Windows binary and no cargo: fails and says cargo is missing"
$dir = New-Checkout "0.0.0"
$r = Invoke-Hook $dir $false
Assert "exit non-zero" ($r.Exit -ne 0)
Assert "the error names cargo" ($r.Stderr -match "no cargo")
Assert "the error says the release has no Windows binary yet" ($r.Stderr -match "no Windows binary on GitHub yet")
Assert "nothing installed" (-not (Test-Path -LiteralPath (Join-Path $dir "bin\herdr-nvim.exe")))
Assert-NoDownloadLeft $dir

Finish
