//! Asking a tab's nvim over its socket (msgpack-rpc), not by starting an
//! `nvim --remote-expr` for each question: that is a whole nvim starting
//! (12–50 ms) to send one request, and opening a tab's nvim asks several.
//! Unix sockets only; Windows' named pipes keep `--remote-expr`.
//!
//! The answers are what `--remote-expr` prints, so callers don't change:
//! Lua's `tostring` of the result (`2`, `abc`, `true`, `vim.NIL`, a table's
//! address), and none when nvim reports an error or nothing listens.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

/// `nvim_eval(expr)`, answered within `timeout` (else none: nvim answers no
/// request while it waits at a prompt).
pub(crate) fn eval_within(socket: &Path, expr: &str, timeout: Duration) -> Option<String> {
    let value = request(socket, "nvim_eval", &[expr], timeout)?;
    Some(as_lua_string(&value))
}

/// `nvim_input(keys)`: typed even while nvim waits for a key (a fast call).
pub(crate) fn input_within(socket: &Path, keys: &str, timeout: Duration) -> bool {
    request(socket, "nvim_input", &[keys], timeout).is_some()
}

/// A decoded msgpack value, as far as answers need.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Msg {
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<Msg>),
    Map(Vec<(Msg, Msg)>),
    Ext,
}

fn request(socket: &Path, method: &str, args: &[&str], timeout: Duration) -> Option<Msg> {
    let deadline = Instant::now() + timeout;
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    const ID: u64 = 1;
    stream.write_all(&encode_request(ID, method, args)).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        // Every whole message in the buffer: ours is a response [1, id, error, result].
        let mut at = 0;
        while let Some((message, used)) = decode(&buf[at..]) {
            at += used;
            if let Msg::Array(items) = message {
                if let [Msg::UInt(1) | Msg::Int(1), id, error, result] = items.as_slice() {
                    if matches!(id, Msg::UInt(n) if *n == ID)
                        || matches!(id, Msg::Int(n) if *n == ID as i64)
                    {
                        return (*error == Msg::Nil).then(|| result.clone());
                    }
                }
            }
        }
        buf.drain(..at);
        let left = deadline.checked_duration_since(Instant::now())?;
        stream
            .set_read_timeout(Some(left.max(Duration::from_millis(1))))
            .ok()?;
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// `[0, id, method, [args…]]` (a request with string arguments).
fn encode_request(id: u64, method: &str, args: &[&str]) -> Vec<u8> {
    let mut out = vec![0x94, 0x00];
    put_uint(&mut out, id);
    put_str(&mut out, method);
    put_array_len(&mut out, args.len());
    for arg in args {
        put_str(&mut out, arg);
    }
    out
}

fn put_uint(out: &mut Vec<u8>, n: u64) {
    match n {
        0..=0x7f => out.push(n as u8),
        0x80..=0xff => out.extend([0xcc, n as u8]),
        0x100..=0xffff => {
            out.push(0xcd);
            out.extend((n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(0xce);
            out.extend((n as u32).to_be_bytes());
        }
        _ => {
            out.push(0xcf);
            out.extend(n.to_be_bytes());
        }
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let len = s.len();
    match len {
        0..=31 => out.push(0xa0 | len as u8),
        32..=0xff => out.extend([0xd9, len as u8]),
        0x100..=0xffff => {
            out.push(0xda);
            out.extend((len as u16).to_be_bytes());
        }
        _ => {
            out.push(0xdb);
            out.extend((len as u32).to_be_bytes());
        }
    }
    out.extend(s.as_bytes());
}

fn put_array_len(out: &mut Vec<u8>, len: usize) {
    match len {
        0..=15 => out.push(0x90 | len as u8),
        16..=0xffff => {
            out.push(0xdc);
            out.extend((len as u16).to_be_bytes());
        }
        _ => {
            out.push(0xdd);
            out.extend((len as u32).to_be_bytes());
        }
    }
}

/// One whole message from the front of `buf` and its length; none until it
/// has all arrived.
pub(crate) fn decode(buf: &[u8]) -> Option<(Msg, usize)> {
    let mut at = 0;
    let value = value(buf, &mut at)?;
    Some((value, at))
}

fn take<'a>(buf: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
    let bytes = buf.get(*at..*at + n)?;
    *at += n;
    Some(bytes)
}

fn be(buf: &[u8], at: &mut usize, n: usize) -> Option<u64> {
    Some(
        take(buf, at, n)?
            .iter()
            .fold(0u64, |acc, b| acc << 8 | u64::from(*b)),
    )
}

fn value(buf: &[u8], at: &mut usize) -> Option<Msg> {
    let tag = *buf.get(*at)?;
    *at += 1;
    let text = |bytes: &[u8]| Msg::Str(String::from_utf8_lossy(bytes).into_owned());
    Some(match tag {
        0x00..=0x7f => Msg::UInt(u64::from(tag)),
        0x80..=0x8f => map(buf, at, usize::from(tag & 0x0f))?,
        0x90..=0x9f => array(buf, at, usize::from(tag & 0x0f))?,
        0xa0..=0xbf => text(take(buf, at, usize::from(tag & 0x1f))?),
        0xc0 => Msg::Nil,
        0xc2 => Msg::Bool(false),
        0xc3 => Msg::Bool(true),
        0xc4..=0xc6 => {
            let n = be(buf, at, 1 << (tag - 0xc4))? as usize;
            Msg::Bin(take(buf, at, n)?.to_vec())
        }
        0xc7..=0xc9 => {
            let n = be(buf, at, 1 << (tag - 0xc7))? as usize;
            take(buf, at, n + 1)?;
            Msg::Ext
        }
        0xca => Msg::Float(f64::from(f32::from_bits(be(buf, at, 4)? as u32))),
        0xcb => Msg::Float(f64::from_bits(be(buf, at, 8)?)),
        0xcc..=0xcf => Msg::UInt(be(buf, at, 1 << (tag - 0xcc))?),
        0xd0 => Msg::Int(i64::from(be(buf, at, 1)? as u8 as i8)),
        0xd1 => Msg::Int(i64::from(be(buf, at, 2)? as u16 as i16)),
        0xd2 => Msg::Int(i64::from(be(buf, at, 4)? as u32 as i32)),
        0xd3 => Msg::Int(be(buf, at, 8)? as i64),
        0xd4..=0xd8 => {
            take(buf, at, 1 + (1 << (tag - 0xd4)))?;
            Msg::Ext
        }
        0xd9..=0xdb => {
            let n = be(buf, at, 1 << (tag - 0xd9))? as usize;
            text(take(buf, at, n)?)
        }
        0xdc | 0xdd => {
            let n = be(buf, at, if tag == 0xdc { 2 } else { 4 })? as usize;
            array(buf, at, n)?
        }
        0xde | 0xdf => {
            let n = be(buf, at, if tag == 0xde { 2 } else { 4 })? as usize;
            map(buf, at, n)?
        }
        0xe0..=0xff => Msg::Int(i64::from(tag as i8)),
        0xc1 => return None,
    })
}

fn array(buf: &[u8], at: &mut usize, n: usize) -> Option<Msg> {
    (0..n)
        .map(|_| value(buf, at))
        .collect::<Option<Vec<_>>>()
        .map(Msg::Array)
}

fn map(buf: &[u8], at: &mut usize, n: usize) -> Option<Msg> {
    (0..n)
        .map(|_| Some((value(buf, at)?, value(buf, at)?)))
        .collect::<Option<Vec<_>>>()
        .map(Msg::Map)
}

/// What `nvim --remote-expr` prints for an answer: Lua's `tostring`.
fn as_lua_string(value: &Msg) -> String {
    match value {
        Msg::Nil => "vim.NIL".to_owned(),
        Msg::Bool(b) => b.to_string(),
        Msg::Int(n) => n.to_string(),
        Msg::UInt(n) => n.to_string(),
        // LuaJIT prints a whole float as an integer (`2`), as `{}` does.
        Msg::Float(f) => format!("{f}"),
        Msg::Str(s) => s.clone(),
        Msg::Bin(b) => String::from_utf8_lossy(b).into_owned(),
        Msg::Array(_) | Msg::Map(_) | Msg::Ext => "table: 0x0".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Against a real nvim: each answer is what `--remote-expr` prints for
    /// it, an error or a missing server is none, and a prompt doesn't hold
    /// the caller past its timeout.
    #[test]
    fn answers_as_remote_expr_prints_them() {
        let socket = std::env::temp_dir().join(format!("hnv-rpc-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let Ok(mut nvim) = Command::new("nvim")
            .args(["--headless", "--clean", "--listen"])
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            eprintln!("no nvim: skipped");
            return;
        };
        let start = Instant::now();
        while !socket.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ask = |expr: &str| eval_within(&socket, expr, Duration::from_secs(2));
        let remote = |expr: &str| {
            let out = Command::new("nvim")
                .args(["--headless", "--server"])
                .arg(&socket)
                .args(["--remote-expr", expr])
                .output()
                .unwrap();
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        for expr in [
            "1+1",
            "\"abc\"",
            "v:true",
            "v:null",
            "1.5",
            "2.0",
            "0-7",
            "getpid()",
            "json_encode({'a': [1, 'x']})",
            "repeat('é', 5000)",
        ] {
            assert_eq!(ask(expr), remote(expr), "{expr}");
        }
        assert_eq!(ask("nosuchvar"), None);
        assert_eq!(
            eval_within(
                Path::new("/nonexistent/x.sock"),
                "1",
                Duration::from_secs(1)
            ),
            None
        );
        // A socket that never answers (nvim busy, waiting at a prompt):
        // none, within the timeout.
        let silent = socket.with_extension("silent");
        let _ = std::fs::remove_file(&silent);
        let listener = std::os::unix::net::UnixListener::bind(&silent).unwrap();
        let hold = std::thread::spawn(move || {
            listener.accept().map(|(conn, _)| {
                std::thread::sleep(Duration::from_secs(2));
                drop(conn);
            })
        });
        let asked = Instant::now();
        assert_eq!(
            eval_within(&silent, "1+1", Duration::from_millis(300)),
            None
        );
        assert!(
            asked.elapsed() < Duration::from_millis(900),
            "waited {:?}",
            asked.elapsed()
        );
        let _ = hold.join();
        let _ = std::fs::remove_file(&silent);
        // Typed keys reach nvim (`unstick`'s <C-\\><C-n>).
        assert!(input_within(
            &socket,
            ":let g:typed = 1<CR>",
            Duration::from_secs(2)
        ));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(ask("g:typed").as_deref(), Some("1"));
        let _ = nvim.kill();
        let _ = nvim.wait();
        let _ = std::fs::remove_file(&socket);
    }
}
