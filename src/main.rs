#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod codex;
#[cfg(windows)]
mod installation;
mod model;
mod settings;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod updater;

fn main() {
    if std::env::args().any(|arg| arg == "--claude-statusline") {
        if let Err(error) = claude::run_statusline() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--uninstall-integration") {
        if let Err(error) = claude::uninstall_integration() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    {
        if !std::env::args().any(|arg| arg == "--demo" || arg == "--demo-claude") {
            installation::refresh_display_version();
        }
        ui::run();
    }
    #[cfg(not(windows))]
    eprintln!("TokWatch requires Windows 11.");
}
