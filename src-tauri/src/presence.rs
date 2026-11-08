use crate::history::{self, Session};
use crate::matches::{pretty_mode, Match};
use crate::tracker::unix_now;
use crate::{api, settings, settings::Settings};
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const UPDATE_EVERY: Duration = Duration::from_secs(30);
const RETRY_EVERY: Duration = Duration::from_secs(5);

fn fmt(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    let out: String = out.chars().rev().collect();
    if n < 0 { format!("-{out}") } else { out }
}

fn int(p: &Value, key: &str) -> Option<i64> {
    p.get(key).and_then(Value::as_i64)
}

fn text<'a>(p: &'a Value, key: &str) -> &'a str {
    p.get(key).and_then(Value::as_str).unwrap_or("?")
}

/// Highest win streak across all brawlers. Looks at numeric fields whose name
/// contains "streak" plus max/highest/best, since the exact name isn't documented.
pub fn best_streak(p: &Value) -> Option<i64> {
    let mut best: Option<i64> = None;
    for b in p.get("brawlers")?.as_array()? {
        let Some(obj) = b.as_object() else { continue };
        for (key, val) in obj {
            let k = key.to_lowercase();
            if k.contains("streak") && (k.contains("max") || k.contains("highest") || k.contains("best")) {
                if let Some(n) = val.as_i64() {
                    best = Some(best.map_or(n, |x| x.max(n)));
                }
            }
        }
    }
    best
}

/// The (details, state) text pairs the presence rotates through.
/// `show` = (trophies, ranked, session). The session page only exists while a session is live.
pub fn pages(
    p: &Value,
    live: Option<&(Session, Match)>,
    show: (bool, bool, bool),
) -> Vec<(String, String)> {
    let mut out = Vec::new();

    if show.2 {
        if let Some((s, last)) = live {
            let elo = s.elo_net.map_or(String::new(), |n| format!(" · {n:+} elo"));
            out.push((
                format!(
                    "{} {} · {}",
                    if last.ranked { "Ranked" } else { "Playing" },
                    pretty_mode(&last.mode),
                    last.map
                ),
                format!("Session: {}W-{}L{}", s.wins, s.losses, elo),
            ));
        }
    }
    if show.0 {
        let streak = best_streak(p).map_or("?".to_string(), |n| n.to_string());
        out.push((
            format!(
                "Trophies: {} (Peak {})",
                fmt(int(p, "trophies").unwrap_or(0)),
                fmt(int(p, "highestTrophies").unwrap_or(0))
            ),
            format!("Highest win streak: {streak}"),
        ));
    }
    if show.1 {
        out.push((
            format!(
                "Ranked: {} - {} elo",
                text(p, "rankedRankName"),
                fmt(int(p, "rankedElo").unwrap_or(0))
            ),
            format!(
                "Peak: {} - {} elo",
                text(p, "highestAllTimeRankedRankName"),
                fmt(int(p, "highestAllTimeRankedElo").unwrap_or(0))
            ),
        ));
    }

    if out.is_empty() {
        // Everything is switched off (or no session is live): fall back to the basics.
        return pages(p, None, (true, false, false));
    }
    out
}

fn log(app: &AppHandle, msg: impl Into<String>) {
    let _ = app.emit("presence-log", msg.into());
}

fn connect(client_id: &str) -> Result<DiscordIpcClient, String> {
    let mut client = DiscordIpcClient::new(client_id).map_err(|e| e.to_string())?;
    client.connect().map_err(|e| e.to_string())?;
    Ok(client)
}

/// Runs until the task is aborted by `stop_presence`. Dropping the client closes
/// the IPC connection, which makes Discord clear the activity.
pub async fn run(app: AppHandle, s: Settings) {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let me = api::normalize_tag(&s.player_tag);

    let mut client: Option<DiscordIpcClient> = None;
    let mut page = 0usize;
    let mut warned_discord = false;

    loop {
        if client.is_none() {
            match connect(s.discord_client_id.trim()) {
                Ok(c) => {
                    log(&app, "Connected to Discord.");
                    warned_discord = false;
                    client = Some(c);
                }
                Err(e) => {
                    if !warned_discord {
                        log(&app, format!("Can't reach Discord ({e}). Is the desktop app open? Retrying..."));
                        warned_discord = true;
                    }
                    tokio::time::sleep(RETRY_EVERY).await;
                    continue;
                }
            }
        }

        match api::fetch_player(&s.api_key, &s.player_tag).await {
            Ok(p) => {
                // Pick up page toggles live, and the session from the tracker's saved history.
                let cur = settings::load(&app);
                let live = if cur.presence_session {
                    let all = history::load_history(&app, &me).matches(&me);
                    history::current_session(&all, unix_now())
                } else {
                    None
                };
                let all = pages(&p, live.as_ref(), (cur.presence_trophies, cur.presence_ranked, cur.presence_session));
                let (details, state) = &all[page % all.len()];
                page += 1;

                let act = activity::Activity::new()
                    .details(details)
                    .state(state)
                    .timestamps(activity::Timestamps::new().start(started));

                if let Some(c) = client.as_mut() {
                    match c.set_activity(act) {
                        Ok(()) => log(&app, format!("{details} | {state}")),
                        Err(e) => {
                            log(&app, format!("Lost connection to Discord ({e}). Reconnecting..."));
                            client = None;
                            continue;
                        }
                    }
                }
            }
            Err(e) => log(&app, format!("Update failed: {e}")),
        }

        tokio::time::sleep(UPDATE_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile() -> Value {
        json!({
            "trophies": 51594, "highestTrophies": 51594,
            "rankedRankName": "MYTHIC III", "rankedElo": 5737,
            "highestAllTimeRankedRankName": "LEGENDARY I", "highestAllTimeRankedElo": 7021,
            "brawlers": [{"maxWinStreak": 7}, {"maxWinStreak": 12}, {"name": "x"}]
        })
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(fmt(51594), "51,594");
        assert_eq!(fmt(999), "999");
        assert_eq!(fmt(1_000_000), "1,000,000");
    }

    #[test]
    fn finds_best_streak_and_builds_pages() {
        let p = profile();
        assert_eq!(best_streak(&p), Some(12));
        let pg = pages(&p, None, (true, true, true));
        assert_eq!(pg.len(), 2);
        assert_eq!(pg[0].0, "Trophies: 51,594 (Peak 51,594)");
        assert_eq!(pg[1].0, "Ranked: MYTHIC III - 5,737 elo");
        assert_eq!(best_streak(&json!({})), None);
    }

    #[test]
    fn adds_a_session_page_only_while_playing_and_enabled() {
        let p = profile();
        let live = (
            Session { wins: 3, losses: 1, elo_net: Some(41), ..Default::default() },
            Match { ranked: true, mode: "gemGrab".into(), map: "Gem Fort".into(), ..Default::default() },
        );
        let pg = pages(&p, Some(&live), (true, true, true));
        assert_eq!(pg.len(), 3);
        assert_eq!(pg[0].0, "Ranked Gem Grab · Gem Fort");
        assert_eq!(pg[0].1, "Session: 3W-1L · +41 elo");

        // Session page switched off, or nobody is playing: it disappears.
        assert_eq!(pages(&p, Some(&live), (true, true, false)).len(), 2);
        assert_eq!(pages(&p, None, (true, true, true)).len(), 2);
        // Everything off still shows something.
        assert_eq!(pages(&p, None, (false, false, false)).len(), 1);
    }
}
