//! Session-file mining: hand-rolled ISO-8601 UTC timestamp parsing plus
//! per-agent JSONL parsers. No new runtime dependency (`chrono`/`time`) is
//! pulled in since only UTC `Z`-suffixed timestamps of the fixed
//! `YYYY-MM-DDTHH:MM:SS[.fff]Z` shape are ever seen in pi/claude session
//! files.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawOp {
    Write,
    Edit,
    Read,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawEvent {
    pub path: String,
    pub op: RawOp,
    pub unix_ts: Option<u64>,
}

/// Parse a pi session JSONL file into file-path tool-call events. Skips any
/// line that isn't valid JSON, isn't a `type:"message"` assistant record, or
/// carries a `toolCall` whose `name` isn't `read`/`edit`/`write` — never
/// errors, since a parse failure must never break the picker (brief).
pub(crate) fn parse_pi_session(text: &str) -> Vec<RawEvent> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(unix_ts) = value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let unix_ts = parse_iso8601_unix(&unix_ts);
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for item in content {
            if item.get("type").and_then(Value::as_str) != Some("toolCall") {
                continue;
            }
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let op = match name {
                "write" => RawOp::Write,
                "edit" => RawOp::Edit,
                "read" => RawOp::Read,
                _ => continue,
            };
            let Some(path) = item.pointer("/arguments/path").and_then(Value::as_str) else {
                continue;
            };
            out.push(RawEvent {
                path: path.to_owned(),
                op,
                unix_ts,
            });
        }
    }
    out
}

/// Parse a claude-code session JSONL file into file-path tool-use events.
/// Same defensive contract as `parse_pi_session`: any unparseable line or
/// unrecognized tool name is skipped, never an error.
pub(crate) fn parse_claude_session(text: &str) -> Vec<RawEvent> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(unix_ts) = value.get("timestamp").and_then(Value::as_str) else {
            continue;
        };
        let unix_ts = parse_iso8601_unix(unix_ts);
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for item in content {
            if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let op = match name {
                "Write" => RawOp::Write,
                "MultiEdit" | "NotebookEdit" | "Edit" => RawOp::Edit,
                "Read" => RawOp::Read,
                _ => continue,
            };
            let Some(path) = item.pointer("/input/file_path").and_then(Value::as_str) else {
                continue;
            };
            out.push(RawEvent {
                path: path.to_owned(),
                op,
                unix_ts,
            });
        }
    }
    out
}

pub(crate) struct MinedEdit {
    pub path: String,
    pub newly_created: bool,
    pub last_edit_unix: Option<u64>,
}

pub(crate) struct Mined {
    pub edits: Vec<MinedEdit>,
    pub reads: Vec<String>,
    pub session_start_unix: Option<u64>,
}

/// Mine a session JSONL file for file-path events, dispatching on the pane's
/// reported agent kind. Any kind without a parser (codex today, or a future
/// unknown agent) yields an empty `Mined` -- never a crash or misparse, so
/// callers can always fall through to the git/scrape layers (brief).
///
/// Reduction rule: scan `RawEvent`s in file order (== chronological, oldest
/// first, since JSONL is append-only). For each path, remember the *first*
/// op seen (`newly_created = first_op == RawOp::Write`) and the *latest*
/// `unix_ts` among its Edit/Write events. A path with only Read events (never
/// Edit/Write) goes to `reads`; a path with any Edit/Write event goes to
/// `edits` and is excluded from `reads` even if it was also read.
///
/// Note: `by_path.contains_key` is checked *before* calling `.entry(...)`,
/// not inside `or_insert_with`'s closure -- the closure can't borrow
/// `by_path` immutably while `.entry()` already holds it mutably borrowed.
pub(crate) fn mine_session(agent_kind: &str, text: &str) -> Mined {
    let raw: Vec<RawEvent> = match agent_kind {
        "pi" => parse_pi_session(text),
        "claude" => parse_claude_session(text),
        _ => Vec::new(),
    };

    let session_start_unix = raw.iter().find_map(|e| e.unix_ts);

    struct Acc {
        first_op: RawOp,
        last_edit_unix: Option<u64>,
        ever_edited: bool,
    }
    let mut by_path: std::collections::HashMap<String, Acc> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for event in raw {
        if !by_path.contains_key(&event.path) {
            order.push(event.path.clone());
        }
        let entry = by_path.entry(event.path.clone()).or_insert_with(|| Acc {
            first_op: event.op.clone(),
            last_edit_unix: None,
            ever_edited: false,
        });
        if matches!(event.op, RawOp::Edit | RawOp::Write) {
            entry.ever_edited = true;
            if event.unix_ts.is_some() {
                entry.last_edit_unix = event.unix_ts;
            }
        }
    }

    let mut edits = Vec::new();
    let mut reads = Vec::new();
    for path in order {
        let acc = &by_path[&path];
        if acc.ever_edited {
            edits.push(MinedEdit {
                path: path.clone(),
                newly_created: matches!(acc.first_op, RawOp::Write),
                last_edit_unix: acc.last_edit_unix,
            });
        } else {
            reads.push(path.clone());
        }
    }

    Mined {
        edits,
        reads,
        session_start_unix,
    }
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff]Z` (UTC only) into unix seconds. `None`
/// for any other shape — a malformed timestamp must never panic or bubble
/// an error into the picker; callers simply treat it as "no timestamp".
pub(crate) fn parse_iso8601_unix(s: &str) -> Option<u64> {
    if s.len() < 20 || !s.ends_with('Z') {
        return None;
    }
    if s.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u64 = s.get(11..13)?.parse().ok()?;
    let minute: u64 = s.get(14..16)?.parse().ok()?;
    let second: u64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + (hour * 3600 + minute * 60 + second) as i64) as u64)
}

/// Days since 1970-01-01 for a proleptic-Gregorian y-m-d. Adapted from
/// Howard Hinnant's public-domain `days_from_civil`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11] Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_parses_to_zero() {
        assert_eq!(parse_iso8601_unix("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn real_pi_timestamp_parses() {
        // 2026-07-22T11:07:49.944Z -- fractional seconds are ignored (second
        // granularity is enough for "age" display). Verified independently
        // via `date -u -r 1784718469` => "Wed Jul 22 11:07:49 UTC 2026".
        assert_eq!(
            parse_iso8601_unix("2026-07-22T11:07:49.944Z"),
            Some(1784718469)
        );
    }

    #[test]
    fn real_claude_timestamp_parses() {
        // Verified independently via `date -u -r 1784479915` =>
        // "Sun Jul 19 16:51:55 UTC 2026".
        assert_eq!(
            parse_iso8601_unix("2026-07-19T16:51:55.425Z"),
            Some(1784479915)
        );
    }

    #[test]
    fn leap_year_day_parses() {
        assert_eq!(parse_iso8601_unix("2024-02-29T00:00:00Z"), Some(1709164800));
    }

    #[test]
    fn malformed_input_yields_none() {
        assert_eq!(parse_iso8601_unix("not-a-date"), None);
        assert_eq!(parse_iso8601_unix("2026-07-22 11:07:49Z"), None); // missing T
        assert_eq!(parse_iso8601_unix("2026-07-22T11:07:49"), None); // missing Z
        assert_eq!(parse_iso8601_unix(""), None);
    }

    #[test]
    fn pi_basic_fixture_yields_read_write_edit_ignoring_bash() {
        let text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let events = parse_pi_session(text);
        assert_eq!(events.len(), 3, "bash toolCall must be ignored: {events:?}");
        assert_eq!(events[0].path, "/repo/src/lib.rs");
        assert!(matches!(events[0].op, RawOp::Read));
        // 2026-07-22T11:08:00.000Z, verified via `date -u -r 1784718480`.
        assert_eq!(events[0].unix_ts, Some(1784718480));
        assert_eq!(events[1].path, "/repo/src/new_mod.rs");
        assert!(matches!(events[1].op, RawOp::Write));
        assert_eq!(events[2].path, "/repo/src/lib.rs");
        assert!(matches!(events[2].op, RawOp::Edit));
    }

    #[test]
    fn pi_edits_array_shape_still_yields_path() {
        let text = include_str!("../tests/fixtures/session_pi_edits_array.jsonl");
        let events = parse_pi_session(text);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/repo/src/mod.rs");
        assert!(matches!(events[0].op, RawOp::Edit));
    }

    #[test]
    fn malformed_line_is_skipped_not_fatal() {
        let text = "not json at all\n{\"type\":\"message\",\"timestamp\":\"2026-07-22T11:08:00.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"read\",\"arguments\":{\"path\":\"/repo/a.rs\"}}]}}\n";
        let events = parse_pi_session(text);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/repo/a.rs");
    }

    #[test]
    fn claude_basic_fixture_yields_read_write_edit_multiedit_ignoring_bash() {
        let text = include_str!("../tests/fixtures/session_claude_basic.jsonl");
        let events = parse_claude_session(text);
        assert_eq!(events.len(), 4, "Bash tool_use must be ignored: {events:?}");
        assert_eq!(events[0].path, "/repo/src/lib.rs");
        assert!(matches!(events[0].op, RawOp::Read));
        assert_eq!(events[1].path, "/repo/src/new_mod.rs");
        assert!(matches!(events[1].op, RawOp::Write));
        assert_eq!(events[2].path, "/repo/src/lib.rs");
        assert!(matches!(events[2].op, RawOp::Edit));
        assert_eq!(events[3].path, "/repo/src/lib.rs");
        assert!(matches!(events[3].op, RawOp::Edit)); // MultiEdit folds into Edit
    }

    #[test]
    fn dispatches_by_agent_kind() {
        let pi_text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let mined = mine_session("pi", pi_text);
        assert!(!mined.edits.is_empty());

        let claude_text = include_str!("../tests/fixtures/session_claude_basic.jsonl");
        let mined = mine_session("claude", claude_text);
        assert!(!mined.edits.is_empty());

        let mined = mine_session("codex", pi_text);
        assert!(
            mined.edits.is_empty() && mined.reads.is_empty(),
            "codex has no parser yet -- must degrade to empty, not crash or misparse"
        );

        let mined = mine_session("unknown-future-agent", pi_text);
        assert!(mined.edits.is_empty() && mined.reads.is_empty());
    }

    #[test]
    fn newly_created_true_only_when_first_op_is_write() {
        // pi fixture: /repo/src/new_mod.rs first (only) op is "write".
        // /repo/src/lib.rs first op is "read", later "edit" -> not newly created.
        let text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let mined = mine_session("pi", text);
        let new_mod = mined
            .edits
            .iter()
            .find(|e| e.path == "/repo/src/new_mod.rs")
            .unwrap();
        assert!(new_mod.newly_created);
        let lib = mined
            .edits
            .iter()
            .find(|e| e.path == "/repo/src/lib.rs")
            .unwrap();
        assert!(!lib.newly_created);
    }

    #[test]
    fn reads_exclude_paths_that_were_also_edited() {
        let text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let mined = mine_session("pi", text);
        // /repo/src/lib.rs was read then edited -> only in edits, not reads.
        assert!(!mined.reads.contains(&"/repo/src/lib.rs".to_owned()));
        assert!(mined.edits.iter().any(|e| e.path == "/repo/src/lib.rs"));
    }

    #[test]
    fn session_start_is_earliest_event_timestamp() {
        let text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let mined = mine_session("pi", text);
        // first event's ts, 2026-07-22T11:08:00Z, verified via `date -u -r`.
        assert_eq!(mined.session_start_unix, Some(1784718480));
    }
}
