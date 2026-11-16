//! fff-search backend for the picker's fuzzy fallback tier.
//!
//! [`FffIndex::open`] starts an fff-search scan of the workspace; by the
//! first keystroke the index is warm. [`FffIndex::search`] returns ranked
//! repo-relative hits with match highlight spans.
//!
//! Frecency reuse is read-only: fff.nvim's DB (`stdpath('cache')/fff_nvim`)
//! is copied to a temp dir before opening, since `FrecencyTracker::open`
//! writes (garbage-collects) on open.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fff_search::file_picker::FilePicker;
use fff_search::frecency::FrecencyTracker;
use fff_search::{
    FFFMode, FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser, SharedFilePicker,
    SharedFrecency,
};

/// One ranked hit from the fff tier: the absolute path of the file and
/// byte-offset highlight spans **into its workspace-relative path** (which
/// is exactly what the picker displays for files under the workspace cwd).
pub struct FffHit {
    pub path: String,
    pub highlights: Vec<(usize, usize)>,
}

/// Cap on hits pulled from fff per keystroke. The overlay viewport shows
/// ~a dozen rows; 100 leaves plenty of scroll depth without ferrying
/// thousands of matches per keystroke.
const PAGE_LIMIT: usize = 100;

/// How long `search` will wait for the initial background scan. The scan
/// runs concurrently with the user reading/typing, so this is almost
/// always already satisfied; the bound only guards degenerate cases
/// (enormous cold trees, slow disks).
const SCAN_WAIT: Duration = Duration::from_secs(3);

pub struct FffIndex {
    shared: SharedFilePicker,
    cwd: String,
    /// Temp dir holding the frecency-DB copy (and/or scratch DB); removed
    /// on drop.
    tmp: Option<PathBuf>,
}

impl FffIndex {
    /// Start indexing `cwd`. Returns `None` (the caller leaves the fuzzy
    /// fallback tier empty for this popup) if fff can't be set up.
    /// `use_frecency` (config `[picker] frecency`, default true) gates the
    /// fff.nvim frecency-DB reuse; ranking works without it.
    pub fn open(cwd: &str, use_frecency: bool) -> Option<Self> {
        let shared = SharedFilePicker::default();
        let shared_frecency = SharedFrecency::default();

        // Read-only use of the user's fff.nvim frecency: copy, then open
        // the copy. Never open the user's DB directly (open() GCs = writes).
        let mut tmp = None;
        if use_frecency {
            if let Some((tracker, dir)) = open_frecency_copy() {
                if shared_frecency.init(tracker).is_ok() {
                    tmp = Some(dir);
                }
            }
        }

        FilePicker::new_with_shared_state(
            shared.clone(),
            shared_frecency,
            FilePickerOptions {
                base_path: cwd.into(),
                mode: FFFMode::Ai,
                watch: false,
                ..Default::default()
            },
        )
        .ok()?;

        Some(Self {
            shared,
            cwd: cwd.to_owned(),
            tmp,
        })
    }

    /// Ranked fff hits for `query`, in fff's own score order. Empty on
    /// any failure (scan timeout, lock poison) -- the caller treats that
    /// as "no extra tier".
    pub fn search(&self, query: &str) -> Vec<FffHit> {
        if !self.shared.wait_for_scan(SCAN_WAIT) {
            return Vec::new();
        }
        let Ok(guard) = self.shared.read() else {
            return Vec::new();
        };
        let Some(picker) = guard.as_ref() else {
            return Vec::new();
        };
        let parsed = QueryParser::default().parse(query);
        let results = picker.fuzzy_search(
            &parsed,
            None,
            FuzzySearchOptions {
                max_threads: 0,
                current_file: None,
                pagination: PaginationArgs {
                    offset: 0,
                    limit: PAGE_LIMIT,
                },
                ..Default::default()
            },
        );
        results
            .items
            .iter()
            .zip(results.match_byte_offsets.iter())
            .map(|(item, offsets)| {
                let rel = item.relative_path(picker);
                FffHit {
                    path: format!("{}/{rel}", self.cwd.trim_end_matches('/')),
                    highlights: offsets
                        .iter()
                        .map(|&(s, e)| (s as usize, e as usize))
                        .collect(),
                }
            })
            .collect()
    }
}

impl Drop for FffIndex {
    fn drop(&mut self) {
        if let Some(tmp) = self.tmp.take() {
            let _ = std::fs::remove_dir_all(tmp);
        }
    }
}

/// Copy fff.nvim's default frecency DB (`stdpath('cache')/fff_nvim`) to a
/// temp dir and open the copy. Returns `None` when the DB doesn't exist
/// or anything fails; a torn copy (fff.nvim writing mid-copy) fails the
/// open and lands here too.
fn open_frecency_copy() -> Option<(FrecencyTracker, PathBuf)> {
    let src = nvim_cache_dir(|var| std::env::var_os(var))?.join("fff_nvim");
    if !src.is_dir() {
        return None;
    }
    let tmp = std::env::temp_dir().join(format!("herdr-nvim-fff-{}", std::process::id()));
    if copy_dir(&src, &tmp).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        return None;
    }
    match FrecencyTracker::open(&tmp) {
        Ok(tracker) => Some((tracker, tmp)),
        Err(_) => {
            let _ = std::fs::remove_dir_all(&tmp);
            None
        }
    }
}

/// nvim's `stdpath('cache')` over an env lookup, by nvim's own rule
/// (`src/nvim/os/stdpaths.c`, `get_xdg_home(kXDGCacheHome)`): the base is
/// `$XDG_CACHE_HOME`, else on Windows `$TEMP`, else `~/.cache` (Windows:
/// `~\AppData\Local\Temp`), then `$NVIM_APPNAME` or `nvim`. Windows adds
/// no `-data` to the cache dir, whatever `:help base-directories` says.
fn nvim_cache_dir(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    let base = set("XDG_CACHE_HOME")
        .or_else(|| if cfg!(windows) { set("TEMP") } else { None })
        .map(PathBuf::from)
        .or_else(|| {
            let home = crate::home::home_dir()?;
            Some(if cfg!(windows) {
                home.join(r"AppData\Local\Temp")
            } else {
                home.join(".cache")
            })
        })?;
    Some(base.join(set("NVIM_APPNAME").unwrap_or_else(|| "nvim".into())))
}

/// Shallow copy of the LMDB env dir (data.mdb + lock.mdb; the DB is
/// capped at ~10MiB by fff-search, so this is a few ms at worst).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::nvim_available;
    use std::process::Command;

    type Vars<'a> = [(&'a str, Option<&'a str>)];

    /// What nvim itself answers for `stdpath('cache')` with `vars` set (or
    /// unset, for `None`) on top of this process's environment.
    fn nvim_says(vars: &Vars) -> PathBuf {
        let mut nvim = Command::new("nvim");
        nvim.args(["--headless", "-u", "NONE", "-i", "NONE"])
            .args(["--cmd", "lua io.stdout:write(vim.fn.stdpath('cache'))"])
            .args(["--cmd", "qa!"]);
        for (name, value) in vars {
            match value {
                Some(value) => nvim.env(name, value),
                None => nvim.env_remove(name),
            };
        }
        let out = nvim.output().expect("run nvim");
        PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
    }

    #[test]
    fn cache_dir_is_where_nvim_puts_its_cache() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let elsewhere = std::env::temp_dir().join("herdr-nvim-xdg-cache");
        let elsewhere = elsewhere.to_str().unwrap();
        let cases: [&Vars; 3] = [
            &[("XDG_CACHE_HOME", None), ("NVIM_APPNAME", None)],
            &[("XDG_CACHE_HOME", Some(elsewhere)), ("NVIM_APPNAME", None)],
            &[
                ("XDG_CACHE_HOME", None),
                ("NVIM_APPNAME", Some("herdr-nvim-test")),
            ],
        ];
        for vars in cases {
            let ours = nvim_cache_dir(|name| match vars.iter().find(|(set, _)| *set == name) {
                Some((_, value)) => value.map(OsString::from),
                None => std::env::var_os(name),
            });
            assert_eq!(ours, Some(nvim_says(vars)), "with {vars:?}");
        }
    }
}
