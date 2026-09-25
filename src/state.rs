use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

/// A herdr tab id, `<workspace>:<tab>` (e.g. `w26:t2`). The single place that
/// knows how a tab id splits into its workspace and how it maps to the
/// sanitized key naming its daemon socket and state file.
///
/// A key cannot be turned back into a tab id (`_` may occur in either part),
/// so code holding only keys either asks the daemon for its `$HERDR_TAB_ID`
/// or compares against `key()` of the tab ids herdr reports.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub(crate) struct TabId(String);

impl TabId {
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The workspace prefix before the first `:`. An id without a `:` (never
    /// expected in practice) is returned unchanged rather than panicking.
    pub(crate) fn workspace(&self) -> &str {
        self.0
            .split_once(':')
            .map_or(&self.0, |(workspace, _)| workspace)
    }

    /// The sanitized filename key (see `tab_key`).
    pub(crate) fn key(&self) -> String {
        tab_key(&self.0)
    }

    /// Whether `key` belongs to a tab of `workspace`: every such tab id starts
    /// with `<workspace>:`, so its key starts with that prefix's key. The
    /// separator keeps workspace `w7` from matching `w7B`'s tabs.
    pub(crate) fn key_in_workspace(key: &str, workspace: &str) -> bool {
        key.starts_with(&tab_key(&format!("{workspace}:")))
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Remove a file, treating "already gone" as success.
pub(crate) fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
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

/// Path for the settle-handshake marker `maneuver::open` touches once layout
/// moves are complete and `daemon::sidebar_cmd` polls for before attaching
/// nvim's UI. `nonce` must be caller-unique per open so a marker left by an
/// earlier, aborted open can never be mistaken for the new one's signal.
pub fn ready_marker_path(tab: &str, nonce: u128) -> PathBuf {
    state_dir().join(format!("{}.{nonce}.ready", tab_key(tab)))
}

/// Markers older than this are assumed abandoned: nothing was left to
/// consume and remove them, whether `open()` crashed or the consumer's
/// timeout path gave up before seeing them.
const STALE_MARKER_AGE: Duration = Duration::from_secs(60);

/// Best-effort removal of `*.ready` marker files older than
/// `STALE_MARKER_AGE`, so leaked markers don't accumulate in the state dir.
pub fn sweep_stale_markers() {
    let Ok(entries) = fs::read_dir(state_dir()) else {
        return;
    };
    let Some(cutoff) = SystemTime::now().checked_sub(STALE_MARKER_AGE) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("ready") {
            continue;
        }
        let is_stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < cutoff);
        if is_stale {
            let _ = fs::remove_file(&path);
        }
    }
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
/// stem, which is already in filename form). Used by
/// `daemon::registry::stop_tab_key`, which only ever has the sanitized key on
/// hand -- calling `remove` there would re-sanitize an already-sanitized key,
/// which only happens to be a no-op because sanitization is idempotent.
pub(crate) fn remove_key(key: &str) -> Result<()> {
    remove_file_if_exists(&path_for_key(key)).context("failed to remove state file")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TestEnv;

    fn with_state_dir(test: impl FnOnce()) {
        let _env = TestEnv::new();
        test();
    }

    #[test]
    fn tab_id_splits_workspace_and_key() {
        let tab = TabId::new("w39:t1");
        assert_eq!(tab.workspace(), "w39");
        assert_eq!(tab.key(), "w39_t1");
        // Only the first colon splits; the rest belongs to the tab segment.
        assert_eq!(TabId::new("w39:t1:x").workspace(), "w39");
        // No colon / empty: returned unchanged rather than panicking.
        assert_eq!(TabId::new("w39").workspace(), "w39");
        assert_eq!(TabId::new("").workspace(), "");
    }

    #[test]
    fn key_in_workspace_is_exact() {
        assert!(TabId::key_in_workspace("wA_t1", "wA"));
        assert!(!TabId::key_in_workspace("wAB_t1", "wA"), "prefix trap");
        assert!(!TabId::key_in_workspace("wA", "wA"));
        assert!(!TabId::key_in_workspace("wB_t1", "wA"));
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

    #[test]
    fn sweep_stale_markers_removes_old_but_keeps_fresh() {
        with_state_dir(|| {
            let stale = ready_marker_path("wT:t1", 1);
            let fresh = ready_marker_path("wT:t1", 2);
            fs::create_dir_all(stale.parent().unwrap()).unwrap();
            fs::write(&stale, b"").unwrap();
            fs::write(&fresh, b"").unwrap();
            let old_time =
                std::time::SystemTime::now() - (STALE_MARKER_AGE + Duration::from_secs(1));
            fs::OpenOptions::new()
                .write(true)
                .open(&stale)
                .unwrap()
                .set_modified(old_time)
                .unwrap();

            sweep_stale_markers();

            assert!(!stale.exists(), "stale marker should have been swept");
            assert!(fresh.exists(), "fresh marker should have been kept");
        });
    }
}
