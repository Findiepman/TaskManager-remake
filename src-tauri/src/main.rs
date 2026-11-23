// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tm_lib::win::elevate;

fn main() {
    elevate::wait_for_previous();
    // TM_DEV_AS_INVOKER=1 at build time skips the elevation step, for UI testing only.
    if option_env!("TM_DEV_AS_INVOKER") != Some("1") && !elevate::is_elevated() {
        match elevate::relaunch_elevated() {
            elevate::Launch::Handled => return,
            elevate::Launch::RunLimited => {}
        }
    }
    tm_lib::run();
}
