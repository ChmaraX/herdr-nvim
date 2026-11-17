# What the PowerShell E2E scripts share: the transcript, assertions, and
# running a program with its exit code and output. Dot-sourced
# (`. $PSScriptRoot/common.ps1`) after `Set-StrictMode`.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingPositionalParameters', '', Justification = 'internal helpers')]
param()
$Utf8 = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = $Utf8
$OutputEncoding = $Utf8

class E2EFailure : Exception { E2EFailure([string]$message) : base($message) {} }

$Transcript = [Collections.Generic.List[string]]::new()

function Write-Transcript([string]$text) {
    $Transcript.Add($text)
    [Console]::Out.WriteLine($text)
}

function Assert-That([bool]$ok, [string]$what, [string]$why = '') {
    if ($ok) {
        Write-Transcript "- PASS: $what"
        return
    }
    Write-Transcript "- FAIL: $what$(if ($why) { " ($why)" })"
    throw [E2EFailure]::new($what)
}

function Format-Arg([string]$arg) {
    if ($arg -ne '' -and $arg -notmatch "[\s`"'&|<>^%!;()]") { return $arg }
    "'" + $arg.Replace("'", "''") + "'"
}

function Format-Output([string]$label, [string]$text) {
    if (-not $text) { return "${label}: (empty)" }
    "${label}:`n~~~`n$($text.TrimEnd())`n~~~"
}

# Starts a program with no stdin, capturing its output (UTF-8).
# ArgumentList: each argument arrives as given.
function Open-Native([string]$File, [string[]]$Arguments = @(), [hashtable]$Environment = @{}) {
    $info = [Diagnostics.ProcessStartInfo]::new($File)
    foreach ($arg in $Arguments) { $info.ArgumentList.Add($arg) }
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = $Utf8
    $info.StandardErrorEncoding = $Utf8
    foreach ($name in $Environment.Keys) {
        if ($null -eq $Environment[$name]) { $null = $info.Environment.Remove($name) }
        else { $info.Environment[$name] = $Environment[$name] }
    }
    $process = [Diagnostics.Process]::Start($info)
    $process.StandardInput.Close()
    [pscustomobject]@{
        Process = $process
        Command = (@($File) + $Arguments | ForEach-Object { Format-Arg $_ }) -join ' '
        Stdout = $process.StandardOutput.ReadToEndAsync()
        Stderr = $process.StandardError.ReadToEndAsync()
    }
}

# Waits for an `Open-Native` program and writes it to the transcript.
function Wait-Native($started, [int]$TimeoutSec = 900, [switch]$Quiet) {
    $process = $started.Process
    $stdout = $started.Stdout
    $stderr = $started.Stderr
    $code = if ($process.WaitForExit($TimeoutSec * 1000)) { $process.ExitCode } else { $process.Kill($true); 'timeout' }
    $process.WaitForExit()
    $null = $stdout.Wait(10000)
    $null = $stderr.Wait(10000)
    $result = [pscustomobject]@{
        Command = $started.Command
        Code = $code
        Out = $(if ($stdout.IsCompleted) { $stdout.Result } else { '' })
        Err = $(if ($stderr.IsCompleted) { $stderr.Result } else { '' })
    }
    if (-not $Quiet) {
        Write-Transcript "`n``$($result.Command)`` exit $($result.Code)`n"
        Write-Transcript (Format-Output 'stdout' $result.Out)
        Write-Transcript (Format-Output 'stderr' $result.Err)
    }
    $result
}

function Invoke-Native {
    param(
        [string]$File,
        [string[]]$Arguments = @(),
        [hashtable]$Environment = @{},
        [int]$TimeoutSec = 900,
        [switch]$Quiet
    )
    Wait-Native (Open-Native $File $Arguments $Environment) -TimeoutSec $TimeoutSec -Quiet:$Quiet
}

function Write-Utf8([string]$path, [string]$text) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
    [IO.File]::WriteAllText($path, $text, $Utf8)
}

function Get-ByteContent([string]$path) {
    if (Test-Path -LiteralPath $path) { [IO.File]::ReadAllBytes($path) } else { $null }
}

function Test-SameContent($a, $b) {
    if ($null -eq $a -or $null -eq $b) { return $null -eq $a -and $null -eq $b }
    [Linq.Enumerable]::SequenceEqual([byte[]]$a, [byte[]]$b)
}

# A string as a PowerShell literal, for a -Command line.
function Format-PsLiteral([string]$text) { "'" + $text.Replace("'", "''") + "'" }
