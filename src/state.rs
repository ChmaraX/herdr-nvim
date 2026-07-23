use std::{env, fs, io::ErrorKind, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(test)]
pub static STATE_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Serialize, Deserialize)]
pub enum Phase {
    Evacuating,
    Open,
}

#[derive(Serialize, Deserialize)]
pub struct StateFile {
    pub phase: Phase,
    pub workspace: String,
    pub tab: String,
    pub anchor: String,
    pub parking_tab: Option<String>,
    pub parked: Vec<String>,
    pub plan_steps: Vec<crate::layout::MoveStep>,
    pub sidebar_pane: Option<String>,
}

/// Sanitize a key (workspace or tab id) for use as a filename. Tab ids
/// contain a colon (e.g. `wG:t1`); replace it with `_` so the id is a valid,
/// single path component (`wG_t1`). Shared by `state` and `daemon` so the
/// orchestrator and the sidebar pane compute the same path.
pub(crate) fn tab_key(key: &str) -> String {
    key.replace(':', "_")
}

fn state_dir() -> PathBuf {
    env::var_os("HERDR_NVIM_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME").map(|path| PathBuf::from(path).join("herdr-nvim"))
        })
        .or_else(|| {
            env::var_os("HOME").map(|path| PathBuf::from(path).join(".local/state/herdr-nvim"))
        })
        .unwrap_or_else(|| PathBuf::from(".herdr-nvim-state"))
}

fn path_for_key(key: &str) -> PathBuf {
    state_dir().join(format!("{key}.json"))
}

pub fn state_path(tab: &str) -> PathBuf {
    path_for_key(&tab_key(tab))
}

pub fn load(tab: &str) -> Result<Option<StateFile>> {
    let path = state_path(tab);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse state file {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read state file {}", path.display()))
        }
    }
}

pub fn save(s: &StateFile) -> Result<()> {
    let path = state_path(&s.tab);
    let parent = path
        .parent()
        .with_context(|| format!("state file path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create state directory {}", parent.display()))?;

    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(s).context("failed to serialize state file")?;
    fs::write(&tmp, bytes)
        .with_context(|| format!("failed to write temp state file {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| {
        format!(
            "failed to replace state file {} with {}",
            path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

pub fn remove(tab: &str) -> Result<()> {
    remove_key(&tab_key(tab))
}

/// Remove the state file for an already-sanitized key (e.g. a socket file
/// stem, which is already in filename form). Used by `daemon::gc`, which
/// only ever has the sanitized key on hand -- calling `remove` there would
/// re-sanitize an already-sanitized key, which only happens to be a no-op
/// because sanitization is idempotent.
pub(crate) fn remove_key(key: &str) -> Result<()> {
    let path = path_for_key(key);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to remove state file {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, sync::MutexGuard};

    use super::*;

    struct StateDirGuard {
        _lock: MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
        dir: PathBuf,
    }

    impl StateDirGuard {
        fn new(dir: PathBuf) -> Self {
            let lock = STATE_DIR_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let old = env::var_os("HERDR_NVIM_STATE_DIR");
            let _ = fs::remove_dir_all(&dir);
            env::set_var("HERDR_NVIM_STATE_DIR", &dir);
            Self {
                _lock: lock,
                old,
                dir,
            }
        }
    }

    impl Drop for StateDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
            match &self.old {
                Some(value) => env::set_var("HERDR_NVIM_STATE_DIR", value),
                None => env::remove_var("HERDR_NVIM_STATE_DIR"),
            }
        }
    }

    fn with_state_dir(test: impl FnOnce()) {
        let _guard = StateDirGuard::new(env::temp_dir().join("hn-state-test"));
        test();
    }

    #[test]
    fn state_roundtrip() {
        with_state_dir(|| {
            use crate::{herdr::Dir, layout::MoveStep};
            let s = StateFile {
                phase: Phase::Open,
                workspace: "wT".into(),
                tab: "wT:t1".into(),
                anchor: "wT:p1".into(),
                parking_tab: None,
                parked: vec![],
                plan_steps: vec![MoveStep {
                    pane: "wT:p2".into(),
                    dir: Dir::Right,
                    target: "wT:p1".into(),
                    ratio: 0.4,
                }],
                sidebar_pane: Some("wT:p9".into()),
            };
            save(&s).unwrap();
            let loaded = load("wT:t1").unwrap().unwrap();
            assert_eq!(loaded.tab, "wT:t1");
            assert_eq!(loaded.plan_steps.len(), 1);
            remove("wT:t1").unwrap();
            assert!(load("wT:t1").unwrap().is_none());
        });
    }

    #[test]
    fn state_path_sanitizes_colon_in_tab_id() {
        with_state_dir(|| {
            let path = state_path("wX:t1");
            assert_eq!(path.file_name().unwrap(), "wX_t1.json");
        });
    }
}
