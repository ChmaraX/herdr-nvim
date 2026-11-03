mod app;
mod bridge;
mod candidates;
mod config;
mod daemon;
mod doctor;
mod extract;
mod fff;
mod gitscan;
mod herdr;
mod home;
mod host;
mod layout;
mod maneuver;
mod openlink;
mod path;
mod pathkey;
mod picker;
mod sessions;
mod state;
#[cfg(test)]
mod test_support;

/// The app's commands its mods wait on (`exec`, no timeout of its own): each
/// ends within `deadline`, whatever it waits for, so a mod is never left
/// waiting (the controller would stay busy, the Files button dead).
const SHORT: &[&str] = &[
    "toggle",
    "open",
    "fs",
    "app-find",
    "app-current",
    "app-reveal",
    "app-preview",
    "app-agents",
    "app-send",
    "pick-file",
    "open-link",
    "app-config",
];

fn deadline(after: std::time::Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(after);
        eprintln!("herdr-nvim: gave up after {}s", after.as_secs());
        std::process::exit(124);
    });
}

fn main() {
    // Prepend common install locations to PATH so `nvim`/`herdr` resolve even
    // when herdr launched us with a minimal PATH (e.g. GUI-started on macOS).
    // Must happen before anything spawns a child or a thread.
    path::augment_path();

    let mode = std::env::args().nth(1).unwrap_or_default();
    // The one place that asks which Herdr this is (`host`).
    let host = host::choose();
    let app = host == host::Host::App;
    if app && SHORT.contains(&mode.as_str()) {
        deadline(std::time::Duration::from_secs(30));
    }
    let code = match (mode.as_str(), app) {
        ("toggle", false) => run(maneuver::toggle_cmd),
        ("toggle", true) => run(app::sidebar::toggle_cmd),
        ("sidebar", _) => run(daemon::sidebar_cmd),
        ("daemon-gc", _) => run(daemon::registry::gc_cmd),
        ("on-event", _) => run(daemon::events::on_event_cmd),
        ("daemons", _) => run(daemon::inventory::daemons_cmd),
        ("doctor", _) => run(doctor::doctor_cmd),
        ("pick-file", _) => run(bridge::pick_file_cmd),
        ("picker", _) => run(picker::picker_cmd),
        ("open-link", false) => run(openlink::open_link_cmd),
        ("open-link", true) => run(app::commands::open_link),
        // Herdr app host only (HERDR_NVIM_HOST=app): see `app`.
        ("app-agents", true) => run(app::commands::agents_cmd),
        ("app-send", true) => run(app::commands::send_cmd),
        ("app-watch", true) => run(app::watch::app_watch_cmd),
        ("open", true) => run(app::commands::open_cmd),
        ("fs", true) => run(app::tree::fs_cmd),
        ("app-find", true) => run(app::tree::find_cmd),
        ("app-current", true) => run(app::tree::current_cmd),
        ("app-reveal", true) => run(app::tree::reveal_cmd),
        ("app-preview", true) => run(app::sidebar::preview_cmd),
        // The install script runs these before the wrapper exists.
        ("app-setup", _) => run(app::setup::setup_cmd),
        ("app-config", _) => run(app::settings::app_config_cmd),
        (
            "app-agents" | "app-send" | "app-watch" | "open" | "fs" | "app-find" | "app-current"
            | "app-reveal" | "app-preview",
            false,
        ) => {
            eprintln!("herdr-nvim: `{mode}` is for the Herdr app (HERDR_NVIM_HOST=app)");
            2
        }
        _ => {
            eprintln!(
                "usage: herdr-nvim <toggle|sidebar|daemon-gc|on-event|daemons|doctor|pick-file|picker|open-link|app-agents|app-send|app-watch|app-setup|open|fs|app-find|app-current|app-reveal|app-preview|app-config>"
            );
            2
        }
    };
    std::process::exit(code);
}

fn run(f: fn() -> anyhow::Result<()>) -> i32 {
    match f() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("herdr-nvim: {e:#}");
            1
        }
    }
}
