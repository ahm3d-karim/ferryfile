#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `ferryfile convert …` and friends run and exit without the tray app.
    if let Some(code) = ferryfile_lib::cli::run_from_env() {
        std::process::exit(code);
    }
    ferryfile_lib::run()
}
