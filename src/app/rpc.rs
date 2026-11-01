//! A small client for herdr-core's API through `hc connect` (JSON-RPC 2.0,
//! one message per line; framed after `hello {framed: true}`).
//!
//! The `hc` CLI covers most of what the app host needs, but not three things:
//! a terminal's labels in a listing (`terminal.list`), the app's layout
//! document (`doc.get`, where spaces and tabs live), and typing text into a
//! terminal without pressing Enter (`terminal.attach`, framed).

use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

/// The `hc` binary (`HERDR_NVIM_HC` overrides; PATH already includes
/// `~/.local/bin`, see `path::augment_path`).
pub(crate) fn hc_bin() -> String {
    env::var("HERDR_NVIM_HC").unwrap_or_else(|_| "hc".to_owned())
}

/// A `hc` command for the CLI paths (`hc read`, `hc send`, `hc events`).
pub(crate) fn hc() -> Command {
    Command::new(hc_bin())
}

/// Line-mode connection: requests and replies are JSON lines.
pub(crate) struct Rpc {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Rpc {
    pub(crate) fn connect() -> Result<Self> {
        let mut child = hc()
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("cannot run `hc connect`: is herdr-core installed?")?;
        let stdin = child.stdin.take().context("hc connect: no stdin")?;
        let reader = BufReader::new(child.stdout.take().context("hc connect: no stdout")?);
        let mut rpc = Self {
            child,
            stdin,
            reader,
            next_id: 1,
        };
        rpc.call("hello", json!({}))?;
        Ok(rpc)
    }

    pub(crate) fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").with_context(|| format!("{method}: core gone"))?;
        self.stdin.flush()?;
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                bail!("{method}: herdr-core closed the connection");
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            return reply(method, message);
        }
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reply(method: &str, message: Value) -> Result<Value> {
    if let Some(error) = message.get("error") {
        let kind = error
            .pointer("/data/kind")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        bail!("{method} failed: {kind} {text}");
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

/// One call on a fresh connection.
pub(crate) fn call(method: &str, params: Value) -> Result<Value> {
    Rpc::connect()?.call(method, params)
}

const HEADER: usize = 9;
const DATA: u8 = 0;
const CREDIT: u8 = 1;
const CLOSE: u8 = 2;

fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_frame(reader: &mut impl Read) -> Option<(u32, u8, Vec<u8>)> {
    let mut header = [0u8; HEADER];
    reader.read_exact(&mut header).ok()?;
    let channel = u32::from_le_bytes(header[0..4].try_into().ok()?);
    let len = u32::from_le_bytes(header[5..9].try_into().ok()?) as usize;
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).ok()?;
    Some((channel, header[4], payload))
}

/// Bracketed paste on: the program wants pasted text wrapped, so newlines in it
/// don't submit (every coding agent asks for this).
const PASTE_ON: &[u8] = b"\x1b[?2004h";
const PASTE_OFF: &[u8] = b"\x1b[?2004l";

/// Whether the screen replay leaves bracketed paste on: the last of the two
/// mode switches wins; none means off.
pub(crate) fn bracketed_paste_on(replay: &[u8]) -> bool {
    let last = |needle: &[u8]| replay.windows(needle.len()).rposition(|w| w == needle);
    match (last(PASTE_ON), last(PASTE_OFF)) {
        (Some(on), Some(off)) => on > off,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The bytes a paste of `text` sends: wrapped when the program asked for
/// bracketed paste, else as typed with newlines as carriage returns.
pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Paste `text` into `terminal` without pressing Enter, like a person pasting:
/// attach as a second viewer at the terminal's own size (so nothing resizes),
/// read the screen replay to learn the program's paste mode, send the bytes,
/// detach.
pub(crate) fn paste(terminal: &str, text: &str) -> Result<()> {
    let facts = call("terminal.get", json!({"terminal": terminal}))?;
    let cols = facts.get("cols").and_then(Value::as_u64).unwrap_or(80);
    let rows = facts.get("rows").and_then(Value::as_u64).unwrap_or(24);

    let mut child = hc()
        .arg("connect")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot run `hc connect`")?;
    let result = (|| -> Result<()> {
        let mut stdin = child.stdin.take().context("hc connect: no stdin")?;
        let mut reader = BufReader::new(child.stdout.take().context("hc connect: no stdout")?);
        stdin.write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"hello\",\"params\":{\"framed\":true}}\n",
        )?;
        stdin.flush()?;
        let mut hello = String::new();
        reader.read_line(&mut hello)?;
        reply(
            "hello",
            serde_json::from_str(&hello).context("bad hello reply")?,
        )?;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some(frame) = read_frame(&mut reader) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });

        let attach = json!({"jsonrpc": "2.0", "id": 1, "method": "terminal.attach",
            "params": {"terminal": terminal, "cols": cols, "rows": rows, "history": 0}});
        stdin.write_all(&encode(0, DATA, attach.to_string().as_bytes()))?;
        stdin.flush()?;

        let deadline = Instant::now() + Duration::from_secs(5);
        let channel = loop {
            let (channel, _kind, payload) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| anyhow!("terminal.attach: no reply from herdr-core"))?;
            if channel != 0 {
                continue;
            }
            let message: Value = serde_json::from_slice(&payload)?;
            if message.get("id").and_then(Value::as_u64) != Some(1) {
                continue;
            }
            let result = reply("terminal.attach", message)?;
            break result
                .get("channel")
                .and_then(Value::as_u64)
                .context("terminal.attach returned no channel")? as u32;
        };
        stdin.write_all(&encode(channel, CREDIT, &(1u32 << 20).to_le_bytes()))?;
        stdin.flush()?;

        // The replay arrives right after the attach; a short quiet gap ends it.
        let mut replay = Vec::new();
        let replay_deadline = Instant::now() + Duration::from_millis(800);
        while let Ok((frame_channel, kind, payload)) = rx.recv_timeout(
            Duration::from_millis(150)
                .min(replay_deadline.saturating_duration_since(Instant::now())),
        ) {
            if frame_channel == channel && kind == DATA {
                replay.extend_from_slice(&payload);
            }
            if Instant::now() >= replay_deadline {
                break;
            }
        }

        let bytes = paste_bytes(text, bracketed_paste_on(&replay));
        stdin.write_all(&encode(channel, DATA, &bytes))?;
        stdin.write_all(&encode(channel, CLOSE, &[]))?;
        stdin.flush()?;
        // Let the core take the input before the connection goes.
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_mode_follows_the_last_switch_in_the_replay() {
        assert!(!bracketed_paste_on(b"plain shell output"));
        assert!(bracketed_paste_on(b"\x1b[?1049h\x1b[?2004hprompt> "));
        assert!(!bracketed_paste_on(b"\x1b[?2004h...\x1b[?2004l$ "));
        assert!(bracketed_paste_on(b"\x1b[?2004l...\x1b[?2004h> "));
    }

    #[test]
    fn multi_line_paste_is_wrapped_only_when_the_program_asked() {
        assert_eq!(
            paste_bytes("a\nb", true),
            b"\x1b[200~a\nb\x1b[201~".to_vec()
        );
        assert_eq!(paste_bytes("a\nb", false), b"a\rb".to_vec());
        // A payload can't end the paste early.
        assert_eq!(
            paste_bytes("x\x1b[201~y", true),
            b"\x1b[200~xy\x1b[201~".to_vec()
        );
    }

    #[test]
    fn frames_round_trip() {
        let frame = encode(7, DATA, b"hello");
        let (channel, kind, payload) = read_frame(&mut frame.as_slice()).unwrap();
        assert_eq!(
            (channel, kind, payload.as_slice()),
            (7, DATA, &b"hello"[..])
        );
    }
}
