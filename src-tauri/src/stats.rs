//! Numbers for the Stats page: win rates and the elo/trophy graphs. Built from stored history.

use crate::history::History;
use crate::matches::Match;
use serde::Serialize;
use std::collections::HashMap;

const MAX_POINTS: usize = 500;

#[derive(Serialize, Default, Clone, Debug)]
pub struct Row {
    pub name: String,
    pub id: i64,
    pub wins: u32,
    pub losses: u32,
}

#[derive(Serialize, Default, Debug)]
pub struct Summary {
    pub total: u32,
    pub wins: u32,
    pub losses: u32,
    pub ranked: u32,
    pub elo_net: Option<i64>,
    pub by_mode: Vec<Row>,
    pub by_map: Vec<Row>,
    pub by_brawler: Vec<Row>,
}

#[derive(Serialize, Default, Debug)]
pub struct Series {
    /// [unix seconds, value]
    pub elo: Vec<[i64; 2]>,
    pub trophies: Vec<[i64; 2]>,
}

fn bump(map: &mut HashMap<String, Row>, key: String, id: i64, win: bool, loss: bool) {
    if key.is_empty() {
        return;
    }
    let r = map.entry(key.clone()).or_insert_with(|| Row { name: key, id, ..Default::default() });
    r.wins += win as u32;
    r.losses += loss as u32;
}

fn sorted(map: HashMap<String, Row>) -> Vec<Row> {
    let mut v: Vec<Row> = map.into_values().collect();
    v.sort_by(|a, b| (b.wins + b.losses).cmp(&(a.wins + a.losses)).then(a.name.cmp(&b.name)));
    v
}

pub fn summarize(matches: &[Match]) -> Summary {
    let mut s = Summary::default();
    let (mut modes, mut maps, mut brawlers) = (HashMap::new(), HashMap::new(), HashMap::new());
    for m in matches.iter().filter(|m| m.complete || !m.ranked) {
        let win = m.result == "victory";
        let loss = m.result == "defeat";
        s.total += 1;
        s.wins += win as u32;
        s.losses += loss as u32;
        if m.ranked {
            s.ranked += 1;
        }
        if let Some(c) = m.elo_change {
            s.elo_net = Some(s.elo_net.unwrap_or(0) + c);
        }
        bump(&mut modes, crate::matches::pretty_mode(&m.mode), 0, win, loss);
        bump(&mut maps, m.map.clone(), 0, win, loss);
        if let Some(me) = m.my_team.and_then(|t| m.teams.get(t)).and_then(|t| t.iter().find(|p| p.me)) {
            bump(&mut brawlers, me.brawler.clone(), me.id, win, loss);
        }
    }
    s.by_mode = sorted(modes);
    s.by_map = sorted(maps);
    s.by_brawler = sorted(brawlers);
    s
}

fn thin(mut pts: Vec<[i64; 2]>) -> Vec<[i64; 2]> {
    if pts.len() <= MAX_POINTS {
        return pts;
    }
    let step = pts.len() as f64 / MAX_POINTS as f64;
    let last = pts.last().copied();
    let mut out: Vec<[i64; 2]> = (0..MAX_POINTS).map(|i| pts[(i as f64 * step) as usize]).collect();
    if let Some(l) = last {
        if out.last() != Some(&l) {
            out.push(l);
        }
    }
    pts.clear();
    out
}

pub fn series(h: &History) -> Series {
    let mut elo = Vec::new();
    let mut tro = Vec::new();
    let mut last_season = None;
    for s in &h.snaps {
        if s.elo > 0 {
            // Break the line at a season reset so it doesn't look like a crash.
            if last_season.is_some() && s.season != last_season {
                elo.push([s.t, 0]); // frontend treats 0 as a gap
            }
            last_season = s.season;
            elo.push([s.t, s.elo]);
        }
        if let Some(t) = s.trophies.filter(|t| *t > 0) {
            tro.push([s.t, t]);
        }
    }
    Series { elo: thin(elo), trophies: thin(tro) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::PlayerRef;
    use crate::store::Snap;

    fn mt(result: &str, brawler: &str, elo: Option<i64>) -> Match {
        Match {
            result: result.into(), complete: true, ranked: true, mode: "gemGrab".into(), map: "Gem Fort".into(),
            elo_change: elo, my_team: Some(0),
            teams: vec![vec![PlayerRef { brawler: brawler.into(), id: 7, me: true, ..Default::default() }]],
            ..Default::default()
        }
    }

    #[test]
    fn counts_wins_per_mode_map_and_brawler() {
        let s = summarize(&[mt("victory", "MORTIS", Some(20)), mt("defeat", "MORTIS", Some(-15)), mt("victory", "LEON", None)]);
        assert_eq!((s.total, s.wins, s.losses, s.elo_net), (3, 2, 1, Some(5)));
        assert_eq!(s.by_brawler[0].name, "MORTIS");
        assert_eq!((s.by_brawler[0].wins, s.by_brawler[0].losses), (1, 1));
        assert_eq!(s.by_mode[0].name, "Gem Grab");
    }

    #[test]
    fn series_skips_missing_values_and_marks_season_resets() {
        let mut h = History::default();
        h.snaps = vec![
            Snap { t: 1, elo: 5000, trophies: Some(100), season: Some(1) },
            Snap { t: 2, elo: 0, trophies: Some(100), season: None },
            Snap { t: 3, elo: 1200, trophies: None, season: Some(2) },
        ];
        let s = series(&h);
        assert_eq!(s.elo, vec![[1, 5000], [3, 0], [3, 1200]]);
        assert_eq!(s.trophies.len(), 2);
    }

    #[test]
    fn thins_long_series() {
        let pts: Vec<[i64; 2]> = (0..5000).map(|i| [i, i]).collect();
        let t = thin(pts);
        assert!(t.len() <= MAX_POINTS + 1);
        assert_eq!(t.last(), Some(&[4999, 4999]));
    }
}
