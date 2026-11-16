//! The installed app mod as its launcher reads it. Written only by the
//! installer (`install::mods`), read by the launcher
//! (`src/bin/herdr-nvim-launch.rs`, which includes this file):
//!
//! ```text
//! mods/herdr-nvim/main.js              the pointer: re-exports gen/<id>/main.js
//! mods/herdr-nvim/gen/<id>/install.json the record: which runtime, home, herdr-app
//! mods/herdr-nvim/bin/herdr-nvim       the launcher
//! ```
//!
//! One file, `main.js`, says which generation is live, for the app (the JS)
//! and for the launcher (the runtime) alike, so one write switches both.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The flags a runtime takes the app context from (`app::env`).
pub const HOME_FLAG: &str = "--app-home";
pub const BIN_FLAG: &str = "--app-bin";

/// `herdr-nvim`, with `.exe` on Windows: the runtime's binary and the
/// launcher alike.
pub fn exe_name() -> String {
    format!("herdr-nvim{}", std::env::consts::EXE_SUFFIX)
}

/// Where generations live inside a mod folder.
pub const GEN: &str = "gen";

/// The preview's cache in the main mod folder, beside the generations (it
/// outlives them).
pub const CACHE: &str = ".cache";

/// `<home>/mods/herdr-nvim`: the main mod.
pub fn mod_dir(home: &Path) -> PathBuf {
    home.join("mods").join("herdr-nvim")
}

/// The launcher the mods, other mods and the tab nvims' callbacks run.
pub fn launcher_path(home: &Path) -> PathBuf {
    mod_dir(home).join("bin").join(exe_name())
}

/// `main.js` pointing at generation `id`.
pub fn pointer(id: &str) -> String {
    format!(
        "// Written by herdr-nvim's installer: this mod's code is in {GEN}/{id}/.\n\
         export * from \"./{GEN}/{id}/main.js\";\n\
         export {{ default }} from \"./{GEN}/{id}/main.js\";\n"
    )
}

/// The generation a `main.js` written by [`pointer`] points at.
pub fn generation(main_js: &str) -> Option<&str> {
    let start = main_js.find(&format!("\"./{GEN}/"))? + GEN.len() + 4;
    let id = &main_js[start..];
    let id = &id[..id.find('/')?];
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct InstallRecord {
    pub schema: u32,
    /// The app home, as the install resolved it.
    pub home: PathBuf,
    /// `herdr-app` for that home.
    pub app_bin: PathBuf,
    /// The runtime it runs: `<data>/herdr-nvim/app/<stamp>`.
    pub runtime: PathBuf,
    /// `herdr-nvim <version>`: which release wrote it.
    pub by: String,
}

impl InstallRecord {
    pub const FILE: &'static str = "install.json";
    pub const SCHEMA: u32 = 1;

    /// The record of the generation `mod_dir/main.js` points at.
    pub fn read_live(mod_dir: &Path) -> Result<Self, String> {
        let main = mod_dir.join("main.js");
        let text = fs::read_to_string(&main)
            .map_err(|e| format!("cannot read {}: {e}", main.display()))?;
        let id = generation(&text)
            .ok_or_else(|| format!("{} points at no herdr-nvim generation", main.display()))?;
        Self::read(&mod_dir.join(GEN).join(id))
    }

    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join(Self::FILE);
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

    pub fn write(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)? + "\n";
        fs::write(dir.join(Self::FILE), text)
    }

    pub fn runtime_exe(&self) -> PathBuf {
        self.runtime.join("bin").join(exe_name())
    }
}
