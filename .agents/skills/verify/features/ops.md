# Ops

CLI subcommands for checking and managing herdr-nvim, and how the plugin gets installed.

## Sub-features

- doctor: `herdr-nvim doctor [--with-agent claude]` runs a live self-test in a scratch workspace named `herdr-nvim-doctor` and then cleans it up. It checks the full-height split, the toggle restoring the layout, the hidden nvim's health, the remote-UI attach and (optionally) agent liveness. It prints OK/FAIL lines and "all doctor checks OK", and exits non-zero on failure.
- daemons list: `herdr-nvim daemons [--json]` shows each hidden nvim with its workspace/tab, RAM (including children such as LSPs), uptime and state (`alive`, `orphaned`, `unresponsive`, `unknown`, plus "N unsaved"). It ends with a total and hint lines.
- daemons stop: `herdr-nvim daemons stop <tab-id>`, `stop --all` or `stop --orphans`. A daemon with unsaved buffers or pending comments is not stopped unless `--force` is given. The next toggle in that tab starts a fresh nvim.
- install: `herdr plugin install ChmaraX/herdr-nvim` or `herdr plugin link <dir>`. It downloads a prebuilt binary, or builds with cargo if that fails. Needs herdr ≥ 0.7.5. Running the binary with no args prints usage and exits 2.

## How to get to it (user POV)

Run the commands in a shell while herdr is running. Install goes through the herdr CLI.

## Driving it in the box

- Box health: run `box doctor <item>` first if anything looks stale or broken. It checks the container, worktree/build stamps, herdr server, plugin link, mock LLM and free disk, and prints the recovery command on failures.
- Product doctor: `box exec <item> -- /opt/herdr-nvim/bin/herdr-nvim doctor`. Takes about 3 s and ends with "all doctor checks OK", exit 0.
- Daemons: `box exec <item> -- /opt/herdr-nvim/bin/herdr-nvim daemons [--json]`. Open a sidebar first (`herdr plugin action invoke toggle --plugin chmarax.herdr-nvim`), or the output is "no nvim daemons running".
- Unsaved work: `box exec <item> -- nvim --server /tmp/herdr-nvim/w1_t1.sock --remote-send 'ggix<Esc>'` dirties the sidebar buffer of tab `w1:t1`. `daemons` then shows "alive · 1 unsaved", `daemons stop w1:t1` refuses with exit 1, and `--force` stops it.
- Install: only the `plugin link` path with a local build, which `box up` / `box reset` already do. Running the binary with no args prints usage and exits 2. Not possible in the box: the release download (offline), other glibc versions, macOS and Windows.

## Gotchas

- `doctor` needs a running herdr. `daemons` works without one, but every state is `unknown`.
- `doctor --with-agent pi` fails its agent-liveness check on herdr 0.9.3 ("unknown option: --workspace"); the other checks still pass. Do not read that FAIL as a regression of your change.
- `doctor --help` prints no help; it just runs the checks.
- The binary lives in the plugin's `bin/` directory and may not be on PATH. Call it by full path.
- `daemons stop` without `--force` refuses daemons with unsaved work or pending comments. Treat that as expected behavior, not a failure.
