# Windows checklist (the real app, by hand)

For a person on Windows 10 1803+ or 11 with the Herdr app installed. Use plain
PowerShell (5.1 unless a step says 7). No Git Bash, no admin. Each step says
what to keep. Put it in a folder per run and fill the rows it names in
[`doc/windows-facts.md`](../../doc/windows-facts.md). Recording the screen
for steps 2 to 7 is the real-app gate's video.

`<ref>` is the branch or tag under test. Set these in the PowerShell window
you work from (steps 2.2, 2.3 and 2.5 use fresh windows on purpose, so they
spell the URL out):

```powershell
$ref = "<ref>"
$raw = "https://raw.githubusercontent.com/ChmaraX/herdr-nvim/$ref"
$h = "$env:USERPROFILE\.config\herdr-app"   # else the "Home:" line of herdr-app --help
$m = "$h\mods\winprobe"
$out = "$env:USERPROFILE\winprobe-proof"; New-Item -ItemType Directory -Force $out | Out-Null
```

## 1. The probe

1. Get `winprobe.exe`: the `winprobe` artifact of the windows-latest CI run for
   `<ref>` (named `winprobe-windows`), or build it with
   `cargo build --release --example winprobe` (`target\release\examples\winprobe.exe`).
2. Install the probe mod. Keep its exe outside Downloads first, so step 2's
   Mark-of-the-Web check isn't confused:
   ```powershell
   New-Item -ItemType Directory -Force "$m\bin" | Out-Null
   foreach ($f in "main.js", "mod.json") { Invoke-WebRequest "$raw/examples/winprobe-mod/$f" -OutFile "$m\$f" }
   Copy-Item <path to>\winprobe.exe "$m\bin\winprobe.exe"
   Copy-Item "$h\keys.json" "$out\keys-before-anything.json"
   ```
3. From plain PowerShell, outside the app: `& "$m\bin\winprobe.exe" --log "$out\cli.jsonl" where`.
4. In the app, open the command palette and run `mod.open` with name
   `winprobe` (or `herdr-app do mod.open --name winprobe` if `herdr-app` is on
   `PATH`). Click into a terminal pane, then press **Run all**. It takes about
   a minute and opens three short-lived panes. Keep the window open until
   every line has ✓ or ✗. Note any console window that flashes up.
5. Press **Survive app quit**. Within a minute, quit the app completely
   (Task Manager shows no Herdr process). Wait two minutes. Reopen it, open
   winprobe again, and press **Read log**. The survive line should count
   ticks "after the app that started it quit"; zero means daemons die with
   the app.
6. Keep: the whole `$h\winprobe-out\<run>\` folder (`results.json`,
   `log.jsonl`), `$out\cli.jsonl`, a screenshot of the winprobe window, and
   whether a console window flashed. These settle rows 1 to 20 and 22.
7. Close the leftover panes. The mod and `$h\winprobe-out` can be deleted at the end.

## 2. Install

1. Keep the state before: `Copy-Item "$h\keys.json" "$out\keys-before.json"`
   and `Copy-Item -Recurse "$h\layouts" "$out\layouts-before"`.
2. In a new plain PowerShell 5.1 window, with `<ref>` filled in:
   ```powershell
   irm https://raw.githubusercontent.com/ChmaraX/herdr-nvim/<ref>/install.ps1 | iex
   ```
   Keep the full output.
3. Again with options, in another new window:
   ```powershell
   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/ChmaraX/herdr-nvim/<ref>/install.ps1))) -App -InstallNvim
   ```
   Keep the output, and the Neovim path it prints. Back in the first window
   for the rest.
4. Mark of the Web, for row 21:
   ```powershell
   & "$m\bin\winprobe.exe" --log "$out\motw.jsonl" motw "$h\mods\herdr-nvim\bin\herdr-nvim.exe"
   ```
   Then download the release's `herdr-nvim-app-x86_64-pc-windows-msvc.tar.gz`
   with a browser, extract it in Explorer, double-click `bin\herdr-nvim.exe`
   and run `motw` on it the same way. Keep: whether SmartScreen prompted
   (a screenshot if it did) and both `motw.jsonl` lines.
5. Repeat 2.2 in a PowerShell 7 (`pwsh`) window. Keep the output.

## 3. The Files tree

Make a test folder, then open it as a space in the app and show Files:

```powershell
$t = "$env:USERPROFILE\winprobe-tree"
New-Item -ItemType Directory -Force "$t\src", "$t\many", "$t\build" | Out-Null
1..600 | ForEach-Object { Set-Content "$t\many\f$_.txt" $_ }
New-Item -ItemType Junction -Path "$t\linked" -Target "$t\src" | Out-Null
Set-Content "$t\src\a.md" "# a"; Set-Content "$t\secret.txt" "x"; attrib +h "$t\secret.txt"
Set-Content "$t\.gitignore" "build/"; Set-Content "$t\build\out.js" "1"; git -C $t init -q
```

Check: `linked` shows as a folder and opens to `a.md`; how `secret.txt` shows
(compare with the app's own Files mod); `many` lists sorted, capped with a
note; `build` is marked ignored. Keep a screenshot of each.

## 4. Preview

Preview `$t\src\a.md` after adding a ```` ```mermaid ```` block, then an image
outside the home folder: `C:\Windows\Web\Wallpaper\Windows\img0.jpg`. Keep
screenshots of both.

## 5. Keys and opening files

1. Press `ctrl-shift-e` (Files), `ctrl-shift-y` (pick), `ctrl-alt-shift-e`
   (reveal). Keep: what each did.
2. Compare keys: `Compare-Object (Get-Content "$out\keys-before.json") (Get-Content "$h\keys.json")`.
   Only herdr-nvim's bindings may be added; none of yours changed. Keep the output.
3. Open a file in nvim from the tree. Keep a screenshot.
4. Other mods' contract, with a tab id from `herdr-app ls`, by `.exe` and as a
   Rust caller would start it (extensionless):
   ```powershell
   & "$h\mods\herdr-nvim\bin\herdr-nvim.exe" open --tab <tab> "$t\src\a.md:3"
   & "$m\bin\winprobe.exe" --log "$out\contract.jsonl" spawn "$h\mods\herdr-nvim\bin\herdr-nvim" open --tab <tab> "$t\src\a.md:3"
   ```
   Both should put nvim on line 3. Keep the output and `contract.jsonl`.

## 6. Upgrade while nvim runs

With nvim open on an edited buffer, install a newer bundle the same way as
step 2.3, with `-Bundle <the herdr-nvim-app-x86_64-pc-windows-msvc.tar.gz of a
newer build>` (the `install-e2e` artifact of a later CI run has one), or
`-Version <a newer tag>` once one is published. Check: the
open nvim still edits and saves; the runtime store
(`$env:USERPROFILE\.local\share\herdr-nvim\app`, one folder per stamp) keeps
the old runtime while that nvim runs, and the next install after closing it
prunes it. Keep `Get-ChildItem` of the store before, during and after, and
the install output.

## 7. Uninstall

``& ([scriptblock]::Create((irm $raw/install.ps1))) -App -Uninstall``

Check and keep:
- `Compare-Object (Get-Content "$out\keys-before.json") (Get-Content "$h\keys.json")` shows nothing;
- the layouts match `$out\layouts-before` (`Compare-Object` on each file);
- `$h\mods\herdr-nvim` and the other herdr-nvim mod are gone, and so is the runtime store;
- the downloaded Neovim is still there, and the output printed its path.
