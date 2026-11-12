//! "Welcome back" recap: how many trophies / how much ranked elo changed while the app was closed.
//!
//! The tracker only sees your profile while the app runs, so at every launch the last reading from
//! the previous run is remembered as a baseline (before the tracker takes a new one). The recap
//! compares that baseline with your profile right now. It stays pending until it's dismissed, so a
//! launch hidden in the tray doesn't lose it, and changes add up across launches.

use crate::{api, history, matches, settings, store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

const FILE: &str = "recap_base.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Base {
    pub set: bool,
    /// Unix seconds of the last reading from before this launch.
    pub t: i64,
    pub trophies: Option<i64>,
    pub elo: i64,
    pub season: Option<i64>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Recap {
    /// When the last reading was taken (unix seconds).
    pub since: i64,
    pub trophy_change: Option<i64>,
    pub trophies_now: Option<i64>,
    pub elo_change: Option<i64>,
    pub elo_now: Option<i64>,
    pub games: u32,
    pub wins: u32,
    pub losses: u32,
}

/// Change in trophies and ranked elo, or None where it can't be told (no earlier reading, no
/// ranked, or a new ranked season whose reset isn't a result).
pub fn deltas(base: &Base, trophies: Option<i64>, elo: i64, season: Option<i64>) -> (Option<i64>, Option<i64>) {
    let tr = match (base.trophies, trophies) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    let el = (base.elo > 0 && elo > 0 && base.season == season).then(|| elo - base.elo);
    (tr, el)
}

/// Call at startup, before the tracker takes its first reading.
pub fn remember_baseline(app: &AppHandle) {
    let s = settings::load(app);
    if s.player_tag.trim().is_empty() {
        return;
    }
    let _g = store::lock();
    let current: Base = store::load(app, FILE);
    if current.set {
        return; // an earlier recap hasn't been seen yet; keep counting from there
    }
    let tag = api::normalize_tag(&s.player_tag);
    let h = history::load_locked(app, &tag);
    if let Some(last) = h.snaps.last() {
        let _ = store::save(app, FILE, &Base { set: true, t: last.t, trophies: last.trophies, elo: last.elo, season: last.season });
    }
}

#[tauri::command]
pub async fn get_recap(app: AppHandle) -> Result<Option<Recap>, String> {
    let base: Base = {
        let _g = store::lock();
        store::load(&app, FILE)
    };
    if !base.set {
        return Ok(None);
    }
    let s = settings::load(&app);
    if s.api_key.is_empty() || s.player_tag.trim().is_empty() {
        return Ok(None);
    }
    let tag = api::normalize_tag(&s.player_tag);
    let player = api::fetch_player(&s.api_key, &tag).await?; // offline: keep the baseline, try again later
    let trophies = player["trophies"].as_i64();
    let elo = player["rankedElo"].as_i64().unwrap_or(0);
    let season = player["rankedSeasonId"].as_i64();
    let (tr, el) = deltas(&base, trophies, elo, season);

    if tr.unwrap_or(0) == 0 && el.unwrap_or(0) == 0 {
        let _g = store::lock();
        let _ = store::save(&app, FILE, &Base::default()); // nothing changed: nothing to show
        return Ok(None);
    }

    // Games from the battle log since the baseline (the log only holds the last 25).
    let (mut games, mut wins, mut losses) = (0u32, 0u32, 0u32);
    if let Ok(log) = api::fetch_battlelog(&s.api_key, &tag).await {
        let items: Vec<Value> = log.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        for m in matches::group_matches(&items, &tag).iter().filter(|m| (m.complete || !m.ranked) && m.end > base.t) {
            games += 1;
            match m.result.as_str() {
                "victory" => wins += 1,
                "defeat" => losses += 1,
                _ => {}
            }
        }
    }
    Ok(Some(Recap {
        since: base.t,
        trophy_change: tr.filter(|d| *d != 0),
        trophies_now: trophies,
        elo_change: el.filter(|d| *d != 0),
        elo_now: (elo > 0).then_some(elo),
        games,
        wins,
        losses,
    }))
}

/// The person closed the recap: start fresh from the next launch.
#[tauri::command]
pub fn ack_recap(app: AppHandle) -> Result<(), String> {
    let _g = store::lock();
    store::save(&app, FILE, &Base::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Base {
        Base { set: true, t: 100, trophies: Some(40_000), elo: 5000, season: Some(7) }
    }

    #[test]
    fn computes_trophy_and_elo_changes() {
        assert_eq!(deltas(&base(), Some(40_250), 5030, Some(7)), (Some(250), Some(30)));
        assert_eq!(deltas(&base(), Some(39_900), 4980, Some(7)), (Some(-100), Some(-20)));
        assert_eq!(deltas(&base(), Some(40_000), 5000, Some(7)), (Some(0), Some(0)));
    }

    #[test]
    fn season_reset_and_no_ranked_are_not_elo_changes() {
        assert_eq!(deltas(&base(), Some(40_100), 1200, Some(8)).1, None, "new season");
        assert_eq!(deltas(&base(), Some(40_100), 0, Some(7)).1, None, "no ranked now");
        let mut b = base();
        b.elo = 0;
        assert_eq!(deltas(&b, Some(40_100), 5000, Some(7)).1, None, "no ranked before");
        assert_eq!(deltas(&b, None, 5000, Some(7)).0, None, "unknown trophies");
    }
}
