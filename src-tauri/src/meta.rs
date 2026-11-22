//! Ranked brawler recommender.
//!
//! The official API has no meta data, no bans and no map pool, so the "meta" is sampled:
//! the battle logs of the top global players are read and every 3v3 round is counted per
//! (map, brawler). Every refresh is kept as its own batch and the batches are added together
//! (battles seen twice are skipped), so the sample grows and the picks settle down instead of
//! reshuffling with each new draw. Bans are entered by hand in the UI.
//!
//! A pick's score is built in layers, each one pulling a thin sample toward something better known:
//! the map result -> the brawler's result across the whole mode -> its class's result in that mode.
//! Then it gets a small nudge for how much top players pick it on that map and for how well its
//! kit suits the mode (`kits::fit`), and is ranked by a cautious estimate (a lower bound), so a
//! lucky 13-0 can't beat a brawler that has proved itself over hundreds of rounds.

use crate::{api, store, tracker::unix_now};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

const FILE: &str = "meta.json";
/// Smoothing for the overall list: pretend every brawler starts with this many wins and losses.
const PRIOR: f64 = 8.0;
/// Smallest number of rounds on a map before a brawler is considered for it.
const MIN_MAP_GAMES: u32 = 4;
/// How many rounds of evidence each layer counts as being worth (see the module notes).
const K_CLASS: f64 = 200.0;
const K_MODE: f64 = 40.0;
const K_MAP: f64 = 50.0;
/// Win-rate points a perfect (+1) kit fit moves the starting guess and the final score.
const KIT_PRIOR: f64 = 0.02;
const KIT_FINAL: f64 = 0.012;
/// Points per doubling-ish of usage (natural log of pick rate vs. average), capped.
const USAGE_PTS: f64 = 0.01;
const USAGE_CAP: f64 = 0.03;
/// Standard deviations subtracted to rank cautiously.
const CAUTION: f64 = 1.0;
/// Newest batches kept, and how old a batch may get before the next refresh drops it.
const MAX_BATCHES: usize = 6;
const MAX_BATCH_AGE: i64 = 28 * 24 * 3600;
/// Rounds needed before a brawler shows up in the overall ranking.
const MIN_OVERALL: u32 = 30;

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
    /// Hashes of the battles this batch counted, so later refreshes don't count them again.
    pub seen: Vec<u64>,
    /// Only on the combined view: how many batches it adds up, the oldest one's time, and how many
    /// rounds the latest refresh added.
    pub batch_count: u32,
    pub oldest: i64,
    pub added: u32,
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
    /// What the list is ordered by: a cautious score, nudged by usage, kit fit and your power level.
    pub pick: f64,
    pub brawler: String,
    pub games: u32,
    /// The raw win rate on this map.
    pub win_rate: f64,
    /// The estimated win rate after pulling thin samples toward better-known ones.
    pub score: f64,
    pub power: Option<i64>,
    /// The brawler's class (None if role data isn't available) and how well it suits the mode, -1..1.
    pub class: Option<String>,
    pub fit: f64,
    /// The brawler's rounds and raw win rate across every map of this mode.
    pub mode_games: u32,
    pub mode_win_rate: Option<f64>,
    /// Share of this map's rounds it appears in, and its rank by that.
    pub usage: f64,
    pub usage_rank: u32,
    /// "high", "medium" or "low", by how many rounds the map figure rests on.
    pub confidence: &'static str,
}

/* ------------------------------ aggregation ------------------------------ */

#[derive(Default)]
pub struct Collector {
    seen: HashSet<u64>,
    /// Hashes added by this run only (what gets stored with the batch).
    fresh: Vec<u64>,
    cells: HashMap<(String, String, bool, i64), Cell>,
    pub rounds: u32,
}

/// Small stable hash (FNV-1a), so stored battle keys stay valid across app versions.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Collector {
    /// Starts with battles that earlier refreshes already counted, which will be skipped.
    pub fn with_seen(seen: HashSet<u64>) -> Self {
        Collector { seen, ..Default::default() }
    }

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
            let h = fnv(&key);
            if !self.seen.insert(h) {
                continue;
            }
            self.fresh.push(h);
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
            seen: self.fresh,
            ..Default::default()
        }
    }
}

/* ------------------------------- batches --------------------------------- */

/// Adds several refreshes together into one view. Players are counted as the most seen in any one
/// batch per tier (the same top players come up each time, so summing would overstate them).
pub fn merge(batches: &[Meta]) -> Meta {
    let mut cells: HashMap<(String, String, bool, i64), Cell> = HashMap::new();
    let mut tier_players: HashMap<String, u32> = HashMap::new();
    let mut ordered: Vec<&Meta> = batches.iter().collect();
    ordered.sort_by_key(|b| b.fetched_at); // oldest first, so the newest names and event ids win
    let mut rounds = 0;
    for b in &ordered {
        rounds += b.rounds;
        for (t, n) in &b.tier_players {
            let e = tier_players.entry(t.clone()).or_insert(0);
            *e = (*e).max(*n);
        }
        for c in &b.cells {
            let e = cells.entry((c.map.clone(), c.tier.clone(), c.ranked, c.id)).or_insert_with(|| Cell { wins: 0, games: 0, ..c.clone() });
            e.wins += c.wins;
            e.games += c.games;
            e.mode = c.mode.clone();
            e.event_id = c.event_id;
            e.brawler = c.brawler.clone();
        }
    }
    Meta {
        fetched_at: ordered.last().map_or(0, |b| b.fetched_at),
        players: tier_players.values().sum(),
        rounds,
        tier_players,
        cells: cells.into_values().collect(),
        batch_count: batches.len() as u32,
        oldest: ordered.first().map_or(0, |b| b.fetched_at),
        ..Default::default()
    }
}

/// Drops batches that are too old, then keeps only the newest few.
pub fn prune(mut batches: Vec<Meta>, now: i64) -> Vec<Meta> {
    batches.retain(|b| now - b.fetched_at <= MAX_BATCH_AGE);
    batches.sort_by_key(|b| b.fetched_at);
    let extra = batches.len().saturating_sub(MAX_BATCHES);
    batches.drain(..extra);
    batches
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
/// the player has. Kit fit is left neutral; see `recommend_with` to include it.
pub fn recommend(
    m: &Meta,
    map: &str,
    ranked_only: bool,
    tiers: &HashSet<String>,
    banned: &HashSet<i64>,
    owned: Option<&HashMap<i64, i64>>,
) -> Vec<Rec> {
    recommend_with(m, map, ranked_only, tiers, banned, owned, &HashMap::new())
}

/// Spread of a win-rate estimate that rests on `n` rounds' worth of evidence.
fn spread(p: f64, n: f64) -> f64 {
    (p * (1.0 - p) / (n + 1.0)).sqrt()
}

/// As `recommend`, with each brawler's class (`classes`: id -> class) for kit fit.
pub fn recommend_with(
    m: &Meta,
    map: &str,
    ranked_only: bool,
    tiers: &HashSet<String>,
    banned: &HashSet<i64>,
    owned: Option<&HashMap<i64, i64>>,
    classes: &HashMap<i64, String>,
) -> Vec<Rec> {
    // This map: brawler -> (name, wins, games).
    let mut here: HashMap<i64, (String, u32, u32)> = HashMap::new();
    let mut mode = String::new();
    for c in pool(m, ranked_only, tiers).filter(|c| c.map == map) {
        let e = here.entry(c.id).or_insert_with(|| (c.brawler.clone(), 0, 0));
        e.1 += c.wins;
        e.2 += c.games;
        if mode.is_empty() {
            mode = c.mode.clone();
        }
    }
    if here.is_empty() {
        return Vec::new();
    }

    // The same mode across all its maps: per brawler, and per class.
    let mut in_mode: HashMap<i64, (u32, u32)> = HashMap::new();
    let mut by_class: HashMap<&str, (u32, u32)> = HashMap::new();
    if !mode.is_empty() {
        for c in pool(m, ranked_only, tiers).filter(|c| c.mode == mode) {
            let e = in_mode.entry(c.id).or_default();
            e.0 += c.wins;
            e.1 += c.games;
            if let Some(cl) = classes.get(&c.id) {
                let e = by_class.entry(cl.as_str()).or_default();
                e.0 += c.wins;
                e.1 += c.games;
            }
        }
    }

    // Usage rank on this map (most rounds first).
    let mut by_use: Vec<(i64, u32)> = here.iter().map(|(id, v)| (*id, v.2)).collect();
    by_use.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let rank_of: HashMap<i64, u32> = by_use.iter().enumerate().map(|(i, (id, _))| (*id, i as u32 + 1)).collect();
    let total: f64 = here.values().map(|v| v.2 as f64).sum();
    let distinct = here.len() as f64;

    let mut out: Vec<Rec> = here
        .into_iter()
        .filter(|(id, (_, _, g))| *g >= MIN_MAP_GAMES && !banned.contains(id) && owned.map_or(true, |o| o.contains_key(id)))
        .map(|(id, (name, w, g))| {
            let class = classes.get(&id).cloned();
            let fit = class.as_deref().map_or(0.0, |c| crate::kits::fit(&mode, c));
            // Layer 1: how this brawler's class does in the mode, nudged by how well it suits it.
            let class_mean = class
                .as_deref()
                .and_then(|c| by_class.get(c))
                .map_or(0.5, |(cw, cg)| (*cw as f64 + K_CLASS * 0.5) / (*cg as f64 + K_CLASS));
            let prior = (class_mean + KIT_PRIOR * fit).clamp(0.35, 0.65);
            // Layer 2: the brawler across the mode, leaving out this map's own rounds.
            let (mw, mg) = in_mode.get(&id).copied().unwrap_or((0, 0));
            let (ow, og) = (mw.saturating_sub(w) as f64, mg.saturating_sub(g) as f64);
            let mode_mean = (ow + K_MODE * prior) / (og + K_MODE);
            // Layer 3: this map.
            let (wf, gf) = (w as f64, g as f64);
            let score = (wf + K_MAP * mode_mean) / (gf + K_MAP);

            let idx = if total > 0.0 { gf / total * distinct } else { 1.0 };
            let usage = (USAGE_PTS * idx.max(1e-6).ln()).clamp(-USAGE_CAP, USAGE_CAP) * (gf / (gf + 20.0));
            let careful = score + usage + KIT_FINAL * fit - CAUTION * spread(score, gf + K_MAP);

            let power = owned.and_then(|o| o.get(&id).copied());
            // Power 11 keeps the full score; a power-6 brawler loses about 9%.
            let factor = power.map_or(1.0, |p| 0.8 + 0.2 * (p.clamp(1, 11) as f64 / 11.0));
            Rec {
                id,
                pick: careful * factor,
                brawler: name,
                games: g,
                win_rate: wf / gf,
                score,
                power,
                class,
                fit,
                mode_games: mg,
                mode_win_rate: (mg > 0).then(|| mw as f64 / mg as f64),
                usage: if total > 0.0 { gf / total } else { 0.0 },
                usage_rank: rank_of.get(&id).copied().unwrap_or(0),
                confidence: if g >= 60 { "high" } else if g >= 20 { "medium" } else { "low" },
            }
        })
        .collect();
    out.sort_by(|a, b| b.pick.partial_cmp(&a.pick).unwrap_or(std::cmp::Ordering::Equal).then(b.games.cmp(&a.games)).then(a.id.cmp(&b.id)));
    out
}

/// One brawler's standing across every map (or every map of one mode).
#[derive(Serialize, Debug)]
pub struct Overall {
    pub id: i64,
    pub brawler: String,
    pub games: u32,
    pub win_rate: f64,
    /// Smoothed win rate; what the list is ordered by.
    pub score: f64,
    /// Share of all player-rounds in the pool, as a fraction. Six brawlers play each round, so
    /// the average brawler sits near 1 / (number of brawlers).
    pub pick_rate: f64,
}

/// Modes present in the sample, most-played first (for the mode filter).
pub fn modes(m: &Meta, ranked_only: bool, tiers: &HashSet<String>) -> Vec<String> {
    let mut by: HashMap<String, u32> = HashMap::new();
    for c in pool(m, ranked_only, tiers) {
        *by.entry(c.mode.clone()).or_default() += c.games;
    }
    let mut v: Vec<(String, u32)> = by.into_iter().filter(|(k, _)| !k.is_empty()).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(k, _)| k).collect()
}

/// Every brawler ranked by smoothed win rate over the whole pool, or just `mode` when given.
///
/// Each round is stored once per side, so the pool's average win rate is 50% by construction and
/// a brawler's figure already reads as "better or worse than the field". Brawlers with fewer than
/// `MIN_OVERALL` rounds are left out; smoothing handles the rest.
pub fn overall(m: &Meta, ranked_only: bool, tiers: &HashSet<String>, mode: Option<&str>) -> Vec<Overall> {
    let mut agg: HashMap<i64, (String, u32, u32)> = HashMap::new();
    let mut total: u64 = 0;
    for c in pool(m, ranked_only, tiers).filter(|c| mode.map_or(true, |md| c.mode == md)) {
        let e = agg.entry(c.id).or_insert_with(|| (c.brawler.clone(), 0, 0));
        e.1 += c.wins;
        e.2 += c.games;
        total += c.games as u64;
    }
    let mut out: Vec<Overall> = agg
        .into_iter()
        .filter(|(_, (_, _, g))| *g >= MIN_OVERALL)
        .map(|(id, (brawler, w, g))| Overall {
            id,
            brawler,
            games: g,
            win_rate: w as f64 / g as f64,
            score: (w as f64 + PRIOR) / (g as f64 + 2.0 * PRIOR),
            pick_rate: if total > 0 { g as f64 / total as f64 } else { 0.0 },
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(b.games.cmp(&a.games)).then(a.id.cmp(&b.id)));
    out
}

/* --------------------------------- I/O ----------------------------------- */

/// What is stored on disk: the batches. Files from before batches existed hold one sample in the
/// top-level fields, which becomes the first batch.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Stored {
    batches: Vec<Meta>,
    #[serde(flatten)]
    legacy: Meta,
}

fn load_batches(app: &AppHandle) -> Vec<Meta> {
    let st: Stored = {
        let _g = store::lock();
        store::load(app, FILE)
    };
    let mut b = st.batches;
    if b.is_empty() && !st.legacy.cells.is_empty() {
        b.push(st.legacy);
    }
    b
}

/// Everything sampled so far, added together.
pub fn load(app: &AppHandle) -> Meta {
    merge(&load_batches(app))
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

    // Earlier refreshes stay (newest few); battles they counted are skipped this time.
    let kept = prune(load_batches(app), unix_now());
    let mut col = Collector::with_seen(kept.iter().flat_map(|b| b.seen.iter().copied()).collect());
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
    let batch = col.finish(tier_players);
    let added = batch.rounds;
    let mut all = kept;
    all.push(batch);
    {
        let _g = store::lock();
        store::save(app, FILE, &Stored { batches: all.clone(), legacy: Meta::default() })?;
    }
    Ok(Meta { added, ..merge(&all) })
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
    fn recommends_cautiously_and_respects_bans_and_ownership() {
        let mk = |id, name: &str, wins, games| Cell { map: "Gem Fort".into(), mode: "gemGrab".into(), ranked: true, tier: "mythic".into(), event_id: 1, id, brawler: name.into(), wins, games };
        let m = Meta { cells: vec![mk(1, "A", 6, 6), mk(2, "B", 140, 200), mk(3, "C", 90, 200), mk(4, "D", 1, 2)], ..Default::default() };
        let r = recommend(&m, "Gem Fort", true, &tiers_for("diamond+"), &HashSet::new(), None);
        // A perfect 6/6 can't beat 140/200: the thin sample is pulled toward 50% and ranked cautiously.
        assert_eq!(r.iter().map(|x| x.id).collect::<Vec<_>>(), vec![2, 1, 3]); // id 4 has < MIN_MAP_GAMES
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

    fn mk_cell(map: &str, mode: &str, id: i64, name: &str, wins: u32, games: u32) -> Cell {
        Cell { map: map.into(), mode: mode.into(), ranked: true, tier: "diamond".into(), id, brawler: name.into(), wins, games, ..Default::default() }
    }

    #[test]
    fn overall_pools_maps_smooths_and_filters_by_mode() {
        let m = Meta {
            cells: vec![
                // SHELLY: 60/100 over two maps, tiny-sample COLT is dropped, MORTIS is 50/100.
                mk_cell("Gem Fort", "gemGrab", 1, "SHELLY", 35, 50),
                mk_cell("Hard Rock Mine", "gemGrab", 1, "SHELLY", 25, 50),
                mk_cell("Gem Fort", "gemGrab", 2, "COLT", 6, 6),
                mk_cell("Gem Fort", "gemGrab", 3, "MORTIS", 50, 100),
                mk_cell("Shooting Star", "bounty", 3, "MORTIS", 10, 40),
            ],
            ..Default::default()
        };
        let tiers: HashSet<String> = ["diamond".to_string()].into_iter().collect();
        let all = overall(&m, true, &tiers, None);
        assert_eq!(all.iter().map(|o| o.brawler.as_str()).collect::<Vec<_>>(), ["SHELLY", "MORTIS"]);
        assert_eq!(all[0].games, 100);
        assert!((all[0].win_rate - 0.6).abs() < 1e-9);
        assert!(all[0].score < all[0].win_rate && all[0].score > 0.5); // pulled toward 50%
        assert!((all[1].win_rate - 60.0 / 140.0).abs() < 1e-9); // MORTIS pooled over both modes
        let total: f64 = 100.0 + 6.0 + 140.0;
        assert!((all[0].pick_rate - 100.0 / total).abs() < 1e-9);

        let bounty = overall(&m, true, &tiers, Some("bounty"));
        assert_eq!(bounty.len(), 1); // MORTIS only, and 40 rounds clears the minimum
        assert_eq!(modes(&m, true, &tiers), ["gemGrab", "bounty"]);
    }

    fn on(map: &str, mode: &str, id: i64, wins: u32, games: u32) -> Cell {
        Cell { map: map.into(), mode: mode.into(), ranked: true, tier: "diamond".into(), id, brawler: format!("B{id}"), wins, games, ..Default::default() }
    }
    fn picks(m: &Meta, map: &str, classes: &HashMap<i64, String>) -> Vec<i64> {
        recommend_with(m, map, true, &tiers_for("diamond+"), &HashSet::new(), None, classes).iter().map(|r| r.id).collect()
    }

    #[test]
    fn a_lucky_thin_sample_cannot_beat_a_proven_brawler() {
        // The case that looked wrong in practice: 13 wins from 13 rounds against 300/500.
        let m = Meta { cells: vec![on("Ring", "hotZone", 1, 13, 13), on("Ring", "hotZone", 2, 300, 500)], ..Default::default() };
        let r = recommend_with(&m, "Ring", true, &tiers_for("diamond+"), &HashSet::new(), None, &HashMap::new());
        assert_eq!(r[0].id, 2);
        assert_eq!(r[1].win_rate, 1.0); // the raw figure is still reported
        assert!(r[1].score < 0.65 && r[1].confidence == "low");
    }

    #[test]
    fn a_brawlers_record_across_the_mode_steadies_its_map_figure() {
        // On this map both have the same 5 rounds (4 wins). Brawler 1 is strong all over Hot Zone,
        // brawler 2 is weak all over it, so 1 should clearly lead.
        let m = Meta {
            cells: vec![
                on("Ring", "hotZone", 1, 4, 5), on("Other", "hotZone", 1, 330, 500),
                on("Ring", "hotZone", 2, 4, 5), on("Other", "hotZone", 2, 200, 500),
            ],
            ..Default::default()
        };
        let r = recommend_with(&m, "Ring", true, &tiers_for("diamond+"), &HashSet::new(), None, &HashMap::new());
        assert_eq!(r[0].id, 1);
        assert!(r[0].score > r[1].score + 0.1);
        assert_eq!(r[0].mode_games, 505);
    }

    #[test]
    fn kit_fit_keeps_an_ill_suited_brawler_from_winning_on_noise() {
        // Same results for a tank and an assassin in Hot Zone; the tank should rank higher.
        let m = Meta { cells: vec![on("Ring", "hotZone", 1, 16, 25), on("Ring", "hotZone", 2, 16, 25)], ..Default::default() };
        let classes: HashMap<i64, String> = [(1, "assassin".to_string()), (2, "tank".to_string())].into();
        let r = recommend_with(&m, "Ring", true, &tiers_for("diamond+"), &HashSet::new(), None, &classes);
        assert_eq!(r.iter().map(|x| x.id).collect::<Vec<_>>(), [2, 1]);
        assert!(r[0].fit > 0.5 && r[1].fit < 0.0 && r[0].class.as_deref() == Some("tank"));
        // With no role data both are neutral and the order falls back to id.
        assert_eq!(picks(&m, "Ring", &HashMap::new()), [1, 2]);
        // The same two in Brawl Ball (where assassins are fine and tanks are great) keep tank first.
        let bb = Meta { cells: vec![on("Arena", "brawlBall", 1, 16, 25), on("Arena", "brawlBall", 2, 16, 25)], ..Default::default() };
        assert_eq!(picks(&bb, "Arena", &classes), [2, 1]);
    }

    #[test]
    fn a_class_that_wins_in_the_data_lifts_its_thin_members() {
        // Controllers win ~60% across Gem Grab in the sample; a controller with 6 map rounds should
        // beat a damage dealer with the same 6 when the damage dealers do poorly.
        let mut cells = vec![on("Fort", "gemGrab", 1, 3, 6), on("Fort", "gemGrab", 2, 3, 6)];
        cells.push(on("Mine", "gemGrab", 10, 600, 1000)); // controller elsewhere
        cells.push(on("Mine", "gemGrab", 11, 400, 1000)); // damage dealer elsewhere
        let classes: HashMap<i64, String> = [(1, "controller".to_string()), (2, "damage".to_string()), (10, "controller".to_string()), (11, "damage".to_string())].into();
        let m = Meta { cells, ..Default::default() };
        assert_eq!(picks(&m, "Fort", &classes), [1, 2]);
    }

    #[test]
    fn usage_edges_a_tie_and_results_are_stable() {
        let m = Meta { cells: vec![on("M", "gemGrab", 1, 30, 50), on("M", "gemGrab", 2, 60, 100), on("M", "gemGrab", 3, 6, 10)], ..Default::default() };
        let r = recommend_with(&m, "M", true, &tiers_for("diamond+"), &HashSet::new(), None, &HashMap::new());
        assert_eq!(r.iter().map(|x| x.usage_rank).collect::<Vec<_>>().len(), 3);
        assert_eq!(r.iter().find(|x| x.id == 2).unwrap().usage_rank, 1);
        assert!((r.iter().map(|x| x.usage).sum::<f64>() - 1.0).abs() < 1e-9);
        // Same data, same answer, every time.
        let again = recommend_with(&m, "M", true, &tiers_for("diamond+"), &HashSet::new(), None, &HashMap::new());
        assert_eq!(r.iter().map(|x| x.id).collect::<Vec<_>>(), again.iter().map(|x| x.id).collect::<Vec<_>>());
    }

    #[test]
    fn refreshes_add_up_without_double_counting() {
        let log = vec![round("20261107T220000.000Z", "victory", "soloRanked"), round("20261107T221000.000Z", "defeat", "soloRanked")];
        let mut first = Collector::default();
        first.add_log("#A", "mythic", &log[..1]);
        let b1 = Meta { fetched_at: 100, ..first.finish(HashMap::from([("mythic".to_string(), 3)])) };
        assert_eq!(b1.seen.len(), 1);

        // Second refresh sees both battles, one of which the first already counted.
        let mut second = Collector::with_seen(b1.seen.iter().copied().collect());
        second.add_log("#A", "mythic", &log);
        let b2 = Meta { fetched_at: 200, ..second.finish(HashMap::from([("mythic".to_string(), 5)])) };
        assert_eq!(b2.rounds, 1);

        let all = merge(&[b1.clone(), b2.clone()]);
        assert_eq!((all.rounds, all.batch_count, all.oldest, all.fetched_at), (2, 2, 100, 200));
        assert_eq!(all.tier_players["mythic"], 5); // most seen in one batch, not 3 + 5
        assert_eq!(cell(&all, 1).games, 2);
        assert_eq!(cell(&all, 1).wins, 1); // won one, lost one
        assert!(all.seen.is_empty());
    }

    #[test]
    fn old_batches_are_dropped_and_only_the_newest_few_kept() {
        let now = 100 * 24 * 3600;
        let b = |at: i64| Meta { fetched_at: at, ..Default::default() };
        let mut v = vec![b(now - 40 * 24 * 3600)]; // too old
        for d in 0..8 {
            v.push(b(now - d * 24 * 3600));
        }
        let kept = prune(v, now);
        assert_eq!(kept.len(), MAX_BATCHES);
        assert!(kept.windows(2).all(|w| w[0].fetched_at < w[1].fetched_at));
        assert_eq!(kept.last().unwrap().fetched_at, now);
    }

    #[test]
    fn a_file_from_before_batches_still_loads_as_one_batch() {
        let old = r#"{"fetched_at": 55, "players": 9, "rounds": 3, "tier_players": {"mythic": 9},
                      "cells": [{"map":"M","mode":"gemGrab","ranked":true,"tier":"mythic","event_id":1,"id":7,"brawler":"X","wins":2,"games":3}]}"#;
        let st: Stored = serde_json::from_str(old).unwrap();
        assert!(st.batches.is_empty() && st.legacy.cells.len() == 1);
        let m = merge(&[st.legacy]);
        assert_eq!((m.rounds, m.fetched_at, m.batch_count), (3, 55, 1));
        // And the new format round-trips.
        let new = Stored { batches: vec![Meta { fetched_at: 9, rounds: 4, ..Default::default() }], legacy: Meta::default() };
        let back: Stored = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!((back.batches.len(), back.batches[0].rounds), (1, 4));
    }
}
