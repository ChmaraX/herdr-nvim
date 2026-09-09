Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$bin = Join-Path $root "bin"
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
if (-not [System.IO.Path]::IsPathRooted($targetDir)) {
    $targetDir = Join-Path $root $targetDir
}
$target = Join-Path $targetDir "release\herdr-nvim.exe"

New-Item -ItemType Directory -Force -Path $bin | Out-Null
Push-Location $root
try {
    $vsDevCmdCandidates = @(
        (Join-Path ${env:ProgramFiles} "Microsoft Visual Studio\2022\Professional\Common7\Tools\VsDevCmd.bat"),
        (Join-Path ${env:ProgramFiles} "Microsoft Visual Studio\2022\Enterprise\Common7\Tools\VsDevCmd.bat"),
        (Join-Path ${env:ProgramFiles} "Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat"),
        (Join-Path ${env:ProgramFiles} "Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat")
    )
    $vsDevCmd = $null
    foreach ($candidate in $vsDevCmdCandidates) {
        if (Test-Path -LiteralPath $candidate) {
            $vsDevCmd = $candidate
            break
        }
    }
    if (-not $vsDevCmd) {
        throw "Visual Studio 2022 Developer Command Prompt was not found"
    }

    $buildCommand = "call `"$vsDevCmd`" -arch=x64 && cargo build --release"
    & cmd.exe /d /c $buildCommand
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

if (-not (Test-Path -LiteralPath $target)) {
    throw "Build completed but $target was not created"
}
Copy-Item -LiteralPath $target -Destination (Join-Path $bin "herdr-nvim.exe") -Force
