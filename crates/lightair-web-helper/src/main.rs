//! Chromium's helper process (renderer, GPU, utility…) for Lightair's web pages: Chromium starts
//! it with `--type=…`; it runs until Chromium is done with it.

// Windows: no console window for each of Chromium's processes
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    std::process::exit(lightair_web::helper_main().unwrap_or(0));
}
