# herdr-nvim's installer on Windows, for the Herdr desktop app, the herdr TUI,
# or both. Windows PowerShell 5.1 or PowerShell 7; no Git Bash, no admin.
#
#   irm https://raw.githubusercontent.com/ChmaraX/herdr-nvim/main/install.ps1 | iex
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/ChmaraX/herdr-nvim/main/install.ps1))) -App -InstallNvim
#
#   install.ps1 [-App] [-Tui] [-Uninstall] [-Force] [-InstallNvim] [-Version vX.Y.Z] [-Bundle FILE]
#               [-AppHome DIR] [-AppBin PATH] [-PanelWidth PX] [-Tree native|nvim] [-- more app options]
#
# With neither -App nor -Tui: whichever Herdr is installed (both if both).
# Run it again to upgrade.
#
# App: downloads the release's app bundle (herdr-nvim, its Lua, the two mods),
# checks it against the release's SHA-256, unpacks it with Windows' own tar,
# and runs its `herdr-nvim app-install` with the app open. Without Neovim
# 0.10+ it asks to install Neovim's own release (no admin); -InstallNvim says
# yes up front (no one to ask). -Uninstall takes it all out (-Force: also
# nvims with unsaved work) and keeps the Neovim it downloaded.
#
# TUI: `herdr plugin install ChmaraX/herdr-nvim` (or `herdr plugin update`).

# Everything runs in this one script block's scope: under `irm | iex` the
# script runs in the caller's session, whose variables and settings it must
# not change, and which it must not end (no `exit`; a failure throws).
& {
    param(
        [switch]$App,
        [switch]$Tui,
        [switch]$Uninstall,
        [switch]$Force,
        [switch]$InstallNvim,
        [string]$Version,
        [string]$Bundle,
        [string]$AppHome,
        [string]$AppBin,
        [string]$PanelWidth,
        [string]$Tree,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Rest
    )

    # What was asked, in one place the functions below read.
    $o = [pscustomobject]@{
        App = [bool]$App; Tui = [bool]$Tui; Uninstall = [bool]$Uninstall; Force = [bool]$Force
        InstallNvim = [bool]$InstallNvim; Version = $Version; Bundle = $Bundle; AppHome = $AppHome
        AppBin = $AppBin; PanelWidth = $PanelWidth; Tree = $Tree; Rest = @($Rest | Where-Object { $_ })
    }
    Set-StrictMode -Version 2.0
    $ErrorActionPreference = "Stop"
    $ProgressPreference = "SilentlyContinue"
    $repo = "ChmaraX/herdr-nvim"
    $plugin = "chmarax.herdr-nvim"

    function Say([string]$message) { [Console]::Out.WriteLine("herdr-nvim: $message") }

    # A Windows program from System32 by full path: never Git's curl or tar
    # that a PATH may put first.
    function System32([string]$name) {
        $path = Join-Path $env:SystemRoot "System32\$name"
        if (-not (Test-Path -LiteralPath $path)) { throw "herdr-nvim: no $path (Windows 10 1803 or later has it)" }
        $path
    }

    # One argument as the program's command line spells it (the rules
    # CommandLineToArgvW and Rust read back).
    function Quote([string]$arg) {
        if ($arg -ne "" -and $arg -notmatch '[\s"]') { return $arg }
        $out = '"'
        $slashes = 0
        foreach ($c in $arg.ToCharArray()) {
            if ($c -eq '\') { $slashes++; continue }
            if ($c -eq '"') { $out += ('\' * (2 * $slashes + 1)) + '"' }
            else { $out += ('\' * $slashes) + $c }
            $slashes = 0
        }
        $out + ('\' * (2 * $slashes)) + '"'
    }

    # Runs a program on this console (its prompts, output and errors are the
    # person's) and returns its exit code. Not `&`: Windows PowerShell 5.1
    # turns a native program's stderr into errors when output is redirected.
    function Invoke-Program([string]$path, [string[]]$arguments) {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $path
        $info.Arguments = (@($arguments) | ForEach-Object { Quote $_ }) -join " "
        $info.UseShellExecute = $false
        $process = [System.Diagnostics.Process]::Start($info)
        $process.WaitForExit()
        $process.ExitCode
    }

    function Get-AppHome {
        if ($o.AppHome) { return $o.AppHome }
        if ($env:HERDR_APP_HOME) { return $env:HERDR_APP_HOME }
        Join-Path $HOME ".config\herdr-app"
    }

    function Test-App {
        (Get-Command herdr-app.exe -CommandType Application -ErrorAction SilentlyContinue) -or
            (Get-Process -Name herdr-app -ErrorAction SilentlyContinue) -or
            (Test-Path -LiteralPath (Get-AppHome))
    }

    function Test-Tui {
        [bool](Get-Command herdr -CommandType Application -ErrorAction SilentlyContinue)
    }

    function Get-Target {
        # A 32-bit PowerShell on 64-bit Windows reports the real machine in PROCESSOR_ARCHITEW6432.
        $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
        # ARM64 Windows 11 runs the x64 build; the probe below says if it can't.
        if ($arch -eq "AMD64" -or $arch -eq "ARM64") { return "x86_64-pc-windows-msvc" }
        throw "herdr-nvim: no prebuilt herdr-nvim for $arch; build from source (README: From source)"
    }

    function Save-Url([string]$url, [string]$path) {
        $code = Invoke-Program (System32 "curl.exe") @("--fail", "--silent", "--show-error", "--location", "--retry", "2", "--output", $path, $url)
        if ($code -ne 0) { throw "herdr-nvim: could not download $url (releases before the Windows app bundle have none)" }
    }

    function Install-App {
        $work = Join-Path ([System.IO.Path]::GetTempPath()) ("herdr-nvim-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
        New-Item -ItemType Directory -Path $work | Out-Null
        try {
            $archive = Join-Path $work "app.tar.gz"
            if ($o.Bundle) {
                if (-not (Test-Path -LiteralPath $o.Bundle)) { throw "herdr-nvim: no bundle at $($o.Bundle)" }
                Copy-Item -LiteralPath $o.Bundle -Destination $archive
                if (Test-Path -LiteralPath "$($o.Bundle).sha256") { Copy-Item -LiteralPath "$($o.Bundle).sha256" -Destination "$archive.sha256" }
            }
            else {
                $name = "herdr-nvim-app-$(Get-Target).tar.gz"
                $base = if ($o.Version) { "https://github.com/$repo/releases/download/$($o.Version)" } else { "https://github.com/$repo/releases/latest/download" }
                Say "downloading $name ($(if ($o.Version) { $o.Version } else { 'latest' }))"
                Save-Url "$base/$name" $archive
                Save-Url "$base/$name.sha256" "$archive.sha256"
            }
            if (Test-Path -LiteralPath "$archive.sha256") {
                $want = ((Get-Content -LiteralPath "$archive.sha256" -TotalCount 1) -split '\s+')[0].ToLowerInvariant()
                $got = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($got -ne $want) { throw "herdr-nvim: the bundle does not match its SHA-256; nothing installed" }
            }
            if ((Invoke-Program (System32 "tar.exe") @("-xzf", $archive, "-C", $work)) -ne 0) {
                throw "herdr-nvim: could not unpack the bundle"
            }
            $dir = Get-ChildItem -LiteralPath $work -Directory -Filter "herdr-nvim-app-*" | Select-Object -First 1
            $exe = if ($dir) { Join-Path $dir.FullName "bin\herdr-nvim.exe" } else { $null }
            if (-not $exe -or -not (Test-Path -LiteralPath $exe)) { throw "herdr-nvim: the bundle has no bin\herdr-nvim.exe" }
            # The binary runs here (no argument: usage, exit 2); anything else
            # (blocked, another architecture) means it can't.
            $info = New-Object System.Diagnostics.ProcessStartInfo
            $info.FileName = $exe
            $info.UseShellExecute = $false
            $info.RedirectStandardError = $true
            $probe = [System.Diagnostics.Process]::Start($info)
            $null = $probe.StandardError.ReadToEnd()
            $probe.WaitForExit()
            if ($probe.ExitCode -ne 2) { throw "herdr-nvim: the prebuilt herdr-nvim does not run on this machine (exit $($probe.ExitCode)); build from source (README: From source)" }

            $arguments = @("app-install")
            if ($o.Uninstall) { $arguments += "--uninstall" }
            if ($o.Force) { $arguments += "--force" }
            if ($o.InstallNvim) { $arguments += "--install-nvim" }
            if ($o.AppHome) { $arguments += @("--home", $o.AppHome) }
            if ($o.AppBin) { $arguments += @("--app-bin", $o.AppBin) }
            if ($o.PanelWidth) { $arguments += @("--panel-width", $o.PanelWidth) }
            if ($o.Tree) { $arguments += @("--tree", $o.Tree) }
            if ($o.Rest) { $arguments += @($o.Rest | Where-Object { $_ -ne "--" }) }
            $code = Invoke-Program $exe $arguments
            if ($code -ne 0) { throw "herdr-nvim: the install stopped (exit $code)" }
        }
        finally {
            Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    function Install-Tui {
        $herdr = Get-Command herdr -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $herdr) { throw "herdr-nvim: the herdr TUI is not installed (https://herdr.dev)" }
        $listed = (& $herdr.Path plugin list 2>$null) | Where-Object { $_ -like "*- $plugin *" } | Select-Object -First 1
        if ($o.Uninstall) {
            if (-not $listed) { Say "the TUI plugin is not installed"; return }
            & $herdr.Path plugin uninstall $plugin
            Say "removed the TUI plugin"
        }
        elseif ($listed -and $listed -like "*``[local:*") {
            Say "the TUI plugin is linked from a checkout ($listed); left as it is"
        }
        elseif ($listed) {
            & $herdr.Path plugin update --yes $plugin
            Say "updated the TUI plugin"
        }
        else {
            & $herdr.Path plugin install --yes $repo
            Say "installed the TUI plugin; bind its keys in ~/.config/herdr/config.toml (https://github.com/$repo#herdr-tui)"
        }
    }

    $wantApp = [bool]$o.App
    $wantTui = [bool]$o.Tui
    if (-not $wantApp -and -not $wantTui) {
        $wantApp = [bool](Test-App)
        $wantTui = Test-Tui
        if (-not $wantApp -and -not $wantTui) { throw "herdr-nvim: found neither the Herdr app nor the herdr TUI; install one first (https://herdr.dev)" }
    }
    if ($o.Tree -and $o.Tree -ne "native" -and $o.Tree -ne "nvim") { throw "herdr-nvim: -Tree is native or nvim, not $($o.Tree)" }
    if ($wantApp) { Install-App }
    if ($wantTui) { Install-Tui }
} @args
