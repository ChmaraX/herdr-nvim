//! `herdr-nvim daemons` -- list the hidden per-tab nvim daemons (which herdr
//! tab each belongs to, how much memory it and its children such as LSP
//! servers hold, how long it has run) and stop one by hand without closing
//! its tab.
//!
//! Daemons are enumerated from the same runtime-dir registry that `gc` and
//! the close hooks use (`daemon::daemon_keys`), queried over their own socket
//! for pid / unsaved buffers / pending comments, named through
//! `herdr api snapshot`, and measured with one OS process-table snapshot.
//! Stopping goes through the shared `daemon::stop_tab_key`.

use std::{collections::HashMap, env, process::Command};

use anyhow::{bail, Result};
use serde::Serialize;

use crate::{
    config::Sidebar,
    daemon::{daemon_keys, remote_expr, socket_path_for_key, stop_tab_key, UNSAVED_BUFFERS_EXPR},
    herdr::{CliHerdr, Herdr, TabInfo},
    state::tab_key,
};

const USAGE: &str = "usage: herdr-nvim daemons [--json]\n       \
                     herdr-nvim daemons stop <tab-id> [--force]\n       \
                     herdr-nvim daemons stop --all [--force]\n       \
                     herdr-nvim daemons stop --orphans";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DaemonState {
    /// Answers on its socket and its tab is open in herdr.
    Alive,
    /// Answers on its socket but its tab is gone from herdr.
    Orphaned,
    /// Registered, but nothing answers on its socket (stale or hung).
    Unresponsive,
    /// Answers, but herdr could not be asked whether its tab still exists.
    Unknown,
}

impl DaemonState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::Orphaned => "orphaned",
            Self::Unresponsive => "unresponsive",
            Self::Unknown => "unknown",
        }
    }
}

/// One registered daemon. `None` fields could not be determined.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct DaemonInfo {
    pub tab_id: String,
    /// Sanitized tab key (the runtime-dir registry entry's stem).
    pub key: String,
    pub workspace_label: Option<String>,
    pub tab_label: Option<String>,
    pub tab_number: Option<u64>,
    pub pid: Option<u32>,
    /// Resident memory of the daemon plus all of its descendants (LSPs, jobs).
    pub rss_bytes: Option<u64>,
    pub uptime_secs: Option<u64>,
    pub state: DaemonState,
    pub unsaved_buffers: Option<u32>,
    pub pending_comments: Option<u32>,
    /// Whether herdr still has the tab: `None` when herdr was unreachable.
    #[serde(skip)]
    pub tab_open: Option<bool>,
}

impl DaemonInfo {
    /// `workspace / tab` as herdr shows it, or `None` for a tab herdr does
    /// not (or could not be asked to) know.
    fn display_name(&self) -> Option<String> {
        if self.tab_open != Some(true) {
            return None;
        }
        let workspace = self
            .workspace_label
            .clone()
            .unwrap_or_else(|| self.tab_id.split(':').next().unwrap_or("").to_owned());
        let tab = self
            .tab_label
            .clone()
            .or_else(|| self.tab_number.map(|number| number.to_string()))
            .unwrap_or_else(|| self.tab_id.clone());
        Some(format!("{workspace} / {tab}"))
    }

    /// What to call the daemon in stop messages.
    fn name(&self) -> String {
        self.display_name().unwrap_or_else(|| self.tab_id.clone())
    }

    fn has_pending_work(&self) -> bool {
        self.unsaved_buffers.unwrap_or(0) > 0 || self.pending_comments.unwrap_or(0) > 0
    }
}

/// Vimscript returning `"<pid> <unsaved> <comments> <tab id>"` in one round
/// trip. Comments are only counted if the plugin's comment store is already
/// loaded, so the probe never loads modules into the daemon.
fn probe_expr() -> String {
    let comments = "luaeval(\"(function() local c = package.loaded['herdr-nvim.comments'] \
                    if not c then return 0 end local ok, l = pcall(c.list) \
                    return ok and #l or 0 end)()\")";
    format!("join([getpid(), {UNSAVED_BUFFERS_EXPR}, {comments}, $HERDR_TAB_ID], ' ')")
}

struct Probe {
    pid: Option<u32>,
    unsaved: Option<u32>,
    comments: Option<u32>,
    tab_id: Option<String>,
}

fn probe(key: &str, sidebar: &Sidebar) -> Option<Probe> {
    let out = remote_expr(&socket_path_for_key(key), &probe_expr(), sidebar)?;
    let mut parts = out.splitn(4, ' ');
    let mut number = || parts.next().and_then(|part| part.parse().ok());
    let pid = number();
    let unsaved = number();
    let comments = number();
    let tab_id = parts
        .next()
        .map(str::to_owned)
        .filter(|tab| !tab.is_empty() && tab_key(tab) == key);
    Some(Probe {
        pid,
        unsaved,
        comments,
        tab_id,
    })
}

/// Every registered daemon, sorted by tab id. herdr being unreachable is not
/// an error: names are then unknown and responsive daemons are `Unknown`.
pub(crate) fn collect(h: &mut dyn Herdr, sidebar: &Sidebar) -> Result<Vec<DaemonInfo>> {
    let keys = daemon_keys()?.unwrap_or_default();
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let tabs = h.tab_infos().ok();
    let procs = ProcessTable::load();
    let mut daemons: Vec<DaemonInfo> = keys
        .into_iter()
        .map(|key| {
            let probe = probe(&key, sidebar);
            let tab = tabs
                .as_ref()
                .and_then(|tabs| tabs.iter().find(|tab| tab_key(&tab.tab_id) == key));
            let tab_open = tabs.as_ref().map(|_| tab.is_some());
            let tab_id = probe
                .as_ref()
                .and_then(|probe| probe.tab_id.clone())
                .or_else(|| tab.map(|tab| tab.tab_id.clone()))
                .unwrap_or_else(|| key.replacen('_', ":", 1));
            let state = match (&probe, tab_open) {
                (None, _) => DaemonState::Unresponsive,
                (Some(_), Some(true)) => DaemonState::Alive,
                (Some(_), Some(false)) => DaemonState::Orphaned,
                (Some(_), None) => DaemonState::Unknown,
            };
            let pid = probe.as_ref().and_then(|probe| probe.pid);
            let (rss_bytes, uptime_secs) = match (pid, &procs) {
                (Some(pid), Some(procs)) => (procs.tree_rss(pid), procs.uptime(pid)),
                _ => (None, None),
            };
            let TabInfo {
                workspace_label,
                tab_label,
                tab_number,
                ..
            } = tab.cloned().unwrap_or(TabInfo {
                tab_id: String::new(),
                workspace_id: String::new(),
                workspace_label: None,
                tab_label: None,
                tab_number: None,
            });
            DaemonInfo {
                tab_id,
                key,
                workspace_label,
                tab_label,
                tab_number,
                pid,
                rss_bytes,
                uptime_secs,
                state,
                unsaved_buffers: probe.as_ref().and_then(|probe| probe.unsaved),
                pending_comments: probe.as_ref().and_then(|probe| probe.comments),
                tab_open,
            }
        })
        .collect();
    daemons.sort_by(|a, b| a.tab_id.cmp(&b.tab_id));
    Ok(daemons)
}

/// Which daemons `daemons stop` targets.
#[derive(Debug, PartialEq)]
pub(crate) enum Target {
    /// A tab id (`w26:t2`) or its key form (`w26_t2`).
    Tab(String),
    All,
    Orphans,
}

#[derive(Debug, Default)]
pub(crate) struct StopOutcome {
    /// One `stopped ...` line per daemon stopped.
    pub stopped: Vec<String>,
    /// One explanation per daemon left running for lack of `--force`.
    pub refused: Vec<String>,
}

/// Stop the targeted daemons. A daemon with unsaved buffers or pending
/// comments is refused unless `force` -- except an orphan, whose tab (and so
/// whatever it held) is already gone.
pub(crate) fn stop(
    h: &mut dyn Herdr,
    sidebar: &Sidebar,
    target: &Target,
    force: bool,
) -> Result<StopOutcome> {
    if let Target::Tab(tab) = target {
        let key = tab_key(tab);
        if !daemon_keys()?.unwrap_or_default().contains(&key) {
            bail!("no nvim daemon for tab {tab}");
        }
    }
    let daemons = collect(h, sidebar)?;
    if *target == Target::Orphans && daemons.iter().any(|d| d.tab_open.is_none()) {
        bail!("cannot tell which daemons are orphaned: herdr is unreachable");
    }
    let mut outcome = StopOutcome::default();
    for daemon in daemons {
        let selected = match target {
            Target::Tab(tab) => daemon.key == tab_key(tab),
            Target::All => true,
            Target::Orphans => daemon.tab_open == Some(false),
        };
        if !selected {
            continue;
        }
        let orphan = daemon.tab_open == Some(false);
        if !force && !orphan && daemon.has_pending_work() {
            outcome.refused.push(format!(
                "not stopping {}: it has {} unsaved buffer(s) and {} pending comment(s) \
                 (use --force to discard them)",
                daemon.name(),
                daemon.unsaved_buffers.unwrap_or(0),
                daemon.pending_comments.unwrap_or(0),
            ));
            continue;
        }
        stop_tab_key(&daemon.key, sidebar)?;
        outcome.stopped.push(match daemon.rss_bytes {
            Some(rss) => format!("stopped {} (freed {})", daemon.name(), format_bytes(rss)),
            None => format!("stopped {}", daemon.name()),
        });
    }
    Ok(outcome)
}

pub fn daemons_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let command = parse_args(&args)?;
    let sidebar = crate::config::load().sidebar;
    let mut herdr = CliHerdr;
    match command {
        Invocation::List { json } => {
            let daemons = collect(&mut herdr, &sidebar)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&daemons)?);
            } else {
                print!("{}", render_table(&daemons));
            }
            Ok(())
        }
        Invocation::Stop { target, force } => {
            let outcome = stop(&mut herdr, &sidebar, &target, force)?;
            for line in &outcome.stopped {
                println!("{line}");
            }
            if outcome.stopped.is_empty() && outcome.refused.is_empty() {
                println!(
                    "{}",
                    if target == Target::Orphans {
                        "no orphaned nvim daemons"
                    } else {
                        "no nvim daemons running"
                    }
                );
            }
            if !outcome.refused.is_empty() {
                bail!("{}", outcome.refused.join("\n"));
            }
            Ok(())
        }
    }
}

#[derive(Debug, PartialEq)]
enum Invocation {
    List { json: bool },
    Stop { target: Target, force: bool },
}

fn parse_args(args: &[String]) -> Result<Invocation> {
    let (stop, rest) = match args.first().map(String::as_str) {
        Some("stop") => (true, &args[1..]),
        _ => (false, args),
    };
    let (mut json, mut force, mut all, mut orphans) = (false, false, false, false);
    let mut tabs = Vec::new();
    for arg in rest {
        match arg.as_str() {
            "--json" if !stop => json = true,
            "--force" if stop => force = true,
            "--all" if stop => all = true,
            "--orphans" if stop => orphans = true,
            other if stop && !other.starts_with('-') => tabs.push(other.to_owned()),
            other => bail!("unexpected argument {other:?}\n{USAGE}"),
        }
    }
    if !stop {
        return Ok(Invocation::List { json });
    }
    let target = match (tabs.as_slice(), all, orphans) {
        ([tab], false, false) => Target::Tab(tab.clone()),
        ([], true, false) => Target::All,
        ([], false, true) => Target::Orphans,
        _ => bail!("daemons stop needs exactly one of <tab-id>, --all, --orphans\n{USAGE}"),
    };
    Ok(Invocation::Stop { target, force })
}

fn render_table(daemons: &[DaemonInfo]) -> String {
    if daemons.is_empty() {
        return "no nvim daemons running\n".to_owned();
    }
    let dash = || "—".to_owned();
    let mut rows = vec![[
        "TAB".to_owned(),
        "WORKSPACE / TAB".to_owned(),
        "RAM".to_owned(),
        "UP".to_owned(),
        "STATE".to_owned(),
    ]];
    for daemon in daemons {
        let mut state = daemon.state.as_str().to_owned();
        if let Some(n @ 1..) = daemon.unsaved_buffers {
            state.push_str(&format!(" · {n} unsaved"));
        }
        if let Some(n @ 1..) = daemon.pending_comments {
            state.push_str(&format!(" · {n} comment(s)"));
        }
        rows.push([
            daemon.tab_id.clone(),
            daemon.display_name().unwrap_or_else(dash),
            daemon.rss_bytes.map_or_else(dash, format_bytes),
            daemon.uptime_secs.map_or_else(dash, format_uptime),
            state,
        ]);
    }
    let mut widths = [0usize; 4];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        for (width, cell) in widths.iter().zip(row) {
            out.push_str(&format!("{cell:<width$}   "));
        }
        out.push_str(&row[4]);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&summary(daemons));
    out.push('\n');
    out
}

/// `3 daemons · 2.2 GB total`.
fn summary(daemons: &[DaemonInfo]) -> String {
    let total: u64 = daemons.iter().filter_map(|d| d.rss_bytes).sum();
    let noun = if daemons.len() == 1 {
        "daemon"
    } else {
        "daemons"
    };
    format!("{} {noun} · {} total", daemons.len(), format_bytes(total))
}

/// One-line daemon overview for `herdr-nvim doctor`.
pub(crate) fn doctor_summary(h: &mut dyn Herdr, sidebar: &Sidebar) -> String {
    let daemons = match collect(h, sidebar) {
        Ok(daemons) => daemons,
        Err(error) => return format!("could not list daemons: {error:#}"),
    };
    if daemons.is_empty() {
        return "no nvim daemons running".to_owned();
    }
    let count = |state| daemons.iter().filter(|d| d.state == state).count();
    let mut line = summary(&daemons);
    for state in [
        DaemonState::Orphaned,
        DaemonState::Unresponsive,
        DaemonState::Unknown,
    ] {
        match count(state) {
            0 => {}
            n => line.push_str(&format!(" · {n} {}", state.as_str())),
        }
    }
    line.push_str(" (details: herdr-nvim daemons)");
    line
}

fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes / MB)
    } else {
        format!("{:.0} KB", bytes / 1024.0)
    }
}

fn format_uptime(secs: u64) -> String {
    let (days, hours, mins) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

struct Proc {
    ppid: u32,
    rss_bytes: u64,
    uptime_secs: u64,
}

/// One snapshot of the OS process table: enough to sum a daemon's memory
/// together with its descendants' (LSP servers, jobs) and read its age.
pub(crate) struct ProcessTable {
    procs: HashMap<u32, Proc>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcessTable {
    /// `None` when the process table cannot be read; callers show "—".
    pub(crate) fn load() -> Option<Self> {
        let procs = read_processes()?;
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&pid, proc_) in &procs {
            if proc_.ppid != pid {
                children.entry(proc_.ppid).or_default().push(pid);
            }
        }
        Some(Self { procs, children })
    }

    /// `pid` and all of its descendants that are still running.
    pub(crate) fn tree(&self, pid: u32) -> Vec<u32> {
        let mut seen = vec![pid];
        let mut index = 0;
        while index < seen.len() {
            for &child in self.children.get(&seen[index]).into_iter().flatten() {
                // Guards against ppid cycles from pid reuse (Windows).
                if !seen.contains(&child) {
                    seen.push(child);
                }
            }
            index += 1;
        }
        seen
    }

    pub(crate) fn tree_rss(&self, pid: u32) -> Option<u64> {
        self.procs.get(&pid)?;
        Some(
            self.tree(pid)
                .iter()
                .filter_map(|pid| self.procs.get(pid))
                .map(|proc_| proc_.rss_bytes)
                .sum(),
        )
    }

    #[cfg(test)]
    pub(crate) fn rss(&self, pid: u32) -> Option<u64> {
        self.procs.get(&pid).map(|proc_| proc_.rss_bytes)
    }

    fn uptime(&self, pid: u32) -> Option<u64> {
        self.procs.get(&pid).map(|proc_| proc_.uptime_secs)
    }
}

/// Unix: one `ps` call; `rss` is in KiB, `etime` is `[[dd-]hh:]mm:ss`.
#[cfg(not(windows))]
fn read_processes() -> Option<HashMap<u32, Proc>> {
    let output = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss=,etime="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(
        text.lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse().ok()?;
                let ppid = fields.next()?.parse().ok()?;
                let rss_kib: u64 = fields.next()?.parse().ok()?;
                let uptime_secs = parse_etime(fields.next()?)?;
                Some((
                    pid,
                    Proc {
                        ppid,
                        rss_bytes: rss_kib * 1024,
                        uptime_secs,
                    },
                ))
            })
            .collect(),
    )
}

/// Windows: one PowerShell `Get-CimInstance Win32_Process` call (slow-ish,
/// ~1s, but only run by this manual command); working set stands in for RSS.
#[cfg(windows)]
fn read_processes() -> Option<HashMap<u32, Proc>> {
    let script = "$now = Get-Date; Get-CimInstance Win32_Process | ForEach-Object { \
                  $up = if ($_.CreationDate) { [int64]($now - $_.CreationDate).TotalSeconds } else { 0 }; \
                  \"$($_.ProcessId) $($_.ParentProcessId) $($_.WorkingSetSize) $up\" }";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(
        text.lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse().ok()?;
                let ppid = fields.next()?.parse().ok()?;
                let rss_bytes = fields.next()?.parse().ok()?;
                let uptime_secs = fields.next()?.parse().ok()?;
                Some((
                    pid,
                    Proc {
                        ppid,
                        rss_bytes,
                        uptime_secs,
                    },
                ))
            })
            .collect(),
    )
}

/// `ps` elapsed time `[[dd-]hh:]mm:ss` in seconds.
#[cfg_attr(windows, allow(dead_code))]
fn parse_etime(etime: &str) -> Option<u64> {
    let (days, clock) = match etime.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, etime),
    };
    let mut secs = 0;
    for part in clock.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::{
        config::Config,
        daemon::tests::{
            nvim_available, process_alive, RuntimeEnvGuard, StateEnvGuard, TestDaemon,
        },
        herdr::MockHerdr,
    };

    fn tab(tab_id: &str, workspace: &str, label: &str) -> TabInfo {
        TabInfo {
            tab_id: tab_id.to_owned(),
            workspace_id: tab_id.split(':').next().unwrap().to_owned(),
            workspace_label: Some(workspace.to_owned()),
            tab_label: Some(label.to_owned()),
            tab_number: None,
        }
    }

    /// A herdr whose snapshot has tabs `a` and `b` open but not `c`; every
    /// query answers the same (`calls` of them).
    fn herdr_with_tabs(calls: usize) -> MockHerdr {
        let tabs = vec![
            tab("wHnDa:t1", "novu", "api"),
            tab("wHnDb:t1", "herdr-nvim", "2"),
            tab("wHnOther:t1", "other", "1"),
        ];
        MockHerdr {
            tab_infos_results: (0..calls).map(|_| Ok(tabs.clone())).collect(),
            ..Default::default()
        }
    }

    fn row<'a>(daemons: &'a [DaemonInfo], tab_id: &str) -> &'a DaemonInfo {
        daemons
            .iter()
            .find(|d| d.tab_id == tab_id)
            .unwrap_or_else(|| panic!("no row for {tab_id}"))
    }

    // The real scenario from issue #34: several hidden daemons, one holding
    // memory through a child process, one with unsaved work, one whose tab
    // is gone. List them, then free them by hand without losing work.
    #[test]
    fn lists_and_stops_real_daemons_by_hand() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let state = StateEnvGuard::new();
        let runtime = RuntimeEnvGuard::new();
        state.point_into(&runtime);
        let config = Config::default();
        let sidebar = &config.sidebar;

        let heavy = TestDaemon::spawn("wHnDa:t1", &config);
        let dirty = TestDaemon::spawn("wHnDb:t1", &config);
        let orphan = TestDaemon::spawn("wHnDc:t1", &config);

        // A child process standing in for an LSP server, plus a pending
        // review comment in the same daemon.
        #[cfg(not(windows))]
        let job = "jobstart(['sleep', '300'])";
        #[cfg(windows)]
        let job = "jobstart(['ping', '-n', '300', '127.0.0.1'])";
        let child: u32 = remote_expr(&heavy.socket, &format!("jobpid({job})"), sidebar)
            .expect("start child job")
            .parse()
            .expect("child pid");
        let child_guard = KillOnDrop(child);
        remote_expr(
            &heavy.socket,
            "luaeval(\"require('herdr-nvim.comments').add(vim.api.nvim_get_current_buf(), 1, 1, 'look')\")",
            sidebar,
        )
        .expect("add a comment");
        remote_expr(
            &dirty.socket,
            r#"execute('setlocal noswapfile | call setline(1, "unsaved")')"#,
            sidebar,
        )
        .expect("dirty a buffer");

        let mut herdr = herdr_with_tabs(8);
        let daemons = collect(&mut herdr, sidebar).unwrap();
        assert_eq!(daemons.len(), 3);

        let heavy_row = row(&daemons, "wHnDa:t1");
        assert_eq!(heavy_row.state, DaemonState::Alive);
        assert_eq!(heavy_row.display_name().as_deref(), Some("novu / api"));
        assert_eq!(
            heavy_row.pid.map(|p| p.to_string()),
            Some(heavy.pid.clone())
        );
        assert_eq!(heavy_row.pending_comments, Some(1));
        assert_eq!(heavy_row.unsaved_buffers, Some(0));
        assert!(heavy_row.uptime_secs.is_some());
        let procs = ProcessTable::load().expect("process table");
        let heavy_pid = heavy_row.pid.unwrap();
        assert!(
            procs.tree(heavy_pid).contains(&child),
            "child job not counted as the daemon's descendant"
        );
        assert!(heavy_row.rss_bytes.is_some_and(|rss| rss > 0));
        let own = procs.rss(heavy_pid).unwrap();
        let child_rss = procs.rss(child).unwrap();
        assert!(child_rss > 0);
        assert!(procs.tree_rss(heavy_pid).unwrap() >= own + child_rss);

        let dirty_row = row(&daemons, "wHnDb:t1");
        assert_eq!(dirty_row.state, DaemonState::Alive);
        assert_eq!(dirty_row.unsaved_buffers, Some(1));
        assert_eq!(
            dirty_row.pid.map(|p| p.to_string()),
            Some(dirty.pid.clone())
        );

        let orphan_row = row(&daemons, "wHnDc:t1");
        assert_eq!(orphan_row.state, DaemonState::Orphaned);
        assert_eq!(orphan_row.display_name(), None);

        let table = render_table(&daemons);
        assert!(table.contains("novu / api"), "{table}");
        assert!(table.contains("3 daemons · "), "{table}");
        assert!(
            table
                .lines()
                .any(|l| l.starts_with("wHnDc:t1") && l.contains("—") && l.contains("orphaned")),
            "{table}"
        );

        // herdr unreachable: still listed, names unknown, never orphaned.
        let mut unreachable = MockHerdr {
            tab_infos_results: VecDeque::from([Err(anyhow::anyhow!("no herdr"))]),
            ..Default::default()
        };
        let blind = collect(&mut unreachable, sidebar).unwrap();
        assert!(blind.iter().all(|d| d.state == DaemonState::Unknown));
        assert!(blind.iter().all(|d| d.display_name().is_none()));

        // Unsaved work refuses a plain stop...
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDb:t1".into()), false).unwrap();
        assert!(outcome.stopped.is_empty());
        assert!(
            outcome.refused[0].contains("1 unsaved buffer"),
            "{outcome:?}"
        );
        dirty.assert_alive(sidebar);
        // ...but not a forced one.
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDb:t1".into()), true).unwrap();
        assert_eq!(outcome.stopped.len(), 1);
        assert!(
            outcome.stopped[0].starts_with("stopped herdr-nvim / 2"),
            "{outcome:?}"
        );
        dirty.assert_gone(sidebar);
        heavy.assert_alive(sidebar);

        // Orphans go without --force; open tabs stay.
        let outcome = stop(&mut herdr, sidebar, &Target::Orphans, false).unwrap();
        assert_eq!(outcome.stopped.len(), 1, "{outcome:?}");
        orphan.assert_gone(sidebar);
        heavy.assert_alive(sidebar);

        // Key form works; the pending comment protects it until --force, and
        // stopping it takes its child process down too.
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa_t1".into()), false).unwrap();
        assert!(
            outcome.refused[0].contains("1 pending comment"),
            "{outcome:?}"
        );
        heavy.assert_alive(sidebar);
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa_t1".into()), true).unwrap();
        assert!(
            outcome.stopped[0].starts_with("stopped novu / api"),
            "{outcome:?}"
        );
        heavy.assert_gone(sidebar);
        wait_dead(child);
        assert!(
            !process_alive(&child.to_string()),
            "child job outlived its daemon"
        );
        drop(child_guard);

        assert!(collect(&mut herdr, sidebar).unwrap().is_empty());
        let error = stop(&mut herdr, sidebar, &Target::Tab("wHnDa:t1".into()), true).unwrap_err();
        assert!(error
            .to_string()
            .contains("no nvim daemon for tab wHnDa:t1"));
    }

    /// Kills a test's child process if an assertion fails before its daemon
    /// takes it down.
    struct KillOnDrop(u32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            #[cfg(not(windows))]
            let _ = std::process::Command::new("kill")
                .arg(self.0.to_string())
                .output();
            #[cfg(windows)]
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/PID", &self.0.to_string()])
                .output();
        }
    }

    /// nvim stops its jobs on exit, but asynchronously; give it a moment.
    fn wait_dead(pid: u32) {
        for _ in 0..40 {
            if !process_alive(&pid.to_string()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn stop_arguments_need_exactly_one_target() {
        let parse =
            |args: &[&str]| parse_args(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        assert_eq!(parse(&[]).unwrap(), Invocation::List { json: false });
        assert_eq!(parse(&["--json"]).unwrap(), Invocation::List { json: true });
        assert_eq!(
            parse(&["stop", "w1_t1", "--force"]).unwrap(),
            Invocation::Stop {
                target: Target::Tab("w1_t1".into()),
                force: true
            }
        );
        for bad in [
            &["stop"][..],
            &["stop", "w1:t1", "--all"],
            &["stop", "--all", "--orphans"],
            &["stop", "w1:t1", "w2:t1"],
            &["--force"],
            &["stop", "--json", "--all"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn human_sizes_and_durations() {
        assert_eq!(format_bytes(2_254_857_830), "2.1 GB");
        assert_eq!(format_bytes(48 * 1024 * 1024), "48 MB");
        assert_eq!(format_bytes(900 * 1024), "900 KB");
        assert_eq!(format_uptime(3 * 3600 + 31 * 60 + 5), "3h 31m");
        assert_eq!(format_uptime(5 * 3600 + 2 * 60), "5h 02m");
        assert_eq!(format_uptime(12 * 60), "12m");
        assert_eq!(format_uptime(40), "40s");
        assert_eq!(format_uptime(2 * 86_400 + 4 * 3600), "2d 4h");
        assert_eq!(parse_etime("05:07"), Some(307));
        assert_eq!(parse_etime("03:31:00"), Some(12_660));
        assert_eq!(parse_etime("2-00:00:01"), Some(172_801));
        assert_eq!(parse_etime("garbage"), None);
    }
}
