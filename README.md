# Brawl Stars Hub

A standalone Windows app (Tauri v2 + Rust) for looking up Brawl Stars players, tracking favorites, and showing your stats on Discord.

## Features

- **Player search:** full profile for any tag, with a sortable brawler table.
- **Favorites:** save players; they're checked in the background about once a minute.
- **Recent games:** every game the app has saved plus the API's latest 25. A ranked game is a best-of-3, so it shows as one game with its round score (e.g. 2-1) and one elo change. Both teams, brawler icons and the star player are shown; click a player for their brawler, power and tier, or to open their profile.
- **Match details:** click the top row of any game to open a panel with the map image, mode, time played, result, round-by-round results and lengths, elo before/after, your brawler and the star player.
- **Stats & graphs:** overview tiles (battles, win rate, current and best streak, star player rate, average trophies per game, ranked record, net elo), a daily activity heat map (battles or win rate), trophy / ranked-elo graph with 7 day / 30 day / all-time ranges, win rates by brawler, mode and map, recent sessions, and a two-click "Clear history".
- **Ranked picks:** suggests brawlers for a map from the recent games of Diamond-and-higher players.
  - **Top 5 picks** are shown as cards above a full table. They're ranked by a smoothed win rate (so a brawler seen in a handful of rounds can't top the list), nudged by your own brawler's power level. Banned brawlers are removed first.
  - **Data source:** Diamond and higher (default), Diamond only, Mythic only, Mythic and higher, Legendary only, Legendary and higher, or Masters only. It never goes below Diamond. Changing the source doesn't need a new refresh.
  - **Filters:** enter the match's banned brawlers by hand, tick *Only brawlers I own*, and choose *Ranked games only* or all 3v3 rounds. The selected map's image is shown beside the controls.
  - **Refresh sample:** the API can't list players by rank, so the app starts from you and the top trophy players, follows the opponents in their ranked games, and checks each player's rank on their profile (anyone below Diamond is skipped). Choose a sample of 100, 200 or 300 players and a **Speed** (Gentle 2, Normal 4, Fast 8 or Max 12 players looked up at once). A 200-player sample is about 400 requests. If you hit the API's rate limit (HTTP 429) drop to a slower speed.
- **Compare:** two players side by side, including the brawlers they both own.
- **Tray, autostart, notifications:** closing the window can keep the app running in the system tray (right-click the tray icon and choose Quit to exit), the app can start with Windows hidden in the tray, and it can notify you when a favorite starts playing or a rank changes.
- **Activity log:** who is playing, when each tracked player's last game was, and how each game went.
- **Discord Presence page:** choose which pages rotate (latest match, trophies, ranked, win streak, victories, recent record, session, club), write your own two lines with placeholders like `{brawler}`, `{trophies}`, `{elo}`, `{winrate}` (click one to insert it), pin a single page, show brawler/map pictures and a "View Brawl Stars stats" button, and see a live preview of what Discord will show.
- **Welcome back popup:** when you open the app, it shows how many trophies and how much ranked elo you gained or lost while it was closed (only if something changed).
- **Tags are always uppercase** in the search, compare and settings boxes.

## What the Brawl Stars API can and can't do

Some things you might expect aren't in the official API, so the app works around them:

| Feature | How it's done |
| --- | --- |
| **Elo change per ranked game** | Not provided. The app records an elo reading every check and works out each game's change from the readings before and after it (a short wait lets the profile catch up). Games played before tracking started, or while the app was closed, show "elo —". A season reset is never counted as a game result. |
| **"Is this player in a game?"** | Not provided. "Playing now" means a game finished in the last 10 minutes. The battle log also lags a little behind real play. |
| **Per-player stats inside a game** | Not provided. Only brawler, power, brawler tier/trophies and star player are available. |
| **Damage, healing, kills, deaths, zone time** | Not provided. These only appear in the game's own post-match screen, so the app can't show them. |
| **Bans, map pool and meta** | Not provided. Ranked Picks samples the meta from other players' games, and you enter bans and pick the map yourself. The map list is the maps seen in the sample, not the official rotation. |
| **Players by rank** | Not provided. Ranked Picks finds players by crawling ranked opponents and reading each profile's rank, so low tiers may be thinly sampled. A player's rank is their current rank, not their rank when a game was played. |
| **Game history** | The API only keeps 25 battle-log entries (about 8-10 ranked games). The app saves everything it sees, so keep it running to build up history. |

Tracking covers you (from Settings) plus your favorites. Elo for other players is only calculated while they're being tracked.

Brawler icons and map images come from the community [Brawlify](https://brawlify.com) service and are best effort: if they can't be loaded, the app shows names only.

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

Data is saved in `%APPDATA%\com.brawlstarshub.app`: `settings.json` (including your API key in plain text), `favorites.json`, `tracker_state.json`, `activity.json`, `meta.json` (the Ranked Picks sample), `brawler_icons.json` (cached icon list), and one `history_<TAG>.json` per tracked player (saved battle log + elo readings).

## Project layout

```
src/                 Frontend (plain HTML/CSS/JS, no build step)
src-tauri/src/
  api.rs             Brawl Stars API client
  presence.rs        Discord Rich Presence loop
  tracker.rs         Background tracking and the activity log
  matches.rs         Groups battle-log entries into matches; works out elo changes
  history.rs         Saved battle log + elo readings, sessions
  meta.rs            Ranked picks: samples Diamond+ players' games, scores brawlers
  stats.rs           Win rates and graph data
  icons.rs           Brawler icons (Brawlify, best effort)
  store.rs           JSON file storage helpers and the shared lock
  settings.rs        Settings storage
  lib.rs             Tauri commands
```

## Releases and automatic updates

The app updates itself from [GitHub Releases](https://github.com/Sword094/BrawlClient/releases): it checks when it opens (and every few hours), shows an "Update available" box, and installs and restarts when you click **Install and restart**. After an update, a **What's new** screen shows the release notes once. You can also use **Settings > Updates** at any time.

**One-time setup (maintainer)**
1. Create a signing key: `npx tauri signer generate -w ~/.tauri/brawlclient.key` (pick a password).
2. Paste the **public** key (the `.key.pub` file's contents) into `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`.
3. In the GitHub repo: **Settings > Secrets and variables > Actions**, add `TAURI_SIGNING_PRIVATE_KEY` (the contents of the private `.key` file) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Back up the private key. If it's lost, installed copies can't be updated any more.

**Shipping a new version**
1. Raise `version` in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`.
2. Rewrite `RELEASE_NOTES.md` for the new version. This text becomes the release description and the app's "What's new" screen.
3. Commit, then `git tag v0.2.1 && git push && git push --tags` (use your real version number).
4. GitHub Actions builds the installer, signs it, and publishes the release with `latest.json`. Installed apps pick it up from there.

Until the public key is set, the app builds and runs normally; update checks just say updates aren't set up. Local builds (`npm run build`) don't need the key. Only the release workflow signs updates.

## Disclaimer

This is a fan project. It is not affiliated with or endorsed by Supercell or Brawlify. Brawl Stars is a trademark of Supercell. Your API key and data stay on your computer.
