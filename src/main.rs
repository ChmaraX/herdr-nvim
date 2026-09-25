mod bridge;
mod candidates;
mod config;
mod daemon;
mod doctor;
mod extract;
mod fff;
mod gitscan;
mod herdr;
mod layout;
mod maneuver;
mod openlink;
mod path;
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
        _ => {
            eprintln!(
                "usage: herdr-nvim <toggle|sidebar|daemon-gc|on-event|daemons|doctor|pick-file|picker|open-link>"
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
