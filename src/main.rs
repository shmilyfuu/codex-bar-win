#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod usage;

#[cfg(target_os = "windows")]
mod gpui_app;
#[cfg(target_os = "windows")]
mod tray;

#[cfg(target_os = "windows")]
fn main() {
    gpui_app::run();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("codex-bar-win supports Windows only");
}
