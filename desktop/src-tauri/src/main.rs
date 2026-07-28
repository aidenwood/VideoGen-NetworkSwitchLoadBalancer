// Prevents an extra console window on Windows in release. No effect on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `LTX\ Mac\ Farm.app/Contents/MacOS/ltx-mac-farm --selftest` exercises every
    // wizard action headlessly instead of starting the menubar app.
    if args.iter().any(|a| a == "--selftest") {
        std::process::exit(ltx_mac_farm_lib::selftest());
    }
    // `--serve` runs only the web gateway: no menubar, no window. That's how a
    // headless render Mac joins the team view and stays setup-able from a browser.
    if args.iter().any(|a| a == "--serve") {
        std::process::exit(ltx_mac_farm_lib::serve());
    }
    ltx_mac_farm_lib::run();
}
