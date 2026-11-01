//! The session log of an agent running in a Herdr app terminal. The TUI is
//! told the session file by herdr; herdr-core is not, so it is found on disk
//! the way the agents lay it out, from what herdr-core knows about the
//! terminal: which agent it is (its status report or its program), the folder
//! it runs in, and when its program started.
//!
//! - pi: `$PI_CODING_AGENT_DIR` (default `~/.pi/agent`) `/sessions/--<cwd
//!   without its leading /, with / \ : as ->--/<start>_<id>.jsonl`.
//! - Claude Code: `$CLAUDE_CONFIG_DIR` (default `~/.claude`) `/projects/<cwd
//!   with / as -, or every non-alphanumeric as - in newer versions>/<id>.jsonl`.
//!
//! The session is the newest log in that folder written since the program
//! started. Two agents of the same kind in the same folder can't be told
//! apart this way: the one written last wins.

use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use serde_json::Value;

use crate::herdr::AgentSession;

/// Which session layout the terminal's agent uses: `pi` or `claude`.
pub(crate) fn agent_kind(facts: &Value) -> Option<&'static str> {
    let reported = facts.pointer("/status/app").and_then(Value::as_str);
    let program = facts.pointer("/foreground/name").and_then(Value::as_str);
    [reported, program].into_iter().flatten().find_map(|name| {
        let name = name.to_ascii_lowercase();
        if name.contains("claude") {
            Some("claude")
        } else if name == "pi" || name.starts_with("pi-") || name.starts_with("pi ") {
            Some("pi")
        } else {
            None
        }
    })
}

/// Pure: pi's folder name for `cwd`.
pub(crate) fn pi_slug(cwd: &str) -> String {
    let trimmed = cwd.strip_prefix('/').unwrap_or(cwd);
    format!("--{}--", trimmed.replace(['/', '\\', ':'], "-"))
}

/// Pure: Claude Code's folder names for `cwd`, older encoding first.
pub(crate) fn claude_slugs(cwd: &str) -> [String; 2] {
    [
        cwd.replace('/', "-"),
        cwd.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect(),
    ]
}

/// Pure: of `(file, last written)` pairs, the newest written at or after
/// `since` (a little earlier, for clock and file-time granularity).
pub(crate) fn newest_since(
    files: Vec<(PathBuf, SystemTime)>,
    since: SystemTime,
) -> Option<PathBuf> {
    let since = since.checked_sub(Duration::from_secs(2)).unwrap_or(since);
    files
        .into_iter()
        .filter(|(_, written)| *written >= since)
        .max_by_key(|(_, written)| *written)
        .map(|(path, _)| path)
}

/// Pure: `ps -o etime=` (`[[dd-]hh:]mm:ss`) as a duration.
pub(crate) fn parse_etime(text: &str) -> Option<Duration> {
    let text = text.trim();
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let mut seconds = 0u64;
    for part in clock.split(':') {
        seconds = seconds * 60 + part.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs(days * 86_400 + seconds))
}

/// When the terminal's program started: its foreground process's age, else
/// the terminal's creation.
fn started(facts: &Value) -> Option<SystemTime> {
    let from_pid = facts
        .pointer("/foreground/pid")
        .and_then(Value::as_u64)
        .and_then(|pid| {
            let out = Command::new("ps")
                .args(["-o", "etime=", "-p", &pid.to_string()])
                .stderr(Stdio::null())
                .output()
                .ok()?;
            parse_etime(&String::from_utf8_lossy(&out.stdout))
        })
        .and_then(|age| SystemTime::now().checked_sub(age));
    from_pid.or_else(|| {
        let ms = facts.get("created_ms").and_then(Value::as_u64)?;
        Some(SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
    })
}

fn root(var: &str, default: &str) -> Option<PathBuf> {
    env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(default)))
}

fn logs_in(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|path| {
            let written = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            Some((path, written))
        })
        .collect()
}

/// The session log of the agent in the terminal described by `facts`
/// (`terminal.get`), if one can be found.
pub(crate) fn find(facts: &Value) -> Option<AgentSession> {
    let kind = agent_kind(facts)?;
    let since = started(facts)?;
    let mut cwds: Vec<String> = ["/foreground/cwd", "/cwd"]
        .iter()
        .filter_map(|p| facts.pointer(p).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    if let Some(real) = cwds
        .first()
        .and_then(|c| std::fs::canonicalize(c).ok())
        .map(|p| p.to_string_lossy().into_owned())
    {
        cwds.push(real);
    }
    cwds.dedup();
    let dirs: Vec<PathBuf> = match kind {
        "pi" => {
            let sessions = root("PI_CODING_AGENT_DIR", ".pi/agent")?.join("sessions");
            cwds.iter().map(|c| sessions.join(pi_slug(c))).collect()
        }
        _ => {
            let projects = root("CLAUDE_CONFIG_DIR", ".claude")?.join("projects");
            cwds.iter()
                .flat_map(|c| claude_slugs(c))
                .map(|slug| projects.join(slug))
                .collect()
        }
    };
    let logs = dirs.iter().flat_map(|dir| logs_in(dir)).collect();
    let path = newest_since(logs, since)?;
    Some(AgentSession {
        agent: kind.to_owned(),
        kind: "path".to_owned(),
        value: path.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slugs_match_what_the_agents_write() {
        // Folders as found in ~/.pi/agent/sessions and ~/.claude/projects.
        assert_eq!(pi_slug("/Users/adamchmara"), "--Users-adamchmara--");
        assert_eq!(pi_slug("/private/tmp/af-live"), "--private-tmp-af-live--");
        assert_eq!(
            claude_slugs("/private/tmp/nv9076-e2e.apps/agent"),
            [
                "-private-tmp-nv9076-e2e.apps-agent".to_owned(),
                "-private-tmp-nv9076-e2e-apps-agent".to_owned()
            ]
        );
    }

    #[test]
    fn the_agent_is_known_by_its_report_or_its_program() {
        assert_eq!(agent_kind(&json!({"status": {"app": "pi"}})), Some("pi"));
        assert_eq!(
            agent_kind(&json!({"status": {"app": "claude-code"}})),
            Some("claude")
        );
        assert_eq!(
            agent_kind(&json!({"foreground": {"name": "claude"}})),
            Some("claude")
        );
        assert_eq!(agent_kind(&json!({"foreground": {"name": "pip"}})), None);
        assert_eq!(agent_kind(&json!({"status": {"app": "codex"}})), None);
    }

    #[test]
    fn the_newest_log_written_since_the_agent_started_wins() {
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = vec![
            (PathBuf::from("old.jsonl"), t(100)),
            (PathBuf::from("mine.jsonl"), t(500)),
            (PathBuf::from("resumed.jsonl"), t(700)),
        ];
        assert_eq!(
            newest_since(files.clone(), t(400)),
            Some(PathBuf::from("resumed.jsonl"))
        );
        assert_eq!(
            newest_since(files, t(800)),
            None,
            "nothing written since it started"
        );
    }

    #[test]
    fn process_age_parses_like_ps_prints_it() {
        assert_eq!(parse_etime("   05:07\n"), Some(Duration::from_secs(307)));
        assert_eq!(parse_etime("01:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(
            parse_etime("2-00:00:01"),
            Some(Duration::from_secs(172_801))
        );
        assert_eq!(parse_etime(""), None);
    }
}
