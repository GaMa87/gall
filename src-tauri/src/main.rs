// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let raw_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(code) = gall_app::pre_run(&raw_args) {
        std::process::exit(code);
    }
    gall_app::run()
}
