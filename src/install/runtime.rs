//! The runtime store: `<data>/herdr-nvim/app/<stamp>/{bin, lua, plugin,
//! runtime-stamp}`, one folder per distinct runtime. A tab nvim keeps the
//! runtime it started with, so an upgrade adds a folder and prunes the ones
//! nothing runs any more.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{osfs, source::Source};
use crate::app::env::AppEnv;

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
            lines.push_str(&format!("{}  {rel}\n", hex(&Sha256::digest(&bytes))));
        }
        Ok(Stamp(
            hex(&Sha256::digest(lines.as_bytes()))[..16].to_owned(),
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A complete runtime folder of the store.
pub struct RuntimeDir {
    pub path: PathBuf,
    pub stamp: Stamp,
}

/// Which runtimes running tab nvims use, from the daemons' own answers.
pub struct LiveRuntimes {
    stamps: HashSet<String>,
    /// A registered daemon that didn't answer: it may run any runtime.
    unknown: bool,
}

#[derive(Default)]
pub struct PruneReport {
    pub removed: Vec<PathBuf>,
    pub kept_live: Vec<PathBuf>,
    /// Windows refused: something runs from it (a sidebar, the watcher).
    pub kept_locked: Vec<PathBuf>,
}

const STAMP_FILE: &str = "runtime-stamp";
const STAMPED: &[&str] = &["bin", "lua", "plugin"];

pub struct RuntimeStore {
    pub dir: PathBuf,
}

impl RuntimeStore {
    pub fn open() -> Result<Self> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| crate::home::home_dir().map(|home| home.join(".local/share")))
            .context("no home folder (HOME or USERPROFILE)")?;
        Ok(Self {
            dir: data.join("herdr-nvim").join("app"),
        })
    }

    /// What a crashed run left: half-built runtimes, half-removed ones.
    pub fn sweep_leftovers(&self) {
        osfs::sweep(&self.dir, &[".new.", ".trash-"]);
    }

    /// The runtime of `source`, made once: built in `.new.*`, stamped, and
    /// renamed to its stamp, or the existing folder of that stamp.
    pub fn put(&self, source: &Source) -> Result<RuntimeDir> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        let staged = self.dir.join(osfs::unique(".new."));
        let built = (|| -> Result<Stamp> {
            fs::create_dir_all(staged.join("bin"))?;
            let exe = format!("herdr-nvim{}", std::env::consts::EXE_SUFFIX);
            fs::copy(&source.exe, staged.join("bin").join(exe))?;
            fs::copy(
                source.tree.join("herdr-app/install-nvim.sh"),
                staged.join("bin/install-nvim.sh"),
            )?;
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
            match osfs::remove_tree(&path) {
                Ok(()) => report.removed.push(path),
                Err(_) => report.kept_locked.push(path),
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
                report.removed.push(self.dir.clone());
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

/// Asks the registered tab nvims of this app which runtime each runs
/// (`herdr-nvim daemons --json` run by `exe` for `app`).
pub fn live_runtimes(exe: &Path, app: &AppEnv) -> LiveRuntimes {
    #[derive(Deserialize)]
    struct Row {
        state: String,
        #[serde(default)]
        stamp: Option<String>,
    }
    let unknown = LiveRuntimes {
        stamps: HashSet::new(),
        unknown: true,
    };
    let Ok(output) = Command::new(exe)
        .args(app.prefix())
        .args(["daemons", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return unknown;
    };
    let Ok(rows) = serde_json::from_slice::<Vec<Row>>(&output.stdout) else {
        return unknown;
    };
    let mut live = LiveRuntimes {
        stamps: HashSet::new(),
        unknown: false,
    };
    for row in rows {
        match (row.state.as_str(), row.stamp) {
            ("stale", _) => {}
            ("alive" | "orphaned" | "unknown", Some(stamp)) => {
                live.stamps.insert(stamp);
            }
            _ => live.unknown = true,
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
