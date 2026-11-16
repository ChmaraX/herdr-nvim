# The Herdr app install end to end, without the app, on macOS, Linux and
# Windows (PowerShell 7). It installs a release bundle through the bootstrap
# a person runs (install.sh; on Windows install.ps1 under Windows PowerShell
# 5.1) into private folders, against a stand-in herdr-app and hc that are
# real exes (examples/herdr_app_stub.rs; its `check` really loads the mods'
# imports), and checks what a person and the app would see:
#
#   0. the stand-in's check fails a broken mod and passes a good one
#   1. a fresh install (-NvimDownload: with no usable Neovim, which it downloads)
#   2. arguments and exit codes pass through the mod's launcher unchanged
#   3. app-list of a fixture folder equals one golden JSON on every OS
#   4. an upgrade while the old runtime is held by a running watcher and nvim
#   5. migration from main's bash install (Windows: #72's .cmd install), and
#      a foreign mods/herdr-nvim refused with nothing changed
#   8. two installs at once: one finishes, the other waits or refuses cleanly
#   7. recovery from what a crashed install leaves
#   6. uninstall, byte for byte back to before (last: it ends the install);
#      on Windows first with a mod file held open
#
#   pwsh -NoProfile -File herdr-app/e2e/install-e2e.ps1 -Out e2e-out [-Bundle FILE] [-NvimDownload]
#
# Without -Bundle it builds one (herdr-app/bundle.sh, from `cargo build
# --release`). -Out gets transcript.md (every command, its exit code and
# output, every assertion), stub-calls.jsonl (every herdr-app and hc call),
# and per step the app home's and the runtime store's tree listings, keys.json,
# the layout, install.json and config.toml. It exits 1 on the first failed
# assertion, after writing all of that.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', '', Justification = 'the steps read them from the script scope')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingPositionalParameters', '', Justification = 'internal helpers')]
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Bundle,
    [switch]$NvimDownload
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Utf8 = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = $Utf8
$OutputEncoding = $Utf8

class E2EFailure : Exception { E2EFailure([string]$message) : base($message) {} }

$Repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$Exe = if ($IsWindows) { '.exe' } else { '' }
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
if (Test-Path -LiteralPath $Out) { Remove-Item -LiteralPath $Out -Recurse -Force }
New-Item -ItemType Directory -Path $Out | Out-Null
$StubLog = Join-Path $Out 'stub-calls.jsonl'
# Short and free of links: a Unix socket path must stay under 104 bytes, and
# paths are compared as the installer resolves them.
$Base = if ($IsWindows) { if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() } }
elseif (Test-Path -LiteralPath '/private/tmp') { '/private/tmp' } else { '/tmp' }
$Root = Join-Path $Base ('hnv-e2e-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$Transcript = [Collections.Generic.List[string]]::new()
$Holders = [Collections.Generic.List[Diagnostics.Process]]::new()
$MarkLine = "# Written by herdr-nvim's herdr-app/install.sh"

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

# A process that runs until this script ends it: stdin at end, output on this
# console.
function Invoke-Holder([string]$File, [string[]]$Arguments, [hashtable]$Environment = @{}) {
    $info = [Diagnostics.ProcessStartInfo]::new($File)
    foreach ($arg in $Arguments) { $info.ArgumentList.Add($arg) }
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    foreach ($name in $Environment.Keys) { $info.Environment[$name] = $Environment[$name] }
    $process = [Diagnostics.Process]::Start($info)
    $process.StandardInput.Close()
    $Holders.Add($process)
    Write-Transcript "`nholder pid $($process.Id): ``$((@($File) + $Arguments | ForEach-Object { Format-Arg $_ }) -join ' ')``"
    $process
}

function Close-Holder([Diagnostics.Process]$process) {
    if (-not $process.HasExited) { $process.Kill($true) }
    $null = $process.WaitForExit(10000)
    $null = $Holders.Remove($process)
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

function Get-Sha256([string]$path) {
    (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Every entry under `dir`, sorted: `d rel/`, `f rel size sha256[0..12]`, or
# `o rel` (unreadable: a socket, or a file Windows holds).
function Get-TreeListing([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return @('(absent)') }
    [string[]]$lines = @(Get-ChildItem -LiteralPath $dir -Recurse -Force | ForEach-Object {
            $rel = [IO.Path]::GetRelativePath($dir, $_.FullName).Replace('\', '/')
            if ($_.PSIsContainer) { "d $rel/" }
            else {
                try { "f $rel $($_.Length) $((Get-Sha256 $_.FullName).Substring(0, 12))" }
                catch { "o $rel" }
            }
        })
    [Array]::Sort($lines, [StringComparer]::Ordinal)
    $lines
}

function Get-EntryName([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return @() }
    [string[]]$names = @(Get-ChildItem -LiteralPath $dir -Force | ForEach-Object Name)
    [Array]::Sort($names, [StringComparer]::Ordinal)
    $names
}

function Get-StubCall {
    if (-not (Test-Path -LiteralPath $StubLog)) { return @() }
    @(Get-Content -LiteralPath $StubLog -Encoding utf8 | ForEach-Object { $_ | ConvertFrom-Json })
}

# A private user: app home (with a space in its path), XDG folders, nvim's
# registry. The app home has the person's keys.json and personal layout.
function Initialize-PrivateEnv([string]$name) {
    $dir = Join-Path $Root $name
    $vars = [ordered]@{
        HOME = Join-Path $dir 'user'
        HERDR_APP_HOME = Join-Path $dir 'app home'
        XDG_DATA_HOME = Join-Path $dir 'data'
        XDG_CONFIG_HOME = Join-Path $dir 'config'
        XDG_STATE_HOME = Join-Path $dir 'state'
        HERDR_NVIM_RUNTIME_DIR = Join-Path $dir 'run'
        HERDR_NVIM_STATE_DIR = Join-Path $dir 'nvstate'
    }
    foreach ($path in $vars.Values) { New-Item -ItemType Directory -Force -Path $path | Out-Null }
    $fixtures = Join-Path $PSScriptRoot 'fixtures'
    # LF whatever the checkout did: the bytes are compared after uninstall.
    $keys = (Get-Content -Raw -LiteralPath (Join-Path $fixtures 'keys.json')).Replace("`r`n", "`n")
    $layout = (Get-Content -Raw -LiteralPath (Join-Path $fixtures 'layout-mine.json')).Replace("`r`n", "`n")
    Write-Utf8 (Join-Path $vars.HERDR_APP_HOME 'keys.json') $keys
    Write-Utf8 (Join-Path $vars.HERDR_APP_HOME 'layouts/mine.json') $layout
    Write-Utf8 (Join-Path $vars.HERDR_APP_HOME '.e2e-home') $name
    [pscustomobject]$vars | Add-Member -PassThru -NotePropertyName Name -NotePropertyValue $name
}

function Use-PrivateEnv($privateEnv) {
    foreach ($name in 'HOME', 'HERDR_APP_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_STATE_HOME', 'HERDR_NVIM_RUNTIME_DIR', 'HERDR_NVIM_STATE_DIR') {
        Set-Item -Path "env:$name" -Value $privateEnv.$name
    }
}

function Get-Store($privateEnv) { Join-Path $privateEnv.XDG_DATA_HOME 'herdr-nvim/app' }
function Get-ModDir($privateEnv, [string]$mod) { Join-Path $privateEnv.HERDR_APP_HOME "mods/$mod" }
function Get-Launcher($privateEnv) { Join-Path (Get-ModDir $privateEnv 'herdr-nvim') 'bin/herdr-nvim' }
function Get-KeysFile($privateEnv) { Join-Path $privateEnv.HERDR_APP_HOME 'keys.json' }
function Get-LayoutFile($privateEnv) { Join-Path $privateEnv.HERDR_APP_HOME 'layouts/mine.json' }
function Get-ConfigFile($privateEnv) { Join-Path $privateEnv.XDG_CONFIG_HOME 'herdr-nvim/config.toml' }

# `gen/<id>` that `<mod>/main.js` re-exports, or $null.
function Get-Generation([string]$modDir) {
    $pointer = Join-Path $modDir 'main.js'
    if (-not (Test-Path -LiteralPath $pointer)) { return $null }
    if ((Get-Content -Raw -LiteralPath $pointer) -match 'from "\./gen/([^/"]+)/main\.js"') { return $Matches[1] }
    $null
}

# The launcher's record: at the mod's top, or in the generation main.js
# re-exports.
function Get-RecordPath($privateEnv) {
    $mod = Get-ModDir $privateEnv 'herdr-nvim'
    $generation = Get-Generation $mod
    $paths = @(Join-Path $mod 'install.json')
    if ($generation) { $paths = @(Join-Path $mod "gen/$generation/install.json") + $paths }
    $paths | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}

function Read-Record($privateEnv) {
    $path = Get-RecordPath $privateEnv
    if (-not $path) { return $null }
    Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
}

function Save-Snapshot([string]$label, $privateEnv) {
    $dir = Join-Path $Out $label
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $home_ = $privateEnv.HERDR_APP_HOME
    $copies = @{
        'keys.json' = Get-KeysFile $privateEnv
        'layout-mine.json' = Get-LayoutFile $privateEnv
        'install.json' = $(if (Get-RecordPath $privateEnv) { Get-RecordPath $privateEnv } else { Join-Path $Root '(none)' })
        'config.toml' = Get-ConfigFile $privateEnv
    }
    foreach ($name in $copies.Keys) {
        if (Test-Path -LiteralPath $copies[$name]) { Copy-Item -LiteralPath $copies[$name] -Destination (Join-Path $dir $name) }
    }
    Get-TreeListing $home_ | Set-Content -LiteralPath (Join-Path $dir 'tree-app-home.txt') -Encoding utf8NoBOM
    Get-TreeListing (Get-Store $privateEnv) | Set-Content -LiteralPath (Join-Path $dir 'tree-runtime-store.txt') -Encoding utf8NoBOM
    Get-TreeListing $privateEnv.HERDR_NVIM_RUNTIME_DIR | Set-Content -LiteralPath (Join-Path $dir 'tree-daemon-registry.txt') -Encoding utf8NoBOM
    Write-Transcript "`nsnapshot: $label/ ($($privateEnv.Name))"
}

# The bootstrap a person runs, started: install.sh, or on Windows install.ps1
# under Windows PowerShell 5.1.
function Open-Bootstrap([string]$archive, [switch]$Uninstall, [switch]$Force, [switch]$InstallNvim) {
    if ($IsWindows) {
        $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Repo 'install.ps1'), '-App', '-Bundle', $archive, '-AppBin', $StubApp)
        if ($Uninstall) { $arguments += '-Uninstall' }
        if ($Force) { $arguments += '-Force' }
        if ($InstallNvim) { $arguments += '-InstallNvim' }
        # As a person's own Windows PowerShell starts: without this pwsh's
        # module path, which hides 5.1's own modules from it.
        return Open-Native 'powershell.exe' $arguments @{ PSModulePath = $null }
    }
    $arguments = @((Join-Path $Repo 'install.sh'), '--app', '--bundle', $archive)
    if ($Uninstall) { $arguments += '--uninstall' }
    if ($InstallNvim) { $arguments += '--install-nvim' }
    $arguments += @('--', '--app-bin', $StubApp)
    if ($Force) { $arguments += '--force' }
    Open-Native 'sh' $arguments
}

function Invoke-Bootstrap([string]$archive, [switch]$Uninstall, [switch]$Force, [switch]$InstallNvim) {
    Wait-Native (Open-Bootstrap $archive -Uninstall:$Uninstall -Force:$Force -InstallNvim:$InstallNvim)
}

# The mods' path to herdr-nvim, run the way the app runs it (the stand-in's
# exec: Rust's Command, the path as given).
function Invoke-Launcher([string]$launcher, [string[]]$arguments) {
    Invoke-Native $StubApp (@('exec', $launcher) + $arguments)
}

function Assert-Installed($result, [string]$what) {
    Assert-That ($result.Code -eq 0) "$what exits 0" "exit $($result.Code)"
    Assert-That ($result.Out -match 'herdr-nvim: installed in') "$what reports 'installed in'"
}

function Assert-ModsChecked([int]$since) {
    $calls = @(Get-StubCall | Select-Object -Skip $since |
            Where-Object { $_.name -eq 'herdr-app' -and $_.argv.Count -eq 2 -and $_.argv[0] -eq 'check' })
    foreach ($mod in 'herdr-nvim', 'herdr-nvim-header') {
        $passed = @($calls | Where-Object {
                $_.exit -eq 0 -and (Split-Path -Leaf $_.argv[1]) -eq $mod -and
                (Split-Path -Leaf (Split-Path -Parent $_.argv[1])) -eq ".$mod.new"
            })
        Assert-That ($passed.Count -ge 1) "herdr-app check passed on the staged $mod (stub log)"
    }
}

function Assert-Record($privateEnv, [string]$what) {
    $record = Read-Record $privateEnv
    Assert-That ($null -ne $record) "$what wrote the launcher's install.json"
    Assert-That ($record.schema -eq 1 -and $record.app_bin -eq $StubApp -and $record.by -like 'herdr-nvim *') "install.json: schema 1, app_bin as given, by herdr-nvim" ($record | ConvertTo-Json -Compress)
    $marker = Join-Path $record.home '.e2e-home'
    Assert-That ((Test-Path -LiteralPath $marker) -and (Get-Content -Raw -LiteralPath $marker) -eq $privateEnv.Name) "install.json home is this app home" $record.home
    $store = Get-Store $privateEnv
    $stamp = Split-Path -Leaf $record.runtime
    Assert-That ($stamp -match '^[0-9a-f]{16}$' -and (Test-Path -LiteralPath (Join-Path $store "$stamp/bin/herdr-nvim$Exe"))) "install.json runtime is a stamped runtime in the store" $record.runtime
    $record
}

# Bytes the installer must leave in place of the launcher.
function Assert-Launcher($privateEnv) {
    $bin = Join-Path (Get-ModDir $privateEnv 'herdr-nvim') 'bin'
    Assert-That ((@(Get-EntryName $bin) -join ',') -eq "herdr-nvim$Exe") "mods/herdr-nvim/bin holds the launcher alone" (@(Get-EntryName $bin) -join ',')
    Assert-That (Test-SameContent (Get-ByteContent (Join-Path $bin "herdr-nvim$Exe")) (Get-ByteContent (Join-Path $TreeA "bin/herdr-nvim-launch$Exe"))) "the launcher is the bundle's herdr-nvim-launch"
}

# Each mod's main.js re-exports a generation that has the mod's files, and
# none of the mod's own files sit at its top any more (the bash era's).
function Assert-GenLayout($privateEnv) {
    foreach ($mod in 'herdr-nvim', 'herdr-nvim-header') {
        $dir = Get-ModDir $privateEnv $mod
        $generation = Get-Generation $dir
        Assert-That ($generation -and (Test-Path -LiteralPath (Join-Path $dir "gen/$generation/main.js"))) "mods/$mod/main.js re-exports gen/$generation/main.js, which exists"
        $own = @(Get-EntryName (Join-Path $TreeA "herdr-app/mods/$mod") | Where-Object { $_ -notin 'main.js', 'mod.json' })
        $stray = @(Get-EntryName $dir | Where-Object { $_ -in $own })
        Assert-That ($stray.Count -eq 0) "mods/$mod has none of the mod's files at its top" ($stray -join ',')
    }
    $mark = Join-Path (Get-ModDir $privateEnv 'herdr-nvim-header') '.herdr-nvim'
    Assert-That ((Test-Path -LiteralPath $mark) -and (Get-Content -Raw -LiteralPath $mark).Contains($MarkLine)) 'the header mod carries the herdr-nvim mark'
}

function Find-Node($node, [string]$id) {
    if ($null -eq $node -or $node -isnot [pscustomobject]) { return $null }
    if ($node.PSObject.Properties['id'] -and $node.id -eq $id) { return $node }
    if ($node.PSObject.Properties['children']) {
        foreach ($child in $node.children) {
            $found = Find-Node $child $id
            if ($found) { return $found }
        }
    }
    $null
}

# The bundle's herdr-app/keys.json as bindings (section, key, action): this
# OS's set, and the other OSes' (none while the file is one flat set).
function Get-OurKeySet {
    $file = Get-Content -Raw -LiteralPath (Join-Path $TreeA 'herdr-app/keys.json') | ConvertFrom-Json -AsHashtable
    $os = if ($IsWindows) { 'windows' } elseif ($IsMacOS) { 'macos' } else { 'linux' }
    $flatten = {
        param($set)
        foreach ($section in $set.Keys) {
            foreach ($key in $set[$section].Keys) { [pscustomobject]@{ Section = $section; Key = $key; Action = $set[$section][$key] } }
        }
    }
    if (-not $file.Contains($os)) { return [pscustomobject]@{ Here = @(& $flatten $file); Elsewhere = @() } }
    $here = @(& $flatten $file[$os])
    $elsewhere = @($file.Keys | Where-Object { $_ -ne $os } | ForEach-Object { & $flatten $file[$_] } |
            Where-Object { $b = $_; -not ($here | Where-Object { $_.Section -eq $b.Section -and $_.Key -eq $b.Key -and $_.Action -eq $b.Action }) })
    [pscustomobject]@{ Here = $here; Elsewhere = $elsewhere }
}

function Get-Binding($keys, [string]$section, [string]$key) {
    if ($keys.Contains($section) -and $keys[$section] -is [Collections.IDictionary] -and $keys[$section].Contains($key)) { $keys[$section][$key] } else { $null }
}

# keys.json after an install over `$seeded`: this OS's bindings where the key
# was free or already ours, the person's own kept, other OSes' bindings of
# ours gone.
function Assert-KeysMerged($privateEnv, $seeded) {
    $ours = Get-OurKeySet
    $now = Get-Content -Raw -LiteralPath (Get-KeysFile $privateEnv) | ConvertFrom-Json -AsHashtable
    $all = @($ours.Here) + @($ours.Elsewhere)
    foreach ($b in $ours.Here) {
        $before = Get-Binding $seeded $b.Section $b.Key
        $theirs = $null -ne $before -and -not ($all | Where-Object { $_.Section -eq $b.Section -and $_.Key -eq $b.Key -and $_.Action -eq $before })
        $want = if ($theirs) { $before } else { $b.Action }
        $got = Get-Binding $now $b.Section $b.Key
        Assert-That ($got -eq $want) "keys.json $($b.Section) $($b.Key) is $want ($(if ($theirs) { "the person's, kept" } else { "herdr-nvim's" }))" "is $got"
    }
    foreach ($b in $ours.Elsewhere) {
        if ((Get-Binding $seeded $b.Section $b.Key) -eq $b.Action -and -not ($ours.Here | Where-Object { $_.Section -eq $b.Section -and $_.Key -eq $b.Key })) {
            Assert-That ($null -eq (Get-Binding $now $b.Section $b.Key)) "keys.json drops another OS's $($b.Section) $($b.Key) -> $($b.Action)" "is $(Get-Binding $now $b.Section $b.Key)"
        }
    }
    foreach ($section in $seeded.Keys) {
        foreach ($key in $seeded[$section].Keys) {
            $action = $seeded[$section][$key]
            if ($all | Where-Object { $_.Action -eq $action }) { continue }
            Assert-That ((Get-Binding $now $section $key) -eq $action) "keys.json keeps the person's $section $key"
        }
    }
}

function Assert-LayoutPatched($privateEnv) {
    $layout = Get-Content -Raw -LiteralPath (Get-LayoutFile $privateEnv) | ConvertFrom-Json
    $files = Find-Node $layout 'files'
    Assert-That ($files -and $files.content.mod -eq 'herdr-nvim') "the layout's Files slot shows herdr-nvim's tree"
    $sync = @($(if ($layout.PSObject.Properties['floating']) { $layout.floating }) | Where-Object { $_.id -eq 'herdr-nvim-sync' })
    Assert-That ($sync.Count -eq 1) "the layout has the herdr-nvim-sync controller"
    $tabs = Find-Node $layout 'tabs'
    Assert-That ($tabs.content.pane.header.content.mod -eq 'herdr-nvim-header') "the pane header is herdr-nvim-header"
}

function Open-FakeDaemon($privateEnv, [string]$stamp, [switch]$Dirty) {
    $key = 'e2e' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $registry = Join-Path $privateEnv.HERDR_NVIM_RUNTIME_DIR 'app'
    New-Item -ItemType Directory -Force -Path $registry | Out-Null
    if ($IsWindows) {
        $address = "\\.\pipe\herdr-nvim-$key"
        $entry = Join-Path $registry "$key.pipe"
    }
    else {
        $address = Join-Path $registry "$key.sock"
        $entry = $address
    }
    $arguments = @('--headless', '--clean', '--listen', $address)
    if ($Dirty) { $arguments += @('-c', "call setline(1, 'unsaved work')") }
    $process = Invoke-Holder 'nvim' $arguments @{ HERDR_NVIM_RUNTIME_STAMP = $stamp }
    # The registry marker as herdr-nvim writes it: the tab, the pid, and
    # that process's creation time (FILETIME ticks).
    if ($IsWindows) { Write-Utf8 $entry "$key`n$($process.Id)`n$($process.StartTime.ToFileTimeUtc())`n" }
    $deadline = (Get-Date).AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 200
        $answer = Invoke-Native 'nvim' @('--clean', '--headless', '--server', $address, '--remote-expr', '1') -Quiet -TimeoutSec 10
    } until (($answer.Code -eq 0 -and $answer.Out.Trim() -eq '1') -or (Get-Date) -gt $deadline)
    Assert-That ($answer.Out.Trim() -eq '1') "a fake tab nvim answers on $address (stamp $stamp$(if ($Dirty) { ', unsaved work' }))"
    [pscustomobject]@{ Process = $process; Key = $key; Entry = $entry }
}

function Close-FakeDaemon($daemon) {
    Close-Holder $daemon.Process
    Remove-Item -LiteralPath $daemon.Entry -Force -ErrorAction SilentlyContinue
}

function Get-DaemonRow($privateEnv, [string]$key) {
    $result = Invoke-Launcher (Get-Launcher $privateEnv) @('daemons', '--json')
    Assert-That ($result.Code -eq 0) 'daemons --json through the launcher exits 0'
    @($result.Out | ConvertFrom-Json) | Where-Object { $_.key -eq $key } | Select-Object -First 1
}

function Build-Bundle {
    Write-Transcript "`n## Build"
    $cargo = if ($Bundle) { @('build', '--release', '--example', 'herdr_app_stub') } else { @('build', '--release', '--bins', '--example', 'herdr_app_stub') }
    $built = Invoke-Native 'cargo' $cargo -Environment @{ CARGO_TERM_COLOR = 'never' }
    Assert-That ($built.Code -eq 0) "cargo $($cargo -join ' ')"
    $script:StubApp = Join-Path $Root "bin/herdr-app$Exe"
    $script:StubHc = Join-Path $Root "bin/hc$Exe"
    New-Item -ItemType Directory -Force -Path (Join-Path $Root 'bin') | Out-Null
    foreach ($stub in $StubApp, $StubHc) {
        Copy-Item -LiteralPath (Join-Path $Repo "target/release/examples/herdr_app_stub$Exe") -Destination $stub
    }
    # macOS tar would add AppleDouble ._ files for extended attributes.
    $tarEnv = @{ COPYFILE_DISABLE = '1' }
    $tar = if ($IsWindows) { Join-Path $env:SystemRoot 'System32\tar.exe' } else { 'tar' }
    $bundles = Join-Path $Root 'bundles'
    if ($Bundle) {
        $script:BundleA = [IO.Path]::GetFullPath($Bundle, (Get-Location).Path)
    }
    else {
        $bash = if ($IsWindows) { Join-Path $env:ProgramFiles 'Git\bin\bash.exe' } else { 'bash' }
        $made = Invoke-Native $bash @(
            (Join-Path $Repo 'herdr-app/bundle.sh').Replace('\', '/'),
            (Join-Path $Repo "target/release/herdr-nvim$Exe").Replace('\', '/'),
            'e2e',
            (Join-Path $bundles 'a').Replace('\', '/')
        ) -Environment $tarEnv
        Assert-That ($made.Code -eq 0) 'herdr-app/bundle.sh makes bundle A'
        $script:BundleA = Join-Path $bundles 'a/herdr-nvim-app-e2e.tar.gz'
    }
    $tree = Join-Path $bundles 'a-tree'
    New-Item -ItemType Directory -Force -Path $tree | Out-Null
    Assert-That ((Invoke-Native $tar @('-xzf', $BundleA, '-C', $tree) -Environment $tarEnv).Code -eq 0) 'bundle A unpacks'
    $top = @(Get-ChildItem -LiteralPath $tree -Directory)[0]
    $script:TreeA = $top.FullName
    # Bundle B: A with one Lua file changed, so a new runtime stamp.
    $treeB = Join-Path $bundles 'b-tree'
    New-Item -ItemType Directory -Force -Path $treeB | Out-Null
    Assert-That ((Invoke-Native $tar @('-xzf', $BundleA, '-C', $treeB) -Environment $tarEnv).Code -eq 0) 'bundle A unpacks for B'
    $lua = Join-Path $treeB "$($top.Name)/lua/herdr-nvim/init.lua"
    [IO.File]::AppendAllText($lua, "`n-- install-e2e: bundle B`n", $Utf8)
    New-Item -ItemType Directory -Force -Path (Join-Path $bundles 'b') | Out-Null
    $script:BundleB = Join-Path $bundles "b/$($top.Name).tar.gz"
    Assert-That ((Invoke-Native $tar @('-czf', $BundleB, '-C', $treeB, $top.Name) -Environment $tarEnv).Code -eq 0) 'bundle B packs'
    Write-Utf8 "$BundleB.sha256" "$(Get-Sha256 $BundleB)  $($top.Name).tar.gz`n"
}

function Step-StubSelfCheck {
    Write-Transcript "`n## 0. The stand-in's check fails a broken mod and passes a good one"
    $broken = Join-Path $Root 'broken-mod'
    Write-Utf8 (Join-Path $broken 'mod.json') "{`"name`": `"broken`"}`n"
    Write-Utf8 (Join-Path $broken 'main.js') "export * from `"./gen/0000/main.js`";`n"
    $result = Invoke-Native $StubApp @('check', $broken)
    Assert-That ($result.Code -eq 1 -and $result.Err.Contains('./gen/0000/main.js')) 'check fails a main.js whose import does not resolve, and names it'
    $result = Invoke-Native $StubApp @('check', (Join-Path $TreeA 'herdr-app/mods/herdr-nvim'))
    Assert-That ($result.Code -eq 0) "check passes the bundle's own herdr-nvim mod"
}

function Step-FreshInstall {
    Write-Transcript "`n## 1. Fresh install through the bootstrap$(if ($NvimDownload) { ', with no usable Neovim (downloaded)' })"
    $script:PreKeys = Get-ByteContent (Get-KeysFile $Main)
    $script:PreLayout = Get-ByteContent (Get-LayoutFile $Main)
    Save-Snapshot '00-before' $Main
    $since = @(Get-StubCall).Count
    if ($NvimDownload) {
        Write-Utf8 (Get-ConfigFile $Main) "[sidebar]`nnvim_bin = '$(Join-Path $Root "no-nvim/nvim$Exe")'`n"
        $env:HERDR_NVIM_INSTALL_METHOD = 'download'
    }
    try { $result = Invoke-Bootstrap $BundleA -InstallNvim:$NvimDownload }
    finally { Remove-Item env:HERDR_NVIM_INSTALL_METHOD -ErrorAction SilentlyContinue }
    Assert-Installed $result 'the fresh install'
    $script:RecordA = Assert-Record $Main 'the fresh install'
    $script:StampA = Split-Path -Leaf $RecordA.runtime
    Assert-Launcher $Main
    Assert-GenLayout $Main
    Assert-KeysMerged $Main ([Text.Encoding]::UTF8.GetString($PreKeys) | ConvertFrom-Json -AsHashtable)
    Assert-LayoutPatched $Main
    Assert-ModsChecked $since
    if ($NvimDownload) {
        $check = Invoke-Launcher (Get-Launcher $Main) @('app-nvim-check')
        $bin = if ($check.Code -eq 0) { ($check.Out | ConvertFrom-Json).bin } else { '' }
        $downloads = [IO.Path]::GetFullPath((Join-Path $Main.XDG_DATA_HOME 'herdr-nvim/nvim'))
        $inside = $bin -and [IO.Path]::GetFullPath($bin).StartsWith($downloads + [IO.Path]::DirectorySeparatorChar)
        Assert-That ($inside -and (Test-Path -LiteralPath $bin)) "herdr-nvim runs the Neovim it downloaded into $downloads" $bin
    }
    else {
        Write-Transcript '- SKIP: the no-Neovim variant (-NvimDownload)'
    }
    Save-Snapshot '01-fresh-install' $Main
}

function Step-ArgumentFidelity {
    Write-Transcript "`n## 2. Arguments and exit codes through the launcher"
    $launcher = Get-Launcher $Main
    $paths = @($launcher)
    if ($IsWindows) { $paths += "$launcher.exe" }
    $config = Get-ConfigFile $Main
    $before = Get-ByteContent $config
    $odd = "nv `"im`" %PATH% & ^ ! 'q' $([char]0x00E9) $([char]0x6F22) \x\"
    foreach ($path in $paths) {
        $result = Invoke-Launcher $path @('app-nvim-check')
        Assert-That ($result.Code -eq 0 -and ($result.Out | ConvertFrom-Json).ok) "$(Split-Path -Leaf $path): app-nvim-check finds Neovim (exit 0)"
        $result = Invoke-Launcher $path @('app-config', 'nvim-bin', $odd)
        Assert-That ($result.Code -eq 0) "$(Split-Path -Leaf $path): app-config nvim-bin <odd> exits 0"
        $result = Invoke-Launcher $path @('app-nvim-check')
        $missing = @($result.Out -split "`n" | Where-Object { $_.Trim().StartsWith('{') } | ForEach-Object { $_ | ConvertFrom-Json } |
                Where-Object { $_.PSObject.Properties['nvim_missing'] })
        $got = if ($missing.Count) { $missing[0].nvim_missing.bin } else { '(none)' }
        Assert-That ($result.Code -eq 3) "$(Split-Path -Leaf $path): app-nvim-check exits 3 for a missing nvim" "exit $($result.Code)"
        Assert-That ($got -ceq $odd) "$(Split-Path -Leaf $path): app-nvim-check reports exactly the value given: $odd" "got $got"
        $result = Invoke-Launcher $path @('no-such-command')
        Assert-That ($result.Code -eq 2) "$(Split-Path -Leaf $path): an unknown command's exit 2 comes through" "exit $($result.Code)"
        if ($null -eq $before) { Remove-Item -LiteralPath $config -Force } else { [IO.File]::WriteAllBytes($config, $before) }
    }
}

function Step-AppList {
    Write-Transcript "`n## 3. app-list golden listing"
    $fixture = Join-Path $Root 'app-list fixture'
    & (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'app-list-fixture.ps1') $fixture
    Assert-That ($LASTEXITCODE -eq 0) 'the app-list fixture is built' "exit $LASTEXITCODE"
    $result = Invoke-Launcher (Get-Launcher $Main) @('app-list', $fixture)
    Assert-That ($result.Code -eq 0) 'app-list of the fixture exits 0 (through the launcher)' "exit $($result.Code): $($result.Err)"
    $got = $result.Out | ConvertFrom-Json | ConvertTo-Json -Depth 10
    $want = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'fixtures/app-list.golden.json') | ConvertFrom-Json | ConvertTo-Json -Depth 10
    [IO.File]::WriteAllText((Join-Path $Out 'app-list.json'), $got, $Utf8)
    $diff = Compare-Object ($want -split "`n") ($got -split "`n") | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }
    [IO.File]::WriteAllText((Join-Path $Out 'app-list.diff'), ($diff -join "`n"), $Utf8)
    Assert-That (-not $diff) 'app-list matches fixtures/app-list.golden.json (sorted, then capped at 500; links as folders; ignore marks)' "$(@($diff).Count) lines differ, see app-list.diff"
}

function Step-Upgrade {
    Write-Transcript "`n## 4. Upgrade A -> B while A is held"
    $daemon = Open-FakeDaemon $Main $StampA
    $since = @(Get-StubCall).Count
    $watcher = Invoke-Holder (Join-Path $RecordA.runtime "bin/herdr-nvim$Exe") @('--app-home', $Main.HERDR_APP_HOME, '--app-bin', $StubApp, 'app-watch')
    Start-Sleep -Seconds 2
    $events = @(Get-StubCall | Select-Object -Skip $since | Where-Object { $_.name -eq 'hc' -and $_.argv[0] -eq 'events' })
    Assert-That (-not $watcher.HasExited -and $events.Count -ge 1) "A's app-watch runs, waiting on hc events"
    $row = Get-DaemonRow $Main $daemon.Key
    Assert-That ($row -and $row.stamp -eq $StampA -and -not $row.stale) 'the launcher runs A: the fake nvim on A is not stale'

    $since = @(Get-StubCall).Count
    $result = Invoke-Bootstrap $BundleB
    Assert-Installed $result 'the upgrade to B, A held'
    $record = Assert-Record $Main 'the upgrade'
    $script:StampB = Split-Path -Leaf $record.runtime
    Assert-That ($StampB -ne $StampA) 'install.json points at a new runtime, B' "$StampA -> $StampB"
    Assert-That (Test-Path -LiteralPath $RecordA.runtime) "A's runtime is kept while it is held"
    Assert-That ($result.Out -match 'stale') 'the install reports the nvim on A as stale'
    Assert-ModsChecked $since
    Assert-GenLayout $Main
    $row = Get-DaemonRow $Main $daemon.Key
    Assert-That ($row -and $row.stale) 'the launcher runs B: the fake nvim on A is now stale'
    Save-Snapshot '04a-upgrade-held' $Main
    $keys = Get-ByteContent (Get-KeysFile $Main)
    $layout = Get-ByteContent (Get-LayoutFile $Main)

    Close-Holder $watcher
    Close-FakeDaemon $daemon
    $result = Invoke-Bootstrap $BundleB
    Assert-Installed $result 'B again, A released'
    Assert-That (-not (Test-Path -LiteralPath $RecordA.runtime)) "A's runtime is pruned once nothing holds it"
    $store = @(Get-EntryName (Get-Store $Main))
    Assert-That (($store -join ',') -eq $StampB) 'the runtime store holds B alone' ($store -join ',')
    Assert-That (Test-SameContent (Get-ByteContent (Get-KeysFile $Main)) $keys) 'keys.json is byte-identical to after the first B install'
    Assert-That (Test-SameContent (Get-ByteContent (Get-LayoutFile $Main)) $layout) 'the layout is byte-identical to after the first B install'
    Save-Snapshot '04b-upgrade-released' $Main
}

# What main's bash installer left (1bc902b herdr-app/install.sh), or on
# Windows #72's Git Bash install: the mods' files at their top level, the
# wrapper, the header's mark, a stamped runtime; keys and layout done.
function Write-LegacyInstall($privateEnv, [string]$stamp) {
    $runtime = Join-Path (Get-Store $privateEnv) $stamp
    New-Item -ItemType Directory -Force -Path (Join-Path $runtime 'bin') | Out-Null
    Copy-Item -LiteralPath (Join-Path $TreeA "bin/herdr-nvim$Exe") -Destination (Join-Path $runtime "bin/herdr-nvim$Exe")
    Copy-Item -LiteralPath (Join-Path $TreeA 'lua'), (Join-Path $TreeA 'plugin') -Destination $runtime -Recurse
    Write-Utf8 (Join-Path $runtime 'runtime-stamp') "$stamp`n"
    foreach ($mod in 'herdr-nvim', 'herdr-nvim-header') {
        New-Item -ItemType Directory -Force -Path (Join-Path $privateEnv.HERDR_APP_HOME 'mods') | Out-Null
        Copy-Item -LiteralPath (Join-Path $TreeA "herdr-app/mods/$mod") -Destination (Get-ModDir $privateEnv $mod) -Recurse
    }
    Write-Utf8 (Join-Path (Get-ModDir $privateEnv 'herdr-nvim-header') '.herdr-nvim') "$MarkLine`n"
    $bin = Join-Path (Get-ModDir $privateEnv 'herdr-nvim') 'bin'
    $home_ = $privateEnv.HERDR_APP_HOME
    if ($IsWindows) {
        $cmd = { param($path) $path.Replace('%', '%%') }
        $wrapper = Join-Path $bin 'herdr-nvim.cmd'
        $lines = @(
            '@echo off'
            'chcp 65001 >nul'
            "rem $MarkLine; run it again to change this."
            'set "HERDR_NVIM_HOST=app"'
            "set `"HERDR_NVIM_SELF=$(& $cmd $wrapper)`""
            "set `"HERDR_NVIM_PLUGIN_ROOT=$(& $cmd $runtime)`""
            "set `"HERDR_APP_BIN=$(& $cmd $StubApp)`""
            "set `"HERDR_APP_HOME=$(& $cmd $home_)`""
            "`"$(& $cmd (Join-Path $runtime 'bin\herdr-nvim.exe'))`" %*"
        )
        Write-Utf8 $wrapper (($lines -join "`r`n") + "`r`n")
    }
    else {
        $wrapper = Join-Path $bin 'herdr-nvim'
        Write-Utf8 $wrapper @"
#!/bin/sh
$MarkLine; run it again to change this.
export HERDR_NVIM_HOST=app
export HERDR_NVIM_SELF='$wrapper'
export HERDR_NVIM_PLUGIN_ROOT='$runtime'
export HERDR_APP_BIN='$StubApp'
export HERDR_APP_HOME='$home_'
exec '$runtime/bin/herdr-nvim' "`$@"

"@
        [IO.File]::SetUnixFileMode($wrapper, [IO.UnixFileMode]'UserRead, UserWrite, UserExecute, GroupRead, GroupExecute, OtherRead, OtherExecute')
    }
    # Those installs bound cmd-e, cmd-o and cmd-shift-e on every OS.
    $keys = Get-Content -Raw -LiteralPath (Get-KeysFile $privateEnv) | ConvertFrom-Json -AsHashtable
    $keys['global']['cmd-e'] = 'mod.open:herdr-nvim'
    $keys['global']['cmd-o'] = 'herdr-nvim.pick'
    $keys['global']['cmd-shift-e'] = 'herdr-nvim.reveal'
    Write-Utf8 (Get-KeysFile $privateEnv) (($keys | ConvertTo-Json -Depth 10) + "`n")
    Copy-Item -LiteralPath (Join-Path $Out '01-fresh-install/layout-mine.json') -Destination (Get-LayoutFile $privateEnv) -Force
    $wrapper
}

function Step-Migration {
    Write-Transcript "`n## 5. Migration from the $(if ($IsWindows) { "Git Bash era's .cmd" } else { "bash installer's sh" }) install"
    $legacy = Initialize-PrivateEnv 'legacy'
    Use-PrivateEnv $legacy
    $oldStamp = '0123456789abcdef'
    $wrapper = Write-LegacyInstall $legacy $oldStamp
    $result = if ($IsWindows) { Invoke-Native 'cmd.exe' @('/d', '/c', $wrapper, 'app-nvim-check') } else { Invoke-Native $wrapper @('app-nvim-check') }
    Assert-That ($result.Code -eq 0) 'the seeded legacy wrapper runs herdr-nvim (the fixture is a working install)'
    Save-Snapshot '05a-legacy-seeded' $legacy
    $seededKeys = Get-Content -Raw -LiteralPath (Get-KeysFile $legacy) | ConvertFrom-Json -AsHashtable
    $since = @(Get-StubCall).Count
    $installed = Invoke-Bootstrap $BundleB
    Assert-That ($installed.Out + $installed.Err -notmatch "not herdr-nvim's") 'the legacy mods are recognized as herdr-nvim''s'
    Assert-Installed $installed 'the install over the legacy one'
    Assert-That ($installed.Out -match 'already uses herdr-nvim') 'the layout the legacy install patched is left as it is'
    $record = Assert-Record $legacy 'the migration'
    Assert-ModsChecked $since
    Assert-GenLayout $legacy
    Assert-Launcher $legacy
    Assert-KeysMerged $legacy $seededKeys
    Assert-That (-not (Test-Path -LiteralPath $wrapper) -or -not ([IO.File]::ReadAllText($wrapper).Contains($MarkLine))) 'the old wrapper is gone'
    Assert-That (-not (Test-Path -LiteralPath (Join-Path (Get-Store $legacy) $oldStamp))) "the legacy runtime $oldStamp is pruned"
    Assert-That ((Split-Path -Leaf $record.runtime) -eq $StampB) 'install.json points at B'
    $result = Invoke-Launcher (Get-Launcher $legacy) @('app-nvim-check')
    Assert-That ($result.Code -eq 0) 'the launcher runs herdr-nvim after the migration'
    Save-Snapshot '05b-migrated' $legacy

    Write-Transcript "`n### 5b. A foreign mods/herdr-nvim is refused, nothing changed"
    $foreign = Initialize-PrivateEnv 'foreign'
    Use-PrivateEnv $foreign
    $theirs = Get-ModDir $foreign 'herdr-nvim'
    Write-Utf8 (Join-Path $theirs 'mod.json') "{`"name`": `"someone else's`"}`n"
    Write-Utf8 (Join-Path $theirs 'main.js') "export default {};`n"
    Write-Utf8 (Join-Path $theirs 'bin/herdr-nvim') "#!/bin/sh`nexec true`n"
    $before = @(Get-TreeListing $foreign.HERDR_APP_HOME) + @(Get-TreeListing $foreign.XDG_DATA_HOME)
    Save-Snapshot '05c-foreign-before' $foreign
    $result = Invoke-Bootstrap $BundleB
    Assert-That ($result.Code -ne 0) 'the install over a foreign mod fails'
    Assert-That ($result.Out + $result.Err -match "is not herdr-nvim's") 'it says the folder is not herdr-nvim''s'
    $after = @(Get-TreeListing $foreign.HERDR_APP_HOME) + @(Get-TreeListing $foreign.XDG_DATA_HOME)
    Assert-That (($before -join "`n") -ceq ($after -join "`n")) 'the app home and the data folder are unchanged' (Compare-Object $before $after | Out-String)
    Save-Snapshot '05d-foreign-after' $foreign
    Use-PrivateEnv $Main
}

function Step-CrashRecovery {
    Write-Transcript "`n## 7. Recovery from what a crashed install leaves"
    $mods = Join-Path $Main.HERDR_APP_HOME 'mods'
    Move-Item -LiteralPath (Join-Path $mods 'herdr-nvim') -Destination (Join-Path $mods '.herdr-nvim.old')
    Write-Utf8 (Join-Path $mods '.herdr-nvim.new/herdr-nvim/half.js') "// half staged`n"
    Write-Utf8 (Join-Path $mods '.herdr-nvim-header.new/herdr-nvim-header/main.js') "export * from `"./missing.js`";`n"
    $store = Get-Store $Main
    Write-Utf8 (Join-Path $store ".new.e2e-1/bin/herdr-nvim$Exe") 'half copied'
    Write-Utf8 (Join-Path $store '.trash-e2e-2/lua/herdr-nvim/init.lua') '-- half deleted'
    Save-Snapshot '07a-crashed' $Main
    $since = @(Get-StubCall).Count
    $result = Invoke-Bootstrap $BundleB
    Assert-Installed $result 'the install after a crash'
    $leftovers = @(Get-EntryName $mods | Where-Object { $_ -match '^\.herdr-nvim(-header)?\.(new|old)' })
    Assert-That ($leftovers.Count -eq 0) 'no .new or .old mod folder is left' ($leftovers -join ',')
    $record = Assert-Record $Main 'the recovery'
    Assert-That ((Split-Path -Leaf $record.runtime) -eq $StampB) 'the restored mod points at B'
    Assert-GenLayout $Main
    Assert-ModsChecked $since
    $names = @(Get-EntryName $store)
    Assert-That (($names -join ',') -eq $StampB) 'the runtime store holds B alone: .new.* and .trash-* swept' ($names -join ',')
    $result = Invoke-Launcher (Get-Launcher $Main) @('app-nvim-check')
    Assert-That ($result.Code -eq 0) 'the launcher works after the recovery'
    Save-Snapshot '07b-recovered' $Main
}

function Assert-Uninstalled([string]$what) {
    foreach ($mod in 'herdr-nvim', 'herdr-nvim-header') {
        Assert-That (-not (Test-Path -LiteralPath (Get-ModDir $Main $mod))) "$what removes mods/$mod"
    }
    Assert-That (Test-SameContent (Get-ByteContent (Get-KeysFile $Main)) $PreKeys) "$what leaves keys.json byte for byte as before the install"
    Assert-That (Test-SameContent (Get-ByteContent (Get-LayoutFile $Main)) $PreLayout) "$what leaves the layout byte for byte as before the install"
}

function Step-ConcurrentInstall {
    Write-Transcript "`n## 8. Two installs at once (A and B), three times"
    $keys = Get-ByteContent (Get-KeysFile $Main)
    $layout = Get-ByteContent (Get-LayoutFile $Main)
    foreach ($round in 1..3) { Invoke-ConcurrentRound $round $keys $layout }
    $result = Invoke-Bootstrap $BundleB
    Assert-Installed $result 'B once more, alone'
    $names = @(Get-EntryName (Get-Store $Main))
    Assert-That (($names -join ',') -eq $StampB) 'the runtime store holds B alone' ($names -join ',')
    Save-Snapshot '08b-converged' $Main
}

function Invoke-ConcurrentRound([int]$round, $keys, $layout) {
    Write-Transcript "`n### round $round"
    $first = Open-Bootstrap $BundleA
    $second = Open-Bootstrap $BundleB
    $results = @((Wait-Native $first), (Wait-Native $second))
    $done = @($results | Where-Object { $_.Code -eq 0 -and $_.Out -match 'herdr-nvim: installed in' })
    Assert-That ($done.Count -ge 1) 'at least one of the two installs finishes' (($results | ForEach-Object { "exit $($_.Code)" }) -join ', ')
    foreach ($result in $results | Where-Object { $_.Code -ne 0 }) {
        $text = $result.Out + $result.Err
        Assert-That ($text -match 'herdr-nvim: ' -and $text -notmatch 'panicked') 'the other one refuses with a message of its own, no panic'
    }
    $record = Assert-Record $Main 'the two installs'
    Assert-That ((Split-Path -Leaf $record.runtime) -in $StampA, $StampB) 'install.json points at A or B' $record.runtime
    Assert-GenLayout $Main
    $result = Invoke-Launcher (Get-Launcher $Main) @('app-nvim-check')
    Assert-That ($result.Code -eq 0) 'the launcher works after both: nothing it needs was deleted'
    Assert-That (Test-SameContent (Get-ByteContent (Get-KeysFile $Main)) $keys) 'keys.json is unchanged by the two installs'
    Assert-That (Test-SameContent (Get-ByteContent (Get-LayoutFile $Main)) $layout) 'the layout is unchanged by the two installs'
    Save-Snapshot "08a-concurrent-$round" $Main
}

# Windows: a file of mods/herdr-nvim held open (the app reading it) while
# the uninstall runs. It must say what it could not remove and still take
# out everything else.
function Step-UninstallHeld {
    if (-not $IsWindows) {
        Write-Transcript '- SKIP: a held mod folder during uninstall (Windows only: an open file does not block a rename elsewhere)'
        return
    }
    $mod = Get-ModDir $Main 'herdr-nvim'
    $held = [IO.File]::Open((Join-Path $mod 'mod.json'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try { $result = Invoke-Bootstrap $BundleB -Uninstall }
    finally { $held.Dispose() }
    Assert-That (($result.Out + $result.Err) -match [regex]::Escape($mod)) 'the uninstall names the held mods\herdr-nvim'
    Assert-That ($result.Out + $result.Err -notmatch 'panicked') 'no panic'
    Assert-That (-not (Test-Path -LiteralPath (Get-ModDir $Main 'herdr-nvim-header'))) 'the header mod is removed all the same'
    Assert-That (Test-SameContent (Get-ByteContent (Get-KeysFile $Main)) $PreKeys) 'keys.json is back to before the install all the same'
    Assert-That (Test-SameContent (Get-ByteContent (Get-LayoutFile $Main)) $PreLayout) 'the layout is back to before the install all the same'
    Save-Snapshot '06h-uninstall-held' $Main
    # The rest of step 6 starts from a whole install again (that uninstall
    # also pruned runtime B: nothing ran it).
    Assert-Installed (Invoke-Bootstrap $BundleB) 'a reinstall over what the held uninstall left'
    Assert-GenLayout $Main
}

function Step-Uninstall {
    Write-Transcript "`n## 6. Uninstall through the bootstrap"
    Step-UninstallHeld
    $daemon = Open-FakeDaemon $Main $StampB -Dirty
    $result = Invoke-Bootstrap $BundleB -Uninstall
    Assert-That ($result.Code -eq 0) 'the uninstall exits 0' "exit $($result.Code)"
    Assert-That ($result.Out + $result.Err -match 'kept the nvims above') 'it says it kept the nvim with unsaved work'
    Assert-That (-not $daemon.Process.HasExited) 'the nvim with unsaved work still runs'
    Assert-That (Test-Path -LiteralPath (Join-Path (Get-Store $Main) $StampB)) 'its runtime B is kept'
    Assert-Uninstalled 'the uninstall without --force'
    Save-Snapshot '06a-uninstalled-dirty-kept' $Main
    $result = Invoke-Bootstrap $BundleB -Uninstall -Force
    Assert-That ($result.Code -eq 0) 'the uninstall with --force exits 0' "exit $($result.Code)"
    Assert-That ($daemon.Process.WaitForExit(10000)) 'the nvim with unsaved work is stopped with --force'
    $null = $Holders.Remove($daemon.Process)
    Assert-That (-not (Test-Path -LiteralPath $daemon.Entry)) 'its registry entry is gone'
    Assert-Uninstalled 'the uninstall'
    Assert-That (-not (Test-Path -LiteralPath (Get-Store $Main))) 'no runtime is left (the store is gone)'
    if ($NvimDownload) {
        Assert-That ($result.Out -match 'kept the Neovim it downloaded') 'it keeps the Neovim it downloaded, and says where'
    }
    Save-Snapshot '06b-uninstalled' $Main
}

$exitCode = 0
try {
    Get-ChildItem env: | Where-Object { $_.Name -match '^(HERDR_|HC_)' } | ForEach-Object { Remove-Item -LiteralPath "env:$($_.Name)" }
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    Write-Transcript "# install-e2e on $([Runtime.InteropServices.RuntimeInformation]::OSDescription), PowerShell $($PSVersionTable.PSVersion)"
    Write-Transcript "`nroot: $Root"
    Build-Bundle
    $env:HERDR_NVIM_HC = $StubHc
    $env:HERDR_STUB_LOG = $StubLog
    $Main = Initialize-PrivateEnv 'main'
    Use-PrivateEnv $Main
    Step-StubSelfCheck
    Step-FreshInstall
    Step-ArgumentFidelity
    Step-AppList
    Step-Upgrade
    Step-Migration
    Step-ConcurrentInstall
    Step-CrashRecovery
    Step-Uninstall
    Write-Transcript "`n**install-e2e: all steps passed**"
}
catch [E2EFailure] {
    Write-Transcript "`n**install-e2e: FAILED: $($_.Exception.Message)**"
    $exitCode = 1
}
catch {
    Write-Transcript "`n**install-e2e: ERROR: $_**`n~~~`n$($_.ScriptStackTrace)`n~~~"
    $exitCode = 1
}
finally {
    foreach ($process in @($Holders)) { Close-Holder $process }
    if ($exitCode -ne 0 -and (Get-Variable -Name Main -ErrorAction SilentlyContinue) -and $Main) {
        try { Save-Snapshot 'failed' $Main } catch { Write-Transcript "could not snapshot: $_" }
    }
    Set-Content -LiteralPath (Join-Path $Out 'transcript.md') -Value $Transcript -Encoding utf8NoBOM
    if ($exitCode -eq 0) { Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue }
    else { [Console]::Out.WriteLine("private folders kept for a look: $Root") }
}
exit $exitCode
