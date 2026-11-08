use serde::{de::DeserializeOwned, Serialize};
use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};
use tauri::{AppHandle, Manager};

// One lock for all data files. Hold it for a whole load -> modify -> save
// sequence so the background tracker and UI commands can't overwrite each other.
// Never hold it across an `.await`.
static LOCK: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn file_path(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

/// Returns the default value if the file is missing or unreadable.
pub fn load<T: DeserializeOwned + Default>(app: &AppHandle, name: &str) -> T {
    file_path(app, name)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes to a temp file first, then renames, so a crash can't leave a half-written file.
pub fn save<T: Serialize>(app: &AppHandle, name: &str, value: &T) -> Result<(), String> {
    let path = file_path(app, name)?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// One elo/trophy reading for a player, taken by the tracker.
#[derive(Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Snap {
    /// Unix seconds.
    pub t: i64,
    pub elo: i64,
    pub trophies: Option<i64>,
    /// Ranked season id, so a season reset isn't mistaken for a match result.
    pub season: Option<i64>,
}
