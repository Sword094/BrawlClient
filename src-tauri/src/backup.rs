//! Backup / restore of everything the app has saved, plus CSV and image exports.
//!
//! Files are written to the Downloads folder (there is no save dialog); the UI offers "Show in
//! folder". Backups never contain the API key, so a backup file is safe to keep or share.

use crate::{history, matches::Match, settings, store};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const FORMAT: &str = "brawl-stars-hub-backup";
const VERSION: i64 = 1;
const FIXED: [&str; 5] = ["favorites.json", "goals.json", "activity.json", "tracker_state.json", "settings.json"];

/// Only these files are ever read into or written from a backup, so a doctored backup file can't
/// reach any other path.
pub fn is_backup_file(name: &str) -> bool {
    if FIXED.contains(&name) {
        return true;
    }
    name.strip_prefix("history_")
        .and_then(|r| r.strip_suffix(".json"))
        .map_or(false, |id| !id.is_empty() && id.len() <= 16 && id.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn scrub(name: &str, v: &mut Value) {
    if name == "settings.json" {
        if let Some(o) = v.as_object_mut() {
            o.remove("api_key");
        }
    }
}

pub fn build(files: Vec<(String, Value)>, now: i64, app_version: &str) -> Value {
    let mut map = serde_json::Map::new();
    for (name, mut v) in files {
        if is_backup_file(&name) {
            scrub(&name, &mut v);
            map.insert(name, v);
        }
    }
    json!({ "format": FORMAT, "version": VERSION, "created": now, "app_version": app_version, "files": map })
}

/// Validates a backup and returns the files to restore.
pub fn parse(text: &str) -> Result<Vec<(String, Value)>, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "That file isn't a Brawl Stars Hub backup (it isn't valid JSON).".to_string())?;
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err("That file isn't a Brawl Stars Hub backup.".into());
    }
    if v.get("version").and_then(Value::as_i64).map_or(true, |n| n > VERSION) {
        return Err("This backup was made by a newer version of the app. Update the app first.".into());
    }
    let files = v.get("files").and_then(Value::as_object).ok_or("The backup has no data in it.")?;
    let mut out = Vec::new();
    for (name, val) in files {
        if !is_backup_file(name) {
            continue; // ignore anything unexpected rather than writing it
        }
        let mut val = val.clone();
        scrub(name, &mut val);
        out.push((name.clone(), val));
    }
    if out.is_empty() {
        return Err("The backup has no data in it.".into());
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/* ------------------------------ file system ------------------------------ */

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn out_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().download_dir().or_else(|_| data_dir(app).map(|d| d.join("exports")))
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// (year, month, day) for days since 1970-01-01.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// "2026-11-15" and "2026-11-15T21:04:05Z" style stamps (UTC).
fn stamp(t: i64, full: bool) -> String {
    let (y, m, d) = civil(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    if full {
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{y:04}-{m:02}-{d:02}-{:02}{:02}", s / 3600, s % 3600 / 60)
    }
}

/// Letters, digits, dot, dash and underscore only.
pub fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let n = n.trim_matches('.').to_string();
    if n.is_empty() { "export".into() } else { n.chars().take(80).collect() }
}

/// `dir/name`, or `dir/name-2.ext` etc. when it already exists.
fn unique(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|i| dir.join(if ext.is_empty() { format!("{stem}-{i}") } else { format!("{stem}-{i}.{ext}") }))
        .find(|p| !p.exists())
        .unwrap()
}

fn write_out(app: &AppHandle, name: &str, bytes: &[u8]) -> Result<String, String> {
    let path = unique(&out_dir(app)?, &safe_name(name));
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save the file: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

fn collect(app: &AppHandle) -> Result<Vec<(String, Value)>, String> {
    let dir = data_dir(app)?;
    let _g = store::lock();
    let mut files = Vec::new();
    for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_backup_file(&name) {
            continue;
        }
        if let Some(v) = std::fs::read_to_string(e.path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            files.push((name, v));
        }
    }
    Ok(files)
}

pub fn export_backup(app: &AppHandle, app_version: &str) -> Result<String, String> {
    let files = collect(app)?;
    if files.is_empty() {
        return Err("There's nothing to back up yet.".into());
    }
    let now = unix_now();
    let text = serde_json::to_string(&build(files, now, app_version)).map_err(|e| e.to_string())?;
    write_out(app, &format!("brawl-stars-hub-backup-{}.json", stamp(now, false)), text.as_bytes())
}

/// Replaces the files named in the backup. A copy of the current data is kept first.
pub fn restore(app: &AppHandle, text: &str, app_version: &str) -> Result<Value, String> {
    let files = parse(text)?;
    // Safety copy, so a wrong file can be undone.
    let current = collect(app)?;
    let keep = if current.is_empty() { None } else {
        let dir = data_dir(app)?.join("backups");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let now = unix_now();
        let path = dir.join(format!("before-restore-{}.json", stamp(now, false)));
        let body = serde_json::to_string(&build(current, now, app_version)).map_err(|e| e.to_string())?;
        std::fs::write(&path, body).map_err(|e| format!("Couldn't make the safety copy: {e}"))?;
        Some(path.to_string_lossy().into_owned())
    };
    let mut players = 0;
    let _g = store::lock();
    for (name, val) in &files {
        if name == "settings.json" {
            // Keep this computer's API key; take everything else from the backup.
            let mut s: settings::Settings = serde_json::from_value(val.clone()).unwrap_or_default();
            s.api_key = settings::load(app).api_key;
            settings::save(app, &s)?;
        } else {
            store::save(app, name, val)?;
            players += name.starts_with("history_") as usize;
        }
    }
    Ok(json!({ "files": files.len(), "players": players, "safety_copy": keep }))
}

/* --------------------------------- exports -------------------------------- */

fn csv_field(s: &str) -> String {
    // Quote anything with a delimiter or line break; neutralise spreadsheet formulas.
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

fn opt(v: Option<i64>) -> String {
    v.map_or(String::new(), |n| n.to_string())
}

/// One row per game, oldest first.
pub fn matches_csv(ms: &[Match]) -> String {
    let mut out = String::from("date_utc,result,mode,map,ranked,brawler,elo_before,elo_after,elo_change,trophy_change,star_player,placement\r\n");
    let mut rows: Vec<&Match> = ms.iter().filter(|m| m.complete || !m.ranked).collect();
    rows.sort_by_key(|m| m.end);
    for m in rows {
        let me = m.my_team.and_then(|i| m.teams.get(i)).and_then(|t| t.iter().find(|p| p.me));
        let cols = [
            stamp(m.end, true),
            csv_field(&m.result),
            csv_field(&m.mode),
            csv_field(&m.map),
            if m.ranked { "yes".into() } else { "no".into() },
            csv_field(me.map_or("", |p| p.brawler.as_str())),
            opt(m.elo_before),
            opt(m.elo_after),
            opt(m.elo_change),
            opt(m.trophy_change),
            if me.map_or(false, |p| p.star) { "yes".into() } else { "no".into() },
            opt(m.placement),
        ];
        out.push_str(&cols.join(","));
        out.push_str("\r\n");
    }
    out
}

pub fn export_matches_csv(app: &AppHandle, tag: &str) -> Result<String, String> {
    let key = crate::api::normalize_tag(tag);
    let h = history::load_history(app, &key);
    let ms = h.matches(&key);
    if ms.is_empty() {
        return Err("No saved games to export for this player yet.".into());
    }
    write_out(app, &format!("brawl-games-{key}-{}.csv", stamp(unix_now(), false)), matches_csv(&ms).as_bytes())
}

pub fn save_image(app: &AppHandle, name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.len() < 8 || &bytes[1..4] != b"PNG" {
        return Err("That wasn't a PNG image.".into());
    }
    let name = safe_name(name);
    let name = if name.ends_with(".png") { name } else { format!("{name}.png") };
    write_out(app, &name, bytes)
}

/// Opens the file manager with the file selected. Only files in our own output folders.
pub fn reveal(app: &AppHandle, path: &str) -> Result<(), String> {
    let p = PathBuf::from(path);
    let ok = [out_dir(app).ok(), data_dir(app).ok()].into_iter().flatten().any(|d| p.starts_with(&d));
    if !ok || !p.exists() {
        return Err("That file isn't in the app's export folder.".into());
    }
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(&p).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p.parent().unwrap_or(&p)).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::PlayerRef;

    #[test]
    fn only_known_files_are_backed_up() {
        for ok in ["goals.json", "settings.json", "history_2PP0YLQ.json"] {
            assert!(is_backup_file(ok), "{ok}");
        }
        for bad in ["../x.json", "history_.json", "history_../a.json", "meta.json", "app_state.json", "history_A/B.json", "history_ab cd.json", "goals.json.tmp"] {
            assert!(!is_backup_file(bad), "{bad}");
        }
    }

    #[test]
    fn backup_roundtrip_drops_the_api_key_and_unknown_files() {
        let b = build(
            vec![
                ("settings.json".into(), json!({ "api_key": "SECRET", "player_tag": "ABC" })),
                ("goals.json".into(), json!({ "X": [] })),
                ("meta.json".into(), json!({ "big": true })),
            ],
            1_700_000_000,
            "0.2.5",
        );
        let text = b.to_string();
        assert!(!text.contains("SECRET") && !text.contains("big"));
        let back = parse(&text).unwrap();
        assert_eq!(back.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["goals.json", "settings.json"]);
        // A backup edited to carry a key and a path trick is cleaned on the way in.
        let evil = json!({ "format": FORMAT, "version": 1, "files": { "settings.json": { "api_key": "K" }, "../evil.json": {}, "goals.json": {} } });
        let got = parse(&evil.to_string()).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().find(|f| f.0 == "settings.json").unwrap().1.get("api_key").is_none());
    }

    #[test]
    fn rejects_bad_backups() {
        assert!(parse("not json").is_err());
        assert!(parse("{\"format\":\"other\"}").is_err());
        assert!(parse(&json!({ "format": FORMAT, "version": 99, "files": {} }).to_string()).unwrap_err().contains("newer"));
        assert!(parse(&json!({ "format": FORMAT, "version": 1, "files": {} }).to_string()).is_err());
    }

    #[test]
    fn dates_and_names() {
        assert_eq!(stamp(0, true), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(1_794_776_645, true), "2026-11-15T21:04:05Z");
        assert_eq!(stamp(1_794_776_645, false), "2026-11-15-2104");
        assert_eq!(stamp(951_782_400, true), "2000-02-29T00:00:00Z"); // leap day
        assert_eq!(safe_name("../a b/c.png"), "_a_b_c.png");
        assert_eq!(safe_name("..."), "export");
    }

    #[test]
    fn csv_is_escaped_and_oldest_first() {
        let me = |b: &str, star| PlayerRef { brawler: b.into(), me: true, star, ..Default::default() };
        let mk = |end, res: &str, map: &str, elo: Option<i64>, p: PlayerRef| Match {
            end, result: res.into(), mode: "gemGrab".into(), map: map.into(), complete: true, ranked: elo.is_some(),
            elo_change: elo, my_team: Some(0), teams: vec![vec![p]], ..Default::default()
        };
        let ms = vec![
            mk(1_794_776_700, "defeat", "=HYPERLINK(\"x\")", Some(-12), me("SHELLY", false)), // newest first, like history
            mk(1_794_776_645, "victory", "Hard Rock, Mine", Some(14), me("MORTIS", true)),
        ];
        let csv = matches_csv(&ms);
        let lines: Vec<&str> = csv.trim_end().split("\r\n").collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], "2026-11-15T21:04:05Z,victory,gemGrab,\"Hard Rock, Mine\",yes,MORTIS,,,14,,yes,");
        assert!(lines[2].contains("\"'=HYPERLINK(\"\"x\"\")\"")); // formula neutralised and quoted
    }
}
