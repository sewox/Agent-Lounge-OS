// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let start_route = app_lib::initial_window_route();
    app_lib::run_with_start_route(start_route);
}
