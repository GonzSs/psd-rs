// main.rs — Entry point for psd-rs.
//
// This file is intentionally minimal. All logic lives in the library crate.
// main.rs only loads the configuration and hands control to run_daemon().

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = psd_rs::config::load()?;
    psd_rs::run_daemon(config)
}
