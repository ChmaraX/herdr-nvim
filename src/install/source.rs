//! What is being installed: this herdr-nvim, the launcher built beside it,
//! and the tree they came with (a release bundle, or a checkout).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub struct Source {
    /// Has `lua/herdr-nvim`, `plugin/` and `herdr-app/mods/<each mod>`.
    pub tree: PathBuf,
    pub exe: PathBuf,
    pub launcher: PathBuf,
}

impl Source {
    /// `explicit`, else the first folder above this binary that has the
    /// tree: `<bundle>/bin/herdr-nvim`, `<checkout>/target/release/herdr-nvim`.
    pub fn locate(explicit: Option<&Path>) -> Result<Source> {
        let exe = std::env::current_exe().context("cannot find the herdr-nvim binary")?;
        let launcher =
            exe.with_file_name(format!("herdr-nvim-launch{}", std::env::consts::EXE_SUFFIX));
        if !launcher.is_file() {
            bail!(
                "no {} beside {}: install from a release bundle, or build with `cargo build --release`",
                launcher.display(),
                exe.display()
            );
        }
        let tree = match explicit {
            Some(dir) if is_tree(dir) => dir.to_path_buf(),
            Some(dir) => bail!(
                "{} has no herdr-nvim tree (lua/, plugin/, herdr-app/mods/)",
                dir.display()
            ),
            None => exe
                .ancestors()
                .skip(1)
                .find(|dir| is_tree(dir))
                .map(Path::to_path_buf)
                .with_context(|| {
                    format!(
                        "no herdr-nvim tree above {}; pass --source DIR",
                        exe.display()
                    )
                })?,
        };
        Ok(Source {
            tree,
            exe,
            launcher,
        })
    }

    pub fn mod_dir(&self, name: &str) -> PathBuf {
        self.tree.join("herdr-app").join("mods").join(name)
    }
}

fn is_tree(dir: &Path) -> bool {
    dir.join("lua/herdr-nvim").is_dir()
        && dir.join("plugin").is_dir()
        && dir.join("herdr-app/mods/herdr-nvim/main.js").is_file()
        && dir
            .join("herdr-app/mods/herdr-nvim-header/main.js")
            .is_file()
}
