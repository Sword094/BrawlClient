//! Discord Rich Presence: builds the "views" that rotate on your Discord status, fills in
//! {placeholders} for custom text, and keeps the IPC connection to the Discord desktop app alive.

use crate::history::{self, History, Session};
use crate::matches::{pretty_mode, Match, PlayerRef};
use crate::tracker::unix_now;
use crate::{api, icons, settings, settings::Settings};
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const UPDATE_EVERY: Duration = Duration::from_secs(30);
const RETRY_EVERY: Duration = Duration::from_secs(5);
const RECENT_GAMES: usize = 20;
const STREAK_MIN: u32 = 3;

/* ------------------------------ small helpers ----------------------------- */

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

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
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

/// Discord rejects text shorter than 2 or longer than 128 characters.
fn discord_text(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut out: String = t.chars().take(128).collect();
    if out.chars().count() < 2 {
        out.push('\u{00A0}');
    }
    Some(out)
}

/* --------------------------------- match info -------------------------------- */

fn finished(m: &Match) -> bool {
    m.complete || !m.ranked
}

pub fn latest(matches: &[Match]) -> Option<&Match> {
    matches.iter().find(|m| finished(m))
}

fn my_ref(m: &Match) -> Option<&PlayerRef> {
    m.teams.iter().flatten().find(|p| p.me)
}

fn result_text(m: &Match) -> String {
    if let Some(p) = m.placement {
        return format!("#{p}");
    }
    match m.result.as_str() {
        "victory" => "Victory".into(),
        "defeat" => "Defeat".into(),
        "draw" => "Draw".into(),
        _ => "Played".into(),
    }
}

fn kind_label(m: &Match) -> String {
    if m.ranked {
        "Ranked".into()
    } else if m.kind == "ranked" {
        "Trophies".into()
    } else if m.kind.is_empty() {
        "Game".into()
    } else {
        title_case(&m.kind)
    }
}

fn change_text(m: &Match) -> String {
    match (m.elo_change, m.trophy_change) {
        (Some(c), _) => format!("{c:+} elo"),
        (None, Some(c)) => format!("{c:+}"),
        _ => "—".into(),
    }
}

/// Current win streak: consecutive victories, newest first.
pub fn win_streak(matches: &[Match]) -> u32 {
    matches.iter().filter(|m| finished(m)).take_while(|m| m.result == "victory").count() as u32
}

fn record(matches: &[Match], limit: usize) -> (u32, u32) {
    let (mut w, mut l) = (0, 0);
    for m in matches.iter().filter(|m| finished(m)).take(limit) {
        match m.result.as_str() {
            "victory" => w += 1,
            "defeat" => l += 1,
            _ => {}
        }
    }
    (w, l)
}

fn win_rate(w: u32, l: u32) -> Option<u32> {
    (w + l > 0).then(|| ((w as f64 / (w + l) as f64) * 100.0).round() as u32)
}

/* ------------------------------- the data set -------------------------------- */

pub struct Ctx {
    pub player: Value,
    /// Newest first.
    pub matches: Vec<Match>,
    pub live: Option<(Session, Match)>,
    /// brawler id (text) -> image URL
    pub icons: HashMap<String, String>,
    pub tag: String,
}

/// Fetches everything one refresh needs. The battle log is optional: if it can't be fetched,
/// saved history is used.
pub async fn context(app: &AppHandle, s: &Settings) -> Result<Ctx, String> {
    let tag = api::normalize_tag(&s.player_tag);
    let player = api::fetch_player(&s.api_key, &tag).await?;
    let mut hist: History = history::load_history(app, &tag);
    if let Ok(log) = api::fetch_battlelog(&s.api_key, &tag).await {
        let items = log.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        hist.add_items(&items);
    }
    let matches = hist.matches(&tag);
    let live = history::current_session(&matches, unix_now());
    let icons = if s.presence_images { icons::get(app).await } else { HashMap::new() };
    Ok(Ctx { player, matches, live, icons, tag })
}

/* --------------------------------- placeholders ------------------------------- */

#[derive(Serialize, Clone, Debug)]
pub struct Placeholder {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
}

pub fn placeholders(ctx: &Ctx) -> Vec<Placeholder> {
    let p = &ctx.player;
    let last = latest(&ctx.matches);
    let me = last.and_then(my_ref);
    let (w, l) = record(&ctx.matches, usize::MAX);
    let ph = |key, label, value: String| Placeholder { key, label, value };
    let num = |k: &str| int(p, k).map_or("?".to_string(), fmt);
    vec![
        ph("trophies", "Current trophies", num("trophies")),
        ph("highest", "Highest trophies ever", num("highestTrophies")),
        ph("rank", "Current ranked rank", text(p, "rankedRankName").to_string()),
        ph("elo", "Current ranked Elo", num("rankedElo")),
        ph("brawler", "Brawler from your last battle", me.map_or("?".into(), |m| title_case(&m.brawler))),
        ph("power", "That brawler's power level", me.map_or("?".into(), |m| m.power.to_string())),
        ph("mode", "Last game mode", last.map_or("?".into(), |m| pretty_mode(&m.mode))),
        ph("map", "Last map", last.map_or("?".into(), |m| m.map.clone())),
        ph("result", "Last result (Victory / Defeat / #3)", last.map_or("?".into(), result_text)),
        ph("change", "Trophies/elo won or lost last game", last.map_or("—".into(), change_text)),
        ph("streak", "Current win streak", win_streak(&ctx.matches).to_string()),
        ph("winrate", "Win rate of saved battles", win_rate(w, l).map_or("?".into(), |r| format!("{r}%"))),
        ph("name", "Your in-game name", text(p, "name").to_string()),
        ph("tag", "Your player tag", format!("#{}", ctx.tag)),
        ph("club", "Your club", p["club"]["name"].as_str().unwrap_or("No club").to_string()),
        ph("level", "Experience level", num("expLevel")),
        ph("wins3v3", "Total 3v3 victories", num("3vs3Victories")),
    ]
}

/// Replaces every known {placeholder}; unknown ones are left as typed.
pub fn render(template: &str, ph: &[Placeholder]) -> String {
    let mut out = template.to_string();
    for p in ph {
        out = out.replace(&format!("{{{}}}", p.key), &p.value);
    }
    out
}

/* ------------------------------------ views ----------------------------------- */

#[derive(Serialize, Clone, Debug, Default)]
pub struct View {
    pub id: &'static str,
    pub details: String,
    pub state: String,
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
}

fn view(id: &'static str, details: String, state: String) -> View {
    View { id, details, state, ..Default::default() }
}

/// All views that should currently be on Discord, in rotation order. A view is included if
/// its toggle is on, or if it is the pinned one. A pinned view hides all the others.
pub fn views(ctx: &Ctx, s: &Settings) -> Vec<View> {
    let p = &ctx.player;
    let ph = placeholders(ctx);
    let last = latest(&ctx.matches);
    let pin = s.presence_pin.as_str();
    let want = |id: &str, on: bool| on || id == pin;
    let mut out: Vec<View> = Vec::new();

    if want("latest", s.presence_latest) {
        if let Some(m) = last {
            let who = my_ref(m).map_or(String::new(), |r| format!(" · {} · Power {}", title_case(&r.brawler), r.power));
            out.push(view(
                "latest",
                format!("Last match: {}{who}", result_text(m)),
                format!("{} · {} · {}", kind_label(m), pretty_mode(&m.mode), m.map),
            ));
        }
    }
    if want("session", s.presence_session) {
        if let Some((sess, m)) = &ctx.live {
            let elo = sess.elo_net.map_or(String::new(), |n| format!(" · {n:+} elo"));
            out.push(view(
                "session",
                format!("{} {} · {}", if m.ranked { "Ranked" } else { "Playing" }, pretty_mode(&m.mode), m.map),
                format!("Session: {}W-{}L{}", sess.wins, sess.losses, elo),
            ));
        }
    }
    if want("trophies", s.presence_trophies) {
        out.push(view(
            "trophies",
            format!(
                "Trophies: {} · Peak: {}",
                fmt(int(p, "trophies").unwrap_or(0)),
                fmt(int(p, "highestTrophies").unwrap_or(0))
            ),
            format!("Highest win streak: {}", best_streak(p).map_or("?".to_string(), |n| n.to_string())),
        ));
    }
    if want("ranked", s.presence_ranked) {
        out.push(view(
            "ranked",
            format!("Ranked: {} - {} elo", text(p, "rankedRankName"), fmt(int(p, "rankedElo").unwrap_or(0))),
            format!(
                "Peak: {} - {} elo",
                text(p, "highestAllTimeRankedRankName"),
                fmt(int(p, "highestAllTimeRankedElo").unwrap_or(0))
            ),
        ));
    }
    let streak = win_streak(&ctx.matches);
    if id_on("streak", pin, s.presence_streak && streak >= STREAK_MIN) && streak > 0 {
        out.push(view(
            "streak",
            format!("{streak}-win streak"),
            best_streak(p).map_or("Keep it going!".to_string(), |b| format!("Best streak: {b}")),
        ));
    }
    if want("victories", s.presence_victories) {
        out.push(view(
            "victories",
            format!("3v3 victories: {}", int(p, "3vs3Victories").map_or("?".into(), fmt)),
            format!(
                "Solo: {} · Duo: {}",
                int(p, "soloVictories").map_or("?".into(), fmt),
                int(p, "duoVictories").map_or("?".into(), fmt)
            ),
        ));
    }
    let (w, l) = record(&ctx.matches, RECENT_GAMES);
    if want("record", s.presence_record) && w + l > 0 {
        out.push(view(
            "record",
            format!("Recent record: {w}W - {l}L"),
            format!("{} win rate (last {})", win_rate(w, l).map_or("?".into(), |r| format!("{r}%")), w + l),
        ));
    }
    if want("club", s.presence_club) {
        if let Some(name) = p["club"]["name"].as_str() {
            out.push(view("club", format!("Club: {name}"), p["club"]["tag"].as_str().unwrap_or("Brawl Stars").to_string()));
        }
    }
    if want("custom", s.presence_custom) {
        out.push(view("custom", render(&s.presence_line1, &ph), render(&s.presence_line2, &ph)));
    }

    // A pin that matches nothing (e.g. the streak ended) falls back to the normal rotation.
    if !pin.is_empty() {
        if let Some(v) = out.iter().find(|v| v.id == pin).cloned() {
            out = vec![v];
        } else {
            return views(ctx, &Settings { presence_pin: String::new(), ..s.clone() });
        }
    }
    if out.is_empty() {
        out.push(view(
            "trophies",
            format!("Trophies: {}", fmt(int(p, "trophies").unwrap_or(0))),
            "Brawl Stars".to_string(),
        ));
    }

    // Pictures: your brawler from the last battle (large) and the map (small badge).
    if s.presence_images {
        let me = last.and_then(my_ref);
        let large = me.and_then(|r| ctx.icons.get(&r.id.to_string()).cloned());
        let small = last
            .filter(|m| m.event_id != 0)
            .map(|m| format!("https://cdn.brawlify.com/maps/regular/{}.png", m.event_id));
        for v in &mut out {
            if large.is_some() {
                v.large_image = large.clone();
                v.large_text = me.map(|r| title_case(&r.brawler));
                if small.is_some() {
                    v.small_image = small.clone();
                    v.small_text = last.map(|m| m.map.clone());
                }
            }
        }
    }
    out
}

fn id_on(id: &str, pin: &str, on: bool) -> bool {
    on || id == pin
}

/* ------------------------------------ loop ------------------------------------ */

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

        // Re-read settings every cycle so presence changes apply within 30 seconds.
        let cur = settings::load(&app);
        let mut live_settings = cur.clone();
        live_settings.api_key = s.api_key.clone();
        live_settings.player_tag = s.player_tag.clone();

        match context(&app, &live_settings).await {
            Ok(ctx) => {
                let all = views(&ctx, &cur);
                let v = &all[page % all.len()];
                page += 1;
                let _ = app.emit("presence-view", v.id);

                let details = discord_text(&v.details);
                let state = discord_text(&v.state);
                let mut act = activity::Activity::new().timestamps(activity::Timestamps::new().start(started));
                if let Some(d) = &details {
                    act = act.details(d);
                }
                if let Some(st) = &state {
                    act = act.state(st);
                }
                let mut assets = activity::Assets::new();
                let has_assets = v.large_image.is_some();
                if let Some(u) = &v.large_image {
                    assets = assets.large_image(u);
                }
                if let Some(t) = &v.large_text {
                    assets = assets.large_text(t);
                }
                if let Some(u) = &v.small_image {
                    assets = assets.small_image(u);
                }
                if let Some(t) = &v.small_text {
                    assets = assets.small_text(t);
                }
                if has_assets {
                    act = act.assets(assets);
                }
                let url = format!("https://brawlify.com/stats/profile/{}", ctx.tag);
                if cur.presence_button {
                    act = act.buttons(vec![activity::Button::new("View Brawl Stars stats", &url)]);
                }

                if let Some(c) = client.as_mut() {
                    match c.set_activity(act) {
                        Ok(()) => log(&app, format!("{} | {}", v.details, v.state)),
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
    use crate::matches::PlayerRef;
    use serde_json::json;

    fn profile() -> Value {
        json!({
            "name": "Tester", "trophies": 51594, "highestTrophies": 51594, "expLevel": 189,
            "rankedRankName": "MYTHIC III", "rankedElo": 5737,
            "highestAllTimeRankedRankName": "LEGENDARY I", "highestAllTimeRankedElo": 7021,
            "3vs3Victories": 8120, "soloVictories": 1300, "duoVictories": 900,
            "club": {"tag": "#CLUB", "name": "Banana Brawlers"},
            "brawlers": [{"maxWinStreak": 7}, {"maxWinStreak": 12}, {"name": "x"}]
        })
    }

    fn game(result: &str, ranked: bool, change: Option<i64>) -> Match {
        Match {
            result: result.into(), complete: true, ranked, mode: "heist".into(), map: "Bridge Too Far".into(),
            kind: if ranked { "soloRanked".into() } else { "ranked".into() }, event_id: 15000100,
            elo_change: if ranked { change } else { None }, trophy_change: if ranked { None } else { change },
            my_team: Some(0),
            teams: vec![vec![PlayerRef { id: 16000044, brawler: "ANGELO".into(), power: 11, me: true, ..Default::default() }]],
            ..Default::default()
        }
    }

    fn ctx(matches: Vec<Match>) -> Ctx {
        let mut icons = HashMap::new();
        icons.insert("16000044".to_string(), "https://example.com/angelo.png".to_string());
        Ctx { player: profile(), matches, live: None, icons, tag: "ABC123".into() }
    }

    fn ids(v: &[View]) -> Vec<&str> {
        v.iter().map(|x| x.id).collect()
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(fmt(51594), "51,594");
        assert_eq!(fmt(999), "999");
        assert_eq!(fmt(1_000_000), "1,000,000");
    }

    #[test]
    fn finds_best_streak() {
        assert_eq!(best_streak(&profile()), Some(12));
        assert_eq!(best_streak(&json!({})), None);
    }

    #[test]
    fn fills_placeholders_and_leaves_unknown_ones() {
        let c = ctx(vec![game("defeat", true, Some(-14))]);
        let ph = placeholders(&c);
        assert_eq!(render("{brawler} main · {trophies} trophies", &ph), "Angelo main · 51,594 trophies");
        assert_eq!(render("{rank} · {winrate} win rate", &ph), "MYTHIC III · 0% win rate");
        assert_eq!(render("{result} {change} on {map} {nope}", &ph), "Defeat -14 elo on Bridge Too Far {nope}");
        assert_eq!(render("{tag} {club} {level} {wins3v3}", &ph), "#ABC123 Banana Brawlers 189 8,120");
    }

    #[test]
    fn default_rotation_has_the_enabled_views() {
        let s = Settings::default();
        let v = views(&ctx(vec![game("victory", true, Some(20))]), &s);
        // latest, trophies, ranked, victories, record. No streak (needs 3), club off, custom off.
        assert_eq!(ids(&v), vec!["latest", "trophies", "ranked", "victories", "record"]);
        assert_eq!(v[0].details, "Last match: Victory · Angelo · Power 11");
        assert_eq!(v[0].state, "Ranked · Heist · Bridge Too Far");
    }

    #[test]
    fn streak_view_needs_three_wins_in_a_row() {
        let s = Settings::default();
        let two = ctx(vec![game("victory", false, None), game("victory", false, None), game("defeat", false, None)]);
        assert!(!ids(&views(&two, &s)).contains(&"streak"));
        let three = ctx(vec![game("victory", false, None), game("victory", false, None), game("victory", false, None)]);
        let v = views(&three, &s);
        let streak = v.iter().find(|x| x.id == "streak").unwrap();
        assert_eq!(streak.details, "3-win streak");
    }

    #[test]
    fn custom_text_and_pin() {
        let c = ctx(vec![game("victory", true, Some(20))]);
        let mut s = Settings::default();
        s.presence_custom = true;
        assert_eq!(ids(&views(&c, &s)).last(), Some(&"custom"));

        // Pinning the custom text shows only it, even with its rotation toggle off.
        s.presence_custom = false;
        s.presence_pin = "custom".into();
        let v = views(&c, &s);
        assert_eq!(ids(&v), vec!["custom"]);
        assert_eq!(v[0].details, "Angelo main · 51,594 trophies");

        // A pin that no longer exists (the streak ended) falls back to the rotation.
        let ended = ctx(vec![game("defeat", true, Some(-20))]);
        s.presence_pin = "streak".into();
        assert!(views(&ended, &s).len() > 1);
    }

    #[test]
    fn pictures_follow_the_last_battle_and_can_be_off() {
        let c = ctx(vec![game("victory", true, Some(20))]);
        let mut s = Settings::default();
        let v = views(&c, &s);
        assert_eq!(v[0].large_image.as_deref(), Some("https://example.com/angelo.png"));
        assert_eq!(v[0].small_image.as_deref(), Some("https://cdn.brawlify.com/maps/regular/15000100.png"));
        assert_eq!(v[0].small_text.as_deref(), Some("Bridge Too Far"));
        s.presence_images = false;
        assert!(views(&c, &s)[0].large_image.is_none());
    }

    #[test]
    fn session_view_appears_only_while_playing() {
        let mut c = ctx(vec![game("victory", true, Some(20))]);
        let s = Settings::default();
        assert!(!ids(&views(&c, &s)).contains(&"session"));
        c.live = Some((Session { wins: 3, losses: 1, elo_net: Some(41), ..Default::default() }, game("victory", true, Some(20))));
        let v = views(&c, &s);
        let sess = v.iter().find(|x| x.id == "session").unwrap();
        assert_eq!(sess.details, "Ranked Heist · Bridge Too Far");
        assert_eq!(sess.state, "Session: 3W-1L · +41 elo");
    }

    #[test]
    fn never_sends_empty_or_overlong_text() {
        assert_eq!(discord_text("   "), None);
        assert_eq!(discord_text("a").unwrap().chars().count(), 2);
        assert_eq!(discord_text(&"x".repeat(300)).unwrap().chars().count(), 128);
    }

    #[test]
    fn everything_off_still_shows_something() {
        let mut s = Settings::default();
        s.presence_latest = false; s.presence_trophies = false; s.presence_ranked = false;
        s.presence_session = false; s.presence_victories = false; s.presence_record = false;
        assert_eq!(ids(&views(&ctx(vec![]), &s)), vec!["trophies"]);
    }
}
