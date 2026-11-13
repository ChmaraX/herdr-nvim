# herdr plugin build hook on Windows: fetch the prebuilt binary for this
# platform, or build from source as a fallback. The Windows twin of install.sh.
# Runs under Windows PowerShell 5.1 (herdr calls powershell.exe).
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root "bin"
$exe = Join-Path $bin "herdr-nvim.exe"
$download = Join-Path $bin "herdr-nvim.download.exe"
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

function Install-Prebuilt {
    if (-not $target) {
        Say "no prebuilt binary for $arch"
        return $false
    }
    $url = "https://github.com/ChmaraX/herdr-nvim/releases/download/v$version/herdr-nvim-$target.exe"
    try {
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $download
        }
        catch {
            Say "could not download $url ($($_.Exception.Message))"
            return $false
        }
        # Every build prints usage and exits 2 when run with no arguments, so
        # any other result means the binary never ran (truncated, blocked, ...).
        try { $status = Invoke-Exe $download } catch { $status = $_.Exception.Message }
        if ($status -ne 2) {
            Say "prebuilt binary for $target did not run (exit $status)"
            return $false
        }
        Move-Item -LiteralPath $download -Destination $exe -Force
        return $true
    }
    finally {
        Remove-Item -LiteralPath $download -Force -ErrorAction SilentlyContinue
    }
}

# cargo finds the MSVC build tools itself (any Visual Studio or Build Tools
# install, or a developer shell), so no VsDevCmd.bat is needed.
function Install-FromSource {
    $cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $cargo) {
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
    Copy-Item -LiteralPath (Join-Path $targetDir "release\herdr-nvim.exe") -Destination $exe -Force
}

if (-not (Install-Prebuilt)) {
    Say "falling back to a source build"
    Install-FromSource
}
