//! Self-updating through GitHub Releases (tauri-plugin-updater) and the "What's new" screen.
//!
//! The updater downloads `latest.json` from the newest release, compares versions, and installs a
//! build that was signed with the project's private key (checked against `pubkey` in
//! tauri.conf.json). The "What's new" text is the release description from GitHub.

use crate::{api, store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

pub const REPO: &str = "Sword094/BrawlClient";
const STATE_FILE: &str = "app_state.json";

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct AppState {
    /// The app version that last showed (or was already past) its "What's new" screen.
    last_seen_version: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct UpdateInfo {
    pub current: String,
    pub version: String,
    pub notes: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WhatsNew {
    pub version: String,
    pub previous: String,
    pub notes: String,
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// The public key is set in tauri.conf.json once the signing key exists; until then updates are off.
fn configured(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(Value::as_str)
        .map_or(false, |k| !k.trim().is_empty())
}

const NOT_SET_UP: &str = "Updates aren't set up in this build (no signing key in tauri.conf.json).";

/// Is a newer release published? `Ok(None)` means you're up to date.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    if !configured(&app) {
        return Err(NOT_SET_UP.into());
    }
    let found = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Couldn't check for updates: {e}"))?;
    Ok(found.map(|u| UpdateInfo { current: u.current_version.clone(), version: u.version.clone(), notes: u.body.clone().unwrap_or_default() }))
}

/// Downloads and installs the newest release, then restarts the app. Emits `update-progress`.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    if !configured(&app) {
        return Err(NOT_SET_UP.into());
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Couldn't check for updates: {e}"))?
        .ok_or("You're already on the newest version.")?;

    let mut got: u64 = 0;
    let progress = app.clone();
    let finished = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                got += chunk as u64;
                let _ = progress.emit("update-progress", json!({ "got": got, "total": total }));
            },
            move || {
                let _ = finished.emit("update-progress", json!({ "done": true }));
            },
        )
        .await
        .map_err(|e| format!("Update failed: {e}"))?;
    app.restart()
}

/// Release description for `version` from GitHub (empty if it can't be fetched).
async fn release_notes(version: &str) -> String {
    let url = format!("https://api.github.com/repos/{REPO}/releases/tags/v{version}");
    let res = api::client()
        .get(url)
        .header("User-Agent", "BrawlClient-updater")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;
    let Ok(res) = res else { return String::new() };
    if !res.status().is_success() {
        return String::new();
    }
    res.json::<Value>().await.ok().and_then(|v| v["body"].as_str().map(str::to_string)).unwrap_or_default()
}

/// Decides whether to show "What's new": only when the app was updated since it last ran.
/// A first install just records the version. `force` always returns the current version's notes.
fn decide(last_seen: &str, current: &str, force: bool) -> Option<String> {
    if force {
        return Some(last_seen.to_string());
    }
    if last_seen.is_empty() || last_seen == current {
        return None;
    }
    Some(last_seen.to_string())
}

#[tauri::command]
pub async fn whats_new(app: AppHandle, force: Option<bool>) -> Result<Option<WhatsNew>, String> {
    let current = current_version(&app);
    let last = {
        let _g = store::lock();
        let st: AppState = store::load(&app, STATE_FILE);
        if st.last_seen_version.is_empty() {
            // First launch of any version: nothing to announce, just remember it.
            let _ = store::save(&app, STATE_FILE, &AppState { last_seen_version: current.clone() });
        }
        st.last_seen_version
    };
    let Some(previous) = decide(&last, &current, force.unwrap_or(false)) else { return Ok(None) };
    let notes = release_notes(&current).await;
    Ok(Some(WhatsNew { version: current, previous, notes }))
}

/// Called when the person closes the "What's new" screen.
#[tauri::command]
pub fn ack_whats_new(app: AppHandle) -> Result<(), String> {
    let _g = store::lock();
    store::save(&app, STATE_FILE, &AppState { last_seen_version: current_version(&app) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_new_only_after_an_update() {
        assert_eq!(decide("", "0.2.0", false), None, "first install");
        assert_eq!(decide("0.2.0", "0.2.0", false), None, "same version");
        assert_eq!(decide("0.1.0", "0.2.0", false), Some("0.1.0".into()), "updated");
        assert!(decide("0.2.0", "0.2.0", true).is_some(), "forced from Settings");
    }
}
