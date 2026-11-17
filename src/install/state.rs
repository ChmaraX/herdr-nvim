//! Whether a mod folder is herdr-nvim's, in every format this repo ever
//! wrote.

use std::{fs, path::Path};

use anyhow::{bail, Result};

use super::mods::ModName;

/// The line every install wrote into what it owns: the bash installer's
/// wrappers (`# …`, `rem # …`) and the header mod's `.herdr-nvim` mark.
pub const MARK: &str = "# Written by herdr-nvim's herdr-app/install.sh";

/// The mark file in both mods (the bash installer marked the header only).
pub const MARK_FILE: &str = ".herdr-nvim";

pub enum Ownership {
    Absent,
    /// Written by an install of this repo, in any format it ever wrote: the
    /// mark file in either mod (this installer, and the bash one's header),
    /// the bash installer's sh wrapper (macOS, Linux), the Git Bash era's
    /// `herdr-nvim.cmd` (Windows testers of #72).
    Ours,
    /// Someone else's folder of that name: never touched.
    Foreign,
}

/// Reads a mod folder.
pub fn read_mod(dir: &Path) -> Ownership {
    if fs::symlink_metadata(dir).is_err() {
        return Ownership::Absent;
    }
    let ours = [MARK_FILE, "bin/herdr-nvim", "bin/herdr-nvim.cmd"]
        .iter()
        .any(|rel| {
            fs::read(dir.join(rel))
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(MARK))
        });
    if ours {
        Ownership::Ours
    } else {
        Ownership::Foreign
    }
}

/// Proof that the app home's mod folders are herdr-nvim's or absent: what
/// changes them asks for it, and takes the home from it, so no path skips
/// the check.
pub struct Owned {
    home: std::path::PathBuf,
}

impl Owned {
    pub fn home(&self) -> &Path {
        &self.home
    }
}

/// Checks both mod folders of `home`.
pub fn claim(home: &Path) -> Result<Owned> {
    for name in ModName::ALL {
        let dir = name.live(home);
        if let Ownership::Foreign = read_mod(&dir) {
            bail!(
                "{} is not herdr-nvim's: move it away first; nothing changed",
                dir.display()
            );
        }
    }
    Ok(Owned {
        home: home.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrapper main's `herdr-app/install.sh` writes (1bc902b).
    const SH: &str = "#!/bin/sh\n# Written by herdr-nvim's herdr-app/install.sh; run it again to change this.\nexport HERDR_NVIM_HOST=app\nexport HERDR_NVIM_SELF='/Users/a b/.config/herdr-app/mods/herdr-nvim/bin/herdr-nvim'\nexport HERDR_NVIM_PLUGIN_ROOT='/Users/a b/.local/share/herdr-nvim/app/0123456789abcdef'\nexport HERDR_APP_BIN='/Applications/Herdr.app/Contents/MacOS/Herdr'\nexport HERDR_APP_HOME='/Users/a b/.config/herdr-app'\nexec '/Users/a b/.local/share/herdr-nvim/app/0123456789abcdef/bin/herdr-nvim' \"$@\"\n";

    /// The wrapper #72's Git Bash install writes.
    const CMD: &str = "@echo off\r\nchcp 65001 >nul\r\nrem # Written by herdr-nvim's herdr-app/install.sh; run it again to change this.\r\nset \"HERDR_NVIM_HOST=app\"\r\nset \"HERDR_NVIM_PLUGIN_ROOT=C:\\Users\\Jo 100%%\\.local\\share\\herdr-nvim\\app\\0123456789abcdef\"\r\n\"C:\\Users\\Jo 100%%\\.local\\share\\herdr-nvim\\app\\0123456789abcdef\\bin\\herdr-nvim.exe\" %*\r\n";

    fn mod_with(files: &[(&str, &str)]) -> tempfile_dir::Dir {
        let dir = tempfile_dir::Dir::new();
        for (rel, text) in files {
            let path = dir.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn every_wrapper_format_this_repo_wrote_is_ours() {
        for files in [
            &[("bin/herdr-nvim", SH), ("main.js", "")][..],
            &[("bin/herdr-nvim.cmd", CMD)],
        ] {
            assert!(
                matches!(read_mod(&mod_with(files).0), Ownership::Ours),
                "{files:?}"
            );
        }
    }

    #[test]
    fn a_folder_of_the_same_name_without_our_mark_is_foreign() {
        let theirs = mod_with(&[("main.js", ""), ("bin/herdr-nvim", "#!/bin/sh\nexec x\n")]);
        assert!(matches!(read_mod(&theirs.0), Ownership::Foreign));
        let header = mod_with(&[(MARK_FILE, MARK)]);
        // The bash installer's header mark; this installer marks both mods so.
        assert!(matches!(read_mod(&header.0), Ownership::Ours));
        assert!(matches!(
            read_mod(&header.0.join("missing")),
            Ownership::Absent
        ));
    }

    mod tempfile_dir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(crate::osfs::unique("hn-state-"));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
