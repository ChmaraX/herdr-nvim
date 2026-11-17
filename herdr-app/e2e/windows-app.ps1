# herdr-nvim in the real Herdr desktop app on Windows, for CI (the
# windows-real-app workflow) or a Windows machine that may be changed: it
# installs herdr-core and the app with their official installers into this
# user's profile, starts the app on a private home and core session, runs the
# Windows facts probe against it (doc/windows-facts.md), installs herdr-nvim
# with the one-line install's "with options" form, drives Files, opening a
# file, Preview and the keys with the app's own commands (as run.sh does on
# macOS), and uninstalls.
#
#   pwsh -NoProfile -File herdr-app/e2e/windows-app.ps1 -Out DIR -Bundle FILE -Winprobe EXE -ScriptUrl URL
#
# -Out gets transcript.md, summary.md (one line a step), screenshots (the
# app's own frame `*.png` and the desktop `*-desktop.png`), the probe's record
# (winprobe/), app.log, the core's state and the app home's files. Every
# step runs whatever an earlier non-critical step did; a failed critical step
# skips the rest. It exits 1 if any step failed.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingPositionalParameters', '', Justification = 'internal helpers')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', '', Justification = 'the steps read them from the script scope')]
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [Parameter(Mandatory = $true)][string]$Bundle,
    [Parameter(Mandatory = $true)][string]$Winprobe,
    [Parameter(Mandatory = $true)][string]$ScriptUrl
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
. (Join-Path $PSScriptRoot 'common.ps1')

if (-not $IsWindows) { throw 'windows-app.ps1 is for Windows (run.sh is the macOS run)' }
$Repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
$Bundle = [IO.Path]::GetFullPath($Bundle, (Get-Location).Path)
$Winprobe = [IO.Path]::GetFullPath($Winprobe, (Get-Location).Path)
if (Test-Path -LiteralPath $Out) { Remove-Item -LiteralPath $Out -Recurse -Force }
New-Item -ItemType Directory -Path $Out | Out-Null
# Under the profile, not %TEMP%: a mod reads files only inside the home folder.
$Root = Join-Path $env:USERPROFILE ('.cache\hnv-win-' + [Guid]::NewGuid().ToString('N').Substring(0, 6))
$AppDir = Join-Path $env:LOCALAPPDATA 'Programs\herdr-app'
$CoreDir = Join-Path $env:LOCALAPPDATA 'Programs\herdr-core'
$AppExe = Join-Path $AppDir 'herdr-app.exe'
$Results = [Collections.Generic.List[object]]::new()
$Notes = [Collections.Generic.List[string]]::new()
$script:Stopped = $false
$script:AddedUserPath = $null
$script:DaemonB = $null
$script:App = $null
# The Neovim this script talks to the daemons with. The app and the core
# start without it on PATH; the install step adds its folder to the user
# PATH in the registry, as winget or an installer would, while Herdr runs.
$Nvim = (Get-Command nvim -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$NvimDir = Split-Path -Parent $Nvim

# --- steps ------------------------------------------------------------------

function Add-Note([string]$text) {
    $Notes.Add($text)
    Write-Transcript "- NOTE: $text"
}

function Invoke-Step([string]$name, [string]$about, [scriptblock]$body, [switch]$Critical) {
    if ($script:Stopped) {
        $Results.Add([pscustomobject]@{ Step = $name; Result = 'SKIP'; Detail = 'a critical step before it failed' })
        return
    }
    Write-Transcript "`n## $name`: $about"
    $Notes.Clear()
    $result = 'PASS'
    $detail = ''
    try { & $body }
    catch [E2EFailure] { $result = 'FAIL'; $detail = $_.Exception.Message }
    catch { $result = 'ERROR'; $detail = "$_"; Write-Transcript "~~~`n$($_.ScriptStackTrace)`n~~~" }
    if ($result -ne 'PASS') {
        Write-Transcript "- $result`: $detail"
        try { Save-Shot "FAIL-$name" } catch { Write-Transcript "no failure screenshot: $_" }
        try { Get-AppDoc | ConvertTo-Json -Depth 40 | Set-Content -LiteralPath (Join-Path $Out "FAIL-$name-doc.json") -Encoding utf8NoBOM } catch { Write-Transcript "no app document: $_" }
        if ($Critical) { $script:Stopped = $true }
    }
    $Results.Add([pscustomobject]@{ Step = $name; Result = $result; Detail = (@($detail) + $Notes | Where-Object { $_ }) -join '; ' })
}

# --- the app and the core ----------------------------------------------------

function Invoke-App([string[]]$arguments, [int]$TimeoutSec = 30, [switch]$Quiet) {
    Invoke-Native $AppExe $arguments -TimeoutSec $TimeoutSec -Quiet:$Quiet
}

# `herdr-app do <command> [<json>]`, its JSON answer (fails on an error).
function Invoke-AppCommand([string]$command, [hashtable]$arguments = @{}) {
    $argv = @('do', $command)
    if ($arguments.Count) { $argv += ($arguments | ConvertTo-Json -Compress -Depth 10) }
    $result = Invoke-App $argv
    Assert-That ($result.Code -eq 0) "herdr-app do $command" "exit $($result.Code): $($result.Out)$($result.Err)"
    if ($result.Out.Trim()) { $result.Out | ConvertFrom-Json } else { $null }
}

function Get-AppStatus { $r = Invoke-App @('status') -Quiet; if ($r.Code -eq 0) { $r.Out | ConvertFrom-Json } else { $null } }
function Get-AppList { (Invoke-App @('ls') -Quiet).Out | ConvertFrom-Json }

function Invoke-Hc([string[]]$arguments, [int]$TimeoutSec = 30, [switch]$Quiet) {
    Invoke-Native (Join-Path $CoreDir 'hc.exe') $arguments -TimeoutSec $TimeoutSec -Quiet:$Quiet
}

# A frame drawn now: the mods act when they draw. A window that is shown
# draws by itself; this also covers one that isn't.
function Request-Frame { $null = Invoke-App @('shot', (Join-Path $Root '.frame.png')) -TimeoutSec 10 -Quiet }

function Wait-Until([int]$seconds, [scriptblock]$test) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ($true) {
        $ok = $false
        try { $ok = [bool](& $test) } catch { $ok = $false }
        if ($ok) { return $true }
        if ((Get-Date) -gt $deadline) { return $false }
        Request-Frame
        Start-Sleep -Milliseconds 300
    }
}

# The app's frame (`herdr-app shot`) and the whole desktop, as a person sees it.
function Save-Shot([string]$name) {
    $frame = Join-Path $Out "$name.png"
    $null = Invoke-App @('shot', $frame) -TimeoutSec 20 -Quiet
    $desktop = Join-Path $Out "$name-desktop.png"
    $grab = @"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
`$b = [Windows.Forms.SystemInformation]::VirtualScreen
`$bmp = New-Object Drawing.Bitmap `$b.Width, `$b.Height
`$g = [Drawing.Graphics]::FromImage(`$bmp)
`$g.CopyFromScreen(`$b.Left, `$b.Top, 0, 0, `$bmp.Size)
`$bmp.Save($(Format-PsLiteral $desktop))
"@
    $r = Invoke-Native 'powershell.exe' @('-NoProfile', '-Command', $grab) @{ PSModulePath = $null } -TimeoutSec 30 -Quiet
    $saved = @($frame, $desktop | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object { Split-Path -Leaf $_ })
    Write-Transcript "screenshot $name`: $(if ($saved) { $saved -join ', ' } else { 'none' })$(if ($r.Code -ne 0) { " (desktop: $($r.Err.Trim()))" })"
}

function Open-App {
    $script:App = Start-Process -FilePath $AppExe -PassThru -RedirectStandardOutput (Join-Path $Root 'app.out.log') -RedirectStandardError (Join-Path $Root 'app.err.log')
    Assert-That (Wait-Until 60 { $null -ne (Get-AppStatus) }) "the app (pid $($script:App.Id)) answers herdr-app status"
}

function Close-App {
    if ($script:App -and -not $script:App.HasExited) {
        $null = Invoke-App @('do', 'app.quit') -TimeoutSec 10 -Quiet
        if (-not $script:App.WaitForExit(10000)) { $script:App.Kill($true); $null = $script:App.WaitForExit(10000) }
    }
}

# --- herdr-nvim's state --------------------------------------------------------

function Get-Launcher { Join-Path $env:HERDR_APP_HOME 'mods\herdr-nvim\bin\herdr-nvim.exe' }

# The tab's live herdr-nvim terminals (`hc` labels them by tab id), and the
# one a pane shows.
function Get-SidebarTerminal([string]$tab) {
    $r = Invoke-Hc @('ls', '--json', '--label', "herdr-nvim.sidebar=$tab") -Quiet
    $live = @(($r.Out | ConvertFrom-Json).terminals | Where-Object { $_.state -eq 'live' } | ForEach-Object { $_.address })
    $shown = @((Get-AppList).spaces | ForEach-Object { $_.tabs } | ForEach-Object { $_.terminals } | Where-Object { $_.terminal -in $live -and $_.pane })
    if ($shown) { $shown[0].terminal } else { $null }
}

# The tab's daemon row (`herdr-nvim daemons`: its tab id is `<space>:<tab>`).
function Get-Daemon([string]$tab) {
    $r = Invoke-Native (Get-Launcher) @('daemons', '--json') -TimeoutSec 30 -Quiet
    if ($r.Code -ne 0) { return $null }
    @($r.Out | ConvertFrom-Json) | Where-Object { $_.tab_id -eq "$($script:Space):$tab" -and $_.pid } | Select-Object -First 1
}

function Invoke-NvimExpr($daemon, [string]$expr) {
    $r = Invoke-Native $Nvim @('--clean', '--headless', '--server', "\\.\pipe\herdr-nvim-$($daemon.key)", '--remote-expr', $expr) -TimeoutSec 10 -Quiet
    if ($r.Code -eq 0) { $r.Out.Trim() } else { $null }
}

# The side panel: hidden, or the stack child it shows (the layout file and
# its saved clicks, as probe.py reads them on macOS).
function Get-Panel {
    $name = (Get-AppStatus).layout
    $layouts = Join-Path $env:HERDR_APP_HOME 'layouts'
    $layout = Get-Content -Raw -LiteralPath (Join-Path $layouts "$name.json") | ConvertFrom-Json
    $state = Join-Path $layouts ".state\$name.json"
    $clicks = if (Test-Path -LiteralPath $state) { (Get-Content -Raw -LiteralPath $state | ConvertFrom-Json).nodes } else { $null }
    $panel = Find-Node $layout 'panel'
    $body = Find-Node $layout 'panel-body'
    $hidden = $panel -and $panel.PSObject.Properties['hidden'] -and $panel.hidden
    if ($clicks -and $clicks.PSObject.Properties['panel'] -and $clicks.panel.PSObject.Properties['hidden']) { $hidden = $clicks.panel.hidden }
    $active = if ($body -and $body.PSObject.Properties['active']) { $body.active } else { $null }
    if ($clicks -and $clicks.PSObject.Properties['panel-body'] -and $clicks.'panel-body'.PSObject.Properties['active']) { $active = $clicks.'panel-body'.active }
    if ($hidden) { 'hidden' } else { $active }
}

function Find-Node($node, [string]$id) {
    if ($null -eq $node -or $node -isnot [pscustomobject]) { return $null }
    if ($node.PSObject.Properties['id'] -and $node.id -eq $id) { return $node }
    if ($node.PSObject.Properties['children']) {
        foreach ($child in $node.children) { $found = Find-Node $child $id; if ($found) { return $found } }
    }
    $null
}

# The app's document (spaces, tabs and their layout trees), through the core.
function Get-AppDoc {
    $hc = Join-Path $CoreDir 'hc.exe'
    $info = [Diagnostics.ProcessStartInfo]::new($hc)
    $info.ArgumentList.Add('connect')
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $process = [Diagnostics.Process]::Start($info)
    $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"hello","params":{}}')
    $process.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"doc.get","params":{"name":"herdr-app"}}')
    $process.StandardInput.Flush()
    # Read until the answer to 2 arrives; only then end the connection (at
    # the end of its input hc connect may stop before it answers).
    $deadline = (Get-Date).AddSeconds(10)
    $doc = $null
    try {
        while ($null -eq $doc -and (Get-Date) -lt $deadline) {
            $line = $process.StandardOutput.ReadLineAsync()
            if (-not $line.Wait(10000) -or $null -eq $line.Result) { break }
            if (-not $line.Result.Trim()) { continue }
            $message = $line.Result | ConvertFrom-Json
            if ($message.PSObject.Properties['id'] -and $message.id -eq 2) { $doc = $message.result.value }
        }
    }
    finally {
        $process.StandardInput.Close()
        if (-not $process.WaitForExit(5000)) { $process.Kill($true) }
    }
    if ($null -eq $doc) { throw 'hc connect gave no app document' }
    $doc
}

# The file a preview in the tab shows.
function Get-Preview([string]$tab) {
    $node = @((Get-AppDoc).spaces | ForEach-Object { $_.tabs } | Where-Object { $_.id -eq $tab })[0].root
    Find-Preview $node
}

function Find-Preview($node) {
    if ($null -eq $node) { return $null }
    $content = if ($node.PSObject.Properties['content']) { $node.content } else { $null }
    if ($content -and $content.PSObject.Properties['mod'] -and $content.mod -eq 'herdr-nvim' -and $content.PSObject.Properties['props'] -and $content.props.PSObject.Properties['preview']) {
        return $content.props.preview
    }
    if ($node.PSObject.Properties['children']) {
        foreach ($child in $node.children) { $found = Find-Preview $child; if ($found) { return $found } }
    }
    $null
}

# A folder on the person's PATH in the registry, as an installer adds it
# (setting a user variable tells running programs the environment changed).
function Add-UserPath([string]$dir) {
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    [Environment]::SetEnvironmentVariable('Path', "$($user.TrimEnd(';'));$dir", 'User')
    $script:AddedUserPath = $dir
}

# install.ps1 from $ScriptUrl, the one-line install's "with options" form,
# under Windows PowerShell 5.1.
function Invoke-OneLineInstall([string[]]$options) {
    $line = "& ([scriptblock]::Create((irm $(Format-PsLiteral $ScriptUrl)))) -App -Bundle $(Format-PsLiteral $Bundle) -AppBin $(Format-PsLiteral $AppExe) $($options -join ' ')"
    Invoke-Native 'powershell.exe' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', $line) @{ PSModulePath = $null } -TimeoutSec 600
}

# --- the run -----------------------------------------------------------------

$exitCode = 0
try {
    Write-Transcript "# herdr-nvim in the real Herdr app on $([Runtime.InteropServices.RuntimeInformation]::OSDescription)"
    Write-Transcript "`nroot: $Root"

    Invoke-Step 'install-herdr' 'herdr-core and the app with their official installers' -Critical {
        $r = Invoke-Native 'powershell.exe' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', 'irm https://releases.herdr.dev/app/install.ps1 | iex') @{ PSModulePath = $null } -TimeoutSec 600
        Assert-That ($r.Code -eq 0) "the app's installer exits 0" "exit $($r.Code)"
        Assert-That (Test-Path -LiteralPath $AppExe) "herdr-app.exe is in $AppDir"
        Assert-That (Test-Path -LiteralPath (Join-Path $CoreDir 'hc.exe')) "hc.exe is in $CoreDir"
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User') -split ';'
        Add-Note "user PATH has herdr-app: $($userPath -contains $AppDir), herdr-core: $($userPath -contains $CoreDir)"
        # The app's and the core's folders, and no Neovim (see $Nvim).
        $env:PATH = (@($AppDir, $CoreDir) + @($env:PATH -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $NvimDir.TrimEnd('\') })) -join ';'
        $app = Invoke-App @('--version')
        $core = Invoke-Hc @('--version')
        Add-Note "app: $($app.Out.Trim()) - core: $($core.Out.Trim())"
        Write-Utf8 (Join-Path $Out 'versions.txt') "$($app.Out.Trim())`n$($core.Out.Trim())`n$AppExe`n$(Join-Path $CoreDir 'hc.exe')`n"
    }

    Invoke-Step 'start-app' 'the app on a private home and core session' -Critical {
        foreach ($name in @(Get-ChildItem env: | Where-Object { $_.Name -match '^(HERDR_|HC_)' } | ForEach-Object Name)) { Remove-Item -LiteralPath "env:$name" }
        $vars = [ordered]@{
            HERDR_APP_HOME = Join-Path $Root 'home'; XDG_STATE_HOME = Join-Path $Root 'state'
            XDG_CONFIG_HOME = Join-Path $Root 'config'; XDG_DATA_HOME = Join-Path $Root 'data'
            HERDR_NVIM_RUNTIME_DIR = Join-Path $Root 'run'; HERDR_NVIM_STATE_DIR = Join-Path $Root 'nvstate'
        }
        foreach ($name in $vars.Keys) { New-Item -ItemType Directory -Force -Path $vars[$name] | Out-Null; Set-Item "env:$name" $vars[$name] }
        # Where the spaces start their terminals.
        New-Item -ItemType Directory -Force -Path (Join-Path $Root 'work') | Out-Null
        $env:HC_SESSION = 'hnwin'
        $env:HERDR_APP_NO_SOUND = '1'; $env:HERDR_TELEMETRY = '0'; $env:DO_NOT_TRACK = '1'
        $env:HERDR_RELEASES_URL = 'http://127.0.0.1:9' # never update the installed app or core
        $env:HERDR_NVIM_INSTALL_METHOD = 'download'
        Write-Utf8 (Join-Path $env:HERDR_APP_HOME 'shared.json') "{`"frame.welcomed`": true, `"layout`": `"main`"}`n"
        $version = ((Invoke-App @('--version') -Quiet).Out.Trim() -split '\s+')[-1]
        Write-Utf8 (Join-Path $env:HERDR_APP_HOME 'update-notes.json') "{`"last_run`": `"$version`", `"presented`": []}`n"
        $r = Invoke-Hc @('server', 'start') -TimeoutSec 60
        Assert-That ($r.Code -eq 0) "hc server start (session $env:HC_SESSION)" "exit $($r.Code)"
        Open-App
        $status = Get-AppStatus
        Write-Utf8 (Join-Path $Out 'status-start.json') ($status | ConvertTo-Json -Depth 10)
        Assert-That ($status.errors.Count -eq 0) 'the app reports no errors' ($status.errors -join '; ')
        Start-Sleep -Seconds 3
        $script:App.Refresh()
        Add-Note "window handle $($script:App.MainWindowHandle), title '$($script:App.MainWindowTitle)'"
        Save-Shot '02-app-started'
        Add-Note "app frame: $(Test-Path -LiteralPath (Join-Path $Out '02-app-started.png')); desktop: $(Test-Path -LiteralPath (Join-Path $Out '02-app-started-desktop.png'))"
    }

    Invoke-Step 'winprobe' 'the Windows facts probe against the real app (doc/windows-facts.md)' {
        $home_ = $env:HERDR_APP_HOME
        $mod = Join-Path $home_ 'mods\winprobe'
        New-Item -ItemType Directory -Force -Path (Join-Path $mod 'bin') | Out-Null
        foreach ($f in 'main.js', 'mod.json') { Copy-Item -LiteralPath (Join-Path $Repo "examples\winprobe-mod\$f") -Destination $mod }
        Copy-Item -LiteralPath $Winprobe -Destination (Join-Path $mod 'bin\winprobe.exe')
        $probeOut = Join-Path $Out 'winprobe'
        New-Item -ItemType Directory -Force -Path $probeOut | Out-Null
        if (Test-Path -LiteralPath (Join-Path $home_ 'keys.json')) { Copy-Item -LiteralPath (Join-Path $home_ 'keys.json') -Destination (Join-Path $probeOut 'keys-before-anything.json') }
        $where = Invoke-Native (Join-Path $mod 'bin\winprobe.exe') @('--log', (Join-Path $probeOut 'cli.jsonl'), 'where')
        Assert-That ($where.Code -eq 0) 'winprobe where, from PowerShell'
        $made = Invoke-AppCommand 'space.new' @{ name = 'probe'; folder = (Join-Path $Root 'work'); tab = 'probe' }
        $made2 = Invoke-AppCommand 'tab.new' @{ name = 'probe shell'; space = $made.space; folder = $Root; argv = @('powershell.exe', '-NoLogo', '-NoProfile') }
        $null = Invoke-AppCommand 'tab.close' @{ id = $made.tab; confirmed = $true }
        $null = Invoke-AppCommand 'pane.focus' @{ terminal = $made2.terminal }
        $null = Invoke-AppCommand 'mod.open' @{ name = 'winprobe' }
        Start-Sleep -Seconds 2
        $null = Invoke-AppCommand 'winprobe.run'
        $outDir = Join-Path $home_ 'winprobe-out'
        $results = { Get-ChildItem -LiteralPath $outDir -Directory | Sort-Object Name | Select-Object -Last 1 | ForEach-Object { Join-Path $_.FullName 'results.json' } }
        $done = Wait-Until 300 {
            $file = & $results
            $data = Get-Content -Raw -LiteralPath $file | ConvertFrom-Json
            $steps = @($data.steps.PSObject.Properties)
            $steps.Count -eq 10 -and -not ($steps | Where-Object { $_.Value.status -eq 'running' })
        }
        Save-Shot '03-winprobe'
        Assert-That $done 'the probe ran all 10 steps within 5 minutes'
        $runDir = Split-Path -Parent (& $results)
        $data = Get-Content -Raw -LiteralPath (& $results) | ConvertFrom-Json
        foreach ($p in $data.steps.PSObject.Properties) { Write-Transcript "  $($p.Name): $($p.Value.status)" }
        # Survive: a 5-minute daemon, then the app quits; its ticks say whether
        # it outlived the app that started it.
        $null = Invoke-AppCommand 'winprobe.survive'
        $log = Join-Path $runDir 'log.jsonl'
        $ticks = { @(Get-Content -LiteralPath $log | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.tag -eq 'survive' -and $_.sub -eq 'tick' }) }
        Assert-That (Wait-Until 30 { (& $ticks).Count -gt 0 }) 'the survive daemon ticks'
        $null = Invoke-AppCommand 'overlay.close'
        Close-App
        Assert-That (-not (Get-Process -Name 'herdr-app' -ErrorAction SilentlyContinue)) 'no herdr-app process is left'
        Start-Sleep -Seconds 30
        $without = @(& $ticks | Where-Object { $_.app_running -eq $false }).Count
        Add-Note "survive: $without ticks after the app quit"
        Open-App
        $null = Invoke-AppCommand 'mod.open' @{ name = 'winprobe' }
        Start-Sleep -Seconds 2
        $null = Invoke-AppCommand 'winprobe.read'
        $null = Wait-Until 20 { (Get-Content -Raw -LiteralPath (Join-Path $runDir 'results.json') | ConvertFrom-Json).PSObject.Properties['survive'] }
        Save-Shot '03-winprobe-after-restart'
        $null = Invoke-AppCommand 'overlay.close'
        Copy-Item -LiteralPath $runDir -Destination $probeOut -Recurse
        $null = Invoke-AppCommand 'space.close' @{ id = $made.space; confirmed = $true }
        # The survive daemon ticks for 5 minutes, from the mod's folder.
        Get-Process -Name 'winprobe', 'winprobe-outside' -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Seconds 1
        Remove-Item -LiteralPath $mod -Recurse -Force
    }
    if (-not $script:Stopped -and (-not $script:App -or $script:App.HasExited)) { Open-App }

    $demo = Join-Path $Root 'work\demo'
    Invoke-Step 'install' 'herdr-nvim from this commit: the one-line install, "with options" form' -Critical {
        $home_ = $env:HERDR_APP_HOME
        $script:KeysBefore = Get-ByteContent (Join-Path $home_ 'keys.json')
        # Neovim installed while Herdr runs: on the user PATH in the
        # registry (as winget or an installer puts it), not in the app's or
        # the core's environment, nor in this install's.
        Add-UserPath $NvimDir
        $r = Invoke-OneLineInstall @()
        Assert-That ($r.Code -eq 0 -and $r.Out -match 'herdr-nvim: installed in') 'the install exits 0 and says installed in' "exit $($r.Code)"
        Assert-That (Test-Path -LiteralPath (Get-Launcher)) 'mods\herdr-nvim\bin\herdr-nvim.exe is there'
        # Mark of the Web (row 21): the one-line install downloads with curl.exe.
        $runtimeExe = Get-ChildItem -LiteralPath (Join-Path $env:XDG_DATA_HOME 'herdr-nvim\app') -Recurse -Filter 'herdr-nvim.exe' | Select-Object -First 1
        foreach ($exe in @((Get-Launcher), $runtimeExe.FullName)) {
            $zone = Get-Item -LiteralPath $exe -Stream 'Zone.Identifier' -ErrorAction SilentlyContinue
            Assert-That (-not $zone) "$(Split-Path -Leaf (Split-Path -Parent (Split-Path -Parent $exe)))\bin\herdr-nvim.exe carries no Mark of the Web"
        }
        $check = Invoke-App @('check', (Join-Path $home_ 'mods\herdr-nvim'))
        Assert-That ($check.Code -eq 0) 'herdr-app check passes the installed mod'
        $keys = Get-Content -Raw -LiteralPath (Join-Path $home_ 'keys.json') | ConvertFrom-Json -AsHashtable
        $ours = (Get-Content -Raw -LiteralPath (Join-Path $Repo 'herdr-app\keys.json') | ConvertFrom-Json -AsHashtable)['windows']
        foreach ($section in $ours.Keys) {
            foreach ($key in $ours[$section].Keys) {
                $got = if ($keys.Contains($section)) { $keys[$section][$key] } else { $null }
                Assert-That ($got -eq $ours[$section][$key]) "keys.json binds $section $key to $($ours[$section][$key])" "is $got"
            }
        }
        $status = Get-AppStatus
        Assert-That ($status.errors.Count -eq 0) 'the app reports no errors after the install' ($status.errors -join '; ')
        Add-Note "layout shown: $($status.layout)"
    }

    Invoke-Step 'files' 'Files opens the tree and the tab''s nvim' -Critical {
        New-Item -ItemType Directory -Force -Path (Join-Path $demo 'src') | Out-Null
        Write-Utf8 (Join-Path $demo 'src\greet.js') "export function greet(name) {`n  return ``hello `${name}``;`n}`n"
        Write-Utf8 (Join-Path $demo 'package.json') "{`"name`":`"demo`"}`n"
        $made = Invoke-AppCommand 'space.new' @{ name = 'e2e'; folder = (Join-Path $Root 'work'); tab = 'made' }
        $script:Space = $made.space
        # Its terminal is a stand-in agent (the picker lists the files an
        # agent in the space worked on).
        $agent = @('powershell.exe', '-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot 'dummy-agent.ps1'))
        $tab = Invoke-AppCommand 'tab.new' @{ name = 'tab A'; space = $made.space; folder = $demo; argv = $agent }
        $script:Tab = $tab.tab
        $script:Agent = $tab.terminal
        $null = Invoke-AppCommand 'tab.close' @{ id = $made.tab; confirmed = $true }
        $null = Invoke-AppCommand 'tab.select' @{ id = $script:Tab }
        $null = Invoke-AppCommand 'pane.focus' @{ terminal = $script:Agent }
        # Exactly what the tab bar's Files button runs (mods/tabs/main.js).
        $null = Invoke-AppCommand 'layout.select' @{ stack = 'panel-body'; child = 'files' }
        $null = Invoke-AppCommand 'layout.show' @{ id = 'panel' }
        Assert-That (Wait-Until 15 { (Get-Panel) -eq 'files' }) 'Files shows the side panel with the tree'
        Assert-That (Wait-Until 30 { Get-SidebarTerminal $script:Tab }) 'Files opens the tab''s nvim in a pane'
        Assert-That (Wait-Until 20 { Get-Daemon $script:Tab }) 'the tab''s nvim daemon runs'
        $script:Daemon = Get-Daemon $script:Tab
        Add-Note "daemon pid $($script:Daemon.pid), key $($script:Daemon.key)"
        $cwd = Invoke-NvimExpr $script:Daemon 'getcwd()'
        Assert-That ($cwd -and ([IO.Path]::GetFullPath($cwd) -eq [IO.Path]::GetFullPath($demo))) "the tab's nvim is in the tab's folder" $cwd
        # The first nvim pane brings the welcome card.
        $null = Wait-Until 5 { @((Get-AppStatus).overlays) -contains 'herdr-nvim' }
        Save-Shot '05-files-welcome'
        $null = Invoke-App @('do', 'overlay.close') -Quiet
        Start-Sleep -Seconds 1
        Save-Shot '05-files-tree-and-nvim'
    }

    Invoke-Step 'open-file' 'a file opened from the tree lands in the tab''s nvim' {
        $ctx = @{ space = $script:Space; tab = $script:Tab; terminal = $script:Agent; cwd = $demo } | ConvertTo-Json -Compress
        # What a click on a file in the tree runs (tree-view.js).
        $r = Invoke-Native (Get-Launcher) @('open', '--no-focus', (Join-Path $demo 'src\greet.js'), '--ctx', $ctx)
        Assert-That ($r.Code -eq 0) 'herdr-nvim open exits 0'
        Assert-That (Wait-Until 15 { (Invoke-NvimExpr $script:Daemon 'expand("%:t")') -eq 'greet.js' }) 'the tab''s nvim shows greet.js'
        # Other mods start it extensionless, as a Rust caller would (row 3).
        $r = Invoke-Native $Winprobe @('--log', (Join-Path $Out 'winprobe\contract.jsonl'), 'spawn', ((Get-Launcher) -replace '\.exe$', ''), 'open', '--no-focus', "$(Join-Path $demo 'package.json'):1", '--ctx', $ctx)
        Assert-That ($r.Code -eq 0) 'winprobe spawn of the extensionless launcher path exits 0'
        Assert-That (Wait-Until 15 { (Invoke-NvimExpr $script:Daemon 'expand("%:t")') -eq 'package.json' }) 'the extensionless path opens package.json in the tab''s nvim'
        Save-Shot '06-file-in-nvim'
    }

    Invoke-Step 'picker' 'alt-o''s picker (the core''s terminal.run with env) opens the picked file in nvim' {
        # What alt-o runs (keys.json).
        $null = Invoke-AppCommand 'herdr-nvim.pick'
        $find = {
            foreach ($t in @(((Invoke-Hc @('ls', '--json') -Quiet).Out | ConvertFrom-Json).terminals)) {
                if ((Invoke-Hc @('get', $t.address, '--json') -Quiet).Out -match '"picker"') { return $t.address }
            }
        }
        Assert-That (Wait-Until 20 { & $find }) 'the picker opens'
        $picker = & $find
        $listed = Wait-Until 20 { (Invoke-Hc @('read', $picker) -Quiet).Out -match 'greet\.js' }
        $screen = (Invoke-Hc @('read', $picker) -Quiet).Out
        Write-Utf8 (Join-Path $Out 'picker-screen.txt') $screen
        Save-Shot '06a-picker'
        Assert-That $listed 'the picker lists src\greet.js (it got HERDR_NVIM_HANDOFF through terminal.run''s env)' $(($screen -split "`n" | Select-Object -First 5) -join ' / ')
        $null = Invoke-Hc @('keys', $picker, 'g', 'r', 'e', 'e', 't')
        Start-Sleep -Milliseconds 500
        $null = Invoke-Hc @('keys', $picker, 'enter')
        Assert-That (Wait-Until 15 { (Invoke-NvimExpr $script:Daemon 'expand("%:t")') -eq 'greet.js' }) 'the pick opens greet.js in the tab''s nvim'
    }

    Invoke-Step 'open-cold' 'a file opened in a tab with no nvim yet: the exec returns while the daemon it started runs' {
        $tab = Invoke-AppCommand 'tab.new' @{ name = 'tab B'; space = $script:Space; folder = $demo; argv = @('powershell.exe', '-NoLogo', '-NoProfile') }
        $script:TabB = $tab.tab
        $ctx = @{ space = $script:Space; tab = $tab.tab; terminal = $tab.terminal; cwd = $demo } | ConvertTo-Json -Compress
        # As the app runs it: stdout and stderr are pipes it reads to their end.
        $before = @(Get-CimInstance Win32_Process | ForEach-Object ProcessId)
        $started = Get-Date
        $r = Invoke-Native (Get-Launcher) @('open', '--no-focus', (Join-Path $demo 'src\greet.js'), '--ctx', $ctx) -TimeoutSec 60
        $took = [int]((Get-Date) - $started).TotalSeconds
        # What it started and what still runs (with when each began).
        Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -notin $before } |
            ForEach-Object { "$($_.CreationDate.ToString('HH:mm:ss.fff')) pid $($_.ProcessId) parent $($_.ParentProcessId) $($_.Name): $($_.CommandLine)" } |
            Set-Content -LiteralPath (Join-Path $Out 'open-cold-processes.txt') -Encoding utf8NoBOM
        Add-Note "open took $took s (started $($started.ToString('HH:mm:ss.fff')))"
        Assert-That ($r.Code -eq 0 -and $took -le 10) "herdr-nvim open returns, exit 0, in $took s (at most 10: it must not wait for the daemon it started)" "exit $($r.Code)"
        Assert-That (Wait-Until 20 { Get-Daemon $tab.tab }) 'tab B''s daemon runs'
        $script:DaemonB = Get-Daemon $tab.tab
        Assert-That (Wait-Until 15 { (Invoke-NvimExpr $script:DaemonB 'expand("%:t")') -eq 'greet.js' }) 'tab B''s nvim shows greet.js'
        Save-Shot '06b-open-cold'
        $null = Invoke-AppCommand 'tab.select' @{ id = $script:Tab }
    }

    Invoke-Step 'preview' 'Preview takes nvim''s place, Source brings nvim back' {
        $md = Join-Path $demo 'preview.md'
        $fence = '```'
        Write-Utf8 $md "# Preview`n`nRendered in place of nvim.`n`n$($fence)mermaid`nflowchart TD`n  A[Windows] --> B{Preview}`n$fence`n"
        $ctx = @{ space = $script:Space; tab = $script:Tab; terminal = $script:Agent; cwd = $demo } | ConvertTo-Json -Compress
        $null = Invoke-Native (Get-Launcher) @('open', '--no-focus', $md, '--ctx', $ctx)
        Assert-That (Wait-Until 15 { (Invoke-NvimExpr $script:Daemon 'expand("%:t")') -eq 'preview.md' }) 'nvim shows preview.md'
        # What the pane header's Preview button runs (mods/herdr-nvim-header).
        $null = Invoke-AppCommand "herdr-nvim.swap:$($script:Tab):$([Uri]::EscapeDataString($md))"
        Assert-That (Wait-Until 15 { Get-Preview $script:Tab }) 'Preview shows the preview in the tab'
        Assert-That (Wait-Until 10 { -not (Get-SidebarTerminal $script:Tab) }) 'nvim''s pane gives the preview its place'
        # The diagram, drawn by herdr-nvim into the preview's cache.
        $cache = Join-Path $env:HERDR_APP_HOME 'mods\herdr-nvim\.cache'
        Assert-That (Wait-Until 30 { Get-ChildItem -LiteralPath $cache -Filter 'mermaid-*.png' -ErrorAction SilentlyContinue }) 'the mermaid diagram is drawn into the preview''s cache'
        Start-Sleep -Seconds 3
        Save-Shot '07-preview'
        $null = Invoke-AppCommand "herdr-nvim.swap:$($script:Tab):"
        Assert-That (Wait-Until 15 { Get-SidebarTerminal $script:Tab }) 'Source brings nvim back'
        Assert-That (Wait-Until 10 { -not (Get-Preview $script:Tab) }) 'and the preview goes'
        Assert-That ((Get-Daemon $script:Tab).pid -eq $script:Daemon.pid) 'the same daemon'
    }

    Invoke-Step 'keys' 'alt-e (Files on Windows), pressed, opens the tree and nvim' {
        # Files off: the panel and nvim's pane close together.
        $null = Invoke-AppCommand 'layout.hide' @{ id = 'panel' }
        Assert-That (Wait-Until 15 { (Get-Panel) -eq 'hidden' }) 'Files off hides the panel'
        Assert-That (Wait-Until 15 { -not (Get-SidebarTerminal $script:Tab) }) 'and closes nvim''s pane'
        $null = Invoke-AppCommand 'pane.focus' @{ terminal = $script:Agent }
        $press = @"
`$shell = New-Object -ComObject WScript.Shell
if (-not `$shell.AppActivate($($script:App.Id))) { [Console]::Error.WriteLine('AppActivate failed'); exit 3 }
Start-Sleep -Milliseconds 500
Add-Type -AssemblyName System.Windows.Forms
[Windows.Forms.SendKeys]::SendWait('%e')
"@
        $r = Invoke-Native 'powershell.exe' @('-NoProfile', '-Command', $press) @{ PSModulePath = $null } -TimeoutSec 30
        Add-Note "key press sent: exit $($r.Code)"
        Assert-That (Wait-Until 15 { (Get-Panel) -eq 'files' -and (Get-SidebarTerminal $script:Tab) }) 'alt-e, pressed, opens the tree and nvim'
        Save-Shot '08-alt-e'
    }

    Invoke-Step 'uninstall' 'closing the tab ends its daemon; uninstall leaves nothing' {
        $daemonPid = [int]$script:Daemon.pid
        if ($script:DaemonB) {
            $pidB = [int]$script:DaemonB.pid
            $null = Invoke-AppCommand 'tab.close' @{ id = $script:TabB; confirmed = $true }
            Assert-That (Wait-Until 20 { -not (Get-Process -Id $pidB -ErrorAction SilentlyContinue) }) "closing tab B ends its daemon ($pidB)"
            Assert-That ([bool](Get-Process -Id $daemonPid -ErrorAction SilentlyContinue)) "tab A's daemon keeps running"
        }
        $null = Invoke-AppCommand 'space.close' @{ id = $script:Space; confirmed = $true }
        Assert-That (Wait-Until 20 { -not (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue) }) "closing the space ends the tab's daemon ($daemonPid)"
        $r = Invoke-OneLineInstall @('-Uninstall')
        Assert-That ($r.Code -eq 0) 'the uninstall exits 0' "exit $($r.Code)"
        $home_ = $env:HERDR_APP_HOME
        $left = @(Get-ChildItem -LiteralPath (Join-Path $home_ 'mods') -Force -ErrorAction SilentlyContinue | ForEach-Object Name | Where-Object { $_ -like '*herdr-nvim*' -and $_ -ne '.herdr-nvim.lock' })
        Assert-That ($left.Count -eq 0) 'no herdr-nvim mod is left' ($left -join ', ')
        Assert-That (Test-SameContent (Get-ByteContent (Join-Path $home_ 'keys.json')) $script:KeysBefore) 'keys.json is byte for byte as before the install'
        $layouts = @(Get-ChildItem -LiteralPath (Join-Path $home_ 'layouts') -Filter '*.json')
        $mentions = @($layouts | Where-Object { (Get-Content -Raw -LiteralPath $_.FullName) -match 'herdr-nvim' } | ForEach-Object Name)
        Assert-That ($mentions.Count -eq 0) 'no layout mentions herdr-nvim' ($mentions -join ', ')
        Assert-That (-not (Test-Path -LiteralPath (Join-Path $env:XDG_DATA_HOME 'herdr-nvim\app'))) 'the runtime store is gone'
        $status = Get-AppStatus
        Assert-That ($status.errors.Count -eq 0) 'the app reports no errors after the uninstall' ($status.errors -join '; ')
        Save-Shot '09-uninstalled'
    }
}
catch {
    Write-Transcript "`n**ERROR outside a step: $_**`n~~~`n$($_.ScriptStackTrace)`n~~~"
    $exitCode = 1
}
finally {
    # Logs first, whatever happened; then everything this run started ends.
    try {
        foreach ($f in 'app.out.log', 'app.err.log') { if (Test-Path -LiteralPath (Join-Path $Root $f)) { Copy-Item -LiteralPath (Join-Path $Root $f) -Destination $Out } }
        foreach ($d in 'state', 'run') { if (Test-Path -LiteralPath (Join-Path $Root $d)) { Copy-Item -LiteralPath (Join-Path $Root $d) -Destination (Join-Path $Out $d) -Recurse -ErrorAction SilentlyContinue } }
        $homeCopy = Join-Path $Out 'app-home'
        if (Test-Path -LiteralPath (Join-Path $Root 'home')) {
            Get-ChildItem -LiteralPath (Join-Path $Root 'home') -Recurse -File -Include '*.json', '*.log', '*.toml' -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -notmatch '\\(gen|node_modules)\\' } | ForEach-Object {
                    $rel = [IO.Path]::GetRelativePath((Join-Path $Root 'home'), $_.FullName)
                    $to = Join-Path $homeCopy $rel
                    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $to) | Out-Null
                    Copy-Item -LiteralPath $_.FullName -Destination $to
                }
        }
        Get-ChildItem -LiteralPath $env:LOCALAPPDATA -Directory -Filter 'herdr*' -ErrorAction SilentlyContinue | ForEach-Object {
            Get-ChildItem -LiteralPath $_.FullName -Recurse -File -Filter '*.log' -ErrorAction SilentlyContinue |
                ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $Out "localappdata-$($_.Name)") }
        }
    }
    catch { Write-Transcript "collecting logs: $_" }
    try {
        Close-App
        $hc = Join-Path $CoreDir 'hc.exe'
        if (Test-Path -LiteralPath $hc) {
            $list = Invoke-Native $hc @('ls', '--json') -TimeoutSec 20 -Quiet
            if ($list.Code -eq 0) { foreach ($t in @(($list.Out | ConvertFrom-Json).terminals)) { $null = Invoke-Native $hc @('kill', $t.address) -TimeoutSec 20 -Quiet } }
            $null = Invoke-Native $hc @('server', 'stop') -TimeoutSec 30 -Quiet
        }
        Get-Process -Name 'nvim', 'herdr-nvim', 'winprobe' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    catch { Write-Transcript "cleanup: $_" }
    if ($script:AddedUserPath) {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ -ne $script:AddedUserPath }
        [Environment]::SetEnvironmentVariable('Path', ($user -join ';'), 'User')
    }
    $failed = @($Results | Where-Object { $_.Result -in 'FAIL', 'ERROR' })
    if ($failed) { $exitCode = 1 }
    $summary = @('| Step | Result | Detail |', '|---|---|---|') + @($Results | ForEach-Object { "| $($_.Step) | $($_.Result) | $(($_.Detail -replace '\|', '/') -replace "`r?`n", ' ') |" })
    Set-Content -LiteralPath (Join-Path $Out 'summary.md') -Value $summary -Encoding utf8NoBOM
    Set-Content -LiteralPath (Join-Path $Out 'transcript.md') -Value $Transcript -Encoding utf8NoBOM
    $summary | ForEach-Object { [Console]::Out.WriteLine($_) }
}
exit $exitCode
