//! Ranked brawler recommender.
//!
//! The official API has no meta data, no bans and no map pool, so the "meta" is sampled:
//! the battle logs of the top global players are read and every 3v3 round is counted per
//! (map, brawler). Win rate is smoothed so a brawler seen 6 times can't beat one seen 400 times.
//! Bans are entered by hand in the UI.

use crate::{api, store, tracker::unix_now};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

const FILE: &str = "meta.json";
/// Smoothing: pretend every brawler starts with this many wins and losses.
const PRIOR: f64 = 8.0;
const MIN_GAMES: u32 = 5;

/// Ranked tiers, lowest to highest. The first word of `rankedRankName` ("MYTHIC III") is the tier.
const TIERS: [&str; 8] = ["bronze", "silver", "gold", "diamond", "mythic", "legendary", "masters", "pro"];
const MIN_TIER: usize = 3; // diamond

pub fn tier_of(rank_name: &str) -> Option<String> {
    let t = rank_name.split_whitespace().next()?.to_lowercase();
    TIERS.contains(&t.as_str()).then_some(t)
}

fn tier_index(t: &str) -> Option<usize> {
    TIERS.iter().position(|x| *x == t)
}

/// Which tiers a data-source choice covers. Never goes below Diamond.
pub fn tiers_for(source: &str) -> HashSet<String> {
    let at_least = |i: usize| TIERS[i..].iter().map(|t| t.to_string()).collect::<HashSet<_>>();
    match source {
        "diamond" | "mythic" | "legendary" | "masters" => [source.to_string()].into(),
        "mythic+" => at_least(4),
        "legendary+" => at_least(5),
        _ => at_least(MIN_TIER), // "diamond+" and anything unknown
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Cell {
    pub map: String,
    pub mode: String,
    pub ranked: bool,
    /// Tier of the player whose battle log this round came from.
    pub tier: String,
    pub event_id: i64,
    pub id: i64,
    pub brawler: String,
    pub wins: u32,
    pub games: u32,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Meta {
    pub fetched_at: i64,
    pub players: u32,
    pub rounds: u32,
    /// Players sampled per tier.
    pub tier_players: HashMap<String, u32>,
    pub cells: Vec<Cell>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct MapInfo {
    pub map: String,
    pub event_id: i64,
    pub mode: String,
    pub games: u32,
}

#[derive(Serialize, Debug)]
pub struct Rec {
    pub id: i64,
    /// Final ranking value: smoothed win rate nudged by how strong the player's copy is.
    pub pick: f64,
    pub brawler: String,
    pub games: u32,
    pub win_rate: f64,
    pub score: f64,
    pub power: Option<i64>,
}

/* ------------------------------ aggregation ------------------------------ */

#[derive(Default)]
pub struct Collector {
    seen: HashSet<String>,
    cells: HashMap<(String, String, bool, i64), Cell>,
    pub rounds: u32,
}

impl Collector {
    /// Adds every 3v3 round of one player's battle log. Rounds already counted from another
    /// player's log (same time, event and players) are skipped.
    ///
    /// `tier` is the log owner's ranked tier; every round from this log is filed under it.
    /// Returns the tags of the other players seen in ranked rounds, for finding more players.
    pub fn add_log(&mut self, owner_tag: &str, tier: &str, items: &[Value]) -> Vec<String> {
        let owner = api::normalize_tag(owner_tag);
        let mut others: Vec<String> = Vec::new();
        for it in items {
            let b = &it["battle"];
            let Some(teams) = b["teams"].as_array() else { continue };
            if teams.len() != 2 {
                continue;
            }
            let result = b["result"].as_str().unwrap_or("");
            if result != "victory" && result != "defeat" {
                continue;
            }
            let mut tags: Vec<String> = teams
                .iter()
                .flat_map(|t| t.as_array().into_iter().flatten())
                .map(|p| p["tag"].as_str().unwrap_or("").to_string())
                .collect();
            tags.sort();
            let key = format!(
                "{}|{}|{}",
                it["battleTime"].as_str().unwrap_or(""),
                it["event"]["id"].as_i64().unwrap_or(0),
                tags.join(",")
            );
            if !self.seen.insert(key) {
                continue;
            }
            let owner_team = teams.iter().position(|t| {
                t.as_array()
                    .map_or(false, |a| a.iter().any(|p| api::normalize_tag(p["tag"].as_str().unwrap_or("")) == owner))
            });
            let Some(owner_team) = owner_team else { continue };

            let ranked = crate::matches::is_ranked_type(b["type"].as_str().unwrap_or(""));
            let map = it["event"]["map"].as_str().unwrap_or("").to_string();
            let mode = b["mode"].as_str().or(it["event"]["mode"].as_str()).unwrap_or("").to_string();
            if map.is_empty() {
                continue;
            }
            self.rounds += 1;
            if ranked {
                for t in &tags {
                    let n = api::normalize_tag(t);
                    if n != owner && !others.contains(&n) {
                        others.push(n);
                    }
                }
            }
            for (ti, team) in teams.iter().enumerate() {
                let win = (ti == owner_team) == (result == "victory");
                for p in team.as_array().into_iter().flatten() {
                    let id = p["brawler"]["id"].as_i64().unwrap_or(0);
                    if id == 0 {
                        continue;
                    }
                    let c = self.cells.entry((map.clone(), tier.to_string(), ranked, id)).or_insert_with(|| Cell {
                        map: map.clone(),
                        tier: tier.to_string(),
                        mode: mode.clone(),
                        ranked,
                        event_id: it["event"]["id"].as_i64().unwrap_or(0),
                        id,
                        brawler: p["brawler"]["name"].as_str().unwrap_or("?").to_string(),
                        ..Default::default()
                    });
                    c.games += 1;
                    c.wins += win as u32;
                }
            }
        }
        others
    }

    pub fn finish(self, tier_players: HashMap<String, u32>) -> Meta {
        Meta {
            fetched_at: unix_now(),
            players: tier_players.values().sum(),
            rounds: self.rounds,
            tier_players,
            cells: self.cells.into_values().collect(),
        }
    }
}

/* ------------------------------- queries --------------------------------- */

fn pool<'a>(m: &'a Meta, ranked_only: bool, tiers: &'a HashSet<String>) -> impl Iterator<Item = &'a Cell> {
    m.cells.iter().filter(move |c| (!ranked_only || c.ranked) && tiers.contains(&c.tier))
}

/// Maps in the sample, most-played first.
pub fn maps(m: &Meta, ranked_only: bool, tiers: &HashSet<String>) -> Vec<MapInfo> {
    let mut by: HashMap<String, MapInfo> = HashMap::new();
    for c in pool(m, ranked_only, tiers) {
        let e = by.entry(c.map.clone()).or_insert_with(|| MapInfo { map: c.map.clone(), event_id: c.event_id, mode: c.mode.clone(), games: 0 });
        e.games += c.games; // six brawlers per round, so this counts player-rounds
    }
    let mut v: Vec<MapInfo> = by.into_values().map(|mut x| { x.games /= 6; x }).collect();
    v.sort_by(|a, b| b.games.cmp(&a.games).then(a.map.cmp(&b.map)));
    v
}

/// All brawlers in the sample (for the ban picker).
pub fn brawlers(m: &Meta) -> Vec<(i64, String)> {
    let mut seen: HashMap<i64, String> = HashMap::new();
    for c in &m.cells {
        seen.entry(c.id).or_insert_with(|| c.brawler.clone());
    }
    let mut v: Vec<(i64, String)> = seen.into_iter().collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// Best picks for `map`, skipping banned brawlers. `owned` (id -> power) restricts to brawlers
/// the player has.
pub fn recommend(
    m: &Meta,
    map: &str,
    ranked_only: bool,
    tiers: &HashSet<String>,
    banned: &HashSet<i64>,
    owned: Option<&HashMap<i64, i64>>,
) -> Vec<Rec> {
    let mut agg: HashMap<i64, (String, u32, u32)> = HashMap::new();
    for c in pool(m, ranked_only, tiers).filter(|c| c.map == map) {
        let e = agg.entry(c.id).or_insert_with(|| (c.brawler.clone(), 0, 0));
        e.1 += c.wins;
        e.2 += c.games;
    }
    let mut out: Vec<Rec> = agg
        .into_iter()
        .filter(|(id, (_, _, g))| *g >= MIN_GAMES && !banned.contains(id) && owned.map_or(true, |o| o.contains_key(id)))
        .map(|(id, (name, w, g))| {
            let score = (w as f64 + PRIOR) / (g as f64 + 2.0 * PRIOR);
            let power = owned.and_then(|o| o.get(&id).copied());
            // Power 11 keeps the full score; a power-6 brawler loses about 9%.
            let factor = power.map_or(1.0, |p| 0.8 + 0.2 * (p.clamp(1, 11) as f64 / 11.0));
            Rec {
            id,
            pick: score * factor,
            brawler: name,
            games: g,
            win_rate: w as f64 / g as f64,
            score,
            power,
            }
        })
        .collect();
    out.sort_by(|a, b| b.pick.partial_cmp(&a.pick).unwrap_or(std::cmp::Ordering::Equal).then(b.games.cmp(&a.games)));
    out
}

/* --------------------------------- I/O ----------------------------------- */

pub fn load(app: &AppHandle) -> Meta {
    let _g = store::lock();
    store::load(app, FILE)
}

/// Finds Diamond-and-above players and reads their battle logs.
///
/// The API can't list players by ranked tier, so it starts from you plus the top trophy players
/// and follows the opponents/teammates seen in their ranked games. Each player's tier comes from
/// their profile; anyone below Diamond is skipped. `target` is how many players to sample and
/// `parallel` how many are looked up at the same time. Emits `meta-progress` as `[done, target]`.
pub async fn refresh(app: &AppHandle, api_key: &str, my_tag: &str, target: u32, parallel: u32) -> Result<Meta, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("A refresh is already running.".into());
    }
    let result = refresh_inner(app, api_key, my_tag, target.clamp(20, 400), parallel.clamp(1, 12) as usize).await;
    RUNNING.store(false, Ordering::SeqCst);
    result
}

/// GET with one retry after a rate-limit pause.
async fn retry<F, Fut>(mut f: F) -> Result<Value, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    let r = f().await;
    if matches!(&r, Err(e) if e.contains("429")) {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        return f().await;
    }
    r
}

/// Profile, then battle log, for one player. `Ok(None)` = unranked or below Diamond.
async fn look_up(api_key: &str, tag: &str) -> Result<Option<(String, Vec<Value>)>, String> {
    let profile = retry(|| api::fetch_player(api_key, tag)).await?;
    let tier = profile["rankedRankName"].as_str().and_then(tier_of);
    let Some(tier) = tier.filter(|t| tier_index(t).map_or(false, |i| i >= MIN_TIER)) else {
        return Ok(None);
    };
    let log = retry(|| api::fetch_battlelog(api_key, tag)).await?;
    Ok(Some((tier, log["items"].as_array().cloned().unwrap_or_default())))
}

async fn refresh_inner(app: &AppHandle, api_key: &str, my_tag: &str, target: u32, parallel: usize) -> Result<Meta, String> {
    let top = api::get(api_key, "/rankings/global/players?limit=40").await?;
    let mut queue: VecDeque<String> = VecDeque::new();
    let me = api::normalize_tag(my_tag);
    if !me.is_empty() {
        queue.push_back(me); // closest to Diamond/Mythic, so its opponents spread downward
    }
    for p in top["items"].as_array().into_iter().flatten() {
        if let Some(t) = p["tag"].as_str() {
            queue.push_back(api::normalize_tag(t));
        }
    }
    if queue.is_empty() {
        return Err("Couldn't find any players to start from.".into());
    }

    let mut col = Collector::default();
    let mut visited: HashSet<String> = HashSet::new();
    let mut tier_players: HashMap<String, u32> = HashMap::new();
    let (mut done, mut looked, mut hard_fails) = (0u32, 0u32, 0u32);
    let max_lookups = target * 3;

    while done < target && looked < max_lookups {
        let mut batch: Vec<String> = Vec::new();
        while batch.len() < parallel {
            let Some(tag) = queue.pop_front() else { break };
            if visited.insert(tag.clone()) {
                batch.push(tag);
            }
        }
        if batch.is_empty() {
            break;
        }
        looked += batch.len() as u32;

        let results = futures::future::join_all(batch.iter().map(|t| look_up(api_key, t))).await;
        for (tag, res) in batch.iter().zip(results) {
            match res {
                Ok(Some((tier, items))) => {
                    hard_fails = 0;
                    for t in col.add_log(tag, &tier, &items) {
                        if !visited.contains(&t) {
                            queue.push_back(t);
                        }
                    }
                    *tier_players.entry(tier).or_insert(0) += 1;
                    done += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    if e.contains("403") || e.contains("429") {
                        hard_fails += 1;
                        if hard_fails >= 8 {
                            return Err(format!("Stopped after repeated errors: {e}"));
                        }
                    }
                }
            }
        }
        let _ = app.emit("meta-progress", [done.min(target), target]);
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    if done == 0 {
        return Err("No Diamond-or-higher players were found. Check your API key and try again.".into());
    }
    let meta = col.finish(tier_players);
    {
        let _g = store::lock();
        store::save(app, FILE, &meta)?;
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn player(tag: &str, id: i64, name: &str) -> Value {
        json!({"tag": tag, "name": tag, "brawler": {"id": id, "name": name, "power": 11}})
    }

    fn round(time: &str, result: &str, ty: &str) -> Value {
        json!({"battleTime": time, "event": {"id": 1, "mode": "gemGrab", "map": "Gem Fort"},
               "battle": {"mode": "gemGrab", "type": ty, "result": result, "teams": [
                 [player("#A", 1, "SHELLY"), player("#B", 2, "COLT"), player("#C", 3, "MORTIS")],
                 [player("#D", 4, "LEON"), player("#E", 5, "CROW"), player("#F", 6, "TARA")]]}})
    }

    fn cell<'a>(m: &'a Meta, id: i64) -> &'a Cell {
        m.cells.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn credits_winners_and_losers_and_dedupes_across_logs() {
        let mut c = Collector::default();
        let log = vec![round("20261107T220000.000Z", "victory", "soloRanked")];
        c.add_log("#A", "mythic", &log);
        // The same round seen from an opponent's log (result flipped) must not double count.
        c.add_log("#D", "mythic", &[round("20261107T220000.000Z", "defeat", "soloRanked")]);
        let m = c.finish(HashMap::from([("mythic".to_string(), 2)]));
        assert_eq!(m.rounds, 1);
        assert_eq!((cell(&m, 1).wins, cell(&m, 1).games), (1, 1)); // owner's team won
        assert_eq!((cell(&m, 4).wins, cell(&m, 4).games), (0, 1)); // opponents lost
    }

    #[test]
    fn skips_draws_and_non_3v3() {
        let mut c = Collector::default();
        c.add_log("#A", "mythic", &[round("20261107T220000.000Z", "draw", "soloRanked")]);
        c.add_log("#A", "mythic", &[json!({"battleTime":"x","battle":{"players":[],"rank":1},"event":{"map":"M"}})]);
        assert_eq!(c.finish(HashMap::new()).rounds, 0);
    }

    #[test]
    fn recommends_by_smoothed_win_rate_and_respects_bans_and_ownership() {
        let mk = |id, name: &str, wins, games| Cell { map: "Gem Fort".into(), mode: "gemGrab".into(), ranked: true, tier: "mythic".into(), event_id: 1, id, brawler: name.into(), wins, games };
        let m = Meta { cells: vec![mk(1, "A", 6, 6), mk(2, "B", 140, 200), mk(3, "C", 90, 200), mk(4, "D", 1, 2)], ..Default::default() };
        let r = recommend(&m, "Gem Fort", true, &tiers_for("diamond+"), &HashSet::new(), None);
        // 6/6 smoothed is (6+5)/(6+10)=0.69; 140/200 smoothed is 0.69 too but with far more games.
        assert_eq!(r.iter().map(|x| x.id).collect::<Vec<_>>(), vec![2, 1, 3]); // id 4 has < MIN_GAMES
        let banned: HashSet<i64> = [2].into();
        assert_eq!(recommend(&m, "Gem Fort", true, &tiers_for("diamond+"), &banned, None)[0].id, 1);
        let owned: HashMap<i64, i64> = [(3, 9)].into();
        let r = recommend(&m, "Gem Fort", true, &tiers_for("diamond+"), &HashSet::new(), Some(&owned));
        assert_eq!((r.len(), r[0].id, r[0].power), (1, 3, Some(9)));
        assert!(recommend(&m, "Other Map", true, &tiers_for("diamond+"), &HashSet::new(), None).is_empty());
    }

    #[test]
    fn a_weak_copy_of_a_brawler_ranks_lower() {
        let mk = |id, wins, games| Cell { map: "M".into(), ranked: true, tier: "mythic".into(), id, brawler: id.to_string(), wins, games, ..Default::default() };
        let m = Meta { cells: vec![mk(1, 60, 100), mk(2, 60, 100)], ..Default::default() };
        let owned: HashMap<i64, i64> = [(1, 6), (2, 11)].into();
        let r = recommend(&m, "M", true, &tiers_for("diamond+"), &HashSet::new(), Some(&owned));
        assert_eq!(r[0].id, 2);
        assert!(r[0].pick > r[1].pick);
    }

    #[test]
    fn tiers_parse_and_sources_never_go_below_diamond() {
        assert_eq!(tier_of("MYTHIC III").as_deref(), Some("mythic"));
        assert_eq!(tier_of("LEGENDARY I").as_deref(), Some("legendary"));
        assert_eq!(tier_of(""), None);
        assert_eq!(tier_of("WHATEVER I"), None);
        let all = tiers_for("diamond+");
        assert!(all.contains("diamond") && all.contains("masters") && !all.contains("gold"));
        assert_eq!(tiers_for("garbage"), all);
        assert_eq!(tiers_for("legendary"), HashSet::from(["legendary".to_string()]));
        assert!(!tiers_for("mythic+").contains("diamond"));
    }

    #[test]
    fn source_choice_filters_by_tier() {
        let mk = |tier: &str, wins, games| Cell { map: "M".into(), ranked: true, tier: tier.into(), id: 1, brawler: "A".into(), wins, games, ..Default::default() };
        let m = Meta { cells: vec![mk("diamond", 10, 20), mk("legendary", 5, 20)], ..Default::default() };
        let none = HashSet::new();
        let d = recommend(&m, "M", true, &tiers_for("diamond"), &none, None);
        assert_eq!((d[0].games, d[0].win_rate), (20, 0.5));
        let all = recommend(&m, "M", true, &tiers_for("diamond+"), &none, None);
        assert_eq!(all[0].games, 40);
        assert!(recommend(&m, "M", true, &tiers_for("masters"), &none, None).is_empty());
    }

    #[test]
    fn ranked_only_filters_the_pool() {
        let mut c = Collector::default();
        c.add_log("#A", "mythic", &[round("20261107T220000.000Z", "victory", "ranked")]);
        let m = c.finish(HashMap::new());
        let t = tiers_for("diamond+");
        assert!(maps(&m, true, &t).is_empty());
        assert_eq!(maps(&m, false, &t), vec![MapInfo { map: "Gem Fort".into(), event_id: 1, mode: "gemGrab".into(), games: 1 }]);
    }
}
