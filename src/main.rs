#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex;
mod model;
mod settings;
#[cfg(windows)]
mod ui;

fn main() {
    #[cfg(windows)]
    ui::run();
    #[cfg(not(windows))]
    eprintln!("TokWatch requires Windows 11.");
}
