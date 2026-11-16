//! The Windows facts probe (doc/windows-facts.md, herdr-app/e2e/WINDOWS-CHECKLIST.md).
//!
//! `winprobe [--log FILE] [--tag TAG] COMMAND [ARGS...]`
//!
//! Every run appends one JSON line to the log (default `<temp>/winprobe/log.jsonl`)
//! and prints the same line: argv as received, the raw command line, the
//! environment that matters, its image, its Job Object, console and parents,
//! plus the command's `result`.
//!
//! Commands:
//!   echo [ARGS...]           record only (argument round trips)
//!   where                    herdr-app and hc: on PATH, beside the running app,
//!                            under %LOCALAPPDATA%\Programs; System32 curl.exe/tar.exe
//!   spawn PROGRAM [ARGS...]  start PROGRAM with Rust std's Command, report what ran
//!   spawn-detached [SECS]    start `tick SECS APP_PID` the way herdr-nvim starts its daemons
//!   tick [SECS [PID]]        log a line every 3 s for SECS (default 120), saying
//!                            whether PID (the app that started it) still runs
//!   copy-self                copy this exe beside the log, print its path
//!   rename-dir DIR           rename DIR aside and back, report each OS error
//!   motw FILE                print FILE's Zone.Identifier stream (Mark of the Web)

use serde_json::{json, Map, Value};
use std::env::{self, consts::EXE_SUFFIX};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: winprobe [--log FILE] [--tag TAG] \
    echo|where|spawn|spawn-detached|tick|copy-self|rename-dir|motw [ARGS...]";
const TICK_EVERY: Duration = Duration::from_secs(3);

struct Probe {
    log: PathBuf,
    tag: Option<String>,
}

enum Job {
    Echo,
    Where,
    Spawn(OsString, Vec<OsString>),
    SpawnDetached(u64),
    Tick(u64, Option<u32>),
    CopySelf,
    RenameDir(PathBuf),
    Motw(PathBuf),
}

fn main() -> ExitCode {
    let Some((probe, name, args)) = parse() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let Some(job) = job(&name, &args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match job {
        Job::Echo => Value::Null,
        Job::Where => where_apps(),
        Job::Spawn(program, rest) => spawn(&program, &rest),
        Job::SpawnDetached(secs) => spawn_detached(&probe, secs),
        Job::Tick(secs, app) => return tick(&probe, &args, secs, app),
        Job::CopySelf => copy_self(&probe),
        Job::RenameDir(dir) => rename_dir(&dir),
        Job::Motw(file) => sys::zone_identifier(&file),
    };
    probe.write(&record(&probe, &name, &args, result));
    ExitCode::SUCCESS
}

fn parse() -> Option<(Probe, String, Vec<OsString>)> {
    let mut argv = env::args_os().skip(1);
    let mut log = env::temp_dir().join("winprobe").join("log.jsonl");
    let mut tag = None;
    loop {
        let word = argv.next()?;
        match word.to_str() {
            Some("--log") => log = argv.next()?.into(),
            Some("--tag") => tag = Some(argv.next()?.to_string_lossy().into_owned()),
            Some(name) => return Some((Probe { log, tag }, name.to_owned(), argv.collect())),
            None => return None,
        }
    }
}

fn job(name: &str, args: &[OsString]) -> Option<Job> {
    let secs = |default| match args.first() {
        Some(s) => s.to_str()?.parse().ok(),
        None => Some(default),
    };
    Some(match name {
        "echo" => Job::Echo,
        "where" => Job::Where,
        "spawn" => Job::Spawn(args.first()?.clone(), args[1..].to_vec()),
        "spawn-detached" => Job::SpawnDetached(secs(120)?),
        "tick" => Job::Tick(
            secs(120)?,
            match args.get(1) {
                Some(pid) => Some(pid.to_str()?.parse().ok()?),
                None => None,
            },
        ),
        "copy-self" => Job::CopySelf,
        "rename-dir" => Job::RenameDir(args.first()?.into()),
        "motw" => Job::Motw(args.first()?.into()),
        _ => return None,
    })
}

impl Probe {
    fn write(&self, line: &Value) {
        let text = format!("{line}\n");
        print!("{text}");
        let appended = self
            .log
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| OpenOptions::new().create(true).append(true).open(&self.log))
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(error) = appended {
            eprintln!("winprobe: cannot append to {}: {error}", self.log.display());
        }
    }

    fn forward(&self, command: &mut Command) {
        command.arg("--log").arg(&self.log);
        if let Some(tag) = &self.tag {
            command.arg("--tag").arg(tag);
        }
    }
}

fn record(probe: &Probe, name: &str, args: &[OsString], result: Value) -> Value {
    json!({
        "ts_ms": now_ms(),
        "pid": std::process::id(),
        "tag": probe.tag,
        "sub": name,
        "argv": env::args_os().map(|a| lossy(&a)).collect::<Vec<_>>(),
        "args": args.iter().map(|a| lossy(a)).collect::<Vec<_>>(),
        "command_line": sys::command_line(),
        "image": env::current_exe().ok().map(|p| p.display().to_string()),
        "cwd": env::current_dir().ok().map(|p| p.display().to_string()),
        "temp_dir": env::temp_dir().display().to_string(),
        "log": probe.log.display().to_string(),
        "env": interesting_env(),
        "job": sys::job(),
        "console_window": sys::console_window(),
        "ancestors": sys::ancestors(),
        "result": result,
    })
}

fn interesting_env() -> Map<String, Value> {
    const NAMED: [&str; 10] = [
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "COMSPEC",
        "SYSTEMROOT",
    ];
    let mut vars: Vec<(String, String)> = env::vars_os()
        .map(|(k, v)| (lossy(&k), lossy(&v)))
        .filter(|(k, _)| {
            let k = k.to_ascii_uppercase();
            NAMED.contains(&k.as_str())
                || ["HERDR_", "HC_", "WINPROBE_"]
                    .iter()
                    .any(|p| k.starts_with(p))
        })
        .collect();
    vars.sort();
    vars.into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect()
}

fn where_apps() -> Value {
    let app = format!("herdr-app{EXE_SUFFIX}");
    let hc = format!("hc{EXE_SUFFIX}");
    let on_path = |name: &str| -> Vec<String> {
        env::var_os("PATH")
            .map(|path| {
                env::split_paths(&path)
                    .map(|d| d.join(name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.is_file())
            .map(|p| p.display().to_string())
            .collect()
    };
    let running = sys::running_images(&app);
    let beside: Vec<Value> = running
        .iter()
        .map(|image| {
            let sibling = Path::new(image).with_file_name(&hc);
            json!({"app": image, "hc": sibling.display().to_string(), "hc_exists": sibling.is_file()})
        })
        .collect();
    let programs = env::var_os("LOCALAPPDATA").map(|d| {
        let mut found = Vec::new();
        find_under(
            &PathBuf::from(d).join("Programs"),
            &[&app, &hc],
            3,
            &mut found,
        );
        found
    });
    let system32 = env::var_os("SystemRoot").map(|root| {
        let dir = PathBuf::from(root).join("System32");
        json!({"curl.exe": dir.join("curl.exe").is_file(), "tar.exe": dir.join("tar.exe").is_file()})
    });
    json!({
        "on_path": {app.clone(): on_path(&app), hc.clone(): on_path(&hc)},
        "running_app": running,
        "beside_running_app": beside,
        "local_programs": programs,
        "system32": system32,
    })
}

fn find_under(dir: &Path, names: &[&str], depth: u32, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth > 0 {
            find_under(&path, names, depth - 1, found);
        } else if names
            .iter()
            .any(|n| entry.file_name().to_string_lossy().eq_ignore_ascii_case(n))
        {
            found.push(path.display().to_string());
        }
    }
}

/// How Rust std's `Command` resolves and quotes: the child's own record
/// (its stdout) shows the path that ran and the argv it got.
fn spawn(program: &OsString, args: &[OsString]) -> Value {
    let started = Instant::now();
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let child = stdout
                .lines()
                .last()
                .and_then(|l| serde_json::from_str::<Value>(l).ok())
                .unwrap_or_else(|| Value::String(stdout.into_owned()));
            json!({
                "program": lossy(program),
                "code": out.status.code(),
                "ms": started.elapsed().as_millis() as u64,
                "child": child,
                "stderr": String::from_utf8_lossy(&out.stderr),
            })
        }
        Err(error) => json!({"program": lossy(program), "error": os_error(&error)}),
    }
}

fn spawn_detached(probe: &Probe, secs: u64) -> Value {
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return json!({"error": os_error(&error)}),
    };
    let app = sys::app_pid();
    let mut command = Command::new(exe);
    probe.forward(&mut command);
    command.arg("tick").arg(secs.to_string());
    if let Some(pid) = app {
        command.arg(pid.to_string());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (spawned, flags) = sys::spawn_detached(&mut command);
    match spawned {
        // Not waited for: it must outlive this process.
        Ok(child) => {
            json!({"child_pid": child.id(), "seconds": secs, "app_pid": app, "flags": flags})
        }
        Err(error) => json!({"error": os_error(&error), "flags": flags}),
    }
}

/// A stand-in daemon: one full record at start (its Job Object shows whether
/// breakaway worked), then a short line every few seconds until `secs`.
fn tick(probe: &Probe, args: &[OsString], secs: u64, app: Option<u32>) -> ExitCode {
    probe.write(&record(
        probe,
        "tick-start",
        args,
        json!({"seconds": secs, "app_pid": app}),
    ));
    let started = Instant::now();
    let mut n = 0u64;
    while started.elapsed() < Duration::from_secs(secs) {
        sleep(TICK_EVERY);
        n += 1;
        probe.write(&json!({
            "ts_ms": now_ms(),
            "pid": std::process::id(),
            "tag": probe.tag,
            "sub": "tick",
            "n": n,
            "app_running": app.map(sys::alive),
        }));
    }
    probe.write(&json!({"ts_ms": now_ms(), "pid": std::process::id(), "tag": probe.tag, "sub": "tick-end", "n": n}));
    ExitCode::SUCCESS
}

/// A copy outside the folder under test, so renaming that folder isn't
/// refused just because this probe runs from it.
fn copy_self(probe: &Probe) -> Value {
    let dir = probe
        .log
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(env::temp_dir);
    let target = dir.join(format!("winprobe-outside{EXE_SUFFIX}"));
    let copied = fs::create_dir_all(&dir)
        .and_then(|()| env::current_exe())
        .and_then(|exe| fs::copy(exe, &target));
    match copied {
        Ok(_) => json!({"path": target.display().to_string()}),
        Err(error) => json!({"path": target.display().to_string(), "error": os_error(&error)}),
    }
}

/// Renames DIR aside and back. Every attempt's OS error is kept: a refusal
/// that clears after a moment (a scanner) differs from one that doesn't (a
/// running exe, an open handle).
fn rename_dir(dir: &Path) -> Value {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let aside = dir.with_file_name(format!("{name}.winprobe-aside"));
    let image_inside = match (
        env::current_exe().and_then(fs::canonicalize),
        fs::canonicalize(dir),
    ) {
        (Ok(image), Ok(dir)) => Some(image.starts_with(dir)),
        _ => None,
    };
    let away = retry(5, Duration::from_millis(300), || fs::rename(dir, &aside));
    let moved = away.last().is_some_and(Value::is_null);
    let back = moved.then(|| retry(20, Duration::from_millis(250), || fs::rename(&aside, dir)));
    let restored = back.as_ref().map(|b| b.last().is_some_and(Value::is_null));
    json!({
        "dir": dir.display().to_string(),
        "aside": aside.display().to_string(),
        "image_inside_dir": image_inside,
        "moved": moved,
        "attempts": away,
        "restored": restored,
        "restore_attempts": back,
    })
}

/// One entry per attempt: null for success, else the OS error. Stops at the
/// first success.
fn retry(times: u32, pause: Duration, mut attempt: impl FnMut() -> io::Result<()>) -> Vec<Value> {
    let mut tries = Vec::new();
    for i in 0..times {
        match attempt() {
            Ok(()) => {
                tries.push(Value::Null);
                break;
            }
            Err(error) => tries.push(os_error(&error)),
        }
        if i + 1 < times {
            sleep(pause);
        }
    }
    tries
}

fn os_error(error: &io::Error) -> Value {
    json!({"kind": format!("{:?}", error.kind()), "os": error.raw_os_error(), "message": error.to_string()})
}

fn lossy(s: &std::ffi::OsStr) -> String {
    s.to_string_lossy().into_owned()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(windows)]
mod sys {
    use serde_json::{json, Value};
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::{mem, ptr};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, QueryFullProcessImageNameW, CREATE_BREAKAWAY_FROM_JOB,
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct Proc {
        pid: u32,
        parent: u32,
        name: String,
    }

    fn wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    pub fn command_line() -> Value {
        // SAFETY: GetCommandLineW returns a NUL-terminated string that lives
        // as long as the process.
        unsafe {
            let p = GetCommandLineW();
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            Value::String(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
        }
    }

    pub fn console_window() -> Value {
        // SAFETY: no arguments; returns null without a console.
        Value::Bool(unsafe { !GetConsoleWindow().is_null() })
    }

    pub fn job() -> Value {
        // SAFETY: plain out-parameters sized to the documented structs; a
        // null job handle means "the job of the calling process".
        unsafe {
            let mut in_job = 0;
            if IsProcessInJob(GetCurrentProcess(), ptr::null_mut(), &mut in_job) == 0 {
                return json!({"error": io::Error::last_os_error().to_string()});
            }
            if in_job == 0 {
                return json!({"in_job": false});
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            let ok = QueryInformationJobObject(
                ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut c_void,
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ptr::null_mut(),
            );
            if ok == 0 {
                return json!({"in_job": true, "limits_error": io::Error::last_os_error().to_string()});
            }
            let flags = info.BasicLimitInformation.LimitFlags;
            json!({
                "in_job": true,
                "limit_flags": format!("{flags:#x}"),
                "kill_on_job_close": flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0,
                "breakaway_ok": flags & JOB_OBJECT_LIMIT_BREAKAWAY_OK != 0,
                "silent_breakaway_ok": flags & JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK != 0,
            })
        }
    }

    fn processes() -> Vec<Proc> {
        let mut out = Vec::new();
        // SAFETY: the snapshot handle is closed below; the entry is sized as
        // the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = mem::zeroed();
            entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut entry) != 0;
            while more {
                out.push(Proc {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    name: wide(&entry.szExeFile),
                });
                more = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    fn image_of(pid: u32) -> Option<String> {
        // SAFETY: the handle is closed before returning; len is the buffer's size.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut buf = vec![0u16; 32768];
            let mut len = buf.len() as u32;
            let ok =
                QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(handle);
            (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    /// Parent, grandparent, ... A parent that already exited may have had
    /// its pid reused, so a surprising entry is a hint, not proof.
    pub fn ancestors() -> Value {
        let all = processes();
        let mut chain = Vec::new();
        let mut pid = std::process::id();
        for _ in 0..6 {
            let Some(me) = all.iter().find(|p| p.pid == pid) else {
                break;
            };
            match all.iter().find(|p| p.pid == me.parent) {
                Some(parent) if parent.pid != 0 => {
                    chain.push(json!({"pid": parent.pid, "name": parent.name, "image": image_of(parent.pid)}));
                    pid = parent.pid;
                }
                _ => {
                    chain.push(json!({"pid": me.parent, "gone": true}));
                    break;
                }
            }
        }
        Value::Array(chain)
    }

    pub fn running_images(name: &str) -> Vec<String> {
        processes()
            .into_iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| image_of(p.pid).unwrap_or(p.name))
            .collect()
    }

    /// The app among this process's ancestors (its name has "herdr"),
    /// else the direct parent: a shell between the app and the probe would
    /// otherwise look like the app quitting.
    pub fn app_pid() -> Option<u32> {
        let all = processes();
        let parent_of = |pid: u32| all.iter().find(|p| p.pid == pid).map(|p| p.parent);
        let direct = parent_of(std::process::id())?;
        let mut pid = direct;
        for _ in 0..6 {
            let Some(p) = all.iter().find(|p| p.pid == pid) else {
                break;
            };
            if p.name.to_ascii_lowercase().contains("herdr") {
                return Some(pid);
            }
            pid = p.parent;
        }
        Some(direct)
    }

    pub fn alive(pid: u32) -> bool {
        processes().iter().any(|p| p.pid == pid)
    }

    /// herdr-nvim's daemon flags plus breakaway from the parent's Job
    /// Object, retried without it where the job forbids breakaway.
    pub fn spawn_detached(command: &mut Command) -> (io::Result<Child>, Value) {
        let base = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
        command.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                command.creation_flags(base);
                (
                    command.spawn(),
                    json!({"breakaway": false, "breakaway_error": e.to_string()}),
                )
            }
            spawned => (spawned, json!({"breakaway": true})),
        }
    }

    pub fn zone_identifier(file: &Path) -> Value {
        let stream = format!("{}:Zone.Identifier", file.display());
        match std::fs::read_to_string(&stream) {
            Ok(text) => json!({"file": file.display().to_string(), "zone_identifier": text}),
            Err(e) => {
                json!({"file": file.display().to_string(), "zone_identifier": null, "error": e.to_string()})
            }
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use serde_json::{json, Value};
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command};

    pub fn command_line() -> Value {
        Value::Null
    }
    pub fn console_window() -> Value {
        Value::Null
    }
    pub fn job() -> Value {
        Value::Null
    }
    pub fn ancestors() -> Value {
        Value::Null
    }
    pub fn running_images(_name: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn app_pid() -> Option<u32> {
        Some(std::os::unix::process::parent_id())
    }
    pub fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    pub fn spawn_detached(command: &mut Command) -> (io::Result<Child>, Value) {
        command.process_group(0);
        (command.spawn(), json!({"process_group": 0}))
    }
    pub fn zone_identifier(file: &Path) -> Value {
        json!({"file": file.display().to_string(), "zone_identifier": null, "error": "Zone.Identifier streams exist on NTFS only"})
    }
}
