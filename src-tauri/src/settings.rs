use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub api_key: String,
    pub player_tag: String,
    pub discord_client_id: String,
    /// Background tracking of you + favorites (activity log, history, elo changes).
    pub tracking_enabled: bool,
    /// The window's X button hides the app to the system tray instead of quitting.
    pub close_to_tray: bool,
    /// Start with Windows (hidden in the tray).
    pub launch_at_login: bool,
    /// Start Discord Rich Presence automatically when the app opens.
    pub auto_start_presence: bool,
    /// Desktop notifications (favorite started playing, rank changes).
    pub notifications: bool,
    /// Per-event switches (each also needs the master switch above).
    pub notify_session: bool,
    pub notify_rank: bool,
    pub notify_goal: bool,
    /// No notifications between `quiet_start` and `quiet_end` ("HH:MM", local time; may wrap midnight).
    pub quiet_enabled: bool,
    pub quiet_start: String,
    pub quiet_end: String,
    /// Minutes east of UTC, reported by the window so quiet hours follow the clock (and DST).
    pub tz_offset_min: i32,
    /// "Just finished" view for a few minutes after a game, and an "idle" view after a break.
    pub presence_finished: bool,
    pub presence_idle: bool,
    /// Which pages the Rich Presence rotates through.
    pub presence_trophies: bool,
    pub presence_ranked: bool,
    pub presence_session: bool,
    pub presence_latest: bool,
    pub presence_streak: bool,
    pub presence_victories: bool,
    pub presence_record: bool,
    pub presence_club: bool,
    /// Custom text lines; {placeholders} are replaced on every refresh.
    pub presence_line1: String,
    pub presence_line2: String,
    /// Add the custom text to the rotation.
    pub presence_custom: bool,
    /// Show only this view ("latest", "trophies", "custom", ...). Empty = rotate.
    pub presence_pin: String,
    /// Brawler picture (large) and map picture (small badge) on Discord.
    pub presence_images: bool,
    /// "View stats" profile button (other people see it, you don't).
    pub presence_button: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            player_tag: String::new(),
            discord_client_id: String::new(),
            tracking_enabled: true,
            close_to_tray: true,
            launch_at_login: false,
            auto_start_presence: false,
            notifications: true,
            notify_session: true,
            notify_rank: true,
            notify_goal: true,
            quiet_enabled: false,
            quiet_start: "23:00".into(),
            quiet_end: "08:00".into(),
            tz_offset_min: 0,
            presence_finished: true,
            presence_idle: false,
            presence_trophies: true,
            presence_ranked: true,
            presence_session: true,
            presence_latest: true,
            presence_streak: true,
            presence_victories: true,
            presence_record: true,
            presence_club: false,
            presence_line1: "{brawler} main · {trophies} trophies".into(),
            presence_line2: "{rank} · {winrate} win rate".into(),
            presence_custom: false,
            presence_pin: String::new(),
            presence_images: true,
            presence_button: true,
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// "07:30" -> minutes after midnight.
pub fn parse_hm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Is `now_utc` (unix seconds) inside the quiet window? A window like 23:00-08:00 wraps midnight;
/// equal start and end means no window.
pub fn in_quiet_hours(s: &Settings, now_utc: i64) -> bool {
    if !s.quiet_enabled {
        return false;
    }
    let (Some(a), Some(b)) = (parse_hm(&s.quiet_start), parse_hm(&s.quiet_end)) else { return false };
    let local = (now_utc + s.tz_offset_min as i64 * 60).rem_euclid(86_400) / 60;
    match a.cmp(&b) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => (a..b).contains(&local),
        std::cmp::Ordering::Greater => local >= a || local < b,
    }
}

/// Should a notification of this kind ("session" | "rank" | "goal") be shown right now?
pub fn allow_notification(s: &Settings, kind: &str, now_utc: i64) -> bool {
    let kind_on = match kind {
        "session" => s.notify_session,
        "rank" => s.notify_rank,
        "goal" => s.notify_goal,
        _ => true,
    };
    s.notifications && kind_on && !in_quiet_hours(s, now_utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(start: &str, end: &str, offset: i32) -> Settings {
        Settings { quiet_enabled: true, quiet_start: start.into(), quiet_end: end.into(), tz_offset_min: offset, ..Default::default() }
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_hm("07:30"), Some(450));
        assert_eq!(parse_hm(" 0:00 "), Some(0));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("7"), None);
        assert_eq!(parse_hm("12:60"), None);
    }

    #[test]
    fn quiet_hours_wrap_midnight_and_follow_the_timezone() {
        let at = |h: i64, m: i64| h * 3600 + m * 60; // UTC time on some day
        let s = quiet("23:00", "08:00", 0);
        assert!(in_quiet_hours(&s, at(23, 0)));
        assert!(in_quiet_hours(&s, at(2, 30)));
        assert!(!in_quiet_hours(&s, at(8, 0)));
        assert!(!in_quiet_hours(&s, at(12, 0)));
        // New York in winter is UTC-5: 03:00 UTC is 22:00 local (awake), 05:00 UTC is 00:00 local (quiet).
        let ny = quiet("23:00", "08:00", -300);
        assert!(!in_quiet_hours(&ny, at(3, 0)));
        assert!(in_quiet_hours(&ny, at(5, 0)));
        // Same-day window, equal times, and off.
        assert!(in_quiet_hours(&quiet("13:00", "14:00", 0), at(13, 30)));
        assert!(!in_quiet_hours(&quiet("13:00", "13:00", 0), at(13, 0)));
        assert!(!in_quiet_hours(&quiet("bad", "08:00", 0), at(2, 0)));
        assert!(!in_quiet_hours(&Settings { quiet_enabled: false, ..quiet("00:00", "23:59", 0) }, at(5, 0)));
    }

    #[test]
    fn each_kind_has_its_own_switch() {
        let mut s = Settings::default();
        assert!(allow_notification(&s, "session", 0) && allow_notification(&s, "rank", 0) && allow_notification(&s, "goal", 0));
        s.notify_rank = false;
        assert!(!allow_notification(&s, "rank", 0) && allow_notification(&s, "goal", 0));
        s.notifications = false;
        assert!(!allow_notification(&s, "goal", 0));
    }
}
