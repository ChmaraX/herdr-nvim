//! Herdr-side TOML config: `~/.config/herdr-nvim/config.toml` (or
//! `HERDR_NVIM_CONFIG` override). Controls the nvim binary used to spawn/attach
//! the daemon, how many trailing pane lines the file-picker scans, and how
//! many entries the picker's default (unfiltered) view shows.
//!
//! Missing file -> defaults, silently. Malformed file -> defaults, with one
//! warning on stderr (never panics or fails the caller).

use std::{env, fs, path::PathBuf};

use serde::Deserialize;

#[derive(Deserialize, Default, PartialEq, Eq, Debug)]
pub struct Config {
    #[serde(default)]
    pub sidebar: Sidebar,
    #[serde(default)]
    pub picker: Picker,
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
pub struct Sidebar {
    #[serde(default = "default_nvim_bin")]
    pub nvim_bin: String,
    #[serde(default)]
    pub position: SidebarPosition,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            nvim_bin: default_nvim_bin(),
            position: SidebarPosition::default(),
        }
    }
}

#[derive(Deserialize, Default, Copy, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

fn default_nvim_bin() -> String {
    "nvim".to_owned()
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
pub struct Picker {
    #[serde(default = "default_scan_lines")]
    pub scan_lines: u32,
    /// Maximum number of entries the picker's default (empty-filter) view
    /// shows from the recency-ordered "touched this session" list. A
    /// non-empty filter query still searches the full underlying candidate
    /// list, not just this capped default view.
    #[serde(default = "default_max_files")]
    pub max_files: u32,
    /// Reuse an existing fff.nvim frecency database (`~/.cache/nvim/fff_nvim`)
    /// for ranking, read-only via a temp copy.
    #[serde(default = "default_true")]
    pub frecency: bool,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            scan_lines: default_scan_lines(),
            max_files: default_max_files(),
            frecency: true,
        }
    }
}

fn default_scan_lines() -> u32 {
    300
}

fn default_max_files() -> u32 {
    20
}

fn default_true() -> bool {
    true
}

/// Path to the config file. `HERDR_NVIM_CONFIG` overrides everything (used by
/// tests and power users); otherwise `~/.config/herdr-nvim/config.toml` (via
/// `XDG_CONFIG_HOME`, falling back to `HOME/.config`).
fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("HERDR_NVIM_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("herdr-nvim").join("config.toml"))
}

/// Load the config file, falling back to defaults if it is missing or cannot
/// be located (e.g. no HOME set). A malformed file falls back to defaults too,
/// after printing one warning to stderr — a bad config must never prevent
/// herdr-nvim from working.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => return Config::default(),
    };
    match toml::from_str(&raw) {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "herdr-nvim: failed to parse config {}: {error} (using defaults)",
                path.display()
            );
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        sync::{Mutex, MutexGuard},
    };

    use super::*;

    static CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Redirects `HERDR_NVIM_CONFIG` to a fresh temp path for the duration of a
    /// test, restoring the prior env on drop.
    struct ConfigEnvGuard {
        _lock: MutexGuard<'static, ()>,
        old: Option<OsString>,
        path: PathBuf,
    }

    impl ConfigEnvGuard {
        fn new() -> Self {
            let lock = CONFIG_ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let path = env::temp_dir().join(format!(
                "hn-config-test-{}-{:?}.toml",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_file(&path);
            let old = env::var_os("HERDR_NVIM_CONFIG");
            env::set_var("HERDR_NVIM_CONFIG", &path);
            Self {
                _lock: lock,
                old,
                path,
            }
        }
    }

    impl Drop for ConfigEnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => env::set_var("HERDR_NVIM_CONFIG", value),
                None => env::remove_var("HERDR_NVIM_CONFIG"),
            }
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn full_file_round_trips() {
        let guard = ConfigEnvGuard::new();
        fs::write(
            &guard.path,
            "[sidebar]\nnvim_bin = \"nvim-custom\"\nposition = \"bottom\"\n\n[picker]\nscan_lines = 500\nmax_files = 50\n",
        )
        .unwrap();

        let config = load();
        assert_eq!(config.sidebar.nvim_bin, "nvim-custom");
        assert_eq!(config.sidebar.position, SidebarPosition::Bottom);
        assert_eq!(config.picker.scan_lines, 500);
        assert_eq!(config.picker.max_files, 50);
    }

    #[test]
    fn partial_file_leaves_picker_at_default() {
        let guard = ConfigEnvGuard::new();
        fs::write(&guard.path, "[sidebar]\nnvim_bin = \"nvim-custom\"\n").unwrap();

        let config = load();
        assert_eq!(config.sidebar.nvim_bin, "nvim-custom");
        assert_eq!(config.sidebar.position, SidebarPosition::Right);
        assert_eq!(config.picker.scan_lines, 300);
        assert_eq!(config.picker.max_files, 20);
    }

    #[test]
    fn partial_picker_table_leaves_max_files_at_default() {
        let guard = ConfigEnvGuard::new();
        fs::write(&guard.path, "[picker]\nscan_lines = 500\n").unwrap();

        let config = load();
        assert_eq!(config.picker.scan_lines, 500);
        assert_eq!(config.picker.max_files, 20);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let _guard = ConfigEnvGuard::new();
        // No file written at guard.path.

        let config = load();
        assert_eq!(config, Config::default());
        assert_eq!(config.sidebar.nvim_bin, "nvim");
        assert_eq!(config.sidebar.position, SidebarPosition::Right);
        assert_eq!(config.picker.scan_lines, 300);
        assert_eq!(config.picker.max_files, 20);
    }

    #[test]
    fn malformed_file_yields_defaults_without_panicking() {
        let guard = ConfigEnvGuard::new();
        fs::write(&guard.path, "this is not valid toml [[[").unwrap();

        let config = load();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn frecency_defaults_to_on() {
        let _guard = ConfigEnvGuard::new();

        let config = load();
        assert!(config.picker.frecency);
    }

    #[test]
    fn frecency_can_be_disabled() {
        let guard = ConfigEnvGuard::new();
        fs::write(&guard.path, "[picker]\nfrecency = false\n").unwrap();

        let config = load();
        assert!(!config.picker.frecency);
    }
}
