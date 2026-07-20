use std::{env, fs, io::ErrorKind, path::PathBuf};

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
    pub plan_steps: Vec<PlanStep>,
    pub sidebar_pane: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct PlanStep {
    pub pane: String,
    pub dir: String,
    pub target: String,
    pub ratio: f64,
}

pub fn state_path(workspace: &str) -> PathBuf {
    let base = env::var_os("HERDR_NVIM_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME").map(|path| PathBuf::from(path).join("herdr-nvim"))
        })
        .or_else(|| {
            env::var_os("HOME").map(|path| PathBuf::from(path).join(".local/state/herdr-nvim"))
        })
        .unwrap_or_else(|| PathBuf::from(".herdr-nvim-state"));
    base.join(format!("{workspace}.json"))
}

pub fn load(workspace: &str) -> Result<Option<StateFile>> {
    let path = state_path(workspace);
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
    let path = state_path(&s.workspace);
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

pub fn remove(workspace: &str) -> Result<()> {
    let path = state_path(workspace);
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
    use super::*;

    #[test]
    fn state_roundtrip() {
        std::env::set_var(
            "HERDR_NVIM_STATE_DIR",
            std::env::temp_dir().join("hn-state-test"),
        );
        let s = StateFile {
            phase: Phase::Open,
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            anchor: "wT:p1".into(),
            parking_tab: None,
            parked: vec![],
            plan_steps: vec![PlanStep {
                pane: "wT:p2".into(),
                dir: "right".into(),
                target: "wT:p1".into(),
                ratio: 0.4,
            }],
            sidebar_pane: Some("wT:p9".into()),
        };
        save(&s).unwrap();
        let loaded = load("wT").unwrap().unwrap();
        assert_eq!(loaded.tab, "wT:t1");
        assert_eq!(loaded.plan_steps.len(), 1);
        remove("wT").unwrap();
        assert!(load("wT").unwrap().is_none());
    }
}
