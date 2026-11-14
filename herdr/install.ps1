# herdr plugin build hook on Windows: fetch the prebuilt binary for this
# platform, or build from source as a fallback. The Windows twin of install.sh.
# Runs under Windows PowerShell 5.1 (herdr calls powershell.exe).
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root "bin"
$exe = Join-Path $bin "herdr-nvim.exe"
# The new exe waits here (downloaded, or copied from cargo's output) until
# Install-Exe renames it into place.
$staged = Join-Path $bin "herdr-nvim.new.exe"
# Set when the release has no Windows binary (404), so the no-cargo error can say so.
$notPublished = $false
New-Item -ItemType Directory -Force -Path $bin | Out-Null

$version = (Select-String -LiteralPath (Join-Path $root "herdr-plugin.toml") -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
# A 32-bit PowerShell on 64-bit Windows reports the real machine in PROCESSOR_ARCHITEW6432.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
# Only x64 has a prebuilt. ARM64 builds natively rather than run x64 under emulation.
$target = if ($arch -eq "AMD64") { "x86_64-pc-windows-msvc" } else { $null }

function Say([string]$message) {
    [Console]::Error.WriteLine("herdr-nvim: $message")
}

function Fail([string]$message) {
    Say $message
    exit 1
}

# The child writes to this script's own stdout and stderr, so Windows
# PowerShell 5.1 never turns cargo's progress on stderr into errors.
function Invoke-Exe([string]$path, [string]$arguments = "") {
    $info = New-Object System.Diagnostics.ProcessStartInfo -ArgumentList $path, $arguments
    $info.UseShellExecute = $false
    $info.WorkingDirectory = $root
    $process = [System.Diagnostics.Process]::Start($info)
    $process.WaitForExit()
    $process.ExitCode
}

# Windows can't overwrite or delete a running exe (an open sidebar runs
# bin\herdr-nvim.exe until it closes), but it can rename one. So the current exe
# moves aside under a name of its own, the staged one is renamed in, and if
# that fails the current one moves back: the install never ends without an exe.
function Install-Exe {
    if (Test-Path -LiteralPath $exe) {
        # Earlier installs' exes, gone once their sidebars have closed.
        Get-ChildItem -LiteralPath $bin -Filter "herdr-nvim.old-*.exe" |
            Remove-Item -Force -ErrorAction SilentlyContinue
    }
    $aside = $null
    try {
        if (Test-Path -LiteralPath $exe) {
            $aside = Join-Path $bin ("herdr-nvim.old-" + [Guid]::NewGuid().ToString("N").Substring(0, 8) + ".exe")
            Move-Item -LiteralPath $exe -Destination $aside
        }
        Move-Item -LiteralPath $staged -Destination $exe
    }
    catch {
        if ($aside -and -not (Test-Path -LiteralPath $exe)) {
            Move-Item -LiteralPath $aside -Destination $exe -ErrorAction SilentlyContinue
        }
        Fail "could not install $exe ($($_.Exception.Message))"
    }
    # Still running: a later install removes it.
    if ($aside) { Remove-Item -LiteralPath $aside -Force -ErrorAction SilentlyContinue }
}

function Install-Prebuilt {
    if (-not $target) {
        Say "no prebuilt binary for $arch"
        return $false
    }
    $url = "https://github.com/ChmaraX/herdr-nvim/releases/download/v$version/herdr-nvim-$target.exe"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $staged
    }
    catch {
        $response = $_.Exception.PSObject.Properties["Response"]
        if ($response -and $response.Value -and [int]$response.Value.StatusCode -eq 404) {
            $script:notPublished = $true
        }
        Say "could not download $url ($($_.Exception.Message))"
        return $false
    }
    # Every build prints usage and exits 2 when run with no arguments, so
    # anything else means the binary never ran (truncated, blocked, ...).
    try {
        $status = Invoke-Exe $staged
    }
    catch {
        Say "prebuilt binary for $target did not start ($($_.Exception.Message))"
        return $false
    }
    if ($status -ne 2) {
        Say "prebuilt binary for $target did not run (exit $status)"
        return $false
    }
    Install-Exe
    return $true
}

# cargo finds the MSVC build tools itself (any Visual Studio or Build Tools
# install, or a developer shell), so no VsDevCmd.bat is needed.
function Install-FromSource {
    $cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $cargo) {
        if ($notPublished) {
            Say "v$version has no Windows binary on GitHub yet: a new release gets one a few minutes after it is published, so try again later"
        }
        Fail "no cargo to build from source. Install Rust (https://rustup.rs) and the MSVC build tools (Visual Studio Build Tools, 'Desktop development with C++'), then install the plugin again."
    }
    Say "building from source"
    $status = Invoke-Exe $cargo.Path "build --release"
    if ($status -ne 0) {
        Fail "cargo build --release failed (exit $status). Building on Windows needs the MSVC build tools (Visual Studio Build Tools, 'Desktop development with C++'); cargo's output above says what is missing."
    }
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
    if (-not [System.IO.Path]::IsPathRooted($targetDir)) {
        $targetDir = Join-Path $root $targetDir
    }
    $built = Join-Path $targetDir "release\herdr-nvim.exe"
    if (-not (Test-Path -LiteralPath $built)) {
        Fail "cargo build --release succeeded but $built was not created"
    }
    Copy-Item -LiteralPath $built -Destination $staged -Force
    Install-Exe
}

# Like install.sh's trap: the staged exe never outlives the script, on any path
# (a stale one from an interrupted run included).
try {
    if (-not (Install-Prebuilt)) {
        Say "falling back to a source build"
        Install-FromSource
    }
}
finally {
    Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
}
