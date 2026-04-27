// Prevents a Windows console window from opening alongside the GUI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    forgiven_companion_lib::run();
}
