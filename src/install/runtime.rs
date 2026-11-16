//! The runtime store: `<data>/herdr-nvim/app/<stamp>/{bin, lua, plugin,
//! runtime-stamp}`, one folder per distinct runtime. A tab nvim keeps the
//! runtime it started with, so an upgrade adds a folder and prunes the ones
//! nothing runs any more.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use super::source::Source;
use crate::{
    app::launch_record::exe_name,
    daemon::{inventory, registry},
    osfs,
};

/// 16 lowercase hex digits naming a set of files by their content: sha256
/// over the `shasum -a 256` lines of each file (sorted paths, `/`-separated).
/// The bash installer computed the same text, so one install's stamp is the
/// other's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Stamp(String);

impl Stamp {
    /// Over the files under `root` whose first path part is in `parts` (all
    /// files when `parts` is empty).
    pub fn of(root: &Path, parts: &[&str]) -> Result<Stamp> {
        let mut lines = String::new();
        for rel in osfs::files_under(root)? {
            let first = rel.split('/').next().unwrap_or_default();
            if !parts.is_empty() && !parts.contains(&first) {
                continue;
            }
            let bytes = fs::read(root.join(&rel))?;
            lines.push_str(&format!("{}  {rel}\n", osfs::sha256_hex(&bytes)));
        }
        Ok(Stamp(osfs::sha256_hex(lines.as_bytes())[..16].to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A complete runtime folder of the store; only `RuntimeStore::put` makes one.
pub struct RuntimeDir {
    path: PathBuf,
    stamp: Stamp,
}

impl RuntimeDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Which runtimes running tab nvims use, from the daemons' own answers.
pub struct LiveRuntimes {
    stamps: HashSet<String>,
    /// A registered daemon that didn't answer: it may run any runtime.
    unknown: bool,
}

#[derive(Default)]
pub struct PruneReport {
    pub kept_live: Vec<PathBuf>,
    /// Windows refused: something runs from it (a sidebar, the watcher).
    pub kept_locked: Vec<PathBuf>,
}

const STAMP_FILE: &str = "runtime-stamp";
/// A runtime being built (the bash installer's name for it too).
const NEW: &str = ".new.";
const STAMPED: &[&str] = &["bin", "lua", "plugin"];

/// Held while it lives (`RuntimeStore::lock`).
pub struct StoreLock(#[allow(dead_code)] fs::File);

pub struct RuntimeStore {
    pub dir: PathBuf,
}

impl RuntimeStore {
    pub fn open() -> Result<Self> {
        Ok(Self {
            dir: crate::home::data_dir()
                .context("no home folder (HOME or USERPROFILE)")?
                .join("herdr-nvim")
                .join("app"),
        })
    }

    /// An exclusive lock on the store, held from the sweep through the prune,
    /// so installs into two app homes don't remove each other's half-built
    /// or just-made runtime. Beside the store, not in it: the store goes
    /// when it is empty, a lock file never does.
    pub fn lock(&self) -> Result<StoreLock> {
        fs::create_dir_all(self.dir.parent().unwrap_or(&self.dir))?;
        let path = self.dir.with_extension("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        if file.try_lock().is_err() {
            println!("herdr-nvim: another herdr-nvim install uses the runtimes; waiting for it");
            file.lock()
                .with_context(|| format!("cannot lock {}", path.display()))?;
        }
        Ok(StoreLock(file))
    }

    /// What a crashed run left: half-built runtimes, half-removed ones.
    pub fn sweep_leftovers(&self) {
        // The store is ours alone: also the names this branch used before
        // (never released) for temp and trash.
        osfs::sweep(&self.dir, &[NEW, ".trash-", ".tmp-"]);
    }

    /// The runtime of `source`, made once: built in `.new.*`, stamped, and
    /// renamed to its stamp, or the existing folder of that stamp.
    pub fn put(&self, source: &Source) -> Result<RuntimeDir> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        let staged = self.dir.join(osfs::unique(NEW));
        let built = (|| -> Result<Stamp> {
            fs::create_dir_all(staged.join("bin"))?;
            fs::copy(&source.exe, staged.join("bin").join(exe_name()))?;
            osfs::copy_tree(&source.tree.join("lua"), &staged.join("lua"))?;
            osfs::copy_tree(&source.tree.join("plugin"), &staged.join("plugin"))?;
            let stamp = Stamp::of(&staged, STAMPED)?;
            fs::write(staged.join(STAMP_FILE), format!("{}\n", stamp.as_str()))?;
            Ok(stamp)
        })();
        let stamp = match built {
            Ok(stamp) => stamp,
            Err(error) => {
                let _ = fs::remove_dir_all(&staged);
                return Err(error.context(format!(
                    "cannot build the runtime in {}",
                    self.dir.display()
                )));
            }
        };
        let path = self.dir.join(stamp.as_str());
        if path.join(STAMP_FILE).is_file() {
            let _ = fs::remove_dir_all(&staged);
        } else {
            osfs::remove_tree(&path)?;
            osfs::rename_retrying(&staged, &path)
                .with_context(|| format!("cannot put the runtime at {}", path.display()))?;
        }
        Ok(RuntimeDir { path, stamp })
    }

    /// Removes each runtime that isn't `keep` and that no running nvim uses.
    /// Nothing is removed while a daemon's runtime is unknown.
    pub fn prune(&self, keep: Option<&RuntimeDir>, live: &LiveRuntimes) -> PruneReport {
        let mut report = PruneReport::default();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return report;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !path.join(STAMP_FILE).is_file() {
                continue;
            }
            if keep.is_some_and(|keep| keep.stamp.as_str() == name) {
                continue;
            }
            if live.unknown || live.stamps.contains(&name) {
                report.kept_live.push(path);
                continue;
            }
            if osfs::remove_tree(&path).is_err() {
                report.kept_locked.push(path);
            }
        }
        // The layout from before versioned runtimes: bin/lua/plugin at the
        // top, run by daemons that report no stamp.
        if self.dir.join("lua").is_dir() && keep.is_none_or(|keep| keep.path != self.dir) {
            if live.unknown || live.stamps.contains("") {
                report.kept_live.push(self.dir.clone());
            } else {
                for part in ["bin", "lua", "plugin", STAMP_FILE] {
                    let _ = osfs::remove_tree(&self.dir.join(part));
                }
            }
        }
        report
    }

    /// Whether `path` is inside the store (a runtime of it).
    pub fn holds(&self, path: &Path) -> bool {
        let canonical = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canonical(path).starts_with(canonical(&self.dir))
    }
}

/// Asks the registered tab nvims of this app which runtime each runs (the
/// app context must be this process's: `AppEnv::export`).
pub fn live_runtimes() -> LiveRuntimes {
    let Ok(keys) = registry::daemon_keys() else {
        return LiveRuntimes {
            stamps: HashSet::new(),
            unknown: true,
        };
    };
    let sidebar = crate::config::load().sidebar;
    let mut live = LiveRuntimes {
        stamps: HashSet::new(),
        unknown: false,
    };
    for daemon in inventory::describe(keys, None, &sidebar) {
        match daemon.answered() {
            Some(probe) => {
                live.stamps.insert(probe.stamp.clone());
            }
            None if daemon.state() == inventory::DaemonState::Stale => {}
            None => live.unknown = true,
        }
    }
    live
}

/// Refuses to run from inside the store this run may prune: an exe can't
/// remove its own folder on Windows, and the runtime it would serve from
/// could go away under it.
pub fn refuse_running_from(store: &RuntimeStore) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find the herdr-nvim binary")?;
    if store.holds(&exe) {
        bail!(
            "this herdr-nvim is an installed runtime ({}); run the installer from a download \
             (install.sh / install.ps1) or a build",
            exe.display()
        );
    }
    Ok(())
}
