// Prevents an extra console window on Windows in release. No effect on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    farmmon_lib::run();
}
