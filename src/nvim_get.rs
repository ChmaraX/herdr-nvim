//! Installing Neovim for herdr-nvim (0.10 or newer), the same on every OS,
//! for the installer and for the app's "Neovim is needed" card:
//! - with Homebrew (macOS, Linux): `brew install neovim` (or upgrade);
//! - else Neovim's own release for this machine, checked against the SHA-256
//!   GitHub lists for it, into `<data>/herdr-nvim/nvim/<tag>-<asset>/` (no
//!   sudo, no admin, the system's own nvim untouched), and `[sidebar]
//!   nvim_bin` pointed at it, so the app's PATH doesn't matter.
//!
//! Downloads and unpacking use the system's own `curl` and `tar` (on Windows
//! the ones in System32: Windows 10 1803 and later; its bsdtar reads zip).

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::install::osfs;

pub enum Method {
    Homebrew(PathBuf),
    /// The release asset's name without its extension, and the extension.
    Release(&'static str, &'static str),
}

/// Homebrew when it is there, else the release asset for this machine.
/// `HERDR_NVIM_INSTALL_METHOD=brew|download` decides instead (the e2e: never
/// Homebrew).
pub fn method() -> Result<Method> {
    let forced = std::env::var("HERDR_NVIM_INSTALL_METHOD").ok();
    let brew = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("brew"))
        .find(|brew| cfg!(unix) && brew.is_file());
    match (forced.as_deref(), brew) {
        (Some("brew"), None) => bail!("HERDR_NVIM_INSTALL_METHOD=brew, but there is no brew"),
        (Some("brew") | None, Some(brew)) => Ok(Method::Homebrew(brew)),
        (Some("download") | None, _) => release_asset(),
        (Some(other), _) => bail!("HERDR_NVIM_INSTALL_METHOD is brew or download, not {other}"),
    }
}

fn release_asset() -> Result<Method> {
    use std::env::consts::{ARCH, OS};
    Ok(match (OS, ARCH) {
        ("macos", "aarch64") => Method::Release("nvim-macos-arm64", "tar.gz"),
        ("macos", "x86_64") => Method::Release("nvim-macos-x86_64", "tar.gz"),
        ("linux", "x86_64") => Method::Release("nvim-linux-x86_64", "tar.gz"),
        ("linux", "aarch64") => Method::Release("nvim-linux-arm64", "tar.gz"),
        ("windows", _) => Method::Release("nvim-win64", "zip"),
        _ => bail!("no Neovim release for {OS} {ARCH}; install it yourself (https://neovim.io)"),
    })
}

/// For the installer's question: "Install it now, <this>?"
pub fn describe(method: &Method) -> String {
    match method {
        Method::Homebrew(_) => "Homebrew's (brew install neovim)".to_owned(),
        Method::Release(..) => format!(
            "Neovim's own release, into {} (no {})",
            dest().map_or_else(
                |_| "~/.local/share/herdr-nvim/nvim".to_owned(),
                |d| d.display().to_string()
            ),
            if cfg!(windows) { "admin" } else { "sudo" }
        ),
    }
}

/// `<data>/herdr-nvim/nvim`.
pub fn dest() -> Result<PathBuf> {
    Ok(crate::install::data_dir()?.join("herdr-nvim").join("nvim"))
}

fn usable() -> Result<String, crate::nvim_check::NvimMissing> {
    crate::nvim_check::check(&crate::config::load().sidebar)
}

/// Makes the configured nvim one herdr-nvim runs on, or says why not.
pub fn install(method: &Method) -> Result<()> {
    if let Ok(version) = usable() {
        println!("herdr-nvim: Neovim is already there: {version}");
        return Ok(());
    }
    match method {
        Method::Homebrew(brew) => {
            let has = Command::new(brew)
                .args(["list", "neovim"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            let verb = if has { "upgrade" } else { "install" };
            println!("herdr-nvim: brew {verb} neovim");
            if !Command::new(brew)
                .args([verb, "neovim"])
                .status()?
                .success()
            {
                bail!("brew {verb} neovim failed");
            }
            // Not the one herdr-nvim finds (another nvim first on its PATH):
            // pointed at Homebrew's.
            if usable().is_err() {
                let prefix = Command::new(brew).arg("--prefix").output()?;
                let prefix = String::from_utf8_lossy(&prefix.stdout).trim().to_owned();
                set_nvim_bin(&Path::new(&prefix).join("bin/nvim"))?;
            }
        }
        Method::Release(asset, ext) => {
            let bin = download(asset, ext)?;
            set_nvim_bin(&bin)?;
            println!(
                "herdr-nvim: installed Neovim in {}",
                bin.parent()
                    .and_then(Path::parent)
                    .unwrap_or(&bin)
                    .display()
            );
        }
    }
    let version =
        usable().map_err(|missing| anyhow::anyhow!("Neovim is still not usable: {missing}"))?;
    println!("herdr-nvim: ready: Neovim {version}");
    Ok(())
}

fn set_nvim_bin(bin: &Path) -> Result<()> {
    crate::app::settings::write_setting("sidebar", "nvim_bin", &bin.to_string_lossy())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

/// The latest release's `asset`, checked and unpacked into its own folder;
/// returns its nvim. A new version goes into a new folder, never over one a
/// running nvim uses.
fn download(asset: &str, ext: &str) -> Result<PathBuf> {
    let file = format!("{asset}.{ext}");
    println!("herdr-nvim: looking up Neovim's latest release");
    let json = fetch_text("https://api.github.com/repos/neovim/neovim/releases/latest")
        .context("could not reach GitHub's API")?;
    let release: Release =
        serde_json::from_str(&json).context("GitHub's API answered no release")?;
    let entry = release
        .assets
        .iter()
        .find(|a| a.name == file)
        .with_context(|| format!("Neovim {} has no {file}", release.tag_name))?;
    let want = entry
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .with_context(|| format!("GitHub lists no SHA-256 for {file}; not installed"))?;

    let dest = dest()?;
    fs::create_dir_all(&dest)?;
    let home = dest.join(format!("{}-{asset}", release.tag_name));
    let bin = home
        .join("bin")
        .join(format!("nvim{}", std::env::consts::EXE_SUFFIX));
    if bin.is_file() && runs(&bin) {
        return Ok(bin);
    }
    let work = dest.join(osfs::unique(".new-"));
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<PathBuf> {
        let archive = work.join(&file);
        println!(
            "herdr-nvim: downloading Neovim {} ({file})",
            release.tag_name
        );
        let status = Command::new(curl())
            .args(["--fail", "--location", "--progress-bar", "--output"])
            .arg(&archive)
            .arg(&entry.browser_download_url)
            .status()
            .context("cannot run curl")?;
        if !status.success() {
            bail!("the download failed");
        }
        if sha256_file(&archive)? != want.to_ascii_lowercase() {
            bail!("the download does not match its SHA-256; not installed");
        }
        let status = Command::new(tar())
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&work)
            .status()
            .context("cannot run tar")?;
        if !status.success() {
            bail!("could not unpack {file}");
        }
        osfs::remove_tree(&home)?;
        osfs::rename_retrying(&work.join(asset), &home)
            .with_context(|| format!("{file} has no {asset}/ folder"))?;
        if !runs(&bin) {
            bail!(
                "the downloaded Neovim does not run on this machine ({})",
                bin.display()
            );
        }
        Ok(bin.clone())
    })();
    let _ = osfs::remove_tree(&work);
    result
}

fn runs(nvim: &Path) -> bool {
    Command::new(nvim)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn fetch_text(url: &str) -> Result<String> {
    let output = Command::new(curl())
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--header", "Accept: application/vnd.github+json", url])
        .stderr(Stdio::inherit())
        .output()
        .context("cannot run curl")?;
    if !output.status.success() {
        bail!("curl {url} failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Windows' own, by full path: never Git's curl or tar that PATH may put first.
fn system32(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join("System32").join(name)
}

fn curl() -> PathBuf {
    if cfg!(windows) {
        system32("curl.exe")
    } else {
        "curl".into()
    }
}

fn tar() -> PathBuf {
    if cfg!(windows) {
        system32("tar.exe")
    } else {
        "tar".into()
    }
}

/// `herdr-nvim app-install-nvim [--pause]`: the card's Install button, in a
/// terminal tab of its own so the person sees what runs (and Homebrew's
/// questions). `--pause`: wait for Enter at the end, then close the tab (a
/// finished terminal keeps its tab).
pub fn install_nvim_cmd() -> Result<()> {
    let pause = crate::argv::args().skip(2).any(|arg| arg == "--pause");
    let result = method().and_then(|method| install(&method));
    if let Err(error) = &result {
        eprintln!("herdr-nvim: {error:#}");
    } else {
        println!("herdr-nvim: open Files again to start nvim.");
    }
    if pause {
        print!("\nPress Enter to close this tab. ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = std::io::stdin().read_line(&mut String::new());
        close_own_tab();
    }
    result
}

/// The app's tab this terminal is in (its `herdr.tab` label), closed.
fn close_own_tab() {
    let Some(terminal) = std::env::var_os("HC_TERMINAL_ID") else {
        return;
    };
    let tab = crate::app::rpc::hc()
        .arg("get")
        .arg(terminal)
        .arg("--json")
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|out| serde_json::from_slice::<serde_json::Value>(&out.stdout).ok())
        .and_then(|got| got["labels"]["herdr.tab"].as_str().map(str::to_owned));
    if let Some(tab) = tab {
        let _ = crate::app::cli::app_do(
            "tab.close",
            &serde_json::json!({"id": tab, "confirmed": true}),
        );
    }
}
