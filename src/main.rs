#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod usage;

#[cfg(target_os = "windows")]
mod fluent_renderer;
#[cfg(target_os = "windows")]
mod windows_app;

#[cfg(target_os = "windows")]
fn main() {
    if let Err(error) = windows_app::run() {
        windows_app::show_fatal_error(&error);
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("codex-bar-win supports Windows only");
}
