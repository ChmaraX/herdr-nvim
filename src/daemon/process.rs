//! One snapshot of the OS process table, shared by everything that needs to
//! look at a daemon as a process rather than over its socket: its memory
//! (with its LSPs and jobs) and age for `herdr-nvim daemons`.

use std::{collections::HashMap, process::Command};

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

    pub(crate) fn uptime(&self, pid: u32) -> Option<u64> {
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
    use super::*;

    #[test]
    fn parses_ps_elapsed_time() {
        assert_eq!(parse_etime("05:07"), Some(307));
        assert_eq!(parse_etime("03:31:00"), Some(12_660));
        assert_eq!(parse_etime("2-00:00:01"), Some(172_801));
        assert_eq!(parse_etime("garbage"), None);
    }
}
