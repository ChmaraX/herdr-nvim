//! Asking the person at the console a yes/no question, even when the
//! installer's stdin is the script it came from (`curl … | sh`).

use std::io::{BufRead, BufReader, Write};

/// `Some(answer)` when someone can answer, `None` when no one is there (an
/// agent's shell, CI). An empty answer is `default`.
pub fn ask(question: &str, default: bool) -> Option<bool> {
    let (mut input, mut output) = console()?;
    write!(
        output,
        "{question} [{}] ",
        if default { "Y/n" } else { "y/N" }
    )
    .ok()?;
    output.flush().ok()?;
    let mut line = String::new();
    input.read_line(&mut line).ok()?;
    Some(match line.trim().to_ascii_lowercase().as_str() {
        "" => default,
        answer => answer == "y" || answer == "yes",
    })
}

/// The controlling terminal: `/dev/tty`, since under `curl … | sh` stdin is
/// the script.
#[cfg(unix)]
fn console() -> Option<(Box<dyn BufRead>, Box<dyn Write>)> {
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let output = tty.try_clone().ok()?;
    Some((Box::new(BufReader::new(tty)), Box::new(output)))
}

/// The console the installer was started from: PowerShell runs it with its
/// own stdin and stdout, also under `irm … | iex`. Only when both are the
/// console: a hidden console (an agent's shell) would wait forever.
#[cfg(not(unix))]
fn console() -> Option<(Box<dyn BufRead>, Box<dyn Write>)> {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return None;
    }
    Some((
        Box::new(BufReader::new(std::io::stdin())),
        Box::new(std::io::stderr()),
    ))
}
