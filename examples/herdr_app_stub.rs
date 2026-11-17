//! A stand-in for the Herdr app's `herdr-app` and herdr-core's `hc`, for the
//! install e2e (`herdr-app/e2e/install-e2e.ps1`). One real exe, copied as
//! `herdr-app[.exe]` and `hc[.exe]`: it acts as the one its file stem names,
//! and argv reaches it the way the app's own spawn delivers it (no `.cmd`, no
//! shell).
//!
//! `herdr-app`:
//! - `status`: `{"layout":"mine"}` (open, on the personal layout `mine`).
//! - `check <dir>`: the mod in `dir` loads: `mod.json` is a JSON object,
//!   `main.js` exists, and every relative `import`/`export … from` of every
//!   file reachable from it resolves to a file inside `dir`. Exit 1 with the
//!   reason otherwise.
//! - `do <command> [json]`: `{}`.
//! - `exec <program> <args…>`: runs `program` with Rust's `Command`, exactly
//!   as given (on Windows an extensionless path finds `<path>.exe`, as the
//!   app's exec would), on this stdout and stderr, and exits with its code.
//!
//! `hc` (a core that has no app document):
//! - `events …`: blocks, writing a blank line now and then, until its reader
//!   is gone, so an `app-watch` holds its runtime.
//! - `connect`: answers `hello`, and `doc.get` with version 0; refuses every
//!   other method.
//! - anything else: exit 1.
//!
//! Every call appends one JSON line to `$HERDR_STUB_LOG`: its name, argv,
//! cwd, `HERDR_APP_HOME` and exit code (`null` for `events`, which only ends
//! when killed).
//!
//! Build: `cargo build --release --example herdr_app_stub`.

use std::{
    collections::HashSet,
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use regex::Regex;
use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let name = env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let code = match (name.as_str(), args.first().map(String::as_str)) {
        ("hc", Some("events")) => {
            log(&name, &args, None);
            events();
            return;
        }
        ("hc", Some("connect")) => connect(),
        ("hc", Some("--version")) => {
            println!("hc stub");
            0
        }
        // No terminal exists here; as herdr-core answers for a missing one.
        ("hc", Some("get" | "kill")) => {
            println!(
                "{}",
                json!({"error": {"kind": "not_found", "message": "no terminal"}})
            );
            3
        }
        ("hc", _) => 1,
        ("herdr-app", _) => app(&args),
        _ => {
            eprintln!("herdr_app_stub: copy me as herdr-app or hc, not {name}");
            2
        }
    };
    log(&name, &args, Some(code));
    std::process::exit(code);
}

fn app(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("status") => {
            println!("{}", json!({"layout": "mine"}));
            0
        }
        Some("check") if args.len() == 2 => match check(Path::new(&args[1])) {
            Ok(()) => 0,
            Err(why) => {
                eprintln!("herdr-app check: {why}");
                1
            }
        },
        Some("do") if args.len() >= 2 => {
            println!("{{}}");
            0
        }
        Some("exec") if args.len() >= 2 => match Command::new(&args[1]).args(&args[2..]).status() {
            Ok(status) => status.code().unwrap_or(1),
            Err(error) => {
                eprintln!("herdr_app_stub exec: cannot run {}: {error}", args[1]);
                127
            }
        },
        _ => {
            eprintln!("herdr_app_stub: herdr-app status | check <dir> | do <cmd> [json] | exec <program> <args…>");
            2
        }
    }
}

fn check(dir: &Path) -> Result<(), String> {
    let manifest = fs::read_to_string(dir.join("mod.json"))
        .map_err(|e| format!("cannot read {}: {e}", dir.join("mod.json").display()))?;
    if !matches!(serde_json::from_str(&manifest), Ok(Value::Object(_))) {
        return Err("mod.json is not a JSON object".to_owned());
    }
    let root = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let main = root.join("main.js");
    if !main.is_file() {
        return Err(format!("no main.js in {}", dir.display()));
    }
    let import =
        Regex::new(r#"(?:\bfrom|\bimport)\s*\(?\s*["'](\.\.?/[^"']*)["']"#).expect("valid regex");
    let mut seen = HashSet::new();
    let mut queue = vec![main];
    while let Some(file) = queue.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let text = fs::read_to_string(&file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let base = file.parent().unwrap_or(&root);
        for spec in import.captures_iter(&text).map(|c| c[1].to_owned()) {
            let target = fs::canonicalize(base.join(&spec))
                .ok()
                .filter(|target| target.is_file() && target.starts_with(&root))
                .ok_or_else(|| {
                    format!("{}: \"{spec}\" is not a file in the mod", rel(&root, &file))
                })?;
            queue.push(target);
        }
    }
    Ok(())
}

fn rel(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned()
}

fn events() {
    let mut out = io::stdout();
    while writeln!(out).and_then(|()| out.flush()).is_ok() {
        thread::sleep(Duration::from_millis(200));
    }
}

fn connect() -> i32 {
    let mut out = io::stdout();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let reply = match request.get("method").and_then(Value::as_str) {
            Some("hello") => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            Some("doc.get") => json!({"jsonrpc": "2.0", "id": id, "result": {"version": 0}}),
            method => json!({"jsonrpc": "2.0", "id": id, "error": {
                "message": format!("{} is not in the stub core", method.unwrap_or("?"))
            }}),
        };
        if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    0
}

fn log(name: &str, args: &[String], exit: Option<i32>) {
    let Some(path) = env::var_os("HERDR_STUB_LOG").map(PathBuf::from) else {
        return;
    };
    let line = json!({
        "name": name,
        "argv": args,
        "cwd": env::current_dir().ok(),
        "HERDR_APP_HOME": env::var_os("HERDR_APP_HOME").map(|h| h.to_string_lossy().into_owned()),
        "exit": exit,
    });
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}
