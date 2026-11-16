//! The two mods in `<home>/mods/`, installed so the live folder is never
//! renamed: Windows refuses to rename a folder while anything holds a file
//! in it, and whether the app does is unknown (doc/windows-facts.md, row 11).
//! One layout on every OS, so macOS proves the Windows path daily:
//!
//! ```text
//! mods/<mod>/
//!   main.js        the pointer: re-exports gen/<id>/main.js (launch_record)
//!   mod.json
//!   .herdr-nvim    the mark: this folder is herdr-nvim's
//!   bin/herdr-nvim the launcher (main mod)
//!   gen/<id>/      the mod's files, and install.json (main mod); <id> is their stamp
//! ```
//!
//! An install builds each mod whole in `mods/.<mod>.new/<mod>` and has the
//! app check it. Then, for both mods, it places what nothing runs yet (the
//! generation, `mod.json`, the mark, the next launcher), and only then
//! switches: each `main.js` (and the launcher) in turn, undone in reverse if
//! one fails. The app reloads a mod when its `main.js` changes.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};

use super::{
    runtime::Stamp,
    source::Source,
    state::{self, Owned, Ownership},
};
use crate::{
    app::{
        env::AppEnv,
        launch_record::{exe_name, launcher_path, pointer, InstallRecord, CACHE, GEN},
    },
    osfs,
};

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
        mods_dir(home).join(self.as_str())
    }

    fn stage(self, home: &Path) -> PathBuf {
        mods_dir(home).join(format!(".{}.new", self.as_str()))
    }

    /// The bash installer's swap leftover.
    fn old(self, home: &Path) -> PathBuf {
        mods_dir(home).join(format!(".{}.old", self.as_str()))
    }
}

pub fn mods_dir(home: &Path) -> PathBuf {
    home.join("mods")
}

/// The generation a mod is built in before it has its name.
const BUILD: &str = "build";
/// The launcher waiting to be switched in, and the one switched out.
const NEXT: &str = ".next-";
const PREV: &str = ".prev-";

/// Repairs what a crashed run left: the bash installer's half-done swap
/// (live gone, its own `.old` there), stages, and half-written or
/// half-removed files.
pub fn recover(owned: &Owned) -> Result<()> {
    let home = owned.home();
    osfs::sweep(&mods_dir(home), &[]);
    for name in ModName::ALL {
        let (live, old) = (name.live(home), name.old(home));
        let old_ours = matches!(state::read_mod(&old), Ownership::Ours);
        if !live.exists() && old_ours {
            osfs::rename_retrying(&old, &live)
                .with_context(|| format!("cannot restore {}", live.display()))?;
        } else if old_ours {
            remove_or_say(&old);
        }
        if stage_is_ours(home, name) {
            remove_or_say(&name.stage(home));
        }
        // Our own folders: also the names this branch used before (never
        // released) for its temp and trash files.
        osfs::sweep(&live, &[".tmp-", ".trash-"]);
        osfs::sweep(&live.join("bin"), &[NEXT]);
        osfs::sweep(&live.join(CACHE), &[crate::app::sidebar::STAGE]);
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

/// Proof that both mods' `main.js` point at the new generation, and the
/// launcher the mods run.
pub struct Swapped {
    launcher: PathBuf,
}

impl Swapped {
    pub fn launcher(&self) -> &Path {
        &self.launcher
    }
}

/// Whether `mods/.<name>.new` holds nothing but a stage this installer
/// made (the mod folder in it carries our mark), or nothing at all.
fn stage_is_ours(home: &Path, name: ModName) -> bool {
    let stage = name.stage(home);
    let Ok(entries) = fs::read_dir(&stage) else {
        return false;
    };
    entries.flatten().all(|entry| {
        entry.file_name() == name.as_str()
            && matches!(state::read_mod(&entry.path()), Ownership::Ours)
    })
}

pub fn stage(
    app: &AppEnv,
    source: &Source,
    record: &InstallRecord,
    owned: &Owned,
) -> Result<Staged> {
    let mut staged = Staged {
        home: owned.home().to_path_buf(),
        mods: Vec::new(),
    };
    for name in ModName::ALL {
        let stage = name.stage(owned.home());
        if stage.exists() && !stage_is_ours(owned.home(), name) {
            bail!(
                "{} is not herdr-nvim's: move it away first; nothing changed",
                stage.display()
            );
        }
        osfs::remove_tree(&stage)?;
        let dir = stage.join(name.as_str());
        let build = dir.join(GEN).join(BUILD);
        // The mark first: a stage a crash cuts short is still known as ours.
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(state::MARK_FILE), format!("{}\n", state::MARK))?;
        osfs::copy_tree(&source.mod_dir(name.as_str()), &build)
            .with_context(|| format!("cannot stage {}", dir.display()))?;
        fs::copy(build.join("mod.json"), dir.join("mod.json"))?;
        if name == ModName::Main {
            record.write(&build)?;
            fs::create_dir_all(dir.join("bin"))?;
            fs::copy(&source.launcher, dir.join("bin").join(exe_name()))?;
        }
        fs::write(dir.join("main.js"), pointer(BUILD))?;
        let checked = match name {
            ModName::Main => check(app, &dir),
            ModName::Header => check_header(app, &dir, &build),
        };
        checked.with_context(|| {
            format!(
                "herdr-app check failed for {}; nothing replaced",
                name.as_str()
            )
        })?;
        let generation = Stamp::of(&build, &[])?;
        osfs::rename_retrying(&build, &dir.join(GEN).join(generation.as_str()))?;
        fs::write(dir.join("main.js"), pointer(generation.as_str()))?;
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
fn check_header(app: &AppEnv, dir: &Path, build: &Path) -> Result<()> {
    let shipped = app.home.join("mods/pane/main.js");
    if shipped.is_file() {
        let repo_base = fs::read(build.join("pane-base.js"))?;
        fs::copy(&shipped, build.join("pane-base.js"))?;
        if check(app, dir).is_ok() {
            return Ok(());
        }
        fs::write(build.join("pane-base.js"), repo_base)?;
    }
    check(app, dir)
}

fn check(app: &AppEnv, dir: &Path) -> Result<()> {
    let output = Command::new(&app.app_bin)
        .arg("check")
        .arg(dir)
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

/// What a switch changed, to put back if a later one fails.
enum Undo {
    /// A fresh mod: its folder goes back to the stage.
    Moved { live: PathBuf, stage: PathBuf },
    /// `main.js`'s bytes before, and the launcher switched out (if any).
    Pointed {
        main_js: PathBuf,
        before: Vec<u8>,
        launcher: Option<(PathBuf, Option<PathBuf>)>,
    },
}

impl Staged {
    /// Places both mods' new generation, then switches both, all or none.
    pub fn swap(self) -> Result<Swapped> {
        for staged in &self.mods {
            let live = staged.name.live(&self.home);
            if live.exists() {
                place(&staged.dir, &live, &staged.generation).with_context(|| {
                    format!("cannot update {}{}", live.display(), holders_hint(&live))
                })?;
            }
        }
        let mut done: Vec<Undo> = Vec::new();
        for staged in &self.mods {
            let live = staged.name.live(&self.home);
            match switch(&staged.dir, &live) {
                Ok(undo) => done.push(undo),
                Err(error) => {
                    for undo in done.into_iter().rev() {
                        undo.apply();
                    }
                    return Err(error).with_context(|| {
                        format!(
                            "cannot switch {} to the new version{}; the previous one is back",
                            live.display(),
                            holders_hint(&live)
                        )
                    });
                }
            }
        }
        for staged in &self.mods {
            tidy(&staged.name.live(&self.home), &staged.generation);
        }
        Ok(Swapped {
            launcher: launcher_path(&self.home),
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

/// Phase one, into a live mod: what nothing runs yet.
fn place(stage: &Path, live: &Path, generation: &Stamp) -> Result<()> {
    let target = live.join(GEN).join(generation.as_str());
    if !target.join("main.js").is_file() {
        fs::create_dir_all(live.join(GEN))?;
        osfs::remove_tree(&target)?;
        osfs::rename_retrying(&stage.join(GEN).join(generation.as_str()), &target)?;
    }
    for file in ["mod.json", state::MARK_FILE] {
        osfs::replace_file(&live.join(file), &fs::read(stage.join(file))?)?;
    }
    let launcher = stage.join("bin").join(exe_name());
    let current = live.join("bin").join(exe_name());
    if launcher.is_file() && fs::read(&current).ok() != Some(fs::read(&launcher)?) {
        fs::create_dir_all(live.join("bin"))?;
        fs::copy(
            &launcher,
            live.join("bin").join(format!("{NEXT}{}", exe_name())),
        )?;
    }
    Ok(())
}

/// Phase two: the mod runs the new version.
fn switch(stage: &Path, live: &Path) -> Result<Undo> {
    if !live.exists() {
        osfs::rename_retrying(stage, live)?;
        return Ok(Undo::Moved {
            live: live.to_path_buf(),
            stage: stage.to_path_buf(),
        });
    }
    let current = live.join("bin").join(exe_name());
    let next = live.join("bin").join(format!("{NEXT}{}", exe_name()));
    let launcher = if next.is_file() {
        let prev = current
            .exists()
            .then(|| live.join("bin").join(osfs::unique(PREV)));
        if let Some(prev) = &prev {
            osfs::rename_retrying(&current, prev)?;
        }
        if let Err(error) = osfs::rename_retrying(&next, &current) {
            if let Some(prev) = &prev {
                let _ = osfs::rename_retrying(prev, &current);
            }
            return Err(error.into());
        }
        Some((current, prev))
    } else {
        None
    };
    let main_js = live.join("main.js");
    let before = fs::read(&main_js).unwrap_or_default();
    let undo = Undo::Pointed {
        main_js: main_js.clone(),
        before,
        launcher,
    };
    if let Err(error) = osfs::replace_file(&main_js, &fs::read(stage.join("main.js"))?) {
        undo.apply();
        return Err(error.into());
    }
    Ok(undo)
}

impl Undo {
    fn apply(self) {
        match self {
            Undo::Moved { live, stage } => {
                let _ = osfs::rename_retrying(&live, &stage);
            }
            Undo::Pointed {
                main_js,
                before,
                launcher,
            } => {
                let _ = osfs::replace_file(&main_js, &before);
                if let Some((current, prev)) = launcher {
                    let _ = fs::remove_file(&current);
                    if let Some(prev) = prev {
                        let _ = osfs::rename_retrying(&prev, &current);
                    }
                }
            }
        }
    }
}

/// After the switch: older generations, launchers switched out, and what
/// older layouts left at the top (the bash era's JS, its wrapper scripts).
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
    remove_except(live, &|name| {
        matches!(name, "main.js" | "mod.json" | "bin" | GEN | CACHE)
            || name == state::MARK_FILE
            // What the app writes into a mod folder (`herdr-app check`).
            || name == "jsconfig.json"
            || name.ends_with(".d.ts")
    });
    remove_except(&live.join(GEN), &|name| name == generation.as_str());
    let launcher = exe_name();
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
            return format!(" (in use by {})", holders.join(", "));
        }
    }
    let _ = dir;
    String::new()
}

fn remove_or_say(dir: &Path) -> bool {
    match osfs::remove_tree(dir) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "herdr-nvim: could not remove {}{}: {error}",
                dir.display(),
                holders_hint(dir)
            );
            false
        }
    }
}

/// Uninstall: both mods, whole, and the bash era's leftovers. Returns the
/// folders Windows kept (in use), already reported.
pub fn remove(owned: &Owned) -> Vec<PathBuf> {
    let home = owned.home();
    let mut kept = Vec::new();
    for name in ModName::ALL {
        let ours = [
            Some(name.live(home)),
            stage_is_ours(home, name).then(|| name.stage(home)),
            matches!(state::read_mod(&name.old(home)), Ownership::Ours).then(|| name.old(home)),
        ];
        for dir in ours.into_iter().flatten() {
            if !remove_or_say(&dir) {
                kept.push(dir);
            }
        }
    }
    retire_pick(home);
    kept
}

/// The picker was its own mod (`herdr-nvim-pick`) before it joined
/// herdr-nvim: that folder goes, when it is that mod.
pub fn retire_pick(home: &Path) {
    let pick = mods_dir(home).join("herdr-nvim-pick");
    let ours = fs::read_to_string(pick.join("mod.json"))
        .is_ok_and(|text| text.contains("\"nvim: open file from agent output\""));
    if ours {
        let _ = osfs::remove_tree(&pick);
    }
}

/// An exclusive lock on this app home's mods for one install or uninstall:
/// a second run waits, so it never removes the first one's stage.
/// The file stays after an uninstall (empty): removing a lock file another
/// run waits on would let a third take a new one beside it.
pub struct InstallLock(#[allow(dead_code)] fs::File);

impl InstallLock {
    pub fn take(home: &Path) -> io::Result<Self> {
        fs::create_dir_all(mods_dir(home))?;
        let path = mods_dir(home).join(".herdr-nvim.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        if file.try_lock().is_err() {
            println!(
                "herdr-nvim: another herdr-nvim install runs on this app home; waiting for it"
            );
            file.lock()?;
        }
        Ok(Self(file))
    }
}
