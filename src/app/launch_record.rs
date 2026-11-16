//! What an install of one app home points at: `mods/herdr-nvim/install.json`.
//! Written only by the installer (`install::mods`), read only by the mod's
//! launcher (`src/bin/herdr-nvim-launch.rs`, which includes this file).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct InstallRecord {
    pub schema: u32,
    /// The app home, as the install resolved it.
    pub home: PathBuf,
    /// `herdr-app` for that home.
    pub app_bin: PathBuf,
    /// The runtime it runs: `<data>/herdr-nvim/app/<stamp>`.
    pub runtime: PathBuf,
    /// `herdr-nvim <version>`: which release wrote it, and the mark that the
    /// mod folder is herdr-nvim's.
    pub by: String,
}

impl InstallRecord {
    pub const FILE: &'static str = "install.json";
    pub const SCHEMA: u32 = 1;

    pub fn read(mod_dir: &Path) -> Result<Self, String> {
        let path = mod_dir.join(Self::FILE);
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let record: Self = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not an install record: {e}", path.display()))?;
        if record.schema != Self::SCHEMA {
            return Err(format!(
                "{} has schema {}, this herdr-nvim reads {}",
                path.display(),
                record.schema,
                Self::SCHEMA
            ));
        }
        Ok(record)
    }

    /// Replaces the record whole: a temp file renamed over it, so the
    /// launcher never reads half of one.
    pub fn write_atomic(&self, mod_dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)? + "\n";
        let temp = mod_dir.join(format!(".{}.{}", Self::FILE, std::process::id()));
        fs::write(&temp, text)?;
        fs::rename(&temp, mod_dir.join(Self::FILE)).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }

    pub fn runtime_exe(&self) -> PathBuf {
        self.runtime
            .join("bin")
            .join(format!("herdr-nvim{}", std::env::consts::EXE_SUFFIX))
    }
}
