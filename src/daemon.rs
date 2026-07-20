// TODO(Task 6): real daemon lifecycle.
pub fn sidebar_shell_cmd(_workspace: &str) -> String {
    let path = std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_else(|| "herdr-nvim".to_owned());
    let path = if path.contains(' ') {
        format!("'{}'", path.replace('\'', "'\\''"))
    } else {
        path
    };
    format!("exec {path} sidebar")
}

pub fn sidebar_cmd() -> anyhow::Result<()> {
    anyhow::bail!("not implemented")
}

pub fn gc_cmd() -> anyhow::Result<()> {
    anyhow::bail!("not implemented")
}
