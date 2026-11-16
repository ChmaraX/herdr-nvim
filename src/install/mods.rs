//! The two mods in `<home>/mods/`, installed so the live folder is never
//! renamed: Windows refuses to rename a folder while anything holds a file
//! in it, and whether the app does is unknown (doc/windows-facts.md). One
//! layout on every OS:
//!
//! ```text
//! mods/herdr-nvim/
//!   main.js        `export … from "./gen/<id>/main.js"`: replaced last, the switch
//!   mod.json
//!   install.json   the launcher's record (main mod)
//!   .herdr-nvim    the mark (header mod)
//!   bin/herdr-nvim the launcher (main mod)
//!   gen/<id>/      the mod's own files; <id> is their stamp
//! ```
//!
//! A new version is built and checked whole in `mods/.<name>.new/<name>`,
//! its `gen/<id>` moved in beside the old one, and `main.js` replaced: the
//! app reloads the mod on that save. Older generations and files of older
//! layouts go after both mods have switched.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};

use super::{osfs, runtime::Stamp, source::Source, state};
use crate::app::{env::AppEnv, launch_record::InstallRecord};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModName {
    Main,
    Header,
}

impl ModName {
    pub const ALL: [ModName; 2] = [ModName::Main, ModName::Header];

    pub fn as_str(self) -> &'static str {
        match self {
            ModName::Main => "herdr-nvim",
            ModName::Header => "herdr-nvim-header",
        }
    }

    pub fn live(self, home: &Path) -> PathBuf {
        home.join("mods").join(self.as_str())
    }

    fn stage(self, home: &Path) -> PathBuf {
        home.join("mods").join(format!(".{}.new", self.as_str()))
    }

    /// The bash installer's swap leftover.
    fn old(self, home: &Path) -> PathBuf {
        home.join("mods").join(format!(".{}.old", self.as_str()))
    }
}

const GEN: &str = "gen";
const LAUNCHER: &str = "herdr-nvim";
/// Repairs what a crashed run left: the bash installer's half-done swap
/// (live gone, `.old` there) and any stage.
pub fn recover(home: &Path) -> Result<()> {
    for name in ModName::ALL {
        let (live, old) = (name.live(home), name.old(home));
        if !live.exists() && old.exists() {
            osfs::rename_retrying(&old, &live)
                .with_context(|| format!("cannot restore {}", live.display()))?;
        }
        osfs::remove_tree(&old)?;
        osfs::remove_tree(&name.stage(home))?;
    }
    Ok(())
}

struct StagedMod {
    name: ModName,
    /// `mods/.<name>.new/<name>`: the whole mod as it will be.
    dir: PathBuf,
    generation: Stamp,
}

/// Both mods built beside the live ones and passed by `herdr-app check`.
/// Dropping it removes the stages.
#[must_use]
pub struct Staged {
    home: PathBuf,
    mods: Vec<StagedMod>,
}

/// Proof both mods run the new version; the launcher to call them through.
pub struct Swapped {
    pub launcher: PathBuf,
}

pub fn stage(app: &AppEnv, source: &Source, record: &InstallRecord) -> Result<Staged> {
    let mut staged = Staged {
        home: app.home.clone(),
        mods: Vec::new(),
    };
    for name in ModName::ALL {
        let stage = name.stage(&app.home);
        osfs::remove_tree(&stage)?;
        let dir = stage.join(name.as_str());
        let build = dir.join(GEN).join(".build");
        osfs::copy_tree(&source.mod_dir(name.as_str()), &build)
            .with_context(|| format!("cannot stage {}", dir.display()))?;
        fs::copy(build.join("mod.json"), dir.join("mod.json"))?;
        match name {
            ModName::Main => {
                fs::create_dir_all(dir.join("bin"))?;
                fs::copy(&source.launcher, dir.join("bin").join(launcher_file()))?;
                record.write_atomic(&dir)?;
            }
            ModName::Header => fs::write(dir.join(state::MARK_FILE), format!("{}\n", state::MARK))?,
        }
        let generation = if name == ModName::Header {
            header_generation(app, &dir, &build)?
        } else {
            switch_to(&dir, &build)?
        };
        if let Err(error) = check(app, &dir) {
            bail!(
                "herdr-app check failed for {}; nothing replaced: {error}",
                name.as_str()
            );
        }
        staged.mods.push(StagedMod {
            name,
            dir,
            generation,
        });
    }
    Ok(staged)
}

/// The header is the app's own pane header plus a button: built on this
/// app's shipped `mods/pane/main.js` when the mod passes the check with it,
/// else on the copy in this repo.
fn header_generation(app: &AppEnv, dir: &Path, build: &Path) -> Result<Stamp> {
    let shipped = app.home.join("mods/pane/main.js");
    if shipped.is_file() {
        let repo_base = fs::read(build.join("pane-base.js"))?;
        fs::copy(&shipped, build.join("pane-base.js"))?;
        let generation = switch_to(dir, build)?;
        if check(app, dir).is_ok() {
            return Ok(generation);
        }
        let generation_dir = dir.join(GEN).join(generation.as_str());
        osfs::rename_retrying(&generation_dir, build)?;
        fs::write(build.join("pane-base.js"), repo_base)?;
    }
    switch_to(dir, build)
}

/// Names the files in `build` by their stamp (`gen/<id>`) and points the
/// mod's `main.js` at them.
fn switch_to(dir: &Path, build: &Path) -> Result<Stamp> {
    let generation = Stamp::of(build, &[])?;
    let target = dir.join(GEN).join(generation.as_str());
    osfs::remove_tree(&target)?;
    osfs::rename_retrying(build, &target)?;
    fs::write(dir.join("main.js"), pointer(&generation))?;
    Ok(generation)
}

fn pointer(generation: &Stamp) -> String {
    let id = generation.as_str();
    format!(
        "// Written by herdr-nvim's installer: this mod's code is in gen/{id}/.\n\
         export * from \"./{GEN}/{id}/main.js\";\n\
         export {{ default }} from \"./{GEN}/{id}/main.js\";\n"
    )
}

fn launcher_file() -> String {
    format!("{LAUNCHER}{}", std::env::consts::EXE_SUFFIX)
}

fn check(app: &AppEnv, dir: &Path) -> Result<()> {
    let output = Command::new(&app.app_bin)
        .arg("check")
        .arg(dir)
        .env("HERDR_APP_HOME", &app.home)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run {}", app.app_bin.display()))?;
    if !output.status.success() {
        bail!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

impl Staged {
    /// Puts both mods' new generation live. A fresh mod folder is moved in
    /// whole; an existing one gets the new generation, launcher, record and
    /// `mod.json` beside the old ones, then its `main.js`.
    pub fn swap(self) -> Result<Swapped> {
        for staged in &self.mods {
            let live = staged.name.live(&self.home);
            let placed = if fs::symlink_metadata(&live).is_err() {
                osfs::rename_retrying(&staged.dir, &live).map_err(anyhow::Error::from)
            } else {
                update(&staged.dir, &live, &staged.generation)
            };
            placed.with_context(|| {
                format!(
                    "cannot put {} in place{}",
                    live.display(),
                    holders_hint(&live)
                )
            })?;
        }
        for staged in &self.mods {
            tidy(&staged.name.live(&self.home), &staged.generation);
        }
        Ok(Swapped {
            launcher: ModName::Main
                .live(&self.home)
                .join("bin")
                .join(launcher_file()),
        })
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        for name in ModName::ALL {
            let _ = osfs::remove_tree(&name.stage(&self.home));
        }
    }
}

fn update(stage: &Path, live: &Path, generation: &Stamp) -> Result<()> {
    let id = generation.as_str();
    let target = live.join(GEN).join(id);
    if !target.join("main.js").is_file() {
        fs::create_dir_all(live.join(GEN))?;
        osfs::remove_tree(&target)?;
        osfs::rename_retrying(&stage.join(GEN).join(id), &target)?;
    }
    let launcher = stage.join("bin").join(launcher_file());
    if launcher.is_file() {
        fs::create_dir_all(live.join("bin"))?;
        osfs::replace_exe(&launcher, &live.join("bin").join(launcher_file()))?;
    }
    for file in [InstallRecord::FILE, state::MARK_FILE, "mod.json", "main.js"] {
        let from = stage.join(file);
        if from.is_file() {
            osfs::replace_file(&live.join(file), &fs::read(&from)?)?;
        }
    }
    Ok(())
}

/// After the switch: older generations, launchers moved aside, and the
/// older layout's copies of our files at the top (now under `gen/`) go.
/// Best effort: what Windows holds stays until the next install.
fn tidy(live: &Path, generation: &Stamp) {
    let remove_except = |dir: &Path, keep: &dyn Fn(&str) -> bool| {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !keep(&name) {
                let _ = osfs::remove_tree(&entry.path());
            }
        }
    };
    let current = live.join(GEN).join(generation.as_str());
    remove_except(live, &|name| {
        matches!(name, "main.js" | "mod.json") || !current.join(name).exists()
    });
    remove_except(&live.join(GEN), &|name| name == generation.as_str());
    let launcher = launcher_file();
    remove_except(&live.join("bin"), &|name| name == launcher);
}

/// Windows: which running programs live under `dir`, for the error.
fn holders_hint(dir: &Path) -> String {
    #[cfg(windows)]
    {
        let holders: Vec<String> = crate::winproc::processes()
            .unwrap_or_default()
            .into_iter()
            .filter(|proc| crate::pathkey::canonical(&proc.image).starts_with(dir))
            .map(|proc| format!("{} (pid {})", proc.image.display(), proc.pid))
            .collect();
        if !holders.is_empty() {
            return format!("; in use by {}", holders.join(", "));
        }
    }
    let _ = dir;
    String::new()
}

/// Uninstall: both mods, whole, and the bash era's leftovers.
pub fn remove(home: &Path) -> Result<()> {
    for name in ModName::ALL {
        for dir in [name.live(home), name.stage(home), name.old(home)] {
            osfs::remove_tree(&dir).with_context(|| {
                format!("cannot remove {}{}", dir.display(), holders_hint(&dir))
            })?;
        }
    }
    retire_pick(home);
    Ok(())
}

/// The picker was its own mod (`herdr-nvim-pick`) before it joined
/// herdr-nvim: that folder goes, when it is that mod.
pub fn retire_pick(home: &Path) {
    let pick = home.join("mods/herdr-nvim-pick");
    let ours = fs::read_to_string(pick.join("mod.json"))
        .is_ok_and(|text| text.contains("\"nvim: open file from agent output\""));
    if ours {
        let _ = osfs::remove_tree(&pick);
    }
}
