use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// This struct mirrors your application's settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub install_folder: String,
    pub ui_zoom_factor: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            install_folder: std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .to_string_lossy()
                .to_string(),
            ui_zoom_factor: 1.15,
        }
    }
}

// Find the OS-specific config folder (e.g., ~/.config/grm/)
fn get_config_path() -> Option<PathBuf> {
    if let Some(proj_dirs) = ProjectDirs::from("com", "GRM", "grm") {
        let config_dir = proj_dirs.config_dir();

        // Ensure the directory exists before we try to read/write to it
        if !config_dir.exists() {
            let _ = fs::create_dir_all(config_dir);
        }

        Some(config_dir.join("config.json"))
    } else {
        None
    }
}

pub fn load_config() -> AppConfig {
    if let Some(path) = get_config_path() {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(config) = serde_json::from_str(&contents) {
                return config;
            }
        }
    }
    // If it doesn't exist or parsing fails, return defaults
    AppConfig::default()
}

pub fn save_config(config: &AppConfig) {
    if let Some(path) = get_config_path() {
        if let Ok(json) = serde_json::to_string_pretty(config) {
            let _ = fs::write(path, json);
        }
    }
}
