use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub api_key: String,
    pub player_tag: String,
    pub discord_client_id: String,
    /// Background tracking of you + favorites (activity log, history, elo changes).
    pub tracking_enabled: bool,
    /// The window's X button hides the app to the system tray instead of quitting.
    pub close_to_tray: bool,
    /// Start with Windows (hidden in the tray).
    pub launch_at_login: bool,
    /// Start Discord Rich Presence automatically when the app opens.
    pub auto_start_presence: bool,
    /// Desktop notifications (favorite started playing, rank changes).
    pub notifications: bool,
    /// Which pages the Rich Presence rotates through.
    pub presence_trophies: bool,
    pub presence_ranked: bool,
    pub presence_session: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            player_tag: String::new(),
            discord_client_id: String::new(),
            tracking_enabled: true,
            close_to_tray: true,
            launch_at_login: false,
            auto_start_presence: false,
            notifications: true,
            presence_trophies: true,
            presence_ranked: true,
            presence_session: true,
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}
