//! Profiling, for lag the person feels in the Herdr app. While
//! `<state dir>/profile/on` names a file, every herdr-nvim process appends
//! Chrome trace events to it, one JSON object a line: the process itself
//! (`herdr-nvim toggle`, …) and the steps in it that wait on something (app
//! commands, herdr-core calls, nvim requests, a daemon starting). The app's
//! mods append theirs to the same file (`mods/herdr-nvim/profile.js`): the
//! person's actions and how long they waited, slow renders, freezes of the
//! app's main thread, and the lag marks the person sets.
//!
//! `herdr-nvim app-profile on|off [--report]|report [FILE]|status`. The
//! report is `FILE.trace.json` (ui.perfetto.dev opens it) and
//! `FILE.summary.md` (what was slow, and what ran meanwhile).

use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// The mods' events carry this pid (one track per role); ours, our own.
const MODS_PID: u64 = 1;

fn dir() -> PathBuf {
    crate::state::state_dir().join("profile")
}

fn flag() -> PathBuf {
    dir().join("on")
}

/// The file this process writes to, read once: `None` while profiling is off.
fn file() -> Option<&'static Path> {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| {
        let named = fs::read_to_string(flag()).ok()?;
        let path = PathBuf::from(named.trim());
        (!path.as_os_str().is_empty()).then_some(path)
    })
    .as_deref()
}

pub(crate) fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// One line, appended in one write (lines from several processes don't mix).
fn write(event: Value) {
    let Some(path) = file() else { return };
    if let Ok(mut out) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = out.write_all(format!("{event}\n").as_bytes());
    }
}

/// A step that takes time, written when it ends (dropped). Costs nothing
/// while profiling is off.
pub(crate) struct Span {
    name: String,
    start: Option<u64>,
    args: Value,
}

pub(crate) fn span(name: &str) -> Span {
    let on = file().is_some();
    Span {
        name: if on { name.to_owned() } else { String::new() },
        start: on.then(now_us),
        args: json!({}),
    }
}

impl Span {
    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        if self.start.is_some() {
            self.args[key] = value.into();
        }
        self
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(start) = self.start else { return };
        write(json!({
            "ph": "X", "cat": "rust", "name": self.name, "ts": start,
            "dur": now_us().saturating_sub(start), "pid": std::process::id(), "tid": 0,
            "args": self.args,
        }));
    }
}

static PROCESS: OnceLock<(String, u64)> = OnceLock::new();

/// This process starts: its track is named after its command.
pub(crate) fn process_start(mode: &str) {
    if file().is_none() {
        return;
    }
    let name = format!("herdr-nvim {mode}");
    write(
        json!({"ph": "M", "name": "process_name", "pid": std::process::id(), "args": {"name": name}}),
    );
    let _ = PROCESS.set((name, now_us()));
}

/// This process ends (or becomes another program, `exec`): its whole span.
pub(crate) fn process_end(code: i32) {
    let Some((name, start)) = PROCESS.get() else {
        return;
    };
    let args: Vec<String> = env::args()
        .skip(2)
        .filter(|a| !a.starts_with('{'))
        .map(|a| a.chars().take(80).collect())
        .collect();
    write(json!({
        "ph": "X", "cat": "process", "name": name, "ts": start,
        "dur": now_us().saturating_sub(*start), "pid": std::process::id(), "tid": 0,
        "args": {"code": code, "args": args},
    }));
}

pub fn profile_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--ctx" {
            i += 2;
            continue;
        }
        words.push(args[i].as_str());
        i += 1;
    }
    match words.as_slice() {
        ["on"] => on(),
        ["off"] => off(false),
        ["off", "--report"] => off(true),
        ["report"] => print_report(&newest()?),
        ["report", path] => print_report(Path::new(path)),
        ["status"] | [] => {
            let on = fs::read_to_string(flag()).ok().map(|f| f.trim().to_owned());
            println!("{}", json!({"on": on.is_some(), "file": on}));
            Ok(())
        }
        _ => bail!("usage: herdr-nvim app-profile on | off [--report] | report [FILE] | status"),
    }
}

fn on() -> Result<()> {
    if let Ok(file) = fs::read_to_string(flag()) {
        println!("{}", json!({"file": file.trim()}));
        return Ok(());
    }
    let dir = dir();
    fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let file = dir.join(format!("{}.jsonl", stamp(now_us() / 1_000_000)));
    fs::write(&file, "").with_context(|| format!("cannot write {}", file.display()))?;
    let tmp = dir.join("on.tmp");
    fs::write(&tmp, file.to_string_lossy().as_bytes())?;
    fs::rename(&tmp, flag())?;
    println!("{}", json!({"file": file}));
    Ok(())
}

fn off(report: bool) -> Result<()> {
    let Ok(file) = fs::read_to_string(flag()) else {
        bail!("profiling is off");
    };
    crate::state::remove_file_if_exists(&flag())?;
    let file = PathBuf::from(file.trim());
    if report {
        return print_report(&file);
    }
    println!("{}", json!({"file": file}));
    Ok(())
}

/// The newest profile in the profile folder.
fn newest() -> Result<PathBuf> {
    let mut all: Vec<PathBuf> = fs::read_dir(dir())
        .context("no profiles yet (herdr-nvim app-profile on)")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    all.sort();
    all.pop()
        .context("no profiles yet (herdr-nvim app-profile on)")
}

/// `YYYYMMDD-HHMMSS` (UTC) for a unix time in seconds.
fn stamp(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn print_report(file: &Path) -> Result<()> {
    let text =
        fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let events: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let report = Report::new(&events);
    let trace = file.with_extension("trace.json");
    let summary = file.with_extension("summary.md");
    let mut all = events.clone();
    all.push(json!({"ph": "M", "name": "process_name", "pid": MODS_PID, "args": {"name": "herdr-app (herdr-nvim mods)"}}));
    for (tid, name) in MOD_TRACKS {
        all.push(json!({"ph": "M", "name": "thread_name", "pid": MODS_PID, "tid": tid, "args": {"name": name}}));
    }
    fs::write(
        &trace,
        json!({"traceEvents": all, "displayTimeUnit": "ms"}).to_string(),
    )?;
    fs::write(&summary, report.markdown(file))?;
    println!(
        "{}",
        json!({"file": file, "trace": trace, "summary": summary, "headline": report.headline()})
    );
    Ok(())
}

/// A span (`ph: X`) of the profile, in ms from its first event.
#[derive(Clone)]
struct Step {
    name: String,
    cat: String,
    at: f64,
    ms: f64,
    /// Whose: the mods' part (`mod: tree`), else the process.
    track: String,
}

/// The mods' tracks (profile.js `TID`).
const MOD_TRACKS: [(u64, &str); 8] = [
    (0, "actions"),
    (1, "controller"),
    (2, "tree"),
    (3, "preview"),
    (4, "dialogs"),
    (5, "cmd-e"),
    (6, "pane headers"),
    (7, "waits"),
];

struct Report {
    steps: Vec<Step>,
    marks: Vec<f64>,
    renders: BTreeMap<String, u64>,
    seconds: f64,
}

impl Report {
    fn new(events: &[Value]) -> Self {
        let ts = |e: &Value| e.get("ts").and_then(Value::as_f64);
        let t0 = events.iter().filter_map(ts).fold(f64::INFINITY, f64::min);
        let t1 = events
            .iter()
            .filter_map(|e| Some(ts(e)? + e.get("dur").and_then(Value::as_f64).unwrap_or(0.0)))
            .fold(t0, f64::max);
        let names: BTreeMap<u64, String> = events
            .iter()
            .filter(|e| e["ph"] == "M" && e["name"] == "process_name")
            .filter_map(|e| Some((e["pid"].as_u64()?, e["args"]["name"].as_str()?.to_owned())))
            .collect();
        let mut steps = Vec::new();
        let mut marks = Vec::new();
        let mut renders = BTreeMap::new();
        for e in events {
            let str_of = |k: &str| {
                e.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            match e.get("ph").and_then(Value::as_str) {
                Some("X") => {
                    let pid = e["pid"].as_u64().unwrap_or(0);
                    steps.push(Step {
                        name: str_of("name"),
                        cat: str_of("cat"),
                        at: (ts(e).unwrap_or(t0) - t0) / 1000.0,
                        ms: e["dur"].as_f64().unwrap_or(0.0) / 1000.0,
                        track: if pid == MODS_PID {
                            let tid = e["tid"].as_u64().unwrap_or(0);
                            let part = MOD_TRACKS
                                .iter()
                                .find(|(t, _)| *t == tid)
                                .map_or("?", |(_, n)| n);
                            format!("mod: {part}")
                        } else {
                            names
                                .get(&pid)
                                .cloned()
                                .unwrap_or_else(|| format!("pid {pid}"))
                        },
                    });
                }
                Some("i") if e["cat"] == "mark" => marks.push((ts(e).unwrap_or(t0) - t0) / 1000.0),
                Some("C") if e["name"] == "renders" => {
                    for (role, n) in e["args"].as_object().into_iter().flatten() {
                        *renders.entry(role.clone()).or_default() += n.as_u64().unwrap_or(0);
                    }
                }
                _ => {}
            }
        }
        let seconds = if t1 > t0 { (t1 - t0) / 1e6 } else { 0.0 };
        Self {
            steps,
            marks,
            renders,
            seconds,
        }
    }

    fn of(&self, cat: &str) -> Vec<&Step> {
        let mut v: Vec<&Step> = self.steps.iter().filter(|s| s.cat == cat).collect();
        v.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        v
    }

    /// The slowest steps other than `skip` that overlap `from..to` (ms),
    /// but not those running all through it (the watcher, an open picker:
    /// in the background, not what it waited on).
    fn during(&self, from: f64, to: f64, skip: &Step) -> Vec<&Step> {
        let mut v: Vec<&Step> = self
            .steps
            .iter()
            .filter(|s| s.cat != "action" && s.cat != "stall")
            .filter(|s| !(s.at == skip.at && s.name == skip.name))
            .filter(|s| s.at < to && s.at + s.ms > from)
            .filter(|s| !(s.at < from && s.at + s.ms > to))
            .collect();
        v.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        v.truncate(6);
        v
    }

    fn headline(&self) -> String {
        let actions = self.of("action");
        let stalls = self.of("stall");
        let mut parts = vec![format!("{} actions", actions.len())];
        if let Some(a) = actions.first() {
            parts.push(format!("slowest {} {:.0} ms", a.name, a.ms));
        }
        if let Some(s) = stalls.first() {
            parts.push(format!("{} freezes (longest {:.0} ms)", stalls.len(), s.ms));
        }
        if !self.marks.is_empty() {
            parts.push(format!("{} lag marks", self.marks.len()));
        }
        parts.join(", ")
    }

    fn markdown(&self, file: &Path) -> String {
        let mut out = format!(
            "# herdr-nvim profile\n\n`{}`, {:.0} s. {}.\n\nOpen `{}` in https://ui.perfetto.dev for the timeline.\n",
            file.display(),
            self.seconds,
            self.headline(),
            file.with_extension("trace.json").display()
        );
        let line = |s: &Step, base: f64| {
            format!(
                "  - {:>7.1} ms  {} ({}, at {:+.0} ms)\n",
                s.ms,
                s.name,
                s.track,
                s.at - base
            )
        };
        out += "\n## Slowest actions (from the press to the result on screen)\n\n";
        for a in self.of("action").into_iter().take(10) {
            out += &format!(
                "- **{:.0} ms** {} at {:.1} s\n",
                a.ms,
                a.name,
                a.at / 1000.0
            );
            for s in self.during(a.at, a.at + a.ms, a) {
                out += &line(s, a.at);
            }
        }
        let stalls = self.of("stall");
        out += &format!(
            "\n## Main-thread freezes (≥150 ms late timers): {}, {:.0} ms in all\n\n",
            stalls.len(),
            stalls.iter().map(|s| s.ms).sum::<f64>().abs()
        );
        for s in stalls.iter().take(10) {
            out += &format!("- **{:.0} ms** at {:.1} s\n", s.ms, s.at / 1000.0);
            for d in self.during(s.at, s.at + s.ms, s) {
                out += &line(d, s.at);
            }
        }
        out += "\n## Lag marks (the 5 s before each)\n\n";
        for m in &self.marks {
            out += &format!("- at {:.1} s\n", m / 1000.0);
            let mut before: Vec<&Step> = self
                .steps
                .iter()
                .filter(|s| s.at < *m && s.at + s.ms > m - 5000.0)
                .collect();
            before.sort_by(|a, b| b.ms.total_cmp(&a.ms));
            for s in before.into_iter().take(8) {
                out += &format!(
                    "  - {:>7.1} ms  {} [{}] ({}, {:.1} s before)\n",
                    s.ms,
                    s.name,
                    s.cat,
                    s.track,
                    (m - s.at) / 1000.0
                );
            }
        }
        out += "\n## Renders\n\n";
        for (role, n) in &self.renders {
            let slow: Vec<&Step> = self
                .steps
                .iter()
                .filter(|s| s.cat == "render" && s.name == format!("render {role}"))
                .collect();
            let max = slow.iter().map(|s| s.ms).fold(0.0, f64::max);
            out += &format!(
                "- {role}: {n} ({:.1}/s), {} slow (≥3 ms), longest {max:.0} ms\n",
                *n as f64 / self.seconds.max(1.0),
                slow.len()
            );
        }
        out += "\n## Every step, by total time\n\n| step | count | p50 ms | p95 ms | max ms | total ms |\n|---|---|---|---|---|---|\n";
        let mut by: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for s in self
            .steps
            .iter()
            .filter(|s| s.cat != "action" && s.cat != "stall")
        {
            by.entry(s.name.clone()).or_default().push(s.ms);
        }
        let mut rows: Vec<(String, Vec<f64>)> = by.into_iter().collect();
        rows.sort_by(|a, b| b.1.iter().sum::<f64>().total_cmp(&a.1.iter().sum::<f64>()));
        for (name, mut ms) in rows.into_iter().take(40) {
            ms.sort_by(f64::total_cmp);
            let pick = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
            out += &format!(
                "| {name} | {} | {:.1} | {:.1} | {:.1} | {:.0} |\n",
                ms.len(),
                pick(0.5),
                pick(0.95),
                pick(1.0),
                ms.iter().sum::<f64>()
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_utc_calendar_time() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_793_803_223), "20261104-144023");
    }
}
