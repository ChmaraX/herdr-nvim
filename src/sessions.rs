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

/// Per-agent session file shape: everything that differs between pi and
/// claude-code JSONL records, as data rather than as parallel loops. Adding a
/// future agent (codex) means adding a new `Dialect` value, not a new parser
/// function.
pub(crate) struct Dialect {
    /// If set, only records whose top-level `type` equals this are considered
    /// (pi's outer `type:"message"` filter); `None` skips the check (claude
    /// has no such outer filter).
    record_type_filter: Option<&'static str>,
    /// The `type` tag identifying a tool-call content item (`"toolCall"` for
    /// pi, `"tool_use"` for claude).
    content_item_type: &'static str,
    /// JSON pointer, relative to a content item, to the file path argument.
    path_pointer: &'static str,
    /// Maps a tool name to the `RawOp` it represents, or `None` to skip it.
    tool_map: fn(&str) -> Option<RawOp>,
}

fn pi_tool_map(name: &str) -> Option<RawOp> {
    match name {
        "write" => Some(RawOp::Write),
        "edit" => Some(RawOp::Edit),
        "read" => Some(RawOp::Read),
        _ => None,
    }
}

fn claude_tool_map(name: &str) -> Option<RawOp> {
    match name {
        "Write" => Some(RawOp::Write),
        "MultiEdit" | "NotebookEdit" | "Edit" => Some(RawOp::Edit),
        "Read" => Some(RawOp::Read),
        _ => None,
    }
}

pub(crate) const PI_DIALECT: Dialect = Dialect {
    record_type_filter: Some("message"),
    content_item_type: "toolCall",
    path_pointer: "/arguments/path",
    tool_map: pi_tool_map,
};

pub(crate) const CLAUDE_DIALECT: Dialect = Dialect {
    record_type_filter: None,
    content_item_type: "tool_use",
    path_pointer: "/input/file_path",
    tool_map: claude_tool_map,
};

/// Parse a session JSONL file into file-path tool-call events, per `dialect`.
/// Skips any line that isn't valid JSON, doesn't match the dialect's record
/// filter, or carries a tool-call item whose name the dialect doesn't map to
/// a `RawOp` — never errors, since a parse failure must never break the
/// picker (brief).
pub(crate) fn parse_session(text: &str, dialect: &Dialect) -> Vec<RawEvent> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(want_type) = dialect.record_type_filter {
            if value.get("type").and_then(Value::as_str) != Some(want_type) {
                continue;
            }
        }
        let Some(unix_ts) = value.get("timestamp").and_then(Value::as_str) else {
            continue;
        };
        let unix_ts = parse_iso8601_unix(unix_ts);
        let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for item in content {
            if item.get("type").and_then(Value::as_str) != Some(dialect.content_item_type) {
                continue;
            }
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(op) = (dialect.tool_map)(name) else {
                continue;
            };
            let Some(path) = item.pointer(dialect.path_pointer).and_then(Value::as_str) else {
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

/// A single file-path touched during a session (mined from `RawEvent`s),
/// unified across read and edit/write events -- there is no separate
/// "reads have no timestamp" list; every touched path carries whatever
/// timestamp its most recent event had, regardless of which op that was.
pub(crate) struct MinedTouch {
    pub path: String,
    /// True iff any Edit/Write event was seen for this path (a Read-only
    /// path is `false`).
    pub was_edited: bool,
    pub newly_created: bool,
    /// The latest `unix_ts` seen for this path across *all* its events
    /// (read or edit/write) -- "last touched", not "last edited".
    pub last_touch_unix: Option<u64>,
}

pub(crate) struct Mined {
    pub touches: Vec<MinedTouch>,
    /// Timestamp of the first *tool-call* event mined from the session file
    /// (not the session's actual start -- a session can open with plain-text
    /// turns before its first tool call, which this doesn't see).
    pub first_op_unix: Option<u64>,
}

/// Mine a session JSONL file for file-path events, dispatching on the pane's
/// reported agent kind. Any kind without a parser (codex today, or a future
/// unknown agent) yields an empty `Mined` -- never a crash or misparse, so
/// callers can always fall through to the git/scrape layers (brief).
///
/// Reduction rule: scan `RawEvent`s in file order (== chronological, oldest
/// first, since JSONL is append-only). For each path, remember the *first*
/// op seen (`newly_created = first_op == RawOp::Write`), whether *any*
/// Edit/Write event was seen (`was_edited`), and the *latest* `unix_ts`
/// among *all* its events, read or edit/write alike (`last_touch_unix`).
/// Every path that was touched at all -- read, edited, or both -- ends up
/// exactly once in `touches`.
///
/// Note: `by_path.contains_key` is checked *before* calling `.entry(...)` so
/// `order` records each path's first-appearance position exactly once --
/// `.entry()` alone can't distinguish a fresh insert from an existing one
/// without an extra branch on its return value, so the presence check is
/// done up front instead.
pub(crate) fn mine_session(agent_kind: &str, text: &str) -> Mined {
    let raw: Vec<RawEvent> = match agent_kind {
        "pi" => parse_session(text, &PI_DIALECT),
        "claude" => parse_session(text, &CLAUDE_DIALECT),
        _ => Vec::new(),
    };

    let first_op_unix = raw.iter().find_map(|e| e.unix_ts);

    struct Acc {
        first_op: RawOp,
        last_touch_unix: Option<u64>,
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
            last_touch_unix: None,
            ever_edited: false,
        });
        if matches!(event.op, RawOp::Edit | RawOp::Write) {
            entry.ever_edited = true;
        }
        if event.unix_ts.is_some() {
            entry.last_touch_unix = event.unix_ts;
        }
    }

    let touches = order
        .into_iter()
        .map(|path| {
            let acc = &by_path[&path];
            MinedTouch {
                path: path.clone(),
                was_edited: acc.ever_edited,
                newly_created: matches!(acc.first_op, RawOp::Write),
                last_touch_unix: acc.last_touch_unix,
            }
        })
        .collect();

    Mined {
        touches,
        first_op_unix,
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
        let events = parse_session(text, &PI_DIALECT);
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
        let events = parse_session(text, &PI_DIALECT);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/repo/src/mod.rs");
        assert!(matches!(events[0].op, RawOp::Edit));
    }

    #[test]
    fn malformed_line_is_skipped_not_fatal() {
        let text = "not json at all\n{\"type\":\"message\",\"timestamp\":\"2026-07-22T11:08:00.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"read\",\"arguments\":{\"path\":\"/repo/a.rs\"}}]}}\n";
        let events = parse_session(text, &PI_DIALECT);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/repo/a.rs");
    }

    #[test]
    fn claude_basic_fixture_yields_read_write_edit_multiedit_ignoring_bash() {
        let text = include_str!("../tests/fixtures/session_claude_basic.jsonl");
        let events = parse_session(text, &CLAUDE_DIALECT);
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
        assert!(!mined.touches.is_empty());

        let claude_text = include_str!("../tests/fixtures/session_claude_basic.jsonl");
        let mined = mine_session("claude", claude_text);
        assert!(!mined.touches.is_empty());

        let mined = mine_session("codex", pi_text);
        assert!(
            mined.touches.is_empty(),
            "codex has no parser yet -- must degrade to empty, not crash or misparse"
        );

        let mined = mine_session("unknown-future-agent", pi_text);
        assert!(mined.touches.is_empty());
    }

    #[test]
    fn newly_created_true_only_when_first_op_is_write() {
        // pi fixture: /repo/src/new_mod.rs first (only) op is "write".
        // /repo/src/lib.rs first op is "read", later "edit" -> not newly created.
        let text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let mined = mine_session("pi", text);
        let new_mod = mined
            .touches
            .iter()
            .find(|t| t.path == "/repo/src/new_mod.rs")
            .unwrap();
        assert!(new_mod.newly_created);
        assert!(new_mod.was_edited);
        let lib = mined
            .touches
            .iter()
            .find(|t| t.path == "/repo/src/lib.rs")
            .unwrap();
        assert!(!lib.newly_created);
        assert!(lib.was_edited);
    }

    #[test]
    fn read_only_touch_has_was_edited_false_but_keeps_a_real_timestamp() {
        // Closes the semantic gap the old edits/reads split had: a read-only
        // path used to vanish into an untimed `reads: Vec<String>` list; now
        // it's a `MinedTouch` like any other, just with `was_edited: false`.
        let text = "{\"type\":\"session\",\"version\":3,\"id\":\"x\",\"timestamp\":\"2026-07-22T11:07:49.944Z\",\"cwd\":\"/repo\"}\n{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"2026-07-22T11:08:00.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"t1\",\"name\":\"read\",\"arguments\":{\"path\":\"/repo/src/readonly.rs\"}}],\"api\":\"anthropic-messages\",\"provider\":\"anthropic\",\"model\":\"claude-sonnet-5\"}}\n";
        let mined = mine_session("pi", text);
        assert_eq!(mined.touches.len(), 1);
        let touch = &mined.touches[0];
        assert_eq!(touch.path, "/repo/src/readonly.rs");
        assert!(!touch.was_edited);
        assert!(!touch.newly_created);
        // 2026-07-22T11:08:00.000Z, verified via `date -u -r 1784718480`.
        assert_eq!(touch.last_touch_unix, Some(1784718480));
    }

    #[test]
    fn first_op_unix_is_earliest_event_timestamp() {
        let text = include_str!("../tests/fixtures/session_pi_basic.jsonl");
        let mined = mine_session("pi", text);
        // first event's ts, 2026-07-22T11:08:00Z, verified via `date -u -r`.
        assert_eq!(mined.first_op_unix, Some(1784718480));
    }
}
