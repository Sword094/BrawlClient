//! Goals: "reach 7,000 elo" / "reach 52,000 trophies", with progress, pace and an estimated date.
//!
//! Progress is worked out from the elo and trophy readings the tracker already stores, so no extra
//! API calls are needed. Pace comes from the last two weeks of the current elo season; a season
//! reset (a gap in the series) starts the pace window over.

use crate::{api, history, stats, store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tauri::AppHandle;

const FILE: &str = "goals.json";
const DAY: i64 = 86_400;
/// How far back the pace estimate looks.
const WINDOW: i64 = 14 * DAY;
/// Readings closer together than this say nothing reliable about pace.
const MIN_SPAN: i64 = 6 * 3600;
const MAX_GOALS: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Goal {
    pub id: i64,
    /// "elo" or "trophies".
    pub metric: String,
    pub target: i64,
    /// Value when the goal was set, so progress starts at 0%.
    pub start: i64,
    pub created: i64,
    /// Unix seconds, optional.
    pub deadline: Option<i64>,
    pub reached_at: Option<i64>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Progress {
    pub current: Option<i64>,
    /// 0.0 to 1.0.
    pub pct: f64,
    pub remaining: i64,
    /// Average gain per day over the pace window.
    pub per_day: Option<f64>,
    /// Estimated unix time the target is reached at the current pace.
    pub eta: Option<i64>,
    /// Gain per day needed to make the deadline.
    pub need_per_day: Option<f64>,
    /// reached | no_data | on_track | behind | stalled | missed
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct View {
    #[serde(flatten)]
    pub goal: Goal,
    pub progress: Progress,
}

type Points = [[i64; 2]];

/// The latest real reading (0 marks a season gap).
pub fn current(pts: &Points) -> Option<i64> {
    pts.iter().rev().find(|p| p[1] > 0).map(|p| p[1])
}

fn per_day(pts: &Points, now: i64) -> Option<f64> {
    // Only readings after the last season gap, inside the window.
    let from = pts.iter().rposition(|p| p[1] <= 0).map_or(0, |i| i + 1);
    let win: Vec<&[i64; 2]> = pts[from..].iter().filter(|p| p[0] >= now - WINDOW).collect();
    let (first, last) = (win.first()?, win.last()?);
    let span = last[0] - first[0];
    if span < MIN_SPAN {
        return None;
    }
    Some((last[1] - first[1]) as f64 / span as f64 * DAY as f64)
}

pub fn progress(g: &Goal, pts: &Points, now: i64) -> Progress {
    let Some(cur) = current(pts) else {
        return Progress { current: None, pct: 0.0, remaining: (g.target - g.start).max(0), per_day: None, eta: None, need_per_day: None, status: "no_data" };
    };
    let remaining = (g.target - cur).max(0);
    let pct = if g.target <= g.start {
        if cur >= g.target { 1.0 } else { 0.0 }
    } else {
        ((cur - g.start) as f64 / (g.target - g.start) as f64).clamp(0.0, 1.0)
    };
    if cur >= g.target || g.reached_at.is_some() {
        return Progress { current: Some(cur), pct: 1.0, remaining: 0, per_day: None, eta: None, need_per_day: None, status: "reached" };
    }
    let rate = per_day(pts, now);
    let eta = rate.filter(|r| *r > 0.0).map(|r| now + (remaining as f64 / r * DAY as f64).round() as i64);
    let need = g.deadline.filter(|d| *d > now).map(|d| remaining as f64 / ((d - now) as f64 / DAY as f64));
    let status = match (g.deadline, rate) {
        (Some(d), _) if d <= now => "missed",
        (Some(_), Some(r)) => if r >= need.unwrap_or(0.0) { "on_track" } else { "behind" },
        (Some(_), None) => "no_data",
        (None, Some(r)) if r > 0.0 => "on_track",
        (None, Some(_)) => "stalled",
        (None, None) => "no_data",
    };
    Progress { current: Some(cur), pct, remaining, per_day: rate, eta, need_per_day: need, status }
}

fn points<'a>(s: &'a stats::Series, metric: &str) -> &'a Points {
    if metric == "elo" { &s.elo } else { &s.trophies }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

type Book = HashMap<String, Vec<Goal>>;

/// Read-only: goals with their current progress.
pub fn list(app: &AppHandle, tag: &str) -> Vec<View> {
    let key = api::normalize_tag(tag);
    let series = stats::series(&history::load_history(app, &key));
    let now = unix_now();
    let _g = store::lock();
    let book: Book = store::load(app, FILE);
    book.get(&key)
        .map(|goals| goals.iter().map(|g| View { goal: g.clone(), progress: progress(g, points(&series, &g.metric), now) }).collect())
        .unwrap_or_default()
}

/// Records the time of goals that have just been reached (so they stay reached after a season
/// reset) and returns those goals, so the caller can announce each one exactly once.
pub fn mark_reached(app: &AppHandle, tag: &str) -> Vec<Goal> {
    let key = api::normalize_tag(tag);
    let series = stats::series(&history::load_history(app, &key));
    let now = unix_now();
    let _g = store::lock();
    let mut book: Book = store::load(app, FILE);
    let Some(goals) = book.get_mut(&key) else { return Vec::new() };
    let mut done = Vec::new();
    for g in goals.iter_mut().filter(|g| g.reached_at.is_none()) {
        if progress(g, points(&series, &g.metric), now).status == "reached" {
            g.reached_at = Some(now);
            done.push(g.clone());
        }
    }
    if !done.is_empty() {
        let _ = store::save(app, FILE, &book);
    }
    done
}

pub fn add(app: &AppHandle, tag: &str, metric: &str, target: i64, deadline: Option<i64>) -> Result<(), String> {
    let key = api::normalize_tag(tag);
    if metric != "elo" && metric != "trophies" {
        return Err("Pick elo or trophies.".into());
    }
    if target <= 0 {
        return Err("Enter a target above 0.".into());
    }
    let now = unix_now();
    if deadline.map_or(false, |d| d <= now) {
        return Err("The deadline has to be in the future.".into());
    }
    let series = stats::series(&history::load_history(app, &key));
    let start = current(points(&series, metric)).ok_or("No readings yet for this player. Keep the app running while they play, then try again.")?;
    if target <= start {
        return Err(format!("Already at {start}, so that target is already reached. Pick a higher one."));
    }
    let _g = store::lock();
    let mut book: Book = store::load(app, FILE);
    let goals = book.entry(key).or_default();
    if goals.len() >= MAX_GOALS {
        return Err(format!("You can keep up to {MAX_GOALS} goals. Remove one first."));
    }
    let id = goals.iter().map(|g| g.id).max().unwrap_or(0) + 1;
    goals.push(Goal { id, metric: metric.into(), target, start, created: now, deadline, reached_at: None });
    store::save(app, FILE, &book)
}

pub fn remove(app: &AppHandle, tag: &str, id: i64) -> Result<(), String> {
    let key = api::normalize_tag(tag);
    let _g = store::lock();
    let mut book: Book = store::load(app, FILE);
    if let Some(v) = book.get_mut(&key) {
        v.retain(|g| g.id != id);
    }
    store::save(app, FILE, &book)
}

pub fn view_json(app: &AppHandle, tag: &str) -> Value {
    json!({ "goals": list(app, tag), "now": unix_now() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 100 * DAY;

    fn goal(start: i64, target: i64, deadline: Option<i64>) -> Goal {
        Goal { id: 1, metric: "elo".into(), start, target, created: 0, deadline, reached_at: None }
    }

    /// One reading per day for `n` days, ending at NOW, gaining `step` a day from `from`.
    fn climb(from: i64, step: i64, n: i64) -> Vec<[i64; 2]> {
        (0..n).map(|i| [NOW - (n - 1 - i) * DAY, from + step * i]).collect()
    }

    #[test]
    fn pace_progress_and_eta() {
        let pts = climb(5000, 50, 11); // 5000 -> 5500, +50/day
        let p = progress(&goal(5000, 6000, None), &pts, NOW);
        assert_eq!(p.current, Some(5500));
        assert!((p.pct - 0.5).abs() < 1e-9);
        assert_eq!(p.remaining, 500);
        assert!((p.per_day.unwrap() - 50.0).abs() < 1e-6);
        assert_eq!(p.eta, Some(NOW + 10 * DAY));
        assert_eq!(p.status, "on_track");
    }

    #[test]
    fn deadline_decides_on_track_or_behind() {
        let pts = climb(5000, 50, 11);
        let tight = progress(&goal(5000, 6000, Some(NOW + 5 * DAY)), &pts, NOW); // needs 100/day
        assert_eq!(tight.status, "behind");
        assert!((tight.need_per_day.unwrap() - 100.0).abs() < 1e-6);
        let easy = progress(&goal(5000, 6000, Some(NOW + 20 * DAY)), &pts, NOW); // needs 25/day
        assert_eq!(easy.status, "on_track");
        let late = progress(&goal(5000, 6000, Some(NOW - DAY)), &pts, NOW);
        assert_eq!(late.status, "missed");
    }

    #[test]
    fn reached_stalled_and_no_data() {
        let pts = climb(5000, 50, 11);
        assert_eq!(progress(&goal(5000, 5500, None), &pts, NOW).status, "reached");
        assert_eq!(progress(&goal(5000, 5400, None), &pts, NOW).pct, 1.0);
        let flat = climb(5500, 0, 11);
        let p = progress(&goal(5000, 6000, None), &flat, NOW);
        assert_eq!((p.status, p.eta), ("stalled", None));
        assert_eq!(progress(&goal(5000, 6000, None), &[], NOW).status, "no_data");
        // Two readings minutes apart say nothing about pace.
        let burst = [[NOW - 600, 5400], [NOW, 5410]];
        assert_eq!(progress(&goal(5000, 6000, None), &burst, NOW).per_day, None);
    }

    #[test]
    fn season_reset_restarts_the_pace_window() {
        // Climbing before the reset, then a gap (0), then a slow climb from 4000.
        let mut pts = climb(5000, 100, 6);
        pts.push([NOW - 4 * DAY, 0]);
        pts.extend([[NOW - 3 * DAY, 4000], [NOW, 4090]]);
        let p = progress(&goal(4000, 5000, None), &pts, NOW);
        assert!((p.per_day.unwrap() - 30.0).abs() < 1e-6); // 90 over 3 days, not the old +100/day
        assert_eq!(p.current, Some(4090));
    }

    #[test]
    fn old_readings_outside_the_window_are_ignored() {
        let mut pts = vec![[NOW - 60 * DAY, 3000]];
        pts.extend(climb(5000, 20, 6));
        let p = progress(&goal(5000, 6000, None), &pts, NOW);
        assert!((p.per_day.unwrap() - 20.0).abs() < 1e-6);
    }
}
