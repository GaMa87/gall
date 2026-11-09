// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) =
        gall_core::fs::gitrewrite::handle_editor_args(&std::env::args_os().collect::<Vec<_>>())
    {
        std::process::exit(code);
    }
    gall_app::run()
}
