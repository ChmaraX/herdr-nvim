# Windows checklist (the real app, by hand)

The `windows-real-app` workflow already runs herdr-nvim in the real Herdr
app on a hosted Windows runner: the probe and its app-quit check, the
one-line install, Files, a file opened from the tree and in a tab with no
nvim yet, the picker, Preview with a diagram, `alt-e` pressed, uninstall
byte for byte (Actions > windows-real-app > Run workflow; what it settled is
in [`doc/windows-facts.md`](../../doc/windows-facts.md)). This checklist is
only what a runner can't show: your own profile and keyboard, a browser
download, a window that flashes, and the app's own updater.

For a person on Windows 10 1803+ or 11 with the Herdr app installed. Use plain
PowerShell (5.1 unless a step says 7). No Git Bash, no admin. Keep what each
step says in a folder per run.

```powershell
$ref = "<branch or tag under test>"
$raw = "https://raw.githubusercontent.com/ChmaraX/herdr-nvim/$ref"
$h = "$env:USERPROFILE\.config\herdr-app"   # else the "Home:" line of herdr-app --help
$out = "$env:USERPROFILE\herdr-nvim-proof"; New-Item -ItemType Directory -Force $out | Out-Null
```

## 1. Install in your own profile

1. Keep the state before: `Copy-Item "$h\keys.json" "$out\keys-before.json"`
   and `Copy-Item -Recurse "$h\layouts" "$out\layouts-before"`.
2. In a new PowerShell 5.1 window, with `<ref>` filled in, then again in a
   PowerShell 7 (`pwsh`) window:
   ```powershell
   irm https://raw.githubusercontent.com/ChmaraX/herdr-nvim/<ref>/install.ps1 | iex
   ```
   Keep both outputs. A profile folder with a space or a non-ASCII letter in
   its name is worth noting: the runner's is `C:\Users\runneradmin`.
3. `Compare-Object (Get-Content "$out\keys-before.json") (Get-Content "$h\keys.json")`:
   only herdr-nvim's `alt-e`, `alt-o`, `alt-shift-e` may be added (or
   "kept yours" in the install output where you bound them). Keep the output.

## 2. Mark of the Web from a browser (row 21)

Download the release's `herdr-nvim-app-x86_64-pc-windows-msvc.tar.gz` with a
browser, extract it in Explorer, double-click `bin\herdr-nvim.exe`. Keep:
whether SmartScreen prompted (a screenshot if it did), and the output of
`Get-Item <that exe> -Stream Zone.Identifier`.

## 3. Your keyboard and your eyes

1. Press `alt-e` (Files), `alt-o` (pick), `alt-shift-e` (reveal) on your own
   keyboard layout. Keep: what each did. On a layout with AltGr, also type
   a letter that needs AltGr into nvim and an agent's terminal: it must
   still type.
2. While you open Files, click files in the tree and pick with `alt-o`, watch
   for a console window flashing up (row 19: the runner's screenshots can't
   catch a flash). Note any.
3. The tree on a folder like this, opened as a space with Files shown:
   ```powershell
   $t = "$env:USERPROFILE\herdr-nvim-tree"
   New-Item -ItemType Directory -Force "$t\src", "$t\many", "$t\build" | Out-Null
   1..600 | ForEach-Object { Set-Content "$t\many\f$_.txt" $_ }
   New-Item -ItemType Junction -Path "$t\linked" -Target "$t\src" | Out-Null
   Set-Content "$t\src\a.md" "# a"; Set-Content "$t\secret.txt" "x"; attrib +h "$t\secret.txt"
   Set-Content "$t\.gitignore" "build/"; Set-Content "$t\build\out.js" "1"; git -C $t init -q
   ```
   Check: `linked` shows as a folder and opens to `a.md`; how `secret.txt`
   shows (compare with the app's own Files mod); `many` lists sorted, capped
   with a note; `build` is marked ignored. Then Preview an image outside the
   home folder, `C:\Windows\Web\Wallpaper\Windows\img0.jpg`. Keep a
   screenshot of each.

## 4. Upgrade while nvim runs

With nvim open on an edited buffer, install a newer build:
`& ([scriptblock]::Create((irm $raw/install.ps1))) -App -Bundle <the
herdr-nvim-app-*.tar.gz of a later CI run>` (or `-Version <a newer tag>`).
Check: the open nvim still edits and saves; the runtime store
(`$env:USERPROFILE\.local\share\herdr-nvim\app`, one folder per stamp) keeps
the old runtime while that nvim runs, and the next install after closing it
prunes it. Keep `Get-ChildItem` of the store before, during and after.

## 5. Update the app, then use Files

Update the Herdr app with its own updater, reopen it, and press Files. Check:
the tree and nvim open as before. If the update moved `herdr-app.exe`,
herdr-nvim looks the app up again; keep where the exe was before and after
(`Get-Command herdr-app`).

## 6. Uninstall

``& ([scriptblock]::Create((irm $raw/install.ps1))) -App -Uninstall``, then
`Compare-Object (Get-Content "$out\keys-before.json") (Get-Content "$h\keys.json")`
shows nothing, and the layouts match `$out\layouts-before`.
