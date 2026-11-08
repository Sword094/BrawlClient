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
    pub presence_latest: bool,
    pub presence_streak: bool,
    pub presence_victories: bool,
    pub presence_record: bool,
    pub presence_club: bool,
    /// Custom text lines; {placeholders} are replaced on every refresh.
    pub presence_line1: String,
    pub presence_line2: String,
    /// Add the custom text to the rotation.
    pub presence_custom: bool,
    /// Show only this view ("latest", "trophies", "custom", ...). Empty = rotate.
    pub presence_pin: String,
    /// Brawler picture (large) and map picture (small badge) on Discord.
    pub presence_images: bool,
    /// "View stats" profile button (other people see it, you don't).
    pub presence_button: bool,
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
            presence_latest: true,
            presence_streak: true,
            presence_victories: true,
            presence_record: true,
            presence_club: false,
            presence_line1: "{brawler} main · {trophies} trophies".into(),
            presence_line2: "{rank} · {winrate} win rate".into(),
            presence_custom: false,
            presence_pin: String::new(),
            presence_images: true,
            presence_button: true,
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
