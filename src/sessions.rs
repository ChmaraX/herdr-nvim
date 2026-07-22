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
}
