//! "Insights": patterns in the games this app has saved: who you win with, which brawlers beat you,
//! which brawler suits each map, and when you play best. Everything is derived on demand.

use crate::matches::Match;
use serde::Serialize;
use std::collections::HashMap;

/// Smallest sample worth listing, so one lucky game doesn't look like a trend.
const MIN_MATE: u32 = 3;
const MIN_ENEMY: u32 = 4;
const MIN_PAIR: u32 = 3;
const MIN_MAP: u32 = 5;
const MIN_BEST: u32 = 3;

#[derive(Serialize, Default, Clone, Debug, PartialEq)]
pub struct Rec {
    pub name: String,
    /// Brawler id (for the picture) when the row is a brawler; 0 otherwise.
    pub id: i64,
    pub wins: u32,
    pub losses: u32,
}

impl Rec {
    fn games(&self) -> u32 {
        self.wins + self.losses
    }
    fn rate(&self) -> f64 {
        if self.games() == 0 { 0.0 } else { self.wins as f64 / self.games() as f64 }
    }
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct Pair {
    pub mine: String,
    pub mine_id: i64,
    pub theirs: String,
    pub theirs_id: i64,
    pub wins: u32,
    pub losses: u32,
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct MapRow {
    pub map: String,
    pub wins: u32,
    pub losses: u32,
    /// Your best brawler there (at least a few games), if any.
    pub best: Option<Rec>,
}

#[derive(Serialize, Default, Debug)]
pub struct Insights {
    pub games: u32,
    /// Teammates you've played with at least a few times, most games first.
    pub mates: Vec<Rec>,
    /// Enemy brawlers and your record against them, hardest first.
    pub enemies: Vec<Rec>,
    /// Your brawler vs their brawler, worst first.
    pub pairs: Vec<Pair>,
    pub maps: Vec<MapRow>,
    /// [wins, losses] for each local hour 0-23 and each weekday (0 = Sunday).
    pub hours: Vec<[u32; 2]>,
    pub weekdays: Vec<[u32; 2]>,
}

fn add(r: &mut Rec, win: bool, loss: bool) {
    r.wins += win as u32;
    r.losses += loss as u32;
}

/// `tz_min`: minutes east of UTC, so hours and weekdays are the player's local ones.
pub fn build(matches: &[Match], tz_min: i32) -> Insights {
    let mut out = Insights { hours: vec![[0; 2]; 24], weekdays: vec![[0; 2]; 7], ..Default::default() };
    let mut mates: HashMap<String, Rec> = HashMap::new();
    let mut enemies: HashMap<String, Rec> = HashMap::new();
    let mut pairs: HashMap<(String, String), Pair> = HashMap::new();
    let mut maps: HashMap<String, (MapRow, HashMap<String, Rec>)> = HashMap::new();

    for m in matches.iter().filter(|m| m.complete || !m.ranked) {
        let (win, loss) = (m.result == "victory", m.result == "defeat");
        out.games += 1;
        if win || loss {
            let local = m.end + tz_min as i64 * 60;
            let hour = (local.rem_euclid(86_400) / 3_600) as usize;
            // 1970-01-01 was a Thursday (4 when Sunday = 0).
            let wd = ((local.div_euclid(86_400) + 4).rem_euclid(7)) as usize;
            for slot in [&mut out.hours[hour], &mut out.weekdays[wd]] {
                slot[loss as usize] += 1;
            }
        }
        let Some(mt) = m.my_team else { continue };
        let Some(me) = m.teams.get(mt).and_then(|t| t.iter().find(|p| p.me)) else { continue };

        for p in m.teams[mt].iter().filter(|p| !p.me && !p.tag.is_empty()) {
            let r = mates.entry(p.tag.clone()).or_insert_with(|| Rec { name: p.name.clone(), ..Default::default() });
            r.name = p.name.clone(); // latest name
            add(r, win, loss);
        }
        for (i, team) in m.teams.iter().enumerate().filter(|(i, _)| *i != mt) {
            let _ = i;
            for p in team.iter().filter(|p| !p.brawler.is_empty()) {
                let r = enemies.entry(p.brawler.clone()).or_insert_with(|| Rec { name: p.brawler.clone(), id: p.id, ..Default::default() });
                add(r, win, loss);
                if !me.brawler.is_empty() {
                    let k = (me.brawler.clone(), p.brawler.clone());
                    let e = pairs.entry(k).or_insert_with(|| Pair { mine: me.brawler.clone(), mine_id: me.id, theirs: p.brawler.clone(), theirs_id: p.id, ..Default::default() });
                    e.wins += win as u32;
                    e.losses += loss as u32;
                }
            }
        }
        if !m.map.is_empty() {
            let e = maps.entry(m.map.clone()).or_insert_with(|| (MapRow { map: m.map.clone(), ..Default::default() }, HashMap::new()));
            e.0.wins += win as u32;
            e.0.losses += loss as u32;
            if !me.brawler.is_empty() {
                add(e.1.entry(me.brawler.clone()).or_insert_with(|| Rec { name: me.brawler.clone(), id: me.id, ..Default::default() }), win, loss);
            }
        }
    }

    let by_games = |a: &Rec, b: &Rec| b.games().cmp(&a.games()).then(a.name.cmp(&b.name));
    out.mates = mates.into_values().filter(|r| r.games() >= MIN_MATE).collect();
    out.mates.sort_by(by_games);
    out.mates.truncate(12);

    out.enemies = enemies.into_values().filter(|r| r.games() >= MIN_ENEMY).collect();
    out.enemies.sort_by(|a, b| a.rate().partial_cmp(&b.rate()).unwrap().then(b.games().cmp(&a.games())).then(a.name.cmp(&b.name)));

    out.pairs = pairs.into_values().filter(|p| p.wins + p.losses >= MIN_PAIR).collect();
    out.pairs.sort_by(|a, b| {
        let (ra, rb) = (a.wins as f64 / (a.wins + a.losses) as f64, b.wins as f64 / (b.wins + b.losses) as f64);
        ra.partial_cmp(&rb).unwrap().then((b.wins + b.losses).cmp(&(a.wins + a.losses))).then(a.mine.cmp(&b.mine)).then(a.theirs.cmp(&b.theirs))
    });
    out.pairs.truncate(60);

    out.maps = maps
        .into_values()
        .filter(|(row, _)| row.wins + row.losses >= MIN_MAP)
        .map(|(mut row, by)| {
            row.best = by.into_values().filter(|r| r.games() >= MIN_BEST).max_by(|a, b| {
                a.rate().partial_cmp(&b.rate()).unwrap().then(a.games().cmp(&b.games())).then(b.name.cmp(&a.name))
            });
            row
        })
        .collect();
    out.maps.sort_by(|a, b| (b.wins + b.losses).cmp(&(a.wins + a.losses)).then(a.map.cmp(&b.map)));
    out.maps.truncate(30);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::PlayerRef;

    fn p(tag: &str, name: &str, brawler: &str, id: i64, me: bool) -> PlayerRef {
        PlayerRef { tag: tag.into(), name: name.into(), brawler: brawler.into(), id, me, ..Default::default() }
    }
    fn game(result: &str, mate: &str, mine: &str, enemy: &str, map: &str, end: i64) -> Match {
        Match {
            result: result.into(), complete: true, end, map: map.into(), my_team: Some(0),
            teams: vec![
                vec![p("#ME", "Me", mine, 1, true), p(mate, "Mate", "SHELLY", 2, false)],
                vec![p("#E1", "E", enemy, 3, false)],
            ],
            ..Default::default()
        }
    }

    #[test]
    fn teammates_enemies_pairs_and_maps() {
        let mut ms = Vec::new();
        for i in 0..4 { ms.push(game("victory", "#A", "MORTIS", "CROW", "Gem Fort", 1000 + i)); }
        ms.push(game("defeat", "#A", "MORTIS", "CROW", "Gem Fort", 1010));
        ms.push(game("defeat", "#B", "LEON", "CROW", "Gem Fort", 1020)); // #B only once
        let ins = build(&ms, 0);
        assert_eq!(ins.games, 6);
        assert_eq!(ins.mates.len(), 1); // #B is below the minimum
        assert_eq!((ins.mates[0].name.as_str(), ins.mates[0].wins, ins.mates[0].losses), ("Mate", 4, 1));
        assert_eq!((ins.enemies[0].name.as_str(), ins.enemies[0].wins, ins.enemies[0].losses), ("CROW", 4, 2));
        assert_eq!(ins.pairs.len(), 1); // LEON vs CROW has one game
        assert_eq!((ins.pairs[0].mine.as_str(), ins.pairs[0].wins, ins.pairs[0].losses), ("MORTIS", 4, 1));
        assert_eq!(ins.maps.len(), 1);
        assert_eq!(ins.maps[0].best.as_ref().map(|b| b.name.as_str()), Some("MORTIS"));
    }

    #[test]
    fn hours_and_weekdays_use_the_local_clock() {
        // 1970-01-01 03:30 UTC was a Thursday. At UTC-5 that is Wednesday 22:30.
        let ms = vec![game("victory", "#A", "X", "Y", "M", 3 * 3600 + 1800)];
        let utc = build(&ms, 0);
        assert_eq!(utc.hours[3], [1, 0]);
        assert_eq!(utc.weekdays[4], [1, 0]);
        let ny = build(&ms, -300);
        assert_eq!(ny.hours[22], [1, 0]);
        assert_eq!(ny.weekdays[3], [1, 0]);
    }

    #[test]
    fn unfinished_series_and_draws_are_handled() {
        let mut open = game("defeat", "#A", "X", "Y", "M", 100);
        open.ranked = true;
        open.complete = false;
        let draw = game("draw", "#A", "X", "Y", "M", 200);
        let ins = build(&[open, draw], 0);
        assert_eq!(ins.games, 1);
        assert_eq!(ins.hours.iter().map(|h| h[0] + h[1]).sum::<u32>(), 0); // a draw is neither
    }
}
