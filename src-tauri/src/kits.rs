//! What each brawler is *for*. The official API has no roles, so each brawler's class (Tank,
//! Controller, Marksman...) comes from the community Brawlify API, cached for two weeks. If it can't be
//! reached the recommender simply treats every brawler's kit as neutral.
//!
//! `fit` is a rule of thumb for how well a class suits a mode (for example tanks hold a Hot Zone
//! and controllers suit Gem Grab). It is deliberately a small nudge: the sampled win rates carry the
//! ranking, and this only keeps a brawler that makes no sense for the mode from winning on noise.

use crate::{api, store, tracker::unix_now};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;

const CACHE_FILE: &str = "brawler_classes.json";
const MAX_AGE_SECS: i64 = 14 * 24 * 3600;
const URL: &str = "https://api.brawlify.com/v1/brawlers";

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Cache {
    fetched_at: i64,
    /// brawler id (as text) -> class
    classes: HashMap<String, String>,
}

/// Lower-case, single-word class name ("Damage Dealer" -> "damage"), so spelling differences
/// between sources don't matter.
pub fn canon(name: &str) -> String {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "damage dealer" | "damage" | "damagedealer" => "damage".into(),
        _ => n,
    }
}

/// Accepts `{"list": [...]}` or a bare array; each brawler has `class: {name: "..."}`.
pub fn parse(v: &Value) -> HashMap<i64, String> {
    let list = v.get("list").and_then(Value::as_array).or_else(|| v.as_array());
    let mut out = HashMap::new();
    for b in list.into_iter().flatten() {
        let Some(id) = b["id"].as_i64() else { continue };
        let name = b["class"]["name"].as_str().or_else(|| b["class"].as_str()).or_else(|| b["className"].as_str());
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            out.insert(id, canon(n));
        }
    }
    out
}

async fn fetch() -> Result<HashMap<i64, String>, String> {
    let res = api::client().get(URL).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status()));
    }
    let v: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(parse(&v))
}

/// Brawler id -> class. Cached; never fails (worst case: empty, and every kit counts as neutral).
pub async fn get(app: &AppHandle) -> HashMap<i64, String> {
    let cache: Cache = {
        let _g = store::lock();
        store::load(app, CACHE_FILE)
    };
    let as_ids = |c: &HashMap<String, String>| c.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()))).collect::<HashMap<i64, String>>();
    let now = unix_now();
    if !cache.classes.is_empty() && now - cache.fetched_at < MAX_AGE_SECS {
        return as_ids(&cache.classes);
    }
    match fetch().await {
        Ok(found) if !found.is_empty() => {
            let _g = store::lock();
            let _ = store::save(app, CACHE_FILE, &Cache { fetched_at: now, classes: found.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() });
            found
        }
        _ => as_ids(&cache.classes), // stale is better than nothing
    }
}

/// Order: damage, tank, marksman, artillery, controller, support, assassin.
const CLASSES: [&str; 7] = ["damage", "tank", "marksman", "artillery", "controller", "support", "assassin"];

/// How well a class suits a mode, -1 (poor) to +1 (great); 0 when the mode or class isn't known.
pub fn fit(mode: &str, class: &str) -> f64 {
    let row: [f64; 7] = match mode.to_lowercase().as_str() {
        //                     dmg   tank  mark  art   ctrl  supp  assn
        "gemgrab" => [0.2, -0.1, 0.0, 0.3, 0.7, 0.5, -0.3],
        "brawlball" => [0.2, 0.7, -0.3, -0.5, 0.1, 0.0, 0.5],
        "bounty" => [0.3, -0.5, 0.8, 0.4, 0.0, 0.1, -0.4],
        "heist" => [0.6, 0.2, 0.3, 0.5, -0.1, -0.2, 0.1],
        "hotzone" => [0.2, 0.8, -0.3, 0.2, 0.6, 0.4, -0.2],
        "knockout" => [0.4, -0.3, 0.7, 0.3, 0.0, 0.0, 0.2],
        "wipeout" => [0.4, -0.2, 0.5, 0.1, 0.1, 0.0, 0.3],
        _ => return 0.0,
    };
    CLASSES.iter().position(|c| *c == class).map_or(0.0, |i| row[i])
}

/// A short label for a fit value, for the UI.
pub fn fit_label(f: f64) -> &'static str {
    if f >= 0.5 { "great fit" } else if f >= 0.2 { "good fit" } else if f > -0.2 { "neutral" } else if f > -0.5 { "weak fit" } else { "poor fit" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_classes_from_both_shapes() {
        let a = json!({"list": [{"id": 1, "class": {"id": 3, "name": "Damage Dealer"}}, {"id": 2, "class": {"name": "Tank"}}, {"id": 3}]});
        let m = parse(&a);
        assert_eq!((m.get(&1).map(String::as_str), m.get(&2).map(String::as_str), m.get(&3)), (Some("damage"), Some("tank"), None));
        assert_eq!(parse(&json!([{"id": 9, "class": "Controller"}])).get(&9).map(String::as_str), Some("controller"));
    }

    #[test]
    fn fit_reflects_what_each_mode_wants() {
        assert!(fit("hotZone", "tank") > fit("hotZone", "assassin"));
        assert!(fit("gemGrab", "controller") > fit("gemGrab", "tank"));
        assert!(fit("bounty", "marksman") > 0.5 && fit("bounty", "tank") < -0.3);
        assert!(fit("brawlBall", "artillery") < 0.0);
        assert_eq!(fit("somethingNew", "tank"), 0.0);
        assert_eq!(fit("hotZone", "newclass"), 0.0);
        assert_eq!((fit_label(0.8), fit_label(0.0), fit_label(-0.6)), ("great fit", "neutral", "poor fit"));
    }
}
