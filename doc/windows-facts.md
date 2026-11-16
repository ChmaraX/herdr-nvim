# Windows facts

What herdr-nvim on Windows depends on but no one has seen on a real Windows
Herdr app yet. Each row says what the code assumes, what the code does so it
works either way, and which checklist step settles it
([`herdr-app/e2e/WINDOWS-CHECKLIST.md`](../herdr-app/e2e/WINDOWS-CHECKLIST.md)).
When a step settles a fact, replace "unverified" with what was seen, the date,
the app version, and a link to the run's `results.json`.

The probe is `examples/winprobe.rs` plus the mod in `examples/winprobe-mod/`.
"1 · exec-args" means checklist step 1, the mod's `exec-args` line.

| # | Fact | Status | Assumed | What the code does so it works either way | Probe |
|---|---|---|---|---|---|
| 1 | The app's `exec` starts an **extensionless** path (`bin\winprobe`) when only `bin\winprobe.exe` exists | unverified | Yes: the app spawns with Rust std, which appends `.exe` (row 3) | The JS mods name `.exe` explicitly on Windows. TODO(lead): confirm `bin()` in `lib.js` and the header mod. | 1 · exec-args |
| 2 | `exec` passes args to an `.exe` unchanged: `"`, `%PATH%`, `&`, `^`, `!`, `'q'`, spaces, `é`, `漢`, JSON with quotes, `""`, trailing `\` | unverified | Yes: no shell between, MSVC quoting by Rust std | The launcher passes argv through as received; if the probe shows mangling, the fix lives in the launcher alone. TODO(lead): the fallback encoding, if any. | 1 · exec-args (`args`, `command_line`, `ancestors`) |
| 3 | Rust std `Command` resolves an extensionless full path to its `.exe` | verified by reading the source; runtime unverified | Yes | Other mods keep `<appHome>\mods\herdr-nvim\bin\herdr-nvim`; on Windows the file there is `herdr-nvim.exe`. | 1 · std-command; 5 (contract via `winprobe spawn`) |
| 4 | `pane.split {argv}` runs argv verbatim | macOS app 0.1.16: works, args intact (winprobe run on macOS, 2026-11-16); Windows unverified | Yes | App context travels as the argv prefix `--app-home H --app-bin B` (`AppEnv::self_argv`). | 1 · terminals (`pane-split`) |
| 5 | `pane.split {argv, env}` passes env | macOS app 0.1.16 refuses it: `pane.split: unknown argument(s): env; valid arguments: direction, pane, argv, mod, props`; Windows unverified | No | Never sends `env` to `pane.split`; the argv prefix carries the context. | 1 · terminals (`pane-split-env`) |
| 6 | `terminal.open {argv}` and `hc run -- argv` pass args unchanged | macOS: both intact; Windows unverified | Yes | Long-lived terminals start through the core (`terminal.run`) with the runtime exe and the argv prefix. | 1 · terminals (`terminal-open`, `hc-run`) |
| 7 | The core's `terminal.run {env}` reaches the program (the picker's `HERDR_NVIM_HANDOFF`) | not probed: a mod can't call the core's RPC and `hc run` has no env flag | Yes | TODO(lead): keep the handoff in env, or move it to argv if the Windows cargo/e2e run shows it dropped. | none |
| 8 | Processes `exec` starts are in a Job Object with `KILL_ON_JOB_CLOSE`, and whether it allows breakaway | unverified | Unknown | Daemons spawn with `CREATE_NO_WINDOW \| CREATE_NEW_PROCESS_GROUP \| CREATE_BREAKAWAY_FROM_JOB`, retried without breakaway on `ERROR_ACCESS_DENIED` (`winprobe spawn-detached` does exactly this). If they still die with the exec, start them through the core's `terminal.run`, as `app-watch` is. | 1 · detached (`spawned.record.job`, `start.job`) |
| 9 | A daemon started that way outlives the `exec` that started it, and the app quitting | unverified | Yes | As row 8. | 1 · detached (`ticks_after_exec_returned`); 1 · survive (`ticks_without_app`) |
| 10 | `exec` returns while a detached child it started still runs | macOS: yes (11 ms); Windows unverified | Yes, because the child gets null stdio and so holds none of the exec's pipes | Daemons are spawned with null stdio. | 1 · detached (`spawned.ms`) |
| 11 | A loaded mod folder can be renamed while the app runs (nothing started from it) | macOS: yes; Windows unverified | Yes: the app doesn't hold mod files open | Whole-folder swap with about 5 s of retries, then rollback naming the holder. If refused: the generation-pointer swap (`main.js` imports `versions/<stamp>/main.js`). TODO(lead): which one shipped. | 1 · rename-free |
| 12 | Windows refuses renaming a folder while an exe in it runs | macOS: rename succeeds (no such rule); Windows unverified | Yes: `ERROR_ACCESS_DENIED` (5) or a sharing violation (32) | Prune renames a runtime to `.trash-*` first; a refusal means "in use" and keeps it. POSIX relies on the daemon registry alone. | 1 · rename-held (`attempts[].os`) |
| 13 | Path forms from the app: `appHome()`, `panes()[].cwd`, `spaces()[].folder` | unverified | `C:\Users\…` with `\`; maybe lower-case drive, `/` separators or `\\?\` | Path keys fold case and separators on Windows only. TODO(lead): `GetLongPathNameW` in `pathkey::canonical` if 8.3 names show up. | 1 · app |
| 14 | The path form `watchFolder` returns for a changed file | macOS: the same form as watched; Windows unverified | Same form as watched, but with `\` | Compare through the path key, never as raw strings. TODO(lead): where the mods compare watch paths. | 1 · watch (`hit`, `same_form`) |
| 15 | `%TEMP%` and other paths come as 8.3 short names (`C:\Users\LONGNA~1\…`) | unverified | Sometimes, for long user names | As row 13. | 1 · every record's `temp_dir`, `cwd`, `image` |
| 16 | Where `herdr-app.exe` lives, and whether it is on `PATH` | unverified | `%LOCALAPPDATA%\Programs\…`, not on `PATH` | `app-install` takes `--app-bin`, then env, `PATH`, the running app's image, then `%LOCALAPPDATA%\Programs`. TODO(lead): confirm the order shipped. | 1 · where; 1 (plain PowerShell `winprobe where`) |
| 17 | Where `hc.exe` lives | unverified | Beside `herdr-app.exe` or on `PATH` | `hc` is derived: env, then beside `app_bin`, then `PATH`; stored in `install.json` only if neither works. | 1 · where (`beside_running_app`, `on_path`) |
| 18 | Mods' `exec` gets the app's environment (`HERDR_APP_HOME`, `PATH`) | macOS: yes; Windows unverified | Yes | Nothing depends on it: the launcher reads `install.json`, and runtime terminals get the argv prefix. | 1 · exec-args (`record.env`) |
| 19 | `exec` of a console exe opens no console window | unverified | No window: the app spawns with `CREATE_NO_WINDOW` | TODO(lead): if a window flashes, build the launcher with `windows_subsystem = "windows"` or start the runtime with `CREATE_NO_WINDOW`. | 1 · exec-args (`console_window`); watch for flashes during the run |
| 20 | The app's default keys on Windows, and what `cmd-` means there | unverified | Defaults are `ctrl-shift-*`; `cmd-` is the Win key (Win+E opens Explorer) | Windows key set: `ctrl-shift-e` Files, `ctrl-shift-y` pick, `ctrl-alt-shift-e` reveal; merged only where free, so a clash keeps the person's key. Linux keeps `cmd-*`. | 1 · app (`keys`) and `keys-before-anything.json`, both before herdr-nvim is installed; 5 |
| 21 | Mark of the Web on the installed exes, and a SmartScreen prompt | unverified | `irm \| iex` downloads carry no `Zone.Identifier`; a browser download does, and then Explorer prompts | No code signing. TODO(lead): whether `install.ps1` downloads with `Invoke-WebRequest` or `curl.exe`, and whether it unblocks the extracted files. | 2 (`winprobe motw` on the installed exes; the prompt seen or not) |
| 22 | `%SystemRoot%\System32\curl.exe` and `tar.exe` exist | unverified | Yes on Windows 10 1803 and newer | The Neovim download calls them by absolute path, never from `PATH`: a GNU tar there (Git Bash, MSYS2) reads `C:\…` as a remote host. | 1 · where (`system32`) |

## Rust std's extensionless resolution (row 3)

Source: `library/std/src/sys/process/windows.rs`, `resolve_exe`, Rust 1.97.1
(https://github.com/rust-lang/rust/blob/1.97.1/library/std/src/sys/process/windows.rs).

- A path that is not a bare file name (`C:\…\bin\herdr-nvim`) and has no
  `.exe` suffix: std appends `.exe` and uses that path if `GetFileAttributesW`
  finds it. Otherwise it uses the path as given and lets `CreateProcessW`
  fail. So the `.exe` wins even when an extensionless file sits beside it.
- A bare name (`hc`): std searches the child's `PATH` if the command changed
  it, then the running exe's folder, System32, the Windows folder, then the
  parent's `PATH`. It appends `.exe` when the name has no `.`. So it finds
  `hc.exe` and never `hc.cmd`.
- A `.bat` or `.cmd` target runs through `cmd.exe` with batch quoting. The
  launcher being a real `.exe` keeps cmd.exe out of every call.

This covers Rust callers (other mods' engines, herdr-nvim itself). The app's
`exec` (row 1) is a separate question, because the app may not spawn through
Rust std.
