//! Brawler icons. The official API has no images, so ids are mapped to image URLs using the
//! community Brawlify API. Everything here is best-effort: if it can't be reached the app simply
//! shows names without icons.

use crate::{api, store, tracker::unix_now};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;

const CACHE_FILE: &str = "brawler_icons.json";
const MAX_AGE_SECS: i64 = 7 * 24 * 3600;
const URL: &str = "https://api.brawlify.com/v1/brawlers";

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Cache {
    fetched_at: i64,
    /// brawler id (as text) -> https image URL
    icons: HashMap<String, String>,
}

/// Accepts either `{"list": [...]}` or a bare array of brawler objects.
fn parse(v: &Value) -> HashMap<String, String> {
    let list = v.get("list").and_then(Value::as_array).or_else(|| v.as_array());
    let mut out = HashMap::new();
    for b in list.into_iter().flatten() {
        let Some(id) = b["id"].as_i64() else { continue };
        let url = ["imageUrl", "imageUrl2", "imageUrl3"]
            .iter()
            .find_map(|k| b[*k].as_str())
            .filter(|u| u.starts_with("https://"));
        if let Some(u) = url {
            out.insert(id.to_string(), u.to_string());
        }
    }
    out
}

async fn fetch() -> Result<HashMap<String, String>, String> {
    let res = api::client().get(URL).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status()));
    }
    let v: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(parse(&v))
}

/// Returns the cached map if fresh, otherwise refreshes it. Never fails: worst case is an empty map.
pub async fn get(app: &AppHandle) -> HashMap<String, String> {
    let cache: Cache = {
        let _g = store::lock();
        store::load(app, CACHE_FILE)
    };
    let now = unix_now();
    if !cache.icons.is_empty() && now - cache.fetched_at < MAX_AGE_SECS {
        return cache.icons;
    }
    match fetch().await {
        Ok(icons) if !icons.is_empty() => {
            let _g = store::lock();
            let _ = store::save(app, CACHE_FILE, &Cache { fetched_at: now, icons: icons.clone() });
            icons
        }
        _ => cache.icons, // stale is better than nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_list_and_array_shapes() {
        let a = json!({"list": [{"id": 16000000, "name": "SHELLY", "imageUrl": "https://x/y.png"}]});
        assert_eq!(parse(&a).get("16000000").map(String::as_str), Some("https://x/y.png"));

        let b = json!([{"id": 1, "imageUrl2": "https://x/2.png"}, {"id": 2, "imageUrl": "http://insecure/3.png"}, {"name": "no id"}]);
        let m = parse(&b);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("1"));
    }
}
