// Prevents an extra console window on Windows in release. No effect on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `LTX\ Mac\ Farm.app/Contents/MacOS/ltx-mac-farm --selftest` exercises every
    // wizard action headlessly instead of starting the menubar app.
    if std::env::args().any(|a| a == "--selftest") {
        std::process::exit(ltx_mac_farm_lib::selftest());
    }
    ltx_mac_farm_lib::run();
}
