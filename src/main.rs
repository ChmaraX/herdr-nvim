mod daemon;
mod doctor;
mod extract;
mod herdr;
mod layout;
mod maneuver;
mod picker;
mod state;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let code = match mode.as_str() {
        "toggle" => run(maneuver::toggle_cmd),
        "sidebar" => run(daemon::sidebar_cmd),
        "daemon-gc" => run(daemon::gc_cmd),
        "doctor" => run(doctor::doctor_cmd),
        _ => {
            eprintln!("usage: herdr-nvim <toggle|sidebar|daemon-gc|doctor>");
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
