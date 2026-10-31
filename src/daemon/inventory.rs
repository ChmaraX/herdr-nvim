//! `herdr-nvim daemons` -- list the hidden per-tab nvim daemons (which herdr
//! tab each belongs to, how much memory it and its children such as LSP
//! servers hold, how long it has run) and stop one by hand without closing
//! its tab.
//!
//! Daemons are enumerated from the same runtime-dir registry that `gc` and
//! the close hooks use (`registry::daemon_keys`), queried over their own
//! socket for pid / unsaved buffers / pending comments / tab id, named
//! through `herdr api snapshot`, and measured with one OS process-table
//! snapshot. Stopping goes through the shared `registry::stop_probed`.

use std::{env, thread};

use anyhow::{bail, Context, Result};
use serde::{Serialize, Serializer};

use crate::{
    config::Sidebar,
    herdr::{CliHerdr, Herdr, TabInfo},
    state::{tab_key, TabId},
};

use super::{
    liveness,
    process::ProcessTable,
    registry::{daemon_keys, orphans, probe, socket_path_for_key, stop_probed, Probe},
    Liveness, Reply,
};

const USAGE: &str = "usage: herdr-nvim daemons [--json]\n       \
                     herdr-nvim daemons stop <tab-id> [--force]\n       \
                     herdr-nvim daemons stop --all [--force]\n       \
                     herdr-nvim daemons stop --orphans";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DaemonState {
    /// Answers on its socket and its tab is open in herdr.
    Alive,
    /// Answers on its socket but its tab is gone from herdr.
    Orphaned,
    /// Its process holds the socket but does not answer in time: busy or
    /// hung. Stopping it means killing it.
    Hung,
    /// Registered, but nothing listens on its socket: it died without
    /// cleaning up. Stopping it only removes the entry.
    Stale,
    /// Answers, but herdr could not be asked whether its tab still exists.
    Unknown,
}

impl DaemonState {
    /// The one source of the state names, for the table and the JSON alike.
    fn as_str(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::Orphaned => "orphaned",
            Self::Hung => "unresponsive",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

impl Serialize for DaemonState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One registered daemon.
#[derive(Clone, Debug)]
pub(crate) struct DaemonInfo {
    /// Sanitized tab key (the runtime-dir registry entry's stem).
    pub key: String,
    /// herdr's tab for this daemon: `None` if the tab is gone or herdr could
    /// not be asked (see `herdr_reachable`).
    pub tab: Option<TabInfo>,
    pub herdr_reachable: bool,
    /// What the daemon answered to the probe, if anything.
    pub probe: Liveness<Probe>,
    /// The daemon's process: as it reported itself, or, if it does not
    /// answer, the one process listening on its socket.
    pub pid: Option<u32>,
    /// Resident memory of the daemon plus all of its descendants (LSPs, jobs).
    pub rss_bytes: Option<u64>,
    pub uptime_secs: Option<u64>,
}

impl DaemonInfo {
    pub(crate) fn state(&self) -> DaemonState {
        match (&self.probe, &self.tab, self.herdr_reachable) {
            (Liveness::Unresponsive, _, _) => DaemonState::Hung,
            (Liveness::Gone, _, _) => DaemonState::Stale,
            (Liveness::Answered(_), Some(_), _) => DaemonState::Alive,
            (Liveness::Answered(_), None, true) => DaemonState::Orphaned,
            (Liveness::Answered(_), None, false) => DaemonState::Unknown,
        }
    }

    /// Why a plain `daemons stop` leaves this daemon running, if it does:
    /// it holds unsaved buffers or pending comments, or it is hung, so it
    /// can only be killed, losing whatever it holds. Never when its tab is
    /// gone: whatever it held went with the tab.
    fn refusal(&self) -> Option<String> {
        if self.tab_gone() {
            return None;
        }
        let name = self.name();
        match self.state() {
            DaemonState::Hung => Some(format!(
                "not stopping {name}: it is busy or hung, so it can only be killed, \
                 losing any unsaved work in it (use --force to kill it)"
            )),
            _ if self.has_pending_work() => Some(format!(
                "not stopping {name}: it has {} unsaved buffer(s) and {} pending comment(s) \
                 (use --force to discard them)",
                self.unsaved().unwrap_or(0),
                self.comments().unwrap_or(0),
            )),
            _ => None,
        }
    }

    /// herdr confirmed the tab no longer exists (responsive or not).
    fn tab_gone(&self) -> bool {
        self.herdr_reachable && self.tab.is_none()
    }

    /// The tab id, from the daemon itself or from herdr. `None` for an
    /// unresponsive daemon whose tab herdr does not know: a key cannot be
    /// decoded back into an id.
    fn tab_id(&self) -> Option<&TabId> {
        self.probe
            .answered()
            .and_then(|probe| probe.tab_id.as_ref())
            .or(self.tab.as_ref().map(|tab| &tab.tab_id))
    }

    /// The tab id if known, else the key -- either works for `daemons stop`.
    fn tab_ref(&self) -> &str {
        self.tab_id().map_or(&self.key, TabId::as_str)
    }

    fn unsaved(&self) -> Option<u32> {
        self.probe.answered().map(|probe| probe.unsaved)
    }

    fn comments(&self) -> Option<u32> {
        self.probe.answered().map(|probe| probe.comments)
    }

    /// `workspace / tab` as herdr shows it, or `None` for a tab herdr does
    /// not (or could not be asked to) know.
    fn display_name(&self) -> Option<String> {
        let tab = self.tab.as_ref()?;
        let workspace = tab
            .workspace_label
            .clone()
            .unwrap_or_else(|| tab.tab_id.workspace().to_owned());
        let name = tab
            .tab_label
            .clone()
            .or_else(|| tab.tab_number.map(|number| number.to_string()))
            .unwrap_or_else(|| tab.tab_id.to_string());
        Some(format!("{workspace} / {name}"))
    }

    /// What to call the daemon in stop messages.
    fn name(&self) -> String {
        self.display_name()
            .unwrap_or_else(|| self.tab_ref().to_owned())
    }

    fn has_pending_work(&self) -> bool {
        self.unsaved().unwrap_or(0) > 0 || self.comments().unwrap_or(0) > 0
    }
}

/// `--json` shape: one flat object per daemon, state and names derived.
impl Serialize for DaemonInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Row<'a> {
            tab_id: Option<&'a TabId>,
            key: &'a str,
            workspace_label: Option<&'a str>,
            tab_label: Option<&'a str>,
            tab_number: Option<u64>,
            pid: Option<u32>,
            rss_bytes: Option<u64>,
            uptime_secs: Option<u64>,
            state: DaemonState,
            unsaved_buffers: Option<u32>,
            pending_comments: Option<u32>,
        }
        let tab = self.tab.as_ref();
        Row {
            tab_id: self.tab_id(),
            key: &self.key,
            workspace_label: tab.and_then(|tab| tab.workspace_label.as_deref()),
            tab_label: tab.and_then(|tab| tab.tab_label.as_deref()),
            tab_number: tab.and_then(|tab| tab.tab_number),
            pid: self.pid,
            rss_bytes: self.rss_bytes,
            uptime_secs: self.uptime_secs,
            state: self.state(),
            unsaved_buffers: self.unsaved(),
            pending_comments: self.comments(),
        }
        .serialize(serializer)
    }
}

/// Every registered daemon, sorted by key. herdr being unreachable is not
/// an error: names are then unknown and responsive daemons are `Unknown`.
pub(crate) fn collect(h: &mut dyn Herdr, sidebar: &Sidebar) -> Result<Vec<DaemonInfo>> {
    let keys = daemon_keys()?;
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    describe(keys, h.tab_infos().ok().as_deref(), sidebar)
}

/// Probe and measure the daemons for `keys`, naming them from herdr's live
/// `tabs` (`None` when herdr could not be asked). The probes run in
/// parallel, so hung daemons cost one `RPC_TIMEOUT` in all, not one each.
/// A daemon that refuses connections while its process still listens is
/// hung, not stale (see `liveness`).
fn describe(
    keys: Vec<String>,
    tabs: Option<&[TabInfo]>,
    sidebar: &Sidebar,
) -> Result<Vec<DaemonInfo>> {
    let procs = ProcessTable::load();
    let probes: Vec<Result<Reply<Probe>>> = thread::scope(|scope| {
        let probes: Vec<_> = keys
            .iter()
            .map(|key| scope.spawn(|| probe(key, sidebar)))
            .collect();
        probes
            .into_iter()
            .map(|probe| {
                probe
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    keys.into_iter()
        .zip(probes)
        .map(|(key, probe)| {
            let socket = socket_path_for_key(&key);
            let probe = liveness(&socket, probe?, procs.as_ref());
            let tab = tabs
                .and_then(|tabs| tabs.iter().find(|tab| tab.tab_id.key() == key))
                .cloned();
            let pid = match (probe.answered(), &procs) {
                (Some(answer), _) => Some(answer.pid),
                (None, Some(procs)) => procs.daemon(&socket).ok(),
                (None, None) => None,
            };
            let (rss_bytes, uptime_secs) = match (pid, &procs) {
                (Some(pid), Some(procs)) => (procs.tree_rss(pid), procs.uptime(pid)),
                _ => (None, None),
            };
            Ok(DaemonInfo {
                key,
                tab,
                herdr_reachable: tabs.is_some(),
                probe,
                pid,
                rss_bytes,
                uptime_secs,
            })
        })
        .collect()
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
    /// One explanation per daemon that failed to stop.
    pub failed: Vec<String>,
}

/// Stop the targeted daemons. One that `DaemonInfo::refusal` protects is
/// left running unless `force`. `--orphans` needs herdr: without it, every
/// daemon would look orphaned.
pub(crate) fn stop(
    h: &mut dyn Herdr,
    sidebar: &Sidebar,
    target: &Target,
    force: bool,
) -> Result<StopOutcome> {
    let keys = daemon_keys()?;
    let (selected, tabs) = match target {
        Target::Tab(tab) => {
            let key = tab_key(tab);
            if !keys.contains(&key) {
                bail!("no nvim daemon for tab {tab}");
            }
            (vec![key], h.tab_infos().ok())
        }
        Target::All => (keys, h.tab_infos().ok()),
        Target::Orphans => {
            let tabs = h
                .tab_infos()
                .context("cannot tell which daemons are orphaned: herdr is unreachable")?;
            (orphans(&keys, &tabs), Some(tabs))
        }
    };
    let mut outcome = StopOutcome::default();
    let mut doomed = Vec::new();
    for daemon in describe(selected, tabs.as_deref(), sidebar)? {
        match (force, daemon.refusal()) {
            (false, Some(refusal)) => outcome.refused.push(refusal),
            _ => doomed.push(daemon),
        }
    }
    for daemon in doomed {
        // Reuses the probe above instead of asking (and, for a hung daemon,
        // waiting) a second time.
        match (
            stop_probed(&daemon.key, daemon.probe.clone(), sidebar),
            daemon.rss_bytes,
        ) {
            (Ok(()), Some(rss)) => outcome.stopped.push(format!(
                "stopped {} (freed {})",
                daemon.name(),
                format_bytes(rss)
            )),
            (Ok(()), None) => outcome.stopped.push(format!("stopped {}", daemon.name())),
            (Err(error), _) => outcome
                .failed
                .push(format!("could not stop {}: {error:#}", daemon.name())),
        }
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
            let problems: Vec<&String> = outcome.refused.iter().chain(&outcome.failed).collect();
            if outcome.stopped.is_empty() && problems.is_empty() {
                println!(
                    "{}",
                    if target == Target::Orphans {
                        "no orphaned nvim daemons"
                    } else {
                        "no nvim daemons running"
                    }
                );
            }
            if !problems.is_empty() {
                bail!(
                    "{}",
                    problems
                        .iter()
                        .map(|line| line.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                );
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
        let mut state = daemon.state().as_str().to_owned();
        if let Some(n @ 1..) = daemon.unsaved() {
            state.push_str(&format!(" · {n} unsaved"));
        }
        if let Some(n @ 1..) = daemon.comments() {
            state.push_str(&format!(" · {n} comment(s)"));
        }
        rows.push([
            daemon.tab_ref().to_owned(),
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
    out.push_str(&hints(daemons));
    out
}

/// How to act on the listing, so `herdr-nvim daemons` is the only command
/// to remember. The example tab is the heaviest daemon.
fn hints(daemons: &[DaemonInfo]) -> String {
    let example = daemons
        .iter()
        .max_by_key(|daemon| daemon.rss_bytes.unwrap_or(0))
        .map_or("<TAB>", DaemonInfo::tab_ref);
    let mut out = format!(
        "stop one: herdr-nvim daemons stop {example}   (--force discards unsaved work)\n\
         stop all: herdr-nvim daemons stop --all\n"
    );
    if daemons.iter().any(DaemonInfo::tab_gone) {
        out.push_str("orphans:  herdr-nvim daemons stop --orphans   (safe: their tabs are gone)\n");
    }
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
    let count = |state| daemons.iter().filter(|d| d.state() == state).count();
    let mut line = summary(&daemons);
    for state in [
        DaemonState::Orphaned,
        DaemonState::Hung,
        DaemonState::Stale,
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    #[cfg(unix)]
    use crate::daemon::{remote_expr, RPC_TIMEOUT};
    use crate::{
        config::Config,
        herdr::MockHerdr,
        test_support::{kill, nvim_available, process_alive, wait_dead, TestDaemon, TestEnv},
    };

    fn tab(tab_id: &str, workspace: &str, label: &str) -> TabInfo {
        TabInfo {
            tab_id: TabId::new(tab_id),
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

    fn unreachable_herdr() -> MockHerdr {
        MockHerdr {
            tab_infos_results: VecDeque::from([Err(anyhow::anyhow!("no herdr"))]),
            ..Default::default()
        }
    }

    fn row<'a>(daemons: &'a [DaemonInfo], tab_id: &str) -> &'a DaemonInfo {
        daemons
            .iter()
            .find(|d| d.tab_ref() == tab_id)
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
        let _env = TestEnv::new();
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
        let child: u32 = heavy
            .eval(&format!("jobpid({job})"))
            .parse()
            .expect("child pid");
        let child_guard = KillOnDrop(child);
        heavy.eval(
            "luaeval(\"require('herdr-nvim.comments').add(vim.api.nvim_get_current_buf(), { start_line = 1, end_line = 1 }, 'look')\")",
        );
        dirty.dirty_a_buffer();

        let mut herdr = herdr_with_tabs(8);
        let daemons = collect(&mut herdr, sidebar).unwrap();
        assert_eq!(daemons.len(), 3);

        let heavy_row = row(&daemons, "wHnDa:t1");
        assert_eq!(heavy_row.state(), DaemonState::Alive);
        assert_eq!(heavy_row.display_name().as_deref(), Some("novu / api"));
        assert_eq!(heavy_row.pid, Some(heavy.pid));
        assert_eq!(heavy_row.comments(), Some(1));
        assert_eq!(heavy_row.unsaved(), Some(0));
        assert!(heavy_row.uptime_secs.is_some());
        let procs = ProcessTable::load().expect("process table");
        assert!(
            procs.tree(heavy.pid).contains(&child),
            "child job not counted as the daemon's descendant"
        );
        // The child's memory is part of the daemon's total.
        let own = procs.rss(heavy.pid).unwrap();
        assert!(heavy_row.rss_bytes.is_some_and(|rss| rss > own));

        let dirty_row = row(&daemons, "wHnDb:t1");
        assert_eq!(dirty_row.state(), DaemonState::Alive);
        assert_eq!(dirty_row.unsaved(), Some(1));

        // Its tab id comes from the daemon itself, not from herdr.
        let orphan_row = row(&daemons, "wHnDc:t1");
        assert_eq!(orphan_row.state(), DaemonState::Orphaned);
        assert_eq!(orphan_row.display_name(), None);

        let json = serde_json::to_value(&daemons).unwrap();
        assert_eq!(json[0]["tab_id"], "wHnDa:t1");
        assert_eq!(json[0]["state"], "alive");
        assert_eq!(json[0]["workspace_label"], "novu");
        assert_eq!(json[2]["state"], "orphaned");

        let table = render_table(&daemons);
        assert!(table.contains("novu / api"), "{table}");
        assert!(
            table
                .lines()
                .any(|l| l.starts_with("wHnDc:t1") && l.contains("—") && l.contains("orphaned")),
            "{table}"
        );
        // The hints name real commands, and --orphans only with an orphan.
        assert!(table.contains("daemons stop wHnD"), "{table}");
        assert!(table.contains("daemons stop --all"), "{table}");
        assert!(table.contains("daemons stop --orphans"), "{table}");
        let without_orphan = render_table(&daemons[..2]);
        assert!(without_orphan.contains("daemons stop --all"));
        assert!(!without_orphan.contains("--orphans"), "{without_orphan}");

        // herdr unreachable: still listed, names unknown, never orphaned --
        // and `--orphans` refuses rather than treating every tab as gone.
        let blind = collect(&mut unreachable_herdr(), sidebar).unwrap();
        assert!(blind.iter().all(|d| d.state() == DaemonState::Unknown));
        assert!(blind.iter().all(|d| d.display_name().is_none()));
        assert!(!render_table(&blind).contains("--orphans"));
        assert!(stop(&mut unreachable_herdr(), sidebar, &Target::Orphans, false).is_err());
        orphan.assert_alive();

        // Unsaved work refuses a plain stop...
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDb:t1".into()), false).unwrap();
        assert_eq!((outcome.stopped.len(), outcome.refused.len()), (0, 1));
        dirty.assert_alive();
        // ...but not a forced one.
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDb:t1".into()), true).unwrap();
        assert_eq!(outcome.stopped.len(), 1);
        dirty.assert_gone();
        heavy.assert_alive();

        // Orphans go without --force; open tabs stay.
        let outcome = stop(&mut herdr, sidebar, &Target::Orphans, false).unwrap();
        assert_eq!(outcome.stopped.len(), 1, "{outcome:?}");
        orphan.assert_gone();
        heavy.assert_alive();

        // Key form works; the pending comment protects it until --force, and
        // stopping it takes its child process down too.
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa_t1".into()), false).unwrap();
        assert_eq!(outcome.refused.len(), 1);
        heavy.assert_alive();
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa_t1".into()), true).unwrap();
        assert_eq!((outcome.stopped.len(), outcome.failed.len()), (1, 0));
        heavy.assert_gone();
        wait_dead(child);
        assert!(!process_alive(child), "child job outlived its daemon");
        drop(child_guard);

        assert!(collect(&mut herdr, sidebar).unwrap().is_empty());
        assert!(stop(&mut herdr, sidebar, &Target::Tab("wHnDa:t1".into()), true).is_err());
    }

    // Issue #61: a daemon that holds its socket but answers nothing (here
    // SIGSTOPped). Listing must not hang and must show it as unresponsive
    // with its process; a plain stop must refuse to kill it, without
    // forgetting it; --force must kill exactly that process and clean up.
    #[cfg(unix)]
    #[test]
    fn a_hung_daemon_is_listed_then_only_killed_with_force() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let config = Config::default();
        let sidebar = &config.sidebar;
        let hung = TestDaemon::spawn("wHnDa:t1", &config);
        let fine = TestDaemon::spawn("wHnDb:t1", &config);
        hung.hang();
        let mut herdr = herdr_with_tabs(4);
        let bounded = RPC_TIMEOUT + std::time::Duration::from_secs(3);

        let started = std::time::Instant::now();
        let daemons = collect(&mut herdr, sidebar).unwrap();
        assert!(started.elapsed() < bounded, "{:?}", started.elapsed());
        let hung_row = row(&daemons, "wHnDa:t1");
        assert_eq!(hung_row.state(), DaemonState::Hung);
        assert_eq!(hung_row.pid, Some(hung.pid), "found by its --listen argv");
        assert!(hung_row.rss_bytes.is_some());
        assert_eq!(row(&daemons, "wHnDb:t1").state(), DaemonState::Alive);
        let table = render_table(&daemons);
        assert!(
            table
                .lines()
                .any(|l| l.starts_with("wHnDa:t1") && l.ends_with("unresponsive")),
            "{table}"
        );

        let started = std::time::Instant::now();
        let outcome = stop(&mut herdr, sidebar, &Target::All, false).unwrap();
        assert!(started.elapsed() < bounded, "{:?}", started.elapsed());
        assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
        assert!(outcome.refused[0].contains("--force"), "{outcome:?}");
        assert_eq!(outcome.stopped.len(), 1, "the answering daemon still stops");
        fine.assert_gone();
        assert!(process_alive(hung.pid), "a plain stop must not kill it");
        assert_eq!(daemon_keys().unwrap(), [hung.tab.key()], "nor forget it");

        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa:t1".into()), true).unwrap();
        assert_eq!(
            (outcome.stopped.len(), outcome.failed.len()),
            (1, 0),
            "{outcome:?}"
        );
        hung.assert_gone();
    }

    // Issue #61: a hung daemon stops taking connections at all once its
    // listen queue (32 deep) is full of requests that timed out on it -- a
    // few days of toggles and pick-files against a wedged tab. A client then
    // fails just as if nothing listened. It must still count as hung: no
    // twin spawned over it, refused by a plain stop, killed by a forced one.
    #[cfg(unix)]
    #[test]
    fn a_hung_daemon_with_a_full_connection_queue_is_still_hung() {
        use std::time::{Duration, Instant};

        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let config = Config::default();
        let sidebar = &config.sidebar;
        let hung = TestDaemon::spawn("wHnDa:t1", &config);
        hung.hang();
        thread::scope(|scope| {
            for _ in 0..40 {
                scope.spawn(|| remote_expr(&hung.socket, "1", sidebar, Duration::from_secs(2)));
            }
        });
        let started = Instant::now();
        let refused = remote_expr(&hung.socket, "1", sidebar, RPC_TIMEOUT).unwrap();
        assert_eq!(
            (refused, started.elapsed() < Duration::from_secs(2)),
            (Reply::Refused, true),
            "precondition: the full queue refuses a new client at once"
        );

        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let error = crate::daemon::ensure_daemon("wHnDa:t1", &root, &config, &root).unwrap_err();
        assert!(format!("{error:#}").contains("busy or hung"), "{error:#}");
        let procs = ProcessTable::load().expect("process table");
        assert_eq!(procs.listeners(&hung.socket), [hung.pid], "no twin");

        let mut herdr = herdr_with_tabs(3);
        let daemons = collect(&mut herdr, sidebar).unwrap();
        assert_eq!(daemons[0].state(), DaemonState::Hung);
        assert_eq!(daemons[0].pid, Some(hung.pid));
        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa:t1".into()), false).unwrap();
        assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
        assert!(process_alive(hung.pid), "a plain stop must not kill it");
        assert_eq!(daemon_keys().unwrap(), [hung.tab.key()], "nor forget it");

        let outcome = stop(&mut herdr, sidebar, &Target::Tab("wHnDa:t1".into()), true).unwrap();
        assert_eq!(
            (outcome.stopped.len(), outcome.failed.len()),
            (1, 0),
            "{outcome:?}"
        );
        hung.assert_gone();
    }

    /// Kills a test's child process if an assertion fails before its daemon
    /// takes it down.
    struct KillOnDrop(u32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if process_alive(self.0) {
                kill(self.0);
            }
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
    }
}
