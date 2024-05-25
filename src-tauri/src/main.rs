// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;

mod lint;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![repeatlint])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn repeatlint(input: &str) -> Result<String, String> {
    lint::lint(input, Some(Path::new("."))).map_err(|e| format!("ERROR: {:?}", e))
}
