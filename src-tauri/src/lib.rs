mod api;
mod history;
mod icons;
mod matches;
mod meta;
mod presence;
mod settings;
mod stats;
mod store;
mod tracker;
mod updater;

use serde_json::{json, Value};
use settings::Settings;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

type Task = tauri::async_runtime::JoinHandle<()>;

struct PresenceState(Mutex<Option<Task>>);
struct TrackerState(Mutex<Option<Task>>);

/// Starts or stops the background tracker so it matches the setting.
fn apply_tracking(app: &AppHandle, enabled: bool) {
    let state = app.state::<TrackerState>();
    let mut slot = state.0.lock().unwrap_or_else(|e| e.into_inner());
    match (enabled, slot.is_some()) {
        (true, false) => *slot = Some(tauri::async_runtime::spawn(tracker::run(app.clone()))),
        (false, true) => {
            if let Some(h) = slot.take() {
                h.abort();
            }
        }
        _ => {}
    }
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let launcher = app.autolaunch();
    let is_on = launcher.is_enabled().unwrap_or(false);
    if enabled && !is_on {
        let _ = launcher.enable();
    } else if !enabled && is_on {
        let _ = launcher.disable();
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/* ------------------------------ settings -------------------------------- */

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    settings::load(&app)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    settings::save(&app, &settings)?;
    apply_tracking(&app, settings.tracking_enabled);
    apply_autostart(&app, settings.launch_at_login);
    Ok(())
}

/* ------------------------------- players -------------------------------- */

#[tauri::command]
async fn fetch_player(app: AppHandle, tag: String) -> Result<Value, String> {
    let s = settings::load(&app);
    api::fetch_player(&s.api_key, &tag).await
}

/// Matches for any player: the stored history (if we track them) plus whatever the live
/// battle log adds. A ranked best-of-3 is one match. Live entries are not saved from here.
#[tauri::command]
async fn get_matches(app: AppHandle, tag: String) -> Result<Value, String> {
    let s = settings::load(&app);
    let key = api::normalize_tag(&tag);
    let mut h = history::load_history(&app, &key);
    let tracked = !h.items.is_empty() || !h.snaps.is_empty();
    match api::fetch_battlelog(&s.api_key, &key).await {
        Ok(log) => {
            let items = log.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            h.add_items(&items);
        }
        Err(e) if !tracked => return Err(e),
        Err(_) => {} // offline: show what we stored
    }
    let mut ms = h.matches(&key);
    ms.truncate(150);
    Ok(json!({ "matches": ms, "tracked": tracked }))
}

/// Win rates, sessions and graph data from stored history.
#[tauri::command]
fn get_stats(app: AppHandle, tag: String) -> Value {
    let key = api::normalize_tag(&tag);
    let h = history::load_history(&app, &key);
    let ms = h.matches(&key);
    let mut sessions = history::sessions(&ms);
    sessions.truncate(15);
    json!({
        "tracked": !h.items.is_empty() || !h.snaps.is_empty(),
        "summary": stats::summarize(&ms),
        "series": stats::series(&h),
        "days": stats::day_results(&ms),
        "sessions": sessions,
    })
}

/// Sampled meta summary for the recommender UI. With `refresh` set, re-samples first.
#[tauri::command]
async fn get_meta(app: AppHandle, ranked_only: bool, source: String, refresh: bool, limit: Option<u32>, parallel: Option<u32>) -> Result<Value, String> {
    let mut m = meta::load(&app);
    if refresh {
        let s = settings::load(&app);
        m = meta::refresh(&app, &s.api_key, &s.player_tag, limit.unwrap_or(200), parallel.unwrap_or(4)).await?;
    }
    let tiers = meta::tiers_for(&source);
    Ok(json!({
        "fetched_at": m.fetched_at,
        "players": m.players,
        "rounds": m.rounds,
        "tier_players": m.tier_players,
        "maps": meta::maps(&m, ranked_only, &tiers),
        "brawlers": meta::brawlers(&m).into_iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
    }))
}

#[tauri::command]
async fn recommend(
    app: AppHandle,
    map: String,
    ranked_only: bool,
    source: String,
    banned: Vec<i64>,
    only_owned: bool,
) -> Result<Value, String> {
    let m = meta::load(&app);
    let owned = if only_owned {
        let s = settings::load(&app);
        let p = api::fetch_player(&s.api_key, &s.player_tag).await?;
        let map: HashMap<i64, i64> = p["brawlers"]
            .as_array()
            .map(|a| a.iter().filter_map(|b| Some((b["id"].as_i64()?, b["power"].as_i64().unwrap_or(0)))).collect())
            .unwrap_or_default();
        Some(map)
    } else {
        None
    };
    let banned: std::collections::HashSet<i64> = banned.into_iter().collect();
    let recs = meta::recommend(&m, &map, ranked_only, &meta::tiers_for(&source), &banned, owned.as_ref());
    Ok(json!({ "recs": recs }))
}

/// What the Presence page shows: the rotation, the placeholder values and which view is pinned.
#[tauri::command]
async fn get_presence_preview(app: AppHandle) -> Result<Value, String> {
    let s = settings::load(&app);
    if s.api_key.trim().is_empty() || s.player_tag.trim().is_empty() {
        return Err("Set your API key and player tag in Settings first.".into());
    }
    let ctx = presence::context(&app, &s).await?;
    Ok(json!({
        "views": presence::views(&ctx, &s),
        "placeholders": presence::placeholders(&ctx),
    }))
}

/// Deletes the saved games and elo readings for one player (not favorites or settings).
#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn clear_history(app: AppHandle, tag: String) -> Result<(), String> {
    let key = api::normalize_tag(&tag);
    let _g = store::lock();
    store::save(&app, &history::file_name(&key), &history::History::default())
}

#[tauri::command]
async fn get_icons(app: AppHandle) -> HashMap<String, String> {
    icons::get(&app).await
}

/* ------------------------------ favorites ------------------------------- */

#[tauri::command]
fn get_favorites(app: AppHandle) -> Vec<tracker::FavoriteView> {
    tracker::favorites_view(&app)
}

#[tauri::command]
fn add_favorite(app: AppHandle, tag: String, name: String) -> Result<(), String> {
    let tag = tracker::add_favorite(&app, &tag, &name)?;
    // Take the first reading right away instead of waiting for the next cycle.
    let key = settings::load(&app).api_key;
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        if tracker::poll_player(&app2, &key, &tag).await.is_ok() {
            let _ = app2.emit("tracker-update", ());
        }
    });
    Ok(())
}

#[tauri::command]
fn remove_favorite(app: AppHandle, tag: String) -> Result<(), String> {
    tracker::remove_favorite(&app, &tag)
}

/* ---------------------- tracking and activity log ------------------------ */

#[tauri::command]
fn get_tracking_status(app: AppHandle) -> Vec<tracker::TrackedView> {
    tracker::tracking_status(&app)
}

#[tauri::command]
fn get_activity(app: AppHandle, tag: Option<String>) -> Vec<tracker::ActivityEntry> {
    tracker::activity(&app, tag.as_deref())
}

#[tauri::command]
fn clear_activity(app: AppHandle) -> Result<(), String> {
    tracker::clear_activity(&app)
}

#[tauri::command]
fn tracker_running(state: State<TrackerState>) -> bool {
    state.0.lock().map(|s| s.is_some()).unwrap_or(false)
}

/* -------------------------------- presence ------------------------------- */

fn begin_presence(app: &AppHandle) -> Result<(), String> {
    let s = settings::load(app);
    if s.discord_client_id.trim().is_empty() {
        return Err("Set a Discord application ID in Settings first.".into());
    }
    if s.api_key.trim().is_empty() {
        return Err("Set your Brawl Stars API key in Settings first.".into());
    }
    if s.player_tag.trim().is_empty() {
        return Err("Set your player tag in Settings first.".into());
    }

    let state = app.state::<PresenceState>();
    let mut slot = state.0.lock().map_err(|_| "internal error".to_string())?;
    if let Some(old) = slot.take() {
        old.abort();
    }
    *slot = Some(tauri::async_runtime::spawn(presence::run(app.clone(), s)));
    let _ = app.emit("presence-status", true);
    Ok(())
}

#[tauri::command]
fn start_presence(app: AppHandle) -> Result<(), String> {
    begin_presence(&app)
}

#[tauri::command]
fn stop_presence(app: AppHandle, state: State<PresenceState>) -> Result<(), String> {
    let mut slot = state.0.lock().map_err(|_| "internal error".to_string())?;
    if let Some(handle) = slot.take() {
        handle.abort();
    }
    let _ = app.emit("presence-status", false);
    let _ = app.emit("presence-log", "Presence stopped.");
    Ok(())
}

#[tauri::command]
fn presence_running(state: State<PresenceState>) -> bool {
    state.0.lock().map(|s| s.is_some()).unwrap_or(false)
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Brawl Stars Hub", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Brawl Stars Hub")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch just brings the running window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .manage(PresenceState(Mutex::new(None)))
        .manage(TrackerState(Mutex::new(None)))
        .setup(|app| {
            let handle = app.handle().clone();
            let s = settings::load(&handle);

            build_tray(app)?;
            apply_tracking(&handle, s.tracking_enabled);
            apply_autostart(&handle, s.launch_at_login);

            // Started by Windows login: stay in the tray instead of popping up.
            let hidden = std::env::args().any(|a| a == "--hidden");
            if !(hidden && s.close_to_tray) {
                show_main(&handle);
            }

            if s.auto_start_presence {
                if let Err(e) = begin_presence(&handle) {
                    let _ = handle.emit("presence-log", e);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if settings::load(window.app_handle()).close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            fetch_player,
            get_matches,
            get_stats,
            get_icons,
            clear_history,
            get_presence_preview,
            get_meta,
            recommend,
            get_favorites,
            add_favorite,
            remove_favorite,
            get_tracking_status,
            get_activity,
            clear_activity,
            tracker_running,
            start_presence,
            stop_presence,
            presence_running,
            updater::check_update,
            updater::install_update,
            updater::whats_new,
            updater::ack_whats_new,
            app_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running Brawl Stars Hub");
}
