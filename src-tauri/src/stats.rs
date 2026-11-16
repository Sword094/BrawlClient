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
    /// +n = n wins in a row, -n = n losses in a row (newest games).
    pub current_streak: i32,
    pub best_win_streak: u32,
    pub ranked_wins: u32,
    pub ranked_losses: u32,
    /// Games where you were the star player.
    pub star_count: u32,
    /// Unix time of the earliest saved game.
    pub since: Option<i64>,
    /// Average trophy change per non-ranked game that reported one.
    pub avg_trophies: Option<f64>,
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
    let (mut tro_sum, mut tro_n) = (0i64, 0i64);
    let mut run = 0u32;
    // Oldest first, for the best-streak scan; the vector is newest first.
    let done: Vec<&Match> = matches.iter().filter(|m| m.complete || !m.ranked).collect();
    for m in done.iter().rev() {
        if m.result == "victory" {
            run += 1;
            s.best_win_streak = s.best_win_streak.max(run);
        } else {
            run = 0;
        }
    }
    if let Some(first) = done.first() {
        let dir = if first.result == "victory" { 1 } else if first.result == "defeat" { -1 } else { 0 };
        if dir != 0 {
            let n = done.iter().take_while(|m| m.result == first.result).count() as i32;
            s.current_streak = dir * n;
        }
    }
    s.since = done.iter().map(|m| m.start).filter(|t| *t > 0).min();
    for m in done.iter().copied() {
        let win = m.result == "victory";
        let loss = m.result == "defeat";
        s.total += 1;
        s.wins += win as u32;
        s.losses += loss as u32;
        if m.ranked {
            s.ranked += 1;
            s.ranked_wins += win as u32;
            s.ranked_losses += loss as u32;
        }
        if m.teams.iter().flatten().any(|p| p.me && p.star) {
            s.star_count += 1;
        }
        if let Some(c) = m.trophy_change {
            tro_sum += c;
            tro_n += 1;
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
    s.avg_trophies = (tro_n > 0).then(|| tro_sum as f64 / tro_n as f64);
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

/// One `[end time, outcome]` per finished game (1 win, -1 loss, 0 other), for the heat map.
pub fn day_results(matches: &[Match]) -> Vec<[i64; 2]> {
    matches
        .iter()
        .filter(|m| m.complete || !m.ranked)
        .map(|m| [m.end, match m.result.as_str() { "victory" => 1, "defeat" => -1, _ => 0 }])
        .collect()
}

/// One entry per finished game for the graph's win/loss markers: [end time, 1 win / -1 loss / 0 other,
/// 1 if it was ranked]. Oldest first.
pub fn game_marks(matches: &[Match]) -> Vec<[i64; 3]> {
    let mut v: Vec<[i64; 3]> = matches
        .iter()
        .filter(|m| m.complete || !m.ranked)
        .map(|m| [m.end, match m.result.as_str() { "victory" => 1, "defeat" => -1, _ => 0 }, m.ranked as i64])
        .collect();
    v.sort_by_key(|g| g[0]);
    v
}

#[derive(Serialize, Default, Debug)]
pub struct DaySummary {
    pub games: u32,
    pub wins: u32,
    pub losses: u32,
    pub ranked: u32,
    pub stars: u32,
    pub best_streak: u32,
    /// Sum of the API's trophy changes over the day's non-ranked games (None if none reported one).
    pub trophy_net: Option<i64>,
    pub trophy_games: u32,
    /// Sum of elo changes over the day's ranked games that had readings either side.
    pub elo_net: Option<i64>,
    pub elo_games: u32,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub by_brawler: Vec<Row>,
    pub by_mode: Vec<Row>,
}

/// Everything played in `[from, to)` (unix seconds; the window picks the local day's edges).
pub fn day_summary(matches: &[Match], from: i64, to: i64) -> DaySummary {
    let day: Vec<Match> = matches.iter().filter(|m| m.end >= from && m.end < to).cloned().collect();
    let s = summarize(&day);
    let mut d = DaySummary {
        games: s.total, wins: s.wins, losses: s.losses, ranked: s.ranked, stars: s.star_count,
        best_streak: s.best_win_streak, elo_net: s.elo_net,
        by_brawler: s.by_brawler.into_iter().take(4).collect(),
        by_mode: s.by_mode.into_iter().take(4).collect(),
        ..Default::default()
    };
    for m in day.iter().filter(|m| m.complete || !m.ranked) {
        if let Some(c) = m.trophy_change {
            d.trophy_net = Some(d.trophy_net.unwrap_or(0) + c);
            d.trophy_games += 1;
        }
        d.elo_games += m.elo_change.is_some() as u32;
        d.first = Some(d.first.map_or(m.end, |f| f.min(m.end)));
        d.last = Some(d.last.map_or(m.end, |l| l.max(m.end)));
    }
    d
}

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct SeasonRow {
    pub id: i64,
    /// First and last reading seen in this season (unix seconds).
    pub from: i64,
    pub to: i64,
    /// First and last elo reading, and the highest and lowest. The app may have joined mid-season.
    pub start_elo: i64,
    pub end_elo: i64,
    pub peak: i64,
    pub low: i64,
    pub wins: u32,
    pub losses: u32,
    /// Elo won and lost (as a positive number) across this season's ranked games that had a reading
    /// either side, and how many games that covers.
    pub gained: i64,
    pub lost: i64,
    pub elo_games: u32,
}

/// Ranked seasons, newest first, built from the elo readings and the ranked games inside each one.
pub fn seasons(h: &History, matches: &[Match]) -> Vec<SeasonRow> {
    let mut rows: Vec<SeasonRow> = Vec::new();
    for sn in h.snaps.iter().filter(|s| s.elo > 0) {
        let Some(id) = sn.season else { continue };
        match rows.iter_mut().find(|r| r.id == id) {
            Some(r) => {
                r.to = sn.t;
                r.end_elo = sn.elo;
                r.peak = r.peak.max(sn.elo);
                r.low = r.low.min(sn.elo);
            }
            None => rows.push(SeasonRow { id, from: sn.t, to: sn.t, start_elo: sn.elo, end_elo: sn.elo, peak: sn.elo, low: sn.elo, ..Default::default() }),
        }
    }
    for m in matches.iter().filter(|m| m.ranked && m.complete) {
        if let Some(r) = rows.iter_mut().find(|r| m.end >= r.from && m.end <= r.to) {
            r.wins += (m.result == "victory") as u32;
            r.losses += (m.result == "defeat") as u32;
            if let Some(c) = m.elo_change {
                r.gained += c.max(0);
                r.lost += (-c).max(0);
                r.elo_games += 1;
            }
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.from));
    rows
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

    fn game(result: &str, star: bool, tro: Option<i64>, end: i64) -> Match {
        Match {
            result: result.into(), complete: true, end, start: end - 100, trophy_change: tro, my_team: Some(0),
            teams: vec![vec![PlayerRef { me: true, star, ..Default::default() }]],
            ..Default::default()
        }
    }

    #[test]
    fn streaks_stars_and_averages() {
        // newest first: L L W W W L W
        let ms = vec![
            game("defeat", false, Some(-6), 700), game("defeat", true, Some(-8), 600),
            game("victory", false, Some(8), 500), game("victory", true, Some(10), 400),
            game("victory", false, Some(8), 300), game("defeat", false, None, 200), game("victory", false, None, 100),
        ];
        let s = summarize(&ms);
        assert_eq!(s.current_streak, -2);
        assert_eq!(s.best_win_streak, 3);
        assert_eq!(s.star_count, 2);
        assert_eq!(s.since, Some(100)); // the oldest game's start (0) counts as unknown
        assert_eq!(s.avg_trophies, Some(12.0 / 5.0));
        assert_eq!(summarize(&[game("victory", false, None, 5)]).current_streak, 1);
        assert_eq!(summarize(&[]).current_streak, 0);
        assert_eq!(day_results(&ms)[0], [700, -1]);
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

    #[test]
    fn game_marks_are_oldest_first_and_flag_ranked() {
        let mut a = game("victory", false, Some(8), 500);
        a.ranked = true;
        let b = game("defeat", false, Some(-6), 300);
        let mut unfinished = game("victory", false, None, 700);
        unfinished.ranked = true;
        unfinished.complete = false;
        assert_eq!(game_marks(&[unfinished, a, b]), vec![[300, -1, 0], [500, 1, 1]]);
    }

    #[test]
    fn day_summary_only_counts_that_day() {
        let mut ranked = game("victory", false, None, 1_500);
        ranked.ranked = true;
        ranked.elo_change = Some(21);
        let ms = vec![
            game("victory", true, Some(8), 2_100),   // next day
            ranked,                                    // in the day
            game("defeat", false, Some(-6), 1_200),
            game("victory", false, Some(9), 1_100),
            game("victory", false, Some(7), 900),     // previous day
        ];
        let d = day_summary(&ms, 1_000, 2_000);
        assert_eq!((d.games, d.wins, d.losses, d.ranked), (3, 2, 1, 1));
        assert_eq!((d.trophy_net, d.trophy_games), (Some(3), 2));
        assert_eq!((d.elo_net, d.elo_games), (Some(21), 1));
        assert_eq!((d.first, d.last), (Some(1_100), Some(1_500)));
        assert_eq!(day_summary(&ms, 5_000, 6_000).games, 0);
        assert_eq!(day_summary(&ms, 5_000, 6_000).trophy_net, None);
    }

    #[test]
    fn seasons_group_readings_and_ranked_games() {
        let mut h = History::default();
        for (t, elo, season) in [(10, 5000, Some(1)), (20, 5400, Some(1)), (30, 5200, Some(1)), (40, 0, None), (50, 4000, Some(2)), (60, 4300, Some(2))] {
            h.snaps.push(Snap { t, elo, trophies: Some(100), season });
        }
        let mut win = game("victory", false, None, 25);
        win.ranked = true;
        win.elo_change = Some(40);
        let mut loss = game("defeat", false, None, 55);
        loss.ranked = true;
        loss.elo_change = Some(-25);
        let mut loss2 = game("defeat", false, None, 56);
        loss2.ranked = true;
        loss2.elo_change = Some(-15);
        let mut outside = game("victory", false, None, 99);
        outside.ranked = true;
        let rows = seasons(&h, &[win, loss, loss2, outside]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], SeasonRow { id: 2, from: 50, to: 60, start_elo: 4000, end_elo: 4300, peak: 4300, low: 4000, wins: 0, losses: 2, gained: 0, lost: 40, elo_games: 2 });
        assert_eq!((rows[1].peak, rows[1].low, rows[1].end_elo, rows[1].wins), (5400, 5000, 5200, 1));
        assert_eq!((rows[1].gained, rows[1].lost), (40, 0));
    }
}
