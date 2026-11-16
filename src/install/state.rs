//! Whether a mod folder is herdr-nvim's, in every format this repo ever
//! wrote.

use std::{fs, path::Path};

use crate::app::launch_record::InstallRecord;

/// The line every install wrote into what it owns: the bash installer's
/// wrappers (`# …`, `rem # …`) and the header mod's `.herdr-nvim` mark.
pub const MARK: &str = "# Written by herdr-nvim's herdr-app/install.sh";

/// The header mod's mark file (the main mod is marked by its record).
pub const MARK_FILE: &str = ".herdr-nvim";

pub enum Ownership {
    Absent,
    /// Written by an install of this repo, in any format it ever wrote:
    /// `install.json`, the bash installer's sh wrapper (macOS, Linux), the
    /// Git Bash era's `herdr-nvim.cmd` (Windows testers of #72), the
    /// header's mark file.
    Ours,
    /// Someone else's folder of that name: never touched.
    Foreign,
}

/// Reads `<home>/mods/<name>`.
pub fn read_mod(dir: &Path) -> Ownership {
    if fs::symlink_metadata(dir).is_err() {
        return Ownership::Absent;
    }
    let ours = InstallRecord::read(dir).is_ok_and(|record| record.by.starts_with("herdr-nvim"))
        || ["bin/herdr-nvim", "bin/herdr-nvim.cmd", MARK_FILE]
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
                let path = std::env::temp_dir().join(crate::install::osfs::unique("hn-state-"));
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
