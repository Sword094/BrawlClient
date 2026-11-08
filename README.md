# Brawl Stars Hub

A standalone Windows app (Tauri v2 + Rust) for looking up Brawl Stars players, tracking favorites, and showing your stats on Discord.

## Features

- **Player search:** full profile for any tag, with a sortable brawler table.
- **Favorites:** save players; they're checked in the background about once a minute.
- **Recent games:** every game the app has saved plus the API's latest 25. A ranked game is a best-of-3, so it shows as one game with its round score (e.g. 2-1) and one elo change. Both teams, brawler icons and the star player are shown; click a player for their brawler or to open their profile.
- **Stats & graphs:** elo and trophy graphs over time, win rates by brawler/mode/map, and recent sessions.
- **Ranked picks:** suggests brawlers for a map from the recent games of Diamond-and-higher players. You choose the data source (Diamond+, or a single tier such as Mythic or Legendary), hide any brawlers you mark as banned, and optionally show only ones you own. Click **Refresh sample** first. The API can't list players by rank, so the app starts from you and the top trophy players, follows the opponents in their ranked games, and checks each player's rank on their profile; anyone below Diamond is skipped. A 200-player sample is about 400 requests; the Speed setting controls how many players are looked up at once (Normal is a minute or two). Picks are ordered by a smoothed win rate, nudged by your brawler's power level.
- **Compare:** two players side by side, including the brawlers they both own.
- **Tray, autostart, notifications:** the app can live in the system tray, start with Windows, and notify you when a favorite starts playing or a rank changes.
- **Activity log:** who is playing, when each tracked player's last game was, and how each game went.
- **Discord Rich Presence:** rotating every 30 seconds between trophies, ranked elo and (while you're playing) your current session. Each page can be switched off in Settings.

## What the Brawl Stars API can and can't do

Some things you might expect aren't in the official API, so the app works around them:

| Feature | How it's done |
| --- | --- |
| **Elo change per ranked game** | Not provided. The app records an elo reading every check and works out each game's change from the readings before and after it (a short wait lets the profile catch up). Games played before tracking started, or while the app was closed, show "elo —". A season reset is never counted as a game result. |
| **"Is this player in a game?"** | Not provided. "Playing now" means a game finished in the last 10 minutes. The battle log also lags a little behind real play. |
| **Per-player stats inside a game** | Not provided. Only brawler, power, brawler tier/trophies and star player are available. |
| **Game history** | The API only keeps 25 battle-log entries (about 8-10 ranked games). The app saves everything it sees, so keep it running to build up history. |

Tracking covers you (from Settings) plus your favorites. Elo for other players is only calculated while they're being tracked.

## Build it on Windows

1. Install [Node.js](https://nodejs.org/en/download) (LTS).
2. Install [Rust](https://rustup.rs/) (accept the defaults).
3. Install **Visual Studio Build Tools** with the **Desktop development with C++** workload.
4. WebView2 is already part of Windows 10/11.
5. In this folder:
   ```
   npm install
   npm run dev      # opens the app with live reload
   npm run build    # makes the .exe and installers
   ```
   The first build compiles a lot of Rust crates and can take several minutes. After that it's fast.

Build output:
- `src-tauri/target/release/brawl-stars-hub.exe` (portable)
- `src-tauri/target/release/bundle/` (installers)

Run the backend tests with `cargo test` inside `src-tauri`.

## First run

Open **Settings** and fill in:
- **API key** from <https://developer.brawlstars.com> (allow your current public IP).
- **Player tag** (with or without `#`).
- **Discord application ID** from the [Developer Portal](https://discord.com/developers/applications).

Click **Test connection**. Background tracking is on by default; uncheck it if you don't want the app polling the API.

Data is saved in `%APPDATA%\com.brawlstarshub.app`: `settings.json` (including your API key in plain text), `favorites.json`, `tracker_state.json`, `activity.json`, and one `history_<TAG>.json` per tracked player (saved battle log + elo readings).

## Project layout

```
src/                 Frontend (plain HTML/CSS/JS, no build step)
src-tauri/src/
  api.rs             Brawl Stars API client
  presence.rs        Discord Rich Presence loop
  tracker.rs         Background tracking and the activity log
  matches.rs         Groups battle-log entries into matches; works out elo changes
  history.rs         Saved battle log + elo readings, sessions
  meta.rs            Ranked picks: samples top players' games
  stats.rs           Win rates and graph data
  icons.rs           Brawler icons (Brawlify, best effort)
  store.rs           JSON file storage helpers and the shared lock
  settings.rs        Settings storage
  lib.rs             Tauri commands
```
