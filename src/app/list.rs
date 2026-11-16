//! `herdr-nvim app-list <dir> [--limit N]`: one folder's entries as the app's
//! file tree shows them, the same on every OS (the tree's `tree.js
//! listFolder`):
//! - `.git` left out; folders first, then by name, and only then capped at
//!   `N` (500), so a big folder still shows its folders;
//! - a link is what it points to, as the app's own `files` mod lists it: a
//!   link to a folder (a symlink, a Windows junction) is a folder, a broken
//!   one a file;
//! - `hidden`: a dotfile, or on Windows a file with the Hidden attribute (the
//!   tree's `H` shows them);
//! - `ignored`: git ignores it (one repo lookup, the folder's own rules and
//!   its parents'; a tracked file never is, as with `git check-ignore`).

use std::{cmp::Ordering, fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use super::cli;

const LIMIT: usize = 500;

#[derive(Debug, Serialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// More entries than the limit: the last ones by the order above are left out.
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub hidden: bool,
    pub ignored: bool,
}

pub fn list_cmd() -> Result<()> {
    let args: Vec<String> = crate::argv::args().skip(2).collect();
    let usage = "usage: herdr-nvim app-list <dir> [--limit N]";
    let dir = cli::positional(&args, &["--limit"]).context(usage)?;
    let limit = match cli::flag(&args, "--limit")? {
        Some(n) => n.parse().context(usage)?,
        None => LIMIT,
    };
    println!("{}", serde_json::to_string(&list(Path::new(dir), limit)?)?);
    Ok(())
}

pub fn list(dir: &Path, limit: usize) -> Result<Listing> {
    if !dir.is_dir() {
        bail!("not a folder: {}", dir.display());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("cannot list {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let kind = entry.file_type()?;
        let is_dir = if kind.is_symlink() {
            fs::metadata(entry.path()).is_ok_and(|target| target.is_dir())
        } else {
            kind.is_dir()
        };
        let hidden = name.starts_with('.') || hidden_attribute(&entry);
        entries.push(Entry {
            name,
            dir: is_dir,
            hidden,
            ignored: false,
        });
    }
    entries.sort_by(order);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    if let Some(ignores) = Ignores::of(dir) {
        for entry in &mut entries {
            entry.ignored = ignores.ignored(&entry.name, entry.dir);
        }
    }
    Ok(Listing { entries, truncated })
}

/// Folders first, then by name, case aside (a tie by case decides last).
fn order(a: &Entry, b: &Entry) -> Ordering {
    b.dir
        .cmp(&a.dir)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}

#[cfg(windows)]
fn hidden_attribute(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(not(windows))]
fn hidden_attribute(_entry: &fs::DirEntry) -> bool {
    false
}

/// The repo a folder is in, and that folder's path inside its work tree.
struct Ignores {
    repo: git2::Repository,
    index: Option<git2::Index>,
    /// "" for the work tree's top, else "a/b/".
    prefix: String,
}

impl Ignores {
    fn of(dir: &Path) -> Option<Ignores> {
        let repo = git2::Repository::discover(dir).ok()?;
        let top = fs::canonicalize(repo.workdir()?).ok()?;
        let here = fs::canonicalize(dir).ok()?;
        let prefix = repo_path(here.strip_prefix(&top).ok()?)?;
        let index = repo.index().ok();
        Some(Ignores {
            repo,
            index,
            prefix,
        })
    }

    fn ignored(&self, name: &str, dir: bool) -> bool {
        let path = format!("{}{name}", self.prefix);
        if !dir
            && self
                .index
                .as_ref()
                .is_some_and(|index| index.get_path(Path::new(&path), 0).is_some())
        {
            return false;
        }
        let path = if dir { format!("{path}/") } else { path };
        self.repo.is_path_ignored(&path).unwrap_or(false)
    }
}

/// A path inside the work tree as git writes it: names joined by "/", with
/// a trailing "/" ("" for the top).
fn repo_path(inside: &Path) -> Option<String> {
    let mut out = String::new();
    for part in inside.components() {
        out.push_str(part.as_os_str().to_str()?);
        out.push('/');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// The listing fixture `herdr-app/e2e/app-list-fixture.ps1` builds on
    /// every OS: 600 entries and a `.git`.
    fn fixture(at: &Path) {
        let git = Command::new("git")
            .args(["init", "-q"])
            .arg(at)
            .status()
            .unwrap();
        assert!(git.success());
        fs::write(at.join(".gitignore"), "build/\n*.log\n").unwrap();
        fs::write(at.join(".hidden-note"), "x").unwrap();
        fs::write(at.join("app.log"), "x").unwrap();
        fs::write(at.join("notes.md"), "x").unwrap();
        fs::write(at.join("README.md"), "x").unwrap();
        for dir in ["build", "src", "zz-folder"] {
            fs::create_dir(at.join(dir)).unwrap();
        }
        fs::write(at.join("build/out.bin"), "x").unwrap();
        fs::write(at.join("src/main.rs"), "x").unwrap();
        for i in 0..589 {
            fs::write(at.join(format!("w{i:03}.txt")), "x").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("src", at.join("link-dir")).unwrap();
            symlink("notes.md", at.join("link-file")).unwrap();
            symlink("missing", at.join("broken")).unwrap();
        }
    }

    fn golden() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/herdr-app/e2e/fixtures/app-list.golden.json"
        );
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_big_folder_lists_sorted_then_capped_with_links_hidden_and_ignore_marks() {
        let base = std::env::temp_dir().join(format!("hnv-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fixture(&base);
        let listing = list(&base, LIMIT).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        let entry = |name: &str| listing.entries.iter().find(|e| e.name == name).unwrap();

        assert!(listing.truncated, "600 entries, 500 shown");
        assert_eq!(listing.entries.len(), 500);
        assert!(!names.contains(&".git"));
        // Folders first even when their names sort after every file's, and
        // the cap cuts the end of the sorted list.
        assert_eq!(names[..4], ["build", "link-dir", "src", "zz-folder"]);
        assert_eq!(
            names[4..11],
            [
                ".gitignore",
                ".hidden-note",
                "app.log",
                "broken",
                "link-file",
                "notes.md",
                "README.md"
            ]
        );
        assert_eq!(names.last(), Some(&"w488.txt"));
        assert!(!names.contains(&"w489.txt"));

        assert!(entry("link-dir").dir, "a link to a folder is a folder");
        assert!(!entry("link-file").dir && !entry("broken").dir);
        assert!(entry(".gitignore").hidden && !entry("notes.md").hidden);
        assert!(entry("build").ignored && entry("app.log").ignored);
        assert!(!entry("src").ignored && !entry("notes.md").ignored);

        // Inside an ignored folder, everything is ignored.
        let inside = list(&base.join("build"), LIMIT).unwrap();
        assert!(inside.entries[0].ignored && !inside.truncated);

        assert_eq!(serde_json::to_value(&listing).unwrap(), golden());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_tracked_file_is_never_ignored_and_outside_a_repo_nothing_is() {
        let base = std::env::temp_dir().join(format!("hnv-list-tracked-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let plain = base.join("plain");
        fs::create_dir_all(repo.join("sub")).unwrap();
        fs::create_dir_all(&plain).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap();
            assert!(ok.success());
        };
        git(&["init", "-q"]);
        fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
        fs::write(repo.join("sub/kept.log"), "x").unwrap();
        fs::write(repo.join("sub/new.log"), "x").unwrap();
        git(&["add", "-f", "sub/kept.log"]);
        fs::write(plain.join("x.log"), "x").unwrap();

        let sub = list(&repo.join("sub"), LIMIT).unwrap();
        let ignored =
            |l: &Listing, name: &str| l.entries.iter().find(|e| e.name == name).unwrap().ignored;
        assert!(!ignored(&sub, "kept.log"));
        assert!(ignored(&sub, "new.log"));
        assert!(!ignored(&list(&plain, LIMIT).unwrap(), "x.log"));
        assert!(
            list(&plain.join("x.log"), LIMIT).is_err(),
            "a file is no folder"
        );
        fs::remove_dir_all(&base).unwrap();
    }
}
