//! Herdr-side TOML config: `~/.config/herdr-nvim/config.toml` (or
//! `HERDR_NVIM_CONFIG` override). Controls how the sidebar's nvim processes
//! run (binary + environment), how many trailing pane lines the file-picker
//! scans, and how many entries the picker's default (unfiltered) view shows.
//!
//! Missing file -> defaults, silently. Malformed file -> defaults, with one
//! warning on stderr (never panics or fails the caller).

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use serde::Deserialize;

/// Whether `key` is safe as an environment-variable name: non-empty ASCII
/// alphanumeric/underscore (so nvim's `~/.config/<name>` lookups can never be
/// redirected outside the normal nvim namespace by a stray path separator),
/// and never `=`/NUL (which would make `Command::env` itself reject it).
fn is_valid_env_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[derive(Deserialize, Default, PartialEq, Eq, Debug)]
pub struct Config {
    #[serde(default)]
    pub sidebar: Sidebar,
    /// Per-machine Neovim settings; local executable paths must not leak to a remote host.
    #[serde(default)]
    pub remote: BTreeMap<String, Remote>,
    #[serde(default)]
    pub picker: Picker,
    #[serde(default, deserialize_with = "lenient_app")]
    pub app: App,
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
pub struct Remote {
    #[serde(default = "default_nvim_bin")]
    pub nvim_bin: String,
    #[serde(default)]
    pub nvim_env: Vec<String>,
    #[serde(default)]
    pub position: Option<SidebarPosition>,
}

impl Config {
    pub(crate) fn remote_sidebar(&self, machine: &str) -> Sidebar {
        match self.remote.get(machine) {
            Some(remote) => Sidebar {
                nvim_bin: remote.nvim_bin.clone(),
                nvim_env: remote.nvim_env.clone(),
                position: remote.position.unwrap_or(self.sidebar.position),
            },
            None => Sidebar {
                position: self.sidebar.position,
                ..Sidebar::default()
            },
        }
    }
}

/// `[app]`, read leniently: a value herdr-nvim doesn't know warns and falls
/// back to its default, so a typo there never costs the TUI its `[sidebar]`
/// and `[picker]` (one strict parse would fail as a whole).
fn lenient_app<'de, D: serde::Deserializer<'de>>(d: D) -> Result<App, D::Error> {
    Ok(App::from_toml(&toml::Value::deserialize(d)?))
}

impl App {
    fn from_toml(value: &toml::Value) -> Self {
        let mut app = Self::default();
        let Some(table) = value.as_table() else {
            eprintln!("herdr-nvim: config: [app] is not a table; using the defaults");
            return app;
        };
        if let Some(tree) = table.get("tree") {
            match tree.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("native") => app.tree = TreeMode::Native,
                Some("nvim") => app.tree = TreeMode::Nvim,
                _ => eprintln!(
                    "herdr-nvim: config: [app] tree is \"native\" or \"nvim\", not {tree}; using \"native\""
                ),
            }
        }
        if let Some(key) = table.get("tree_key") {
            match key.as_str() {
                Some(key) => app.tree_key = key.to_owned(),
                None => eprintln!(
                    "herdr-nvim: config: [app] tree_key is an nvim key (a string), not {key}; using {:?}",
                    app.tree_key
                ),
            }
        }
        app
    }
}

/// The Herdr desktop app's settings (`[app]`; the TUI plugin ignores them).
#[derive(Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct App {
    /// Which file tree goes with the tab's nvim: herdr-nvim's native tree in
    /// the Files panel (`native`), or nvim's own explorer inside the nvim
    /// pane (`nvim`). Read at every use, so a change applies at once.
    #[serde(default)]
    pub tree: TreeMode,
    /// The key (an nvim lhs) that shows and hides the native tree, in the
    /// tab's nvim and in the tree itself; `""` maps none. nvim reads it when
    /// it starts.
    #[serde(default = "default_tree_key")]
    pub tree_key: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tree: TreeMode::default(),
            tree_key: default_tree_key(),
        }
    }
}

fn default_tree_key() -> String {
    "<leader>e".to_owned()
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum TreeMode {
    #[default]
    Native,
    Nvim,
}

impl TreeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Nvim => "nvim",
        }
    }
}

#[derive(Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Sidebar {
    #[serde(default = "default_nvim_bin")]
    pub nvim_bin: String,
    /// Environment overrides applied to every nvim this plugin spawns or
    /// attaches to (sidebar daemon, `--remote-ui` window, health probes,
    /// open-file clients). Each entry must be `KEY=VALUE`; a typical use is
    /// `NVIM_APPNAME=myapp` to run the sidebar under the nvim config that
    /// lives in `~/.config/myapp` instead of vanilla nvim.
    #[serde(default)]
    pub nvim_env: Vec<String>,
    #[serde(default)]
    pub position: SidebarPosition,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            nvim_bin: default_nvim_bin(),
            nvim_env: Vec::new(),
            position: SidebarPosition::default(),
        }
    }
}

impl Sidebar {
    /// The configured environment overrides as parsed `(key, value)` pairs,
    /// applied to every `nvim_cmd` caller. Each is an env var on the child
    /// (an unordered set; duplicate keys resolve to the last value written).
    /// Malformed entries (no `=`, or a key that is not an env-style name) are
    /// skipped; `load()` warns about them once so a typo degrades to default
    /// behavior instead of breaking the sidebar.
    pub(crate) fn env_override(&self) -> Vec<(&str, &str)> {
        self.nvim_env
            .iter()
            .filter_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                if is_valid_env_key(key) {
                    Some((key, value))
                } else {
                    None
                }
            })
            .collect()
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

/// The config file's path for a message: `config_path`, or the default
/// spelled with `~` when no home is known.
pub(crate) fn config_file_display() -> String {
    config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "~/.config/herdr-nvim/config.toml".to_owned())
}

/// Path to the config file. `HERDR_NVIM_CONFIG` overrides everything (used by
/// tests and power users); otherwise `~/.config/herdr-nvim/config.toml` (via
/// `XDG_CONFIG_HOME`, falling back to `HOME/.config`).
pub(crate) fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("HERDR_NVIM_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(".config")))?;
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
    let parsed: Result<Config, toml::de::Error> = toml::from_str(&raw);
    match parsed {
        Ok(config) => {
            let invalid: Vec<&str> = config
                .sidebar
                .nvim_env
                .iter()
                .filter(|entry| {
                    entry
                        .split_once('=')
                        .is_none_or(|(key, _)| !is_valid_env_key(key))
                })
                .map(String::as_str)
                .collect();
            if !invalid.is_empty() {
                eprintln!(
                    "herdr-nvim: ignoring invalid sidebar.nvim_env entries: {} (each must be KEY=VALUE with an ASCII alphanumeric/_ key)",
                    invalid.join(", ")
                );
            }
            config
        }
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
            "[sidebar]\nnvim_bin = \"nvim-custom\"\nnvim_env = [\"NVIM_APPNAME=myapp\", \"WEIRD=1\"]\nposition = \"bottom\"\n\n[picker]\nscan_lines = 500\nmax_files = 50\n",
        )
        .unwrap();

        let config = load();
        assert_eq!(config.sidebar.nvim_bin, "nvim-custom");
        assert_eq!(
            config.sidebar.env_override(),
            vec![("NVIM_APPNAME", "myapp"), ("WEIRD", "1")]
        );
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
    fn nvim_env_skips_malformed_entries() {
        let side = Sidebar {
            nvim_bin: "nvim".to_owned(),
            nvim_env: vec![
                "NVIM_APPNAME=myapp".to_owned(),
                "no-equals-sign".to_owned(),
                "BAD KEY=oops".to_owned(),
                "path/never=valid".to_owned(),
                "EMPTY=".to_owned(),
            ],
            position: SidebarPosition::Right,
        };
        assert_eq!(
            side.env_override(),
            vec![("NVIM_APPNAME", "myapp"), ("EMPTY", "")]
        );
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

    #[test]
    fn remote_settings_do_not_inherit_local_executable_paths_or_profiles() {
        let config: Config = toml::from_str("[sidebar]\nnvim_bin = '/mac/nvim'\nnvim_env = ['NVIM_APPNAME=local']\nposition = 'bottom'\n[remote.box]\nnvim_bin = '/remote/nvim'\nnvim_env = ['NVIM_APPNAME=work']\n").unwrap();
        let remote = config.remote_sidebar("box");
        assert_eq!(remote.nvim_bin, "/remote/nvim");
        assert_eq!(remote.env_override(), vec![("NVIM_APPNAME", "work")]);
        assert_eq!(remote.position, SidebarPosition::Bottom);
        let defaults = config.remote_sidebar("other");
        assert_eq!(defaults.nvim_bin, "nvim");
        assert!(defaults.nvim_env.is_empty());
        assert_eq!(defaults.position, SidebarPosition::Bottom);
    }

    #[test]
    fn a_bad_app_section_keeps_the_rest_of_the_config() {
        let config: Config = toml::from_str(
            "[sidebar]\nnvim_bin = \"/opt/nvim\"\n[app]\ntree = \"bogus\"\ntree_key = 5\n",
        )
        .unwrap();
        assert_eq!(config.sidebar.nvim_bin, "/opt/nvim");
        assert_eq!(config.app, App::default());
        let config: Config = toml::from_str("app = 3\n").unwrap();
        assert_eq!(config.app, App::default());
        let config: Config = toml::from_str("[app]\ntree = \"nvim\"\ntree_key = \"\"\n").unwrap();
        assert_eq!(
            (config.app.tree, config.app.tree_key.as_str()),
            (TreeMode::Nvim, "")
        );
    }
}
