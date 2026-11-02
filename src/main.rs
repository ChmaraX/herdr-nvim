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

fn main() {
    // Prepend common install locations to PATH so `nvim`/`herdr` resolve even
    // when herdr launched us with a minimal PATH (e.g. GUI-started on macOS).
    // Must happen before anything spawns a child or a thread.
    path::augment_path();

    let mode = std::env::args().nth(1).unwrap_or_default();
    let code = match mode.as_str() {
        "toggle" => run(maneuver::toggle_cmd),
        "sidebar" => run(daemon::sidebar_cmd),
        "daemon-gc" => run(daemon::registry::gc_cmd),
        "on-event" => run(daemon::events::on_event_cmd),
        "daemons" => run(daemon::inventory::daemons_cmd),
        "doctor" => run(doctor::doctor_cmd),
        "pick-file" => run(bridge::pick_file_cmd),
        "picker" => run(picker::picker_cmd),
        "open-link" => run(openlink::open_link_cmd),
        // Herdr app host only (HERDR_NVIM_HOST=app): see `app`.
        "app-agents" => run(app::commands::agents_cmd),
        "app-send" => run(app::commands::send_cmd),
        "app-watch" => run(app::watch::app_watch_cmd),
        "app-setup" => run(app::setup::setup_cmd),
        "open" => run(app::commands::open_cmd),
        "fs" => run(app::tree::fs_cmd),
        "app-find" => run(app::tree::find_cmd),
        "app-current" => run(app::tree::current_cmd),
        "app-reveal" => run(app::tree::reveal_cmd),
        "app-config" => run(app::settings::app_config_cmd),
        _ => {
            eprintln!(
                "usage: herdr-nvim <toggle|sidebar|daemon-gc|on-event|daemons|doctor|pick-file|picker|open-link|app-agents|app-send|app-watch|app-setup|open|fs|app-find|app-current|app-reveal|app-config>"
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
