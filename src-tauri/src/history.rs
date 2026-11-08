//! Long-term local history (JSON, one file per player).
//!
//! The Brawl Stars API only keeps a player's last 25 battle-log entries, so every raw entry the
//! tracker sees is saved here, together with elo/trophy readings over time. Matches (best-of-3
//! ranked series count as ONE match), stats and graphs are all derived from this data on demand.

use crate::api::normalize_tag;
use crate::matches::{self, Match};
use crate::store::{self, Snap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use tauri::AppHandle;

/// Matches further apart than this start a new session.
const SESSION_GAP_SECS: i64 = 30 * 60;
/// A session counts as "live" if its last match ended this recently.
pub const LIVE_WINDOW_SECS: i64 = 10 * 60;
const MAX_ITEMS: usize = 3000;
const MAX_SNAPS: usize = 20_000;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct History {
    /// Raw battle-log entries exactly as the API returned them.
    pub items: Vec<Value>,
    /// Elo/trophy readings, oldest first, run-length compressed (see `add_snap`).
    pub snaps: Vec<Snap>,
}

pub fn file_name(tag: &str) -> String {
    let clean: String = normalize_tag(tag).chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("history_{clean}.json")
}

fn item_key(it: &Value) -> String {
    format!(
        "{}|{}|{}",
        it.get("battleTime").and_then(Value::as_str).unwrap_or(""),
        it.pointer("/event/id").and_then(Value::as_i64).unwrap_or(0),
        it.pointer("/battle/type").and_then(Value::as_str).unwrap_or("")
    )
}

impl History {
    /// Adds entries we haven't stored yet. Returns how many were new.
    pub fn add_items(&mut self, new: &[Value]) -> usize {
        let mut seen: HashSet<String> = self.items.iter().map(item_key).collect();
        let mut added = 0;
        for it in new {
            if it.get("battleTime").and_then(Value::as_str).is_some() && seen.insert(item_key(it)) {
                self.items.push(it.clone());
                added += 1;
            }
        }
        if added > 0 {
            self.items.sort_by(|a, b| {
                let ta = a.get("battleTime").and_then(Value::as_str).unwrap_or("");
                let tb = b.get("battleTime").and_then(Value::as_str).unwrap_or("");
                tb.cmp(ta)
            });
            self.items.truncate(MAX_ITEMS);
        }
        added
    }

    /// Records a reading. Long unchanged stretches are compressed to their first and last
    /// reading, which is all `attach_elo` needs ("what was the elo after this match?").
    pub fn add_snap(&mut self, s: Snap) {
        let same = |a: &Snap, b: &Snap| a.elo == b.elo && a.trophies == b.trophies && a.season == b.season;
        let n = self.snaps.len();
        if n >= 2 && same(&self.snaps[n - 1], &s) && same(&self.snaps[n - 2], &s) {
            self.snaps[n - 1].t = s.t;
        } else {
            self.snaps.push(s);
        }
        if self.snaps.len() > MAX_SNAPS {
            let drop = self.snaps.len() - MAX_SNAPS;
            self.snaps.drain(..drop);
        }
    }

    /// All matches for `my_tag`, newest first, with elo changes where known.
    pub fn matches(&self, my_tag: &str) -> Vec<Match> {
        let mut m = matches::group_matches(&self.items, my_tag);
        matches::attach_elo(&mut m, &self.snaps);
        m
    }
}

/// Caller must already hold `store::lock()`.
pub fn load_locked(app: &AppHandle, tag: &str) -> History {
    store::load(app, &file_name(tag))
}

/// Caller must already hold `store::lock()`.
pub fn save_locked(app: &AppHandle, tag: &str, h: &History) {
    let _ = store::save(app, &file_name(tag), h);
}

/// Takes the lock itself; for UI commands and presence.
pub fn load_history(app: &AppHandle, tag: &str) -> History {
    let _g = store::lock();
    load_locked(app, tag)
}

/* -------------------------------- sessions -------------------------------- */

#[derive(Serialize, Clone, Debug, Default)]
pub struct Session {
    pub start: i64,
    pub end: i64,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    /// Sum of known elo changes; None if no ranked match in the session has one yet.
    pub elo_net: Option<i64>,
}

/// Groups matches (newest first) into sessions, newest session first.
pub fn sessions(all: &[Match]) -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    let mut newer_start: Option<i64> = None; // start of the (newer) match just handled
    for m in all.iter().filter(|m| m.complete || !m.ranked) {
        let new_session = match (out.last(), newer_start) {
            (Some(_), Some(ns)) => ns - m.end > SESSION_GAP_SECS,
            _ => true,
        };
        if new_session {
            out.push(Session { start: m.start, end: m.end, ..Default::default() });
        }
        let s = out.last_mut().expect("session exists");
        s.start = m.start;
        s.matches += 1;
        match m.result.as_str() {
            "victory" => s.wins += 1,
            "defeat" => s.losses += 1,
            _ => {}
        }
        if let Some(c) = m.elo_change {
            s.elo_net = Some(s.elo_net.unwrap_or(0) + c);
        }
        newer_start = Some(m.start);
    }
    out
}

/// The session the player is in right now (last match ended within the live window),
/// plus that last match.
pub fn current_session(all: &[Match], now: i64) -> Option<(Session, Match)> {
    let last = all.iter().find(|m| m.complete || !m.ranked)?;
    if now - last.end > LIVE_WINDOW_SECS {
        return None;
    }
    let s = sessions(all).into_iter().next()?;
    Some((s, last.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(start: i64, end: i64, result: &str, elo: Option<i64>) -> Match {
        Match { start, end, result: result.into(), complete: true, ranked: true, elo_change: elo, ..Default::default() }
    }

    #[test]
    fn groups_sessions_by_gap_and_sums_known_elo() {
        // newest first
        let all = vec![
            m(5000, 5200, "victory", Some(25)),
            m(4700, 4900, "defeat", Some(-18)),
            m(4400, 4600, "victory", None),
            m(100, 300, "victory", Some(20)),
        ];
        let s = sessions(&all);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].matches, s[0].wins, s[0].losses, s[0].elo_net), (3, 2, 1, Some(7)));
        assert_eq!(s[0].start, 4400);
        assert_eq!(s[1].elo_net, Some(20));
    }

    #[test]
    fn current_session_requires_a_recent_match() {
        let all = vec![m(5000, 5200, "victory", Some(25))];
        assert!(current_session(&all, 5200 + 60).is_some());
        assert!(current_session(&all, 5200 + LIVE_WINDOW_SECS + 1).is_none());
    }

    #[test]
    fn unfinished_ranked_series_is_not_counted() {
        let mut open = m(6000, 6100, "", None);
        open.complete = false;
        let all = vec![open, m(5000, 5200, "victory", None)];
        let s = current_session(&all, 5300).unwrap();
        assert_eq!(s.0.matches, 1);
    }

    #[test]
    fn dedupes_items_and_compresses_snaps() {
        let it = serde_json::json!({"battleTime":"20261107T222129.000Z","event":{"id":1},"battle":{"type":"soloRanked"}});
        let mut h = History::default();
        assert_eq!(h.add_items(&[it.clone()]), 1);
        assert_eq!(h.add_items(&[it]), 0);
        for t in 0..5 {
            h.add_snap(Snap { t, elo: 5000, ..Default::default() });
        }
        assert_eq!(h.snaps.len(), 2); // first + latest of the unchanged run
        assert_eq!(h.snaps[1].t, 4);
        h.add_snap(Snap { t: 9, elo: 5020, ..Default::default() });
        assert_eq!(h.snaps.len(), 3);
    }
}
