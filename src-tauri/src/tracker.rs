//! Background tracking of you + your favorites.
//!
//! The Brawl Stars API has no "is this player in a game" flag and no per-match elo change,
//! so both are derived here:
//!  * "Likely playing" = the newest battle in the battle log finished within the last 10 minutes.
//!  * Every poll stores the raw battle-log entries and an elo reading (`history`). Matches
//!    (a ranked best-of-3 series is ONE match) and their elo changes are computed from those.
//!  * The activity log gets one line per finished match. Ranked lines wait up to
//!    `ELO_WAIT_SECS` for the profile's elo to catch up with the battle log.

use crate::history::{self, History};
use crate::matches::{parse_battle_time, pretty_mode, Match};
use crate::store::{self, Snap};
use crate::{api, settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const POLL_EVERY: Duration = Duration::from_secs(60);
const BETWEEN_PLAYERS: Duration = Duration::from_millis(400);
const ACTIVE_WINDOW_SECS: i64 = 600;
/// How long a finished ranked match waits for its elo change before being logged without one.
const ELO_WAIT_SECS: i64 = 180;
const MAX_ACTIVITY: usize = 500;

const FAVORITES_FILE: &str = "favorites.json";
const STATE_FILE: &str = "tracker_state.json";
const ACTIVITY_FILE: &str = "activity.json";

/* ------------------------------ data types ------------------------------ */

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Favorite {
    pub tag: String, // normalized: no '#', uppercase
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct PlayerState {
    pub initialized: bool,
    pub name: String,
    pub trophies: i64,
    pub ranked_rank_name: String,
    /// Numeric rank (higher = better), used to tell promotions from demotions.
    pub ranked_rank: Option<i64>,
    pub ranked_elo: Option<i64>,
    pub season: Option<i64>,
    pub last_seen_battle: String,
    pub last_battle_time: String,
    /// End time (unix) of the newest match already written to the activity log.
    pub logged_end: i64,
    pub active: bool,
    pub last_checked: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ActivityEntry {
    pub ts: i64,
    pub tag: String,
    pub name: String,
    /// "game" | "session" | "info"
    pub kind: String,
    pub text: String,
    pub result: Option<String>,
    pub delta: Option<i64>,
    pub combined: Option<u32>,
    /// "elo" | "trophies" | ""
    pub unit: String,
}

/// Something worth a desktop notification.
pub struct Event {
    /// "session" (started playing) | "rank" (promoted/demoted)
    pub kind: &'static str,
    pub title: String,
    pub body: String,
}

#[derive(Serialize)]
pub struct FavoriteView {
    pub tag: String,
    pub name: String,
    pub state: Option<PlayerState>,
}

#[derive(Serialize)]
pub struct TrackedView {
    pub tag: String,
    pub name: String,
    pub is_self: bool,
    pub is_favorite: bool,
    pub state: Option<PlayerState>,
}

/* ------------------------------- helpers -------------------------------- */

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_recent(battle_time: &str, now: i64) -> bool {
    parse_battle_time(battle_time).map_or(false, |t| now - t <= ACTIVE_WINDOW_SECS)
}

fn bt(b: &Value) -> Option<&str> {
    b.get("battleTime").and_then(Value::as_str)
}

/// "victory" / "defeat" / "draw", or "#3" for placement-based modes.
fn outcome_of(battle: &Value) -> String {
    battle["result"]
        .as_str()
        .map(str::to_string)
        .or_else(|| battle["rank"].as_i64().map(|r| format!("#{r}")))
        .unwrap_or_else(|| "?".to_string())
}

fn label(outcome: &str) -> String {
    if outcome.starts_with('#') {
        return format!("Placed {outcome}");
    }
    let mut c = outcome.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn entry(
    ts: i64,
    tag: &str,
    name: &str,
    kind: &str,
    text: String,
    result: Option<String>,
    delta: Option<i64>,
    unit: &str,
) -> ActivityEntry {
    ActivityEntry {
        ts,
        tag: tag.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        text,
        result,
        delta,
        combined: None,
        unit: unit.to_string(),
    }
}

/// Matches that are over: every non-ranked entry, and ranked series that reached their final round.
fn finished(m: &Match) -> bool {
    m.complete || !m.ranked
}

/* --------------------------- core tracking logic ------------------------- */

/// Pure state transition for one player after one poll. No I/O, so it can be unit tested.
#[allow(clippy::too_many_arguments)]
pub fn apply_poll_logic(
    tag: &str,
    st: &mut PlayerState,
    activity: &mut Vec<ActivityEntry>,
    hist: &mut History,
    events: &mut Vec<Event>,
    player: &Value,
    items: &[Value],
    now: i64,
) {
    let name = player["name"].as_str().unwrap_or("?").to_string();
    let elo = player["rankedElo"].as_i64();
    let rank = player["rankedRank"].as_i64();
    let season = player["rankedSeasonId"].as_i64();
    let newest = items.iter().filter_map(bt).max().unwrap_or("").to_string();

    st.name = name.clone();
    st.trophies = player["trophies"].as_i64().unwrap_or(0);
    st.ranked_rank_name = player["rankedRankName"].as_str().unwrap_or("").to_string();
    st.last_checked = now;

    hist.add_items(items);
    // Players without ranked get elo 0; only ranked matches read it, and the graphs ignore 0.
    hist.add_snap(Snap { t: now, elo: elo.unwrap_or(0), trophies: Some(st.trophies), season });
    let matches = hist.matches(tag);

    // First time we see this player: everything already in the log is saved to the history
    // but not replayed into the activity log.
    if !st.initialized {
        st.initialized = true;
        st.last_seen_battle = newest.clone();
        st.last_battle_time = newest.clone();
        st.logged_end = matches.iter().filter(|m| finished(m)).map(|m| m.end).max().unwrap_or(0);
        st.ranked_elo = elo;
        st.ranked_rank = rank;
        st.season = season;
        st.active = is_recent(&newest, now);
        activity.push(entry(now, tag, &name, "info", format!("Started tracking {name}."), None, None, ""));
        return;
    }

    let active_now = is_recent(&newest, now);
    let first_fresh = items
        .iter()
        .filter_map(bt)
        .filter(|t| *t > st.last_seen_battle.as_str())
        .min()
        .map(str::to_string);

    if active_now && !st.active {
        let ts = first_fresh.as_deref().and_then(parse_battle_time).map_or(now, |t| t - 1);
        activity.push(entry(ts, tag, &name, "session", format!("{name} started playing."), None, None, ""));
        events.push(Event {
            kind: "session",
            title: format!("{name} started playing"),
            body: "A new game just finished.".to_string(),
        });
    }
    if newest.as_str() > st.last_seen_battle.as_str() {
        st.last_seen_battle = newest.clone();
    }

    // Log each newly finished match once, oldest first.
    let mut todo: Vec<&Match> = matches.iter().filter(|m| finished(m) && m.end > st.logged_end).collect();
    todo.reverse();
    for m in todo {
        let elo_unsettled = m.ranked && matches!(m.elo_change, None | Some(0));
        if elo_unsettled && now - m.end < ELO_WAIT_SECS {
            break; // try again next poll; keeps the log in order
        }
        let raw = hist.items.iter().find(|i| bt(i).and_then(parse_battle_time) == Some(m.end));
        let mode = pretty_mode(&m.mode);
        if m.ranked {
            let score = format!("{}-{}", m.wins, m.losses);
            activity.push(entry(
                m.end, tag, &name, "game",
                format!("{} - Ranked {} on {} ({score})", label(&m.result), mode, m.map),
                Some(m.result.clone()),
                m.elo_change,
                "elo",
            ));
        } else {
            let outcome = raw.map(|r| outcome_of(&r["battle"])).unwrap_or_else(|| m.result.clone());
            activity.push(entry(
                m.end, tag, &name, "game",
                format!("{} - {} on {}", label(&outcome), mode, m.map),
                Some(outcome.to_lowercase()),
                raw.and_then(|r| r["battle"]["trophyChange"].as_i64()),
                "trophies",
            ));
        }
        st.logged_end = m.end;
    }

    if let (Some(old), Some(new)) = (st.ranked_rank, rank) {
        if new != old {
            let verb = if new > old { "ranked up to" } else { "dropped to" };
            let text = format!("{name} {verb} {}.", st.ranked_rank_name);
            activity.push(entry(now, tag, &name, "info", text.clone(), None, None, ""));
            events.push(Event { kind: "rank", title: format!("{name}: rank change"), body: text });
        }
    }
    if rank.is_some() {
        st.ranked_rank = rank;
    }

    st.ranked_elo = elo;
    st.season = season;
    if !newest.is_empty() {
        st.last_battle_time = newest.clone();
    }

    if !active_now && st.active {
        let mins = parse_battle_time(&newest).map_or(0, |t| (now - t) / 60);
        activity.push(entry(
            now, tag, &name, "session",
            format!("{name} went idle (last game {mins} min ago)."),
            None, None, "",
        ));
    }
    st.active = active_now;
}

/* ------------------------------ I/O wrappers ----------------------------- */

fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

fn apply_poll(app: &AppHandle, tag: &str, is_self: bool, player: &Value, items: &[Value]) {
    let now = unix_now();
    let guard = store::lock();

    let mut states: HashMap<String, PlayerState> = store::load(app, STATE_FILE);
    let mut activity: Vec<ActivityEntry> = store::load(app, ACTIVITY_FILE);
    let mut hist = history::load_locked(app, tag);
    let mut events: Vec<Event> = Vec::new();

    {
        let st = states.entry(tag.to_string()).or_default();
        apply_poll_logic(tag, st, &mut activity, &mut hist, &mut events, player, items, now);
    }

    activity.sort_by_key(|e| std::cmp::Reverse(e.ts));
    activity.truncate(MAX_ACTIVITY);

    let _ = store::save(app, STATE_FILE, &states);
    let _ = store::save(app, ACTIVITY_FILE, &activity);
    history::save_locked(app, tag, &hist);
    drop(guard);

    if !events.is_empty() && settings::load(app).notifications {
        for e in events {
            // You know when you start playing; only notify about other people doing so.
            if e.kind == "session" && is_self {
                continue;
            }
            notify(app, &e.title, &e.body);
        }
    }
}

/// Fetch profile + battle log for one player and update stored state.
pub async fn poll_player(app: &AppHandle, api_key: &str, tag: &str) -> Result<(), String> {
    let tag = api::normalize_tag(tag);
    let player = api::fetch_player(api_key, &tag).await?;
    let log = api::fetch_battlelog(api_key, &tag).await?;
    let items = log.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let is_self = api::normalize_tag(&settings::load(app).player_tag) == tag;
    apply_poll(app, &tag, is_self, &player, &items);
    Ok(())
}

fn tracked_tags(app: &AppHandle) -> Vec<String> {
    let s = settings::load(app);
    let mut tags = Vec::new();
    let me = api::normalize_tag(&s.player_tag);
    if !me.is_empty() {
        tags.push(me);
    }
    let _guard = store::lock();
    let favs: Vec<Favorite> = store::load(app, FAVORITES_FILE);
    for f in favs {
        if !tags.contains(&f.tag) {
            tags.push(f.tag);
        }
    }
    tags
}

/// Runs until aborted. Checks every tracked player once a minute.
pub async fn run(app: AppHandle) {
    let mut last_error = String::new();
    loop {
        let s = settings::load(&app);
        if !s.api_key.trim().is_empty() {
            for tag in tracked_tags(&app) {
                match poll_player(&app, &s.api_key, &tag).await {
                    Ok(()) => last_error.clear(),
                    Err(e) => {
                        if e != last_error {
                            let _ = app.emit("tracker-error", format!("{tag}: {e}"));
                            last_error = e.clone();
                        }
                        if e.contains("429") {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                    }
                }
                tokio::time::sleep(BETWEEN_PLAYERS).await;
            }
            let _ = app.emit("tracker-update", ());
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/* ---------------------------- command helpers ---------------------------- */

pub fn favorites_view(app: &AppHandle) -> Vec<FavoriteView> {
    let _guard = store::lock();
    let favs: Vec<Favorite> = store::load(app, FAVORITES_FILE);
    let states: HashMap<String, PlayerState> = store::load(app, STATE_FILE);
    favs.into_iter()
        .map(|f| {
            let state = states.get(&f.tag).cloned();
            let name = state
                .as_ref()
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or(f.name);
            FavoriteView { tag: f.tag, name, state }
        })
        .collect()
}

pub fn add_favorite(app: &AppHandle, tag: &str, name: &str) -> Result<String, String> {
    let tag = api::normalize_tag(tag);
    if tag.is_empty() {
        return Err("Missing player tag.".into());
    }
    let _guard = store::lock();
    let mut favs: Vec<Favorite> = store::load(app, FAVORITES_FILE);
    if !favs.iter().any(|f| f.tag == tag) {
        favs.push(Favorite { tag: tag.clone(), name: name.to_string() });
        store::save(app, FAVORITES_FILE, &favs)?;
    }
    Ok(tag)
}

pub fn remove_favorite(app: &AppHandle, tag: &str) -> Result<(), String> {
    let tag = api::normalize_tag(tag);
    let _guard = store::lock();
    let mut favs: Vec<Favorite> = store::load(app, FAVORITES_FILE);
    favs.retain(|f| f.tag != tag);
    store::save(app, FAVORITES_FILE, &favs)
}

pub fn tracking_status(app: &AppHandle) -> Vec<TrackedView> {
    let s = settings::load(app);
    let me = api::normalize_tag(&s.player_tag);
    let _guard = store::lock();
    let favs: Vec<Favorite> = store::load(app, FAVORITES_FILE);
    let states: HashMap<String, PlayerState> = store::load(app, STATE_FILE);

    let mut out = Vec::new();
    if !me.is_empty() {
        let state = states.get(&me).cloned();
        let name = state.as_ref().map(|s| s.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Me".into());
        let is_favorite = favs.iter().any(|f| f.tag == me);
        out.push(TrackedView { tag: me.clone(), name, is_self: true, is_favorite, state });
    }
    for f in favs {
        if f.tag == me {
            continue;
        }
        let state = states.get(&f.tag).cloned();
        let name = state.as_ref().map(|s| s.name.clone()).filter(|n| !n.is_empty()).unwrap_or(f.name);
        out.push(TrackedView { tag: f.tag, name, is_self: false, is_favorite: true, state });
    }
    out
}

pub fn activity(app: &AppHandle, tag: Option<&str>) -> Vec<ActivityEntry> {
    let _guard = store::lock();
    let mut all: Vec<ActivityEntry> = store::load(app, ACTIVITY_FILE);
    if let Some(t) = tag.map(api::normalize_tag).filter(|t| !t.is_empty()) {
        all.retain(|e| e.tag == t);
    }
    all.sort_by_key(|e| std::cmp::Reverse(e.ts));
    all.truncate(300);
    all
}

pub fn clear_activity(app: &AppHandle) -> Result<(), String> {
    let _guard = store::lock();
    store::save(app, ACTIVITY_FILE, &Vec::<ActivityEntry>::new())
}

/// (is this player tracked yet?, battle time -> elo change)

/* --------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(time: &str) -> i64 {
        parse_battle_time(time).unwrap()
    }

    fn player(elo: i64) -> Value {
        json!({"name":"TestPlayer","trophies":51594,"rankedRankName":"MYTHIC III","rankedRank":15,"rankedElo":elo,"rankedSeasonId":50})
    }

    fn teams() -> Value {
        json!([
            [{"tag":"#ME","name":"Me","brawler":{"id":1,"name":"MORTIS","power":11}},
             {"tag":"#A","name":"A","brawler":{"id":2,"name":"LEON","power":11}},
             {"tag":"#B","name":"B","brawler":{"id":3,"name":"CHESTER","power":11}}],
            [{"tag":"#C","name":"C","brawler":{"id":4,"name":"TARA","power":11}},
             {"tag":"#D","name":"D","brawler":{"id":5,"name":"DARRYL","power":11}},
             {"tag":"#E","name":"E","brawler":{"id":6,"name":"CROW","power":11}}]
        ])
    }

    /// One round of a ranked series; `star` marks the final round.
    fn round(time: &str, result: &str, star: bool) -> Value {
        json!({"battleTime": time, "event": {"id": 7, "mode":"gemGrab","map":"Gem Fort"},
               "battle": {"mode":"gemGrab","type":"soloRanked","result":result,"duration":100,
                          "starPlayer": if star { json!({"tag":"#C","name":"C"}) } else { Value::Null },
                          "teams": teams()}})
    }

    fn trophy_game(time: &str, result: &str, change: i64) -> Value {
        json!({"battleTime": time, "event": {"id": 9, "mode":"brawlBall","map":"Sneaky Fields"},
               "battle": {"mode":"brawlBall","type":"ranked","result":result,"trophyChange":change,
                          "duration":90,"teams": teams()}})
    }

    struct Rig {
        st: PlayerState,
        log: Vec<ActivityEntry>,
        hist: History,
        events: Vec<Event>,
    }

    impl Rig {
        fn new() -> Self {
            Rig { st: PlayerState::default(), log: vec![], hist: History::default(), events: vec![] }
        }
        fn poll(&mut self, elo: i64, items: &[Value], now: &str) {
            self.poll_player(&player(elo), items, now);
        }
        fn poll_player(&mut self, p: &Value, items: &[Value], now: &str) {
            apply_poll_logic("ME", &mut self.st, &mut self.log, &mut self.hist, &mut self.events, p, items, at(now));
        }
        fn games(&self) -> Vec<&ActivityEntry> {
            self.log.iter().filter(|e| e.kind == "game").collect()
        }
    }

    /// A finished 2-1 win: three rounds, star on the last.
    fn won_series() -> Vec<Value> {
        vec![
            round("20261107T222129.000Z", "victory", true),
            round("20261107T221901.000Z", "defeat", false),
            round("20261107T221700.000Z", "victory", false),
        ]
    }

    #[test]
    fn first_poll_only_takes_a_baseline() {
        let mut r = Rig::new();
        r.poll(5700, &won_series(), "20261107T222200.000Z");
        assert!(r.st.initialized);
        assert!(r.games().is_empty());
        assert_eq!(r.hist.items.len(), 3);
        assert_eq!(r.hist.snaps.len(), 1);
    }

    #[test]
    fn a_best_of_three_is_one_logged_game_with_one_elo_change() {
        let mut r = Rig::new();
        r.poll(5700, &[], "20261107T221000.000Z");
        // Series finishes; the profile hasn't caught up yet, so nothing is logged yet.
        r.poll(5700, &won_series(), "20261107T222200.000Z");
        assert!(r.games().is_empty());
        // Elo catches up.
        r.poll(5725, &won_series(), "20261107T222300.000Z");
        let g = r.games();
        assert_eq!(g.len(), 1, "3 rounds must be 1 game");
        assert_eq!(g[0].delta, Some(25));
        assert_eq!(g[0].unit, "elo");
        assert!(g[0].text.contains("Victory") && g[0].text.contains("(2-1)"));
        // Polling again never logs it twice.
        r.poll(5725, &won_series(), "20261107T222400.000Z");
        assert_eq!(r.games().len(), 1);
    }

    #[test]
    fn gives_up_waiting_for_elo_and_logs_without_a_change() {
        let mut r = Rig::new();
        r.poll(5700, &[], "20261107T221000.000Z");
        r.poll(5700, &won_series(), "20261107T222200.000Z");
        r.poll(5700, &won_series(), "20261107T222700.000Z"); // > 180s later, elo never moved
        let g = r.games();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].delta, Some(0));
    }

    #[test]
    fn an_unfinished_series_is_not_logged() {
        let mut r = Rig::new();
        r.poll(5700, &[], "20261107T221000.000Z");
        let partial = vec![round("20261107T221901.000Z", "defeat", false), round("20261107T221700.000Z", "victory", false)];
        r.poll(5700, &partial, "20261107T230000.000Z");
        assert!(r.games().is_empty());
    }

    #[test]
    fn non_ranked_games_use_trophy_change() {
        let mut r = Rig::new();
        r.poll(5700, &[], "20261107T221000.000Z");
        r.poll(5700, &[trophy_game("20261107T222129.000Z", "victory", 8)], "20261107T222200.000Z");
        let g = r.games();
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].delta, g[0].unit.as_str()), (Some(8), "trophies"));
    }

    #[test]
    fn logs_session_start_and_idle() {
        let mut r = Rig::new();
        r.poll(5700, &[], "20261107T221000.000Z");
        r.poll(5700, &[trophy_game("20261107T222129.000Z", "victory", 8)], "20261107T222200.000Z");
        assert!(r.log.iter().any(|e| e.text.contains("started playing")));
        assert_eq!(r.events.iter().filter(|e| e.kind == "session").count(), 1);
        r.poll(5700, &[trophy_game("20261107T222129.000Z", "victory", 8)], "20261107T224000.000Z");
        assert!(r.log.iter().any(|e| e.text.contains("went idle")));
    }

    #[test]
    fn announces_rank_changes() {
        let mut r = Rig::new();
        r.poll(5700, &[], "20261107T221000.000Z");
        let mut p = player(5800);
        p["rankedRank"] = json!(16);
        p["rankedRankName"] = json!("LEGENDARY I");
        r.poll_player(&p, &[], "20261107T221100.000Z");
        assert_eq!(r.events.iter().filter(|e| e.kind == "rank").count(), 1);
        assert!(r.log.iter().any(|e| e.text.contains("ranked up to LEGENDARY I")));
    }
}
