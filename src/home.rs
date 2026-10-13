//! Home directory lookup that also works when `HOME` is unset.
//!
//! On Windows `HOME` is not a standard variable: only `USERPROFILE` (and the
//! `HOMEDRIVE`/`HOMEPATH` pair) is guaranteed. herdr's server -- and so every
//! plugin process it spawns -- inherits the environment of whatever launched
//! herdr (a logon task, a GUI shortcut), which usually has no `HOME` at all.
//! Reading `HOME` directly therefore made the picker silently miss
//! `~/.claude/projects` (no session mining, "0 files"), the fff frecency DB,
//! the plugin config and the state dir on Windows.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

fn non_empty(var: &str) -> Option<OsString> {
    env::var_os(var).filter(|value| !value.is_empty())
}

/// `HOME`, else `USERPROFILE`, else `HOMEDRIVE` + `HOMEPATH`; `None` when none
/// of them is set. Callers that need a `String` convert with `to_string_lossy`.
pub(crate) fn home_dir() -> Option<PathBuf> {
    non_empty("HOME")
        .or_else(|| non_empty("USERPROFILE"))
        .or_else(|| {
            let mut joined = non_empty("HOMEDRIVE")?;
            joined.push(non_empty("HOMEPATH")?);
            Some(joined)
        })
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, not two: tests run in parallel threads and all share the
    // process environment, so separate HOME/USERPROFILE tests would race.
    #[test]
    fn home_then_userprofile_then_homedrive_homepath() {
        env::set_var("HOME", "/home/u");
        env::set_var("USERPROFILE", "/profile");
        assert_eq!(home_dir(), Some(PathBuf::from("/home/u")));

        env::remove_var("HOME");
        assert_eq!(home_dir(), Some(PathBuf::from("/profile")));

        env::remove_var("USERPROFILE");
        env::set_var("HOMEDRIVE", "C:");
        env::set_var("HOMEPATH", r"\Users\u");
        assert_eq!(home_dir(), Some(PathBuf::from(r"C:\Users\u")));

        env::remove_var("HOMEDRIVE");
        env::remove_var("HOMEPATH");
        assert_eq!(home_dir(), None);
    }
}
