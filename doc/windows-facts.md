# Windows facts

What herdr-nvim on Windows depends on in the Herdr app, and what was seen.
Most rows are settled by the `windows-real-app` workflow
(`.github/workflows/windows-real-app.yml`, `herdr-app/e2e/windows-app.ps1`):
on a hosted `windows-latest` runner it installs herdr-core and the app with
their official installers, runs the probe (`examples/winprobe.rs` and the mod
in `examples/winprobe-mod/`) against the real app, installs herdr-nvim with
the one-line install and drives it. Settled with **app 0.1.16, core (hc)
0.1.21**, run https://github.com/ChmaraX/herdr-nvim/actions/runs/44102462170 (artifact `windows-real-app`: `winprobe/<run>/results.json`,
`log.jsonl`, `cli.jsonl`, screenshots). What CI can't show is in
[`herdr-app/e2e/WINDOWS-CHECKLIST.md`](../herdr-app/e2e/WINDOWS-CHECKLIST.md).

"1 · exec-args" means the probe's `exec-args` line.

| # | Fact | Status | What the code does | Probe |
|---|---|---|---|---|
| 1 | The app's `exec` starts an **extensionless** path (`bin\winprobe`) when only `bin\winprobe.exe` exists | **Confirmed**: it ran `winprobe.exe`, arguments intact | The JS mods still name `.exe` on Windows (`bin()` in `path.js`, the header mod's copy, the card). | 1 · exec-args |
| 2 | `exec` passes args to an `.exe` unchanged: `"`, `%PATH%`, `&`, `^`, `!`, `'q'`, spaces, `é`, `漢`, JSON with quotes, `""`, trailing `\` | **Confirmed**, extensionless and `.exe` alike | The launcher passes argv through as received; no shell in between. | 1 · exec-args |
| 3 | Rust std `Command` resolves an extensionless full path to its `.exe` | **Confirmed** at runtime (and by the source, below); also through herdr-nvim's own launcher: `winprobe spawn <mods>\herdr-nvim\bin\herdr-nvim open …` opened the file in the tab's nvim | Other mods keep `<appHome>\mods\herdr-nvim\bin\herdr-nvim`. | 1 · std-command; the run's `open-file` step |
| 4 | `pane.split {argv}` runs argv verbatim | **Confirmed** on Windows | App context travels as the argv prefix `--app-home H --app-bin B` (`AppEnv::self_argv`). | 1 · terminals |
| 5 | `pane.split {argv, env}` passes env | **No**: refused as on macOS (`unknown argument(s): env`) | Never sends `env` to `pane.split`. | 1 · terminals |
| 6 | `terminal.open {argv}` and `hc run -- argv` pass args unchanged | **Confirmed** | Long-lived terminals start through the core (`terminal.run`). | 1 · terminals |
| 7 | The core's `terminal.run {env}` reaches the program (the picker's `HERDR_NVIM_HANDOFF`) | **Confirmed**: alt-o's picker listed the agent's file and the pick opened it in nvim | Kept in env. | the run's `picker` step |
| 8 | Processes `exec` starts are in a Job Object; `KILL_ON_JOB_CLOSE`; breakaway | **In a job, without `KILL_ON_JOB_CLOSE`; breakaway refused** (`ERROR_ACCESS_DENIED`) | Daemons ask for breakaway and, refused, start in the job, which does not kill them (rows 9, 10). | 1 · detached |
| 9 | A daemon started that way outlives the `exec`, and the app quitting | **Confirmed**: 10 ticks after the app quit | As row 8. | 1 · detached; survive (the workflow quits and restarts the app) |
| 10 | `exec` returns while a detached child it started still runs | **Was wrong; fixed.** The `exec` waited for the child's whole life (21 s for a 20 s child): Windows gives a child every inheritable handle, and the child held the exec's stdout and stderr pipes. A tree click in a tab with no nvim took 21 s. | Both binaries make their std handles non-inheritable first thing (`src/stdio_inherit.rs`); children get Rust's duplicates of what they should share. Now 54 ms (probe), 1 s (`open` in a cold tab). | 1 · detached; the run's `open-cold` step |
| 11 | A loaded mod folder can be renamed while the app runs (nothing started from it) | **Confirmed** | The generation-pointer swap (`src/install/mods.rs`) never renames a live mod folder anyway: an upgrade places `gen/<new id>/` beside the old one and switches both mods' `main.js`; only uninstall removes the folder. | 1 · rename-free |
| 12 | Windows refuses renaming a folder while an exe in it runs | **Wrong**: the probe's folder was renamed and back while its exe ran | Prune never relied on a refusal alone: a runtime a registered nvim runs is kept (`live_runtimes`). One only a herdr-nvim process holds is moved to `.trash-*`; its delete fails while the exe runs and a later install's sweep takes it. | 1 · rename-held |
| 13 | Path forms from the app: `appHome()`, `panes()[].cwd`, `spaces()[].folder` | `C:\Users\…` with `\`, upper-case drive, long names; `home()` is `C:\Users\<name>` | Path keys fold case and separators on Windows (`path.js key`, `fs.lua key`). | 1 · app |
| 14 | The path form `watchFolder` returns for a changed file | **Another form**: `\\?\C:\…\touch.txt` for a watched `C:\…/…` | `lib.js` passes every watched path through `canonical` (drops `\\?\`, `/` separators); the tree showed a file made after it opened. | 1 · watch |
| 15 | `%TEMP%` and other paths come as 8.3 short names | **Yes for `TEMP`/`TMP`** (`C:\Users\RUNNER~1\…`); the app's own paths are long | herdr-nvim compares no `TEMP` path; `pathkey::canonical` expands short names of paths that exist. | every record's `temp_dir` |
| 16 | Where `herdr-app.exe` lives, and whether it is on `PATH` | **`%LOCALAPPDATA%\Programs\herdr-app\herdr-app.exe`, on the user PATH** (the app's installer adds it) | `locate.rs`/`discover.rs`: `--app-bin`, `$HERDR_APP_BIN`, PATH, the per-user install folders, then a running app's image. | 1 · where |
| 17 | Where `hc.exe` lives | **`%LOCALAPPDATA%\Programs\herdr-core\hc.exe`, on the user PATH; not beside `herdr-app.exe`** (the core's installer) | `rpc.rs hc_bin`: `$HERDR_NVIM_HC`, `hc.exe` beside the app, else PATH, which on Windows includes the registry's PATH (row 23). | 1 · where |
| 18 | Mods' `exec` gets the app's environment (`HERDR_APP_HOME`, `PATH`) | **Confirmed**: the app's own environment, PATH included | Nothing depends on it: the launcher reads `install.json`, terminals get the argv prefix. | 1 · exec-args |
| 19 | `exec` of a console exe opens no console window | **Confirmed**: no console window (`GetConsoleWindow` null), none on the screenshots | Nothing to do. | 1 · exec-args |
| 20 | The app's default keys on Windows | **`ctrl-shift-*` and `alt-*`; `ctrl-shift-e` is `pane.split:down`**, so herdr-nvim's first Windows set lost Files to it, and `ctrl-alt-shift-e` is AltGr+Shift+E on many layouts | **Changed**: Windows set `alt-e` Files, `alt-o` pick, `alt-shift-e` reveal (free in the defaults, `cmd-e`/`cmd-o`/`cmd-shift-e` on macOS). Pressing `alt-e` in the real app opened the tree and nvim. | `keys-before-anything.json`; the run's `install` and `keys` steps |
| 21 | Mark of the Web on the installed exes, and a SmartScreen prompt | **No mark** on the launcher or the runtime exe after the one-line install (curl.exe) | Nothing unblocked, nothing to. A browser-downloaded bundle and its SmartScreen prompt: checklist step 2. | the run's `install` step |
| 22 | `%SystemRoot%\System32\curl.exe` and `tar.exe` exist | **Confirmed** | The Neovim download calls them by absolute path. | 1 · where (`system32`) |
| 23 | The core's terminals get the PATH stored in the registry, not the app's or the core's | **Seen** (new): a terminal's PATH is the machine's and the person's PATH from the registry; `HERDR_*` variables do pass | **Fixed**: the pane said "`nvim` was not found" while the daemon (started from an `exec`, with the app's PATH) ran. `path::augment_path` appends the registry's PATH, read now, in every herdr-nvim process on Windows, so a Neovim installed while Herdr runs is found by all of them. | the run's `install` step adds Neovim to the user PATH while Herdr runs |
| 24 | How the app writes `keys.json` and layouts | **CRLF line ends** (new) | **Fixed**: `edit_json` writes CRLF where the file had it, so an uninstall gives `keys.json` back byte for byte (it gave LF before). The install E2E seeds CRLF fixtures on Windows. | the run's `uninstall` step |

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
