//! Turns the raw battle log into matches.
//!
//! What the real battle log shows (checked against a log from a real account):
//! - A ranked "game" is a best-of-3 series: consecutive entries with identical
//!   teams, and `starPlayer` is only set on the final round.
//! - `battleTime` is when a round ended.
//! - There is no elo/trophy-change field, so changes are worked out from the
//!   elo snapshots the app records (see `attach_elo`).

use crate::{api::normalize_tag, store::Snap};
use serde::Serialize;
use serde_json::Value;

/// A snapshot must be this many seconds after a match ends to count, because the
/// player profile can lag a little behind the battle log.
const SETTLE_SECS: i64 = 30;
/// Rounds further apart than this are never part of the same series.
const MAX_GAP_SECS: i64 = 15 * 60;

static NULL: Value = Value::Null;

#[derive(Serialize, Clone, Debug, Default)]
pub struct PlayerRef {
    pub id: i64,
    pub tag: String,
    pub name: String,
    pub brawler: String,
    pub power: i64,
    /// Brawler trophies / tier as reported for this battle (meaning varies by mode).
    pub trophies: i64,
    pub star: bool,
    /// This is the player whose history is being viewed.
    pub me: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Round {
    pub time: i64,
    pub result: String,
    pub duration: i64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Match {
    pub id: String,
    pub start: i64,
    pub end: i64,
    pub mode: String,
    pub map: String,
    /// Event id; the map's image is keyed by it.
    pub event_id: i64,
    pub kind: String,
    pub ranked: bool,
    pub wins: u32,
    pub losses: u32,
    pub result: String,
    /// The final round has been played (it carries the star player).
    pub complete: bool,
    /// A ranked series that has started but not finished.
    pub in_progress: bool,
    pub rounds: Vec<Round>,
    pub teams: Vec<Vec<PlayerRef>>,
    pub my_team: Option<usize>,
    pub star: Option<PlayerRef>,
    pub elo_before: Option<i64>,
    pub elo_after: Option<i64>,
    pub elo_change: Option<i64>,
}

/// "20261107T222129.000Z" -> unix seconds (UTC).
pub fn parse_battle_time(s: &str) -> Option<i64> {
    if s.len() < 15 || s.as_bytes().get(8) != Some(&b'T') {
        return None;
    }
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(4, 6)?, n(6, 8)?);
    let (h, mi, se) = (n(9, 11)?, n(11, 13)?, n(13, 15)?);
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// "gemGrab" -> "Gem Grab"
pub fn pretty_mode(mode: &str) -> String {
    let mut out = String::new();
    for (i, c) in mode.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            if c.is_uppercase() {
                out.push(' ');
            }
            out.push(c);
        }
    }
    out
}

fn st(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn num(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

fn battle(item: &Value) -> &Value {
    item.get("battle").unwrap_or(&NULL)
}

fn star_tag(item: &Value) -> String {
    battle(item)
        .get("starPlayer")
        .and_then(|s| s.get("tag"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn has_star(item: &Value) -> bool {
    !star_tag(item).is_empty()
}

fn teams_of(item: &Value) -> Vec<Vec<&Value>> {
    let b = battle(item);
    if let Some(teams) = b.get("teams").and_then(Value::as_array) {
        teams
            .iter()
            .filter_map(|t| t.as_array().map(|a| a.iter().collect()))
            .collect()
    } else if let Some(players) = b.get("players").and_then(Value::as_array) {
        vec![players.iter().collect()]
    } else {
        vec![]
    }
}

/// Two rounds belong to the same series only if event, type and all six players match.
fn identity(item: &Value) -> (i64, String, Vec<String>) {
    let ev = item.get("event").map(|e| num(e, "id")).unwrap_or(0);
    let mut tags: Vec<String> = teams_of(item)
        .iter()
        .flatten()
        .map(|p| st(p, "tag"))
        .collect();
    tags.sort();
    (ev, st(battle(item), "type"), tags)
}

fn player_ref(p: &Value, star: &str) -> PlayerRef {
    let b = p.get("brawler").unwrap_or(&NULL);
    PlayerRef {
        id: num(b, "id"),
        tag: st(p, "tag"),
        name: st(p, "name"),
        brawler: st(b, "name"),
        power: num(b, "power"),
        trophies: num(b, "trophies"),
        star: !star.is_empty() && st(p, "tag") == star,
        me: false,
    }
}

fn build_match(group: &[(i64, &Value)], my_tag: &str) -> Match {
    let (end, newest) = group[0];
    let oldest_t = group[group.len() - 1].0;
    let b = battle(newest);
    let ev = newest.get("event").unwrap_or(&NULL);

    let kind = st(b, "type");
    let ranked = is_ranked_type(&kind);
    let mode = {
        let m = st(b, "mode");
        if m.is_empty() { st(ev, "mode") } else { m }
    };

    let rounds: Vec<Round> = group
        .iter()
        .rev()
        .map(|(t, it)| {
            let bb = battle(it);
            Round { time: *t, result: st(bb, "result"), duration: num(bb, "duration") }
        })
        .collect();
    let wins = rounds.iter().filter(|r| r.result == "victory").count() as u32;
    let losses = rounds.iter().filter(|r| r.result == "defeat").count() as u32;
    let complete = has_star(newest);

    let star_t = star_tag(newest);
    let teams: Vec<Vec<PlayerRef>> = teams_of(newest)
        .iter()
        .map(|t| t.iter().map(|p| player_ref(p, &star_t)).collect())
        .collect();
    let star = teams
        .iter()
        .flatten()
        .find(|p| p.star)
        .cloned()
        .or_else(|| {
            b.get("starPlayer")
                .filter(|s| !s.is_null())
                .map(|s| player_ref(s, &star_t))
        });

    let me = normalize_tag(my_tag);
    let my_team = teams
        .iter()
        .position(|t| t.iter().any(|p| normalize_tag(&p.tag) == me));

    let mut teams = teams;
    for p in teams.iter_mut().flatten() {
        p.me = normalize_tag(&p.tag) == me;
    }

    let result = if wins > losses {
        "victory".to_string()
    } else if losses > wins {
        "defeat".to_string()
    } else {
        rounds
            .last()
            .map(|r| r.result.clone())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "unknown".into())
    };

    let first_duration = rounds.first().map(|r| r.duration).unwrap_or(0);
    Match {
        id: end.to_string(),
        start: oldest_t - first_duration,
        end,
        mode,
        map: st(ev, "map"),
        event_id: num(ev, "id"),
        kind,
        ranked,
        wins,
        losses,
        result,
        complete,
        in_progress: ranked && !complete && wins < 2 && losses < 2,
        rounds,
        teams,
        my_team,
        star,
        elo_before: None,
        elo_after: None,
        elo_change: None,
    }
}

/// "soloRanked" / "teamRanked" are the elo-based ranked modes. Plain "ranked" is the old
/// trophy ladder, which reports `trophyChange` and is not a best-of-3.
pub fn is_ranked_type(kind: &str) -> bool {
    let k = kind.to_lowercase();
    k.contains("ranked") && k != "ranked"
}

/// Groups battle-log entries into matches, newest first.
pub fn group_matches(items: &[Value], my_tag: &str) -> Vec<Match> {
    let mut rounds: Vec<(i64, &Value)> = items
        .iter()
        .filter_map(|it| Some((parse_battle_time(it.get("battleTime")?.as_str()?)?, it)))
        .collect();
    rounds.sort_by(|a, b| b.0.cmp(&a.0));

    // Walking newest -> oldest: a round with a star player is the *final* round of
    // its series, so it always starts a new (older) group.
    let mut groups: Vec<Vec<(i64, &Value)>> = Vec::new();
    for r in rounds {
        let starts_new = match groups.last() {
            None => true,
            Some(g) => {
                let last = g[g.len() - 1];
                has_star(r.1) || identity(r.1) != identity(last.1) || last.0 - r.0 > MAX_GAP_SECS
            }
        };
        if starts_new {
            groups.push(vec![r]);
        } else if let Some(g) = groups.last_mut() {
            g.push(r);
        }
    }

    groups.iter().map(|g| build_match(g, my_tag)).collect()
}

/// Works out elo gained/lost per finished ranked match from recorded snapshots.
///
/// For each match, "after" is the last reading taken once the profile has caught up
/// (>= SETTLE_SECS after the match, before the next one starts). "Before" is the
/// previous match's "after". If a reading is missing the change stays unknown
/// rather than guessed.
pub fn attach_elo(matches: &mut [Match], snaps: &[Snap]) {
    if snaps.is_empty() {
        return;
    }
    // Oldest -> newest (the slice is newest-first).
    let order: Vec<usize> = (0..matches.len())
        .rev()
        .filter(|&k| matches[k].ranked && matches[k].complete)
        .collect();

    // (elo, season) of the last settled reading for each match.
    let settled: Vec<Option<(i64, Option<i64>)>> = order
        .iter()
        .enumerate()
        .map(|(n, &k)| {
            let lo = matches[k].end + SETTLE_SECS;
            let hi = order.get(n + 1).map(|&k2| matches[k2].start).unwrap_or(i64::MAX);
            snaps.iter().rev().find(|s| s.t >= lo && s.t < hi).map(|s| (s.elo, s.season))
        })
        .collect();

    for (n, &k) in order.iter().enumerate() {
        let before = if n > 0 {
            settled[n - 1]
        } else {
            snaps.iter().rev().find(|s| s.t <= matches[k].start).map(|s| (s.elo, s.season))
        };
        let after = settled[n];
        matches[k].elo_before = before.map(|b| b.0);
        matches[k].elo_after = after.map(|a| a.0);
        if let (Some(b), Some(a)) = (before, after) {
            // A season reset moves everyone's elo; that isn't a match result.
            let same_season = b.1.is_none() || a.1.is_none() || b.1 == a.1;
            if same_season {
                matches[k].elo_change = Some(a.0 - b.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(s: &str) -> i64 {
        parse_battle_time(s).unwrap()
    }

    fn teams(me: &str) -> Value {
        json!([
            [{"tag": me, "name": "Me", "brawler": {"name": "MORTIS", "power": 11}},
             {"tag": "#A", "name": "A", "brawler": {"name": "LEON", "power": 11}},
             {"tag": "#B", "name": "B", "brawler": {"name": "CHESTER", "power": 11}}],
            [{"tag": "#C", "name": "C", "brawler": {"name": "TARA", "power": 11}},
             {"tag": "#D", "name": "D", "brawler": {"name": "DARRYL", "power": 11}},
             {"tag": "#E", "name": "E", "brawler": {"name": "CROW", "power": 11}}]
        ])
    }

    fn item(time: &str, ev: i64, result: &str, dur: i64, star: bool, teams: Value) -> Value {
        json!({
            "battleTime": time,
            "event": {"id": ev, "mode": "gemGrab", "map": "Gem Fort"},
            "battle": {
                "mode": "gemGrab", "type": "soloRanked", "result": result, "duration": dur,
                "starPlayer": if star { json!({"tag": "#C", "name": "C"}) } else { Value::Null },
                "teams": teams
            }
        })
    }

    #[test]
    fn parses_battle_time() {
        // 2026-11-07 22:21:29 UTC
        assert_eq!(t("20261107T222129.000Z"), 1_794_090_089);
        assert_eq!(t("19700101T000000.000Z"), 0);
        assert!(parse_battle_time("garbage").is_none());
    }

    #[test]
    fn only_elo_modes_count_as_ranked() {
        assert!(is_ranked_type("soloRanked"));
        assert!(is_ranked_type("teamRanked"));
        assert!(!is_ranked_type("ranked")); // trophy ladder
        assert!(!is_ranked_type("friendly"));
    }

    #[test]
    fn prettifies_modes() {
        assert_eq!(pretty_mode("gemGrab"), "Gem Grab");
        assert_eq!(pretty_mode("brawlBall"), "Brawl Ball");
        assert_eq!(pretty_mode("heist"), "Heist");
    }

    #[test]
    fn groups_series_and_splits_on_star() {
        // newest first, like the API
        let items = vec![
            item("20261107T222129.000Z", 1, "victory", 130, true, teams("#ME")),
            item("20261107T221901.000Z", 1, "victory", 114, false, teams("#ME")),
            // 3-round series, lost 1-2
            item("20261107T221211.000Z", 2, "defeat", 109, true, teams("#ME")),
            item("20261107T221003.000Z", 2, "victory", 107, false, teams("#ME")),
            item("20261107T220800.000Z", 2, "defeat", 100, false, teams("#ME")),
            // same teams and event again right after: a rematch, split by the star rule
            item("20261107T220500.000Z", 2, "victory", 60, true, teams("#ME")),
        ];
        let m = group_matches(&items, "#me");
        assert_eq!(m.len(), 3);

        assert_eq!((m[0].wins, m[0].losses), (2, 0));
        assert_eq!(m[0].result, "victory");
        assert!(m[0].complete && m[0].ranked && !m[0].in_progress);
        assert_eq!(m[0].my_team, Some(0));
        assert_eq!(m[0].star.as_ref().unwrap().tag, "#C");
        assert_eq!(m[0].rounds.len(), 2);
        // start = oldest round end minus its duration
        assert_eq!(m[0].start, t("20261107T221901.000Z") - 114);

        assert_eq!((m[1].wins, m[1].losses), (1, 2));
        assert_eq!(m[1].result, "defeat");
        assert_eq!(m[1].rounds.len(), 3);

        assert_eq!(m[2].rounds.len(), 1);
    }

    #[test]
    fn flags_unfinished_series() {
        let items = vec![item("20261107T222129.000Z", 1, "victory", 130, false, teams("#ME"))];
        let m = group_matches(&items, "#ME");
        assert!(m[0].in_progress && !m[0].complete);
    }

    #[test]
    fn works_out_elo_changes() {
        let items = vec![
            item("20261107T222129.000Z", 1, "victory", 130, true, teams("#ME")),
            item("20261107T221901.000Z", 1, "victory", 114, false, teams("#ME")),
            item("20261107T221211.000Z", 2, "defeat", 109, true, teams("#ME")),
            item("20261107T221003.000Z", 2, "defeat", 107, false, teams("#ME")),
        ];
        let mut m = group_matches(&items, "#ME");
        let snaps = vec![
            Snap { t: t("20261107T220800.000Z"), elo: 5700, ..Default::default() },
            Snap { t: t("20261107T221400.000Z"), elo: 5680, ..Default::default() },
            Snap { t: t("20261107T222300.000Z"), elo: 5705, ..Default::default() },
        ];
        attach_elo(&mut m, &snaps);
        // newest first: [gem grab win, bounty loss]
        assert_eq!(m[1].elo_change, Some(-20));
        assert_eq!(m[0].elo_before, Some(5680));
        assert_eq!(m[0].elo_after, Some(5705));
        assert_eq!(m[0].elo_change, Some(25));
    }

    #[test]
    fn does_not_treat_a_season_reset_as_a_match_result() {
        let items = vec![
            item("20261107T222129.000Z", 1, "victory", 130, true, teams("#ME")),
            item("20261107T221901.000Z", 1, "victory", 114, false, teams("#ME")),
        ];
        let mut m = group_matches(&items, "#ME");
        let snaps = vec![
            Snap { t: t("20261107T221000.000Z"), elo: 7000, season: Some(50), ..Default::default() },
            Snap { t: t("20261107T222300.000Z"), elo: 1200, season: Some(51), ..Default::default() },
        ];
        attach_elo(&mut m, &snaps);
        assert_eq!(m[0].elo_after, Some(1200));
        assert_eq!(m[0].elo_change, None);
    }

    #[test]
    fn leaves_elo_unknown_without_snapshots() {
        let items = vec![item("20261107T222129.000Z", 1, "victory", 130, true, teams("#ME"))];
        let mut m = group_matches(&items, "#ME");
        attach_elo(&mut m, &[]);
        assert_eq!(m[0].elo_change, None);
    }

    /// Runs against a real battle log when BATTLELOG_FIXTURE points at one.
    #[test]
    fn real_log() {
        let Ok(path) = std::env::var("BATTLELOG_FIXTURE") else { return };
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let items = v["items"].as_array().unwrap();
        let m = group_matches(items, "#8QGVRY9UP");
        for x in &m {
            println!("{} {} {}-{} rounds={} complete={}", x.mode, x.result, x.wins, x.losses, x.rounds.len(), x.complete);
        }
        assert_eq!(m.iter().map(|x| x.rounds.len()).sum::<usize>(), items.len());
        if let Ok(out) = std::env::var("BATTLELOG_OUT") {
            std::fs::write(out, serde_json::to_string(&m).unwrap()).unwrap();
        }
    }
}
