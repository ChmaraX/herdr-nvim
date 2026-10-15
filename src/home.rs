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

/// `HOME`, else `USERPROFILE`, else `HOMEDRIVE` + `HOMEPATH`; `None` when none
/// of them is set. Callers that need a `String` convert with `to_string_lossy`.
pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_from(|var| env::var_os(var))
}

/// Pure: [`home_dir`] over an injected env lookup; empty values count as unset.
fn home_dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let non_empty = |name: &str| var(name).filter(|value| !value.is_empty());
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

    fn home_with(vars: &[(&str, &str)]) -> Option<PathBuf> {
        home_dir_from(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn prefers_home() {
        let vars = [("HOME", "/home/u"), ("USERPROFILE", r"C:\Users\u")];
        assert_eq!(home_with(&vars), Some(PathBuf::from("/home/u")));
    }

    #[test]
    fn falls_back_to_userprofile_when_home_is_unset_or_empty() {
        let profile = Some(PathBuf::from(r"C:\Users\u"));
        assert_eq!(home_with(&[("USERPROFILE", r"C:\Users\u")]), profile);
        let vars = [("HOME", ""), ("USERPROFILE", r"C:\Users\u")];
        assert_eq!(home_with(&vars), profile);
    }

    #[test]
    fn falls_back_to_homedrive_plus_homepath() {
        let vars = [("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\u")];
        assert_eq!(home_with(&vars), Some(PathBuf::from(r"C:\Users\u")));
        assert_eq!(home_with(&[("HOMEDRIVE", "C:")]), None);
    }

    #[test]
    fn none_when_nothing_is_set() {
        assert_eq!(home_with(&[]), None);
    }
}
