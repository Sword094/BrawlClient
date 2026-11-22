"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);

/* ============================== helpers ============================== */

// Every string from the API goes through esc() before it touches innerHTML.
function esc(value) {
  return String(value ?? "").replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}

const num = (n) => (typeof n === "number" ? n.toLocaleString() : "—");
const sign = (n) => (n > 0 ? "+" : n < 0 ? "−" : "±") + Math.abs(n).toLocaleString();
const normTag = (t) => String(t || "").trim().replace(/^#/, "").toUpperCase().replace(/O/g, "0");

// Tag fields are always uppercase (tags only use 0289PYLQGRJCUV, and O is always 0).
for (const id of ["search-input", "cmp-a", "cmp-b", "set-tag"]) {
  const el = $(id);
  if (!el) continue;
  el.addEventListener("input", () => {
    const at = el.selectionStart, v = el.value.toUpperCase();
    if (v !== el.value) { el.value = v; try { el.setSelectionRange(at, at); } catch {} }
  });
}

// API colors look like "0xffa8e132" (AARRGGBB). Return "#rrggbb".
function apiColor(c) {
  const m = /^0x[0-9a-f]{2}([0-9a-f]{6})$/i.exec(c || "");
  return m ? "#" + m[1] : "#f1eefc";
}

function errText(e) {
  return typeof e === "string" ? e : (e && e.message) || "Something went wrong.";
}

// "20261107T222129.000Z" -> ms since epoch (UTC)
function parseBT(s) {
  const m = /^(\d{4})(\d\d)(\d\d)T(\d\d)(\d\d)(\d\d)/.exec(s || "");
  return m ? Date.UTC(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], +m[6]) : null;
}

function ago(ms) {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.floor(h / 24)} d ago`;
}

const ACTIVE_MS = 10 * 60 * 1000;

// The API has no live status, so "playing" means a game finished in the last 10 minutes.
function statusOf(lastBattleTime) {
  const t = parseBT(lastBattleTime);
  if (t == null) return { active: false, text: "No recent games" };
  const active = Date.now() - t <= ACTIVE_MS;
  return { active, text: active ? `Likely playing · last game ${ago(t)}` : `Last game ${ago(t)}` };
}

function modeName(m) {
  const s = String(m || "?");
  return s.charAt(0).toUpperCase() + s.slice(1).replace(/([A-Z])/g, " $1");
}

let toastTimer = null;
function toast(msg, isErr = false, action = null) {
  const el = $("toast");
  el.textContent = msg;
  el.className = "toast" + (isErr ? " err" : "");
  if (action) {
    const b = document.createElement("button");
    b.className = "toast-act"; b.textContent = action.label;
    b.onclick = () => { action.run(); el.classList.add("hidden"); };
    el.append(" ", b);
  }
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add("hidden"), isErr ? 5000 : action ? 9000 : 2200);
}

/* ============================ insights page ============================ */

let insTag = "", insToken = 0;

function insRows(title, hint, rows, icons) {
  if (!rows.length) return `<div><h2>${esc(title)}</h2><div class="empty small">${esc(hint)}</div></div>`;
  return rateTable(title, rows, icons);
}

function insBars(slots, labels, title) {
  const max = Math.max(1, ...slots.map((s) => s[0] + s[1]));
  const bars = slots.map((s, i) => {
    const n = s[0] + s[1], rate = n ? Math.round((s[0] / n) * 100) : 0;
    const cls = n < 3 ? "" : rate >= 55 ? "good" : rate < 45 ? "bad" : "mid";
    const tip = n ? `${labels[i]}: ${n} game${n === 1 ? "" : "s"} · ${s[0]}W–${s[1]}L · ${rate}% win rate` : `${labels[i]}: no games`;
    return `<div class="tb-col" data-tip="${esc(tip)}"><i class="tb ${cls}" style="height:${n ? Math.max(6, Math.round((n / max) * 100)) : 0}%"></i></div>`;
  }).join("");
  return `<div><h2>${esc(title)}</h2><div class="tb-wrap">${bars}</div></div>`;
}

function insightsHtml(ins) {
  if (ins.games < 5) return `<div class="notice">Not enough saved games yet (${ins.games}). Keep the app running while you play and this fills in.</div>`;
  const hardest = ins.enemies.slice(0, 8);
  const easiest = ins.enemies.length > 8 ? ins.enemies.slice(-8).reverse() : [];
  const pairName = (p) => ({ name: `${p.mine} vs ${p.theirs}`, id: p.mine_id, wins: p.wins, losses: p.losses });
  const worst = ins.pairs.slice(0, 6).filter((p) => p.wins < p.losses).map(pairName);
  const best = ins.pairs.slice().reverse().slice(0, 6).filter((p) => p.wins > p.losses).map(pairName);
  const mapRows = ins.maps.map((m) => {
    const n = m.wins + m.losses, pct = Math.round((m.wins / n) * 100);
    const b = m.best, bn = b ? b.wins + b.losses : 0;
    return `<tr><td class="name">${esc(m.map)}</td><td class="num">${m.wins}–${m.losses}</td>
      <td class="num"><span class="bar"><i style="width:${pct}%"></i></span> ${pct}%</td>
      <td class="name">${b ? `${brIcon(b.id, b.name, 22)}${esc(b.name)} <span class="muted">${b.wins}–${b.losses} (${Math.round((b.wins / bn) * 100)}%)</span>` : `<span class="muted">not enough games</span>`}</td></tr>`;
  }).join("");
  const hourLabels = Array.from({ length: 24 }, (_, h) => new Date(2000, 0, 1, h).toLocaleTimeString([], { hour: "numeric" }));
  const days = [0, 1, 2, 3, 4, 5, 6].map((d) => new Date(2000, 0, 2 + d).toLocaleDateString([], { weekday: "long" }));
  return `
    <div class="ins-grid">${insRows("Who you play with", "Play a few more games with the same people (3 or more) and they appear here.", ins.mates, false)}
      ${insRows("Your best and worst pairings", "Not enough repeat matchups yet.", [...best, ...worst], true)}</div>
    <div class="ins-grid ins-gap">${insRows("Brawlers that give you trouble", "Fewer than 4 games against any one brawler so far.", hardest, true)}
      ${easiest.length ? rateTable("Brawlers you beat most", easiest, true) : ""}</div>
    <div class="section-head"><h2>Maps</h2></div>
    <div class="card">${ins.maps.length ? `<div class="table-wrap"><table><thead><tr><th>Map</th><th>W–L</th><th>Win rate</th><th>Your best brawler there</th></tr></thead><tbody>${mapRows}</tbody></table></div>` : `<div class="empty small">Maps appear after 5 games on the same map.</div>`}</div>
    <div class="section-head"><h2>When you play best</h2></div>
    <div class="card">${insBars(ins.hours, hourLabels, "By hour of day")}<div class="tb-axis muted small"><span>12 AM</span><span>6 AM</span><span>12 PM</span><span>6 PM</span><span>11 PM</span></div>
      <div style="height:14px"></div>${insBars(ins.weekdays, days, "By day of the week")}<div class="tb-axis tb-week muted small">${["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].map((d) => `<span>${d}</span>`).join("")}</div>
      <div class="chart-foot muted small"><span class="chart-key"><span><i style="background:var(--good)"></i>55%+ wins</span><span><i style="background:var(--muted)"></i>about even</span><span><i style="background:var(--bad)"></i>under 45%</span></span> Taller bar = more games. Uncoloured bars have fewer than 3 games.</div></div>`;
}

async function renderInsightsPage() {
  if (!insTag) insTag = normTag(lastSettings.player_tag);
  const opts = await playerOptions(insTag);
  if (!insTag && opts.length) insTag = opts[0].tag;
  $("ins-select").innerHTML = opts.map((o) => `<option value="${esc(o.tag)}" ${o.tag === insTag ? "selected" : ""}>${esc(o.label)}</option>`).join("") || `<option value="">No players yet</option>`;
  if (!insTag) { $("ins-body").innerHTML = `<div class="notice">Set your player tag in Settings first.</div>`; return; }
  const token = ++insToken;
  showSkeleton($("ins-body"), skel.cards(5));
  let r;
  try { r = await invoke("get_insights", { tag: insTag, tzMin: -new Date().getTimezoneOffset() }); }
  catch (e) { $("ins-body").innerHTML = `<div class="error">${esc(errText(e))}</div>`; return; }
  if (token !== insToken) return;
  $("ins-body").innerHTML = r.tracked ? insightsHtml(r.insights) : `<div class="notice">Nothing saved for this player yet. Make them <b>you</b> in Settings or add them to <b>Favorites</b> and keep the app running.</div>`;
}
$("ins-select").onchange = () => { insTag = $("ins-select").value; renderInsightsPage(); };
$("ins-refresh").onclick = () => renderInsightsPage();
(() => {
  const body = $("ins-body");
  const tipEl = () => document.querySelector(".hm-tip"); // created by the Stats page code, so look it up when needed
  body.addEventListener("mouseover", (ev) => {
    const tip = tipEl(), c = ev.target.closest("[data-tip]");
    if (!tip) return;
    if (!c) { tip.hidden = true; return; }
    tip.textContent = c.dataset.tip; tip.hidden = false;
    const r = c.getBoundingClientRect();
    tip.style.left = Math.min(window.innerWidth - tip.offsetWidth - 8, Math.max(8, r.left + r.width / 2 - tip.offsetWidth / 2)) + "px";
    tip.style.top = r.top - tip.offsetHeight - 8 + "px";
  });
  body.addEventListener("mouseleave", () => { const tip = tipEl(); if (tip) tip.hidden = true; });
})();

/* ============================ loading skeletons ============================ */

const skel = {
  cards: (n = 4) => Array.from({ length: n }, () => `<div class="skel card"></div>`).join(""),
  rows: (n = 6) => Array.from({ length: n }, () => `<div class="skel skel-row"></div>`).join(""),
  stats: () => `<div class="skel-tiles">${Array.from({ length: 8 }, () => `<div class="skel tile"></div>`).join("")}</div><div class="skel wide"></div>`,
};
// Only when there is nothing on screen yet, so a refresh doesn't flash.
const showSkeleton = (el, html) => { if (!el.innerHTML.trim() || el.querySelector(".skel")) el.innerHTML = html; };

/* ============================ exports & backup ============================ */

const baseName = (p) => String(p).split(/[\\/]/).pop();
// A file was written to Downloads: say so, with a shortcut to its folder.
function savedToast(path) {
  toast(`Saved ${baseName(path)} to Downloads.`, false, {
    label: "Show in folder",
    run: () => invoke("reveal_file", { path }).catch((e) => toast(errText(e), true)),
  });
}

// The graph on screen, drawn to a PNG. The SVG relies on page CSS, so the computed colours and
// text styles are copied onto a clone first.
async function chartPng() {
  const live = document.querySelector("#stats-body svg.chart");
  if (!live) throw new Error("There's no graph to save yet.");
  const clone = live.cloneNode(true);
  const lv = [live, ...live.querySelectorAll("*")], cv = [clone, ...clone.querySelectorAll("*")];
  const props = ["fill", "stroke", "stroke-width", "stroke-linejoin", "stroke-dasharray", "font-size", "font-family", "font-weight", "opacity"];
  lv.forEach((el, i) => { if (cv[i].closest(".xh")) return; const cs = getComputedStyle(el); cv[i].setAttribute("style", props.map((p) => `${p}:${cs.getPropertyValue(p)}`).join(";")); });
  clone.querySelector(".xh")?.remove(); // the hover crosshair is not part of the picture
  const [, , vw, vh] = live.getAttribute("viewBox").split(" ").map(Number);
  clone.setAttribute("xmlns", "http://www.w3.org/2000/svg");
  clone.setAttribute("width", vw); clone.setAttribute("height", vh);
  const css = getComputedStyle(document.documentElement), col = (n, d) => css.getPropertyValue(n).trim() || d;
  const img = new Image();
  await new Promise((ok, no) => { img.onload = ok; img.onerror = () => no(new Error("Couldn't draw the graph.")); img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(new XMLSerializer().serializeToString(clone)); });
  const pad = 24, head = 68, scale = 2, w = vw + pad * 2, h = vh + head + pad + 18;
  const cv2 = document.createElement("canvas"); cv2.width = w * scale; cv2.height = h * scale;
  const g = cv2.getContext("2d"); g.scale(scale, scale);
  g.fillStyle = col("--panel", "#1c1833"); g.fillRect(0, 0, w, h);
  g.fillStyle = col("--text", "#f1eefc"); g.font = "700 18px system-ui, sans-serif";
  const elo = statsMetric === "elo", who = $("stats-select").selectedOptions[0]?.textContent || "";
  g.fillText(`${elo ? "Ranked elo" : "Trophies"}${who ? " · " + who : ""}`, pad, pad + 14);
  g.fillStyle = col("--muted", "#9a93bd"); g.font = "13px system-ui, sans-serif";
  const gain = document.querySelector("#stats-body .gain");
  g.fillText(gain ? gain.textContent : "", pad, pad + 36);
  g.drawImage(img, pad, head);
  g.fillText("Brawl Stars Hub", pad, h - 12);
  const blob = await new Promise((ok) => cv2.toBlob(ok, "image/png"));
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
}

/* ============================ brawler icons ============================ */

// Best-effort. Tries the id->URL map from the Brawlify API first, then guessed CDN paths.
// If an image fails to load it moves to the next candidate, and finally hides itself.
let icons = {};
const ICON_FALLBACKS = (id) => [
  `https://cdn.brawlify.com/brawlers/borders/${id}.png`,
  `https://cdn.brawlify.com/brawlers/borders/${id}.webp`,
];
function brIcon(id, name, size = 22) {
  if (!id) return "";
  const list = [icons[String(id)], ...ICON_FALLBACKS(id)].filter(Boolean);
  return `<img class="bricon" src="${esc(list[0])}" data-alts="${esc(JSON.stringify(list.slice(1)))}" data-name="${esc(name || "")}" width="${size}" height="${size}" alt="" loading="lazy" />`;
}
document.addEventListener("error", (ev) => {
  const img = ev.target;
  if (!(img instanceof HTMLImageElement) || !img.classList.contains("bricon")) return;
  let alts = [];
  try { alts = JSON.parse(img.dataset.alts || "[]"); } catch (e) { /* ignore */ }
  const next = alts.shift();
  if (next) { img.dataset.alts = JSON.stringify(alts); img.src = next; }
  else if (img.dataset.name && !img.classList.contains("mapimg")) {
    // Out of picture sources: show the brawler's initial instead of an empty gap.
    const b = document.createElement("span");
    b.className = "bricon fallback"; b.textContent = img.dataset.name.trim().charAt(0).toUpperCase();
    b.style.width = b.style.height = (img.getAttribute("width") || 22) + "px";
    b.title = img.dataset.name;
    img.replaceWith(b);
  } else { img.style.display = "none"; }
}, true);

// Map images are keyed by the battle's event id on Brawlify's image server (best effort).
function mapImg(eventId, name) {
  if (!eventId) return "";
  const list = [`https://cdn.brawlify.com/maps/regular/${eventId}.png`, `https://cdn.brawlify.com/maps/${eventId}.png`];
  return `<img class="bricon mapimg" src="${list[0]}" data-alts="${esc(JSON.stringify(list.slice(1)))}" alt="${esc(name || "Map")}" />`;
}

/* ============================ title bar ============================ */

$("win-min").onclick = () => appWindow.minimize();
$("win-max").onclick = () => appWindow.toggleMaximize();
$("win-close").onclick = () => appWindow.close();

/* ============================ navigation ============================ */

let currentPage = "search";
let profileBackTo = "search";
let lastSettings = {};

function showPage(name) {
  currentPage = name;
  document.querySelectorAll(".page").forEach((p) => p.classList.remove("active"));
  $("page-" + name).classList.add("active");
  const highlight = name === "profile" ? profileBackTo : name;
  document.querySelectorAll(".nav-item").forEach((b) => {
    b.classList.toggle("active", b.dataset.page === highlight);
  });
  document.querySelector(".content").scrollTop = 0;

  if (name === "favorites") renderFavorites();
  if (name === "recent") renderRecentPage();
  if (name === "stats") renderStatsPage();
  if (name === "insights") renderInsightsPage();
  if (name === "recommend") initRecommend();
  if (name === "presence") renderPresencePage();
  if (name === "activity") renderActivity();
}

document.querySelectorAll(".nav-item").forEach((b) => {
  b.onclick = () => showPage(b.dataset.page);
});
$("profile-back").onclick = () => showPage(profileBackTo);

/* ============================ favorites state ============================ */

let favTags = new Set();

async function refreshFavTags() {
  try {
    const favs = await invoke("get_favorites");
    favTags = new Set(favs.map((f) => f.tag));
    return favs;
  } catch (e) {
    return [];
  }
}

/* ============================== search ============================== */

async function openProfile(tag, backTo, onError) {
  const back = backTo || currentPage;
  try {
    const player = await invoke("fetch_player", { tag });
    await refreshFavTags();
    profileBackTo = back === "profile" ? profileBackTo : back;
    renderProfile(player);
    showPage("profile");
  } catch (e) {
    (onError || ((m) => toast(m, true)))(errText(e));
  }
}

async function searchProfile(tag) {
  $("search-error").classList.add("hidden");
  $("search-loading").classList.remove("hidden");
  $("search-btn").disabled = true;
  $("my-profile-btn").disabled = true;
  await openProfile(tag, "search", (m) => {
    $("search-error").textContent = m;
    $("search-error").classList.remove("hidden");
  });
  $("search-loading").classList.add("hidden");
  $("search-btn").disabled = false;
  $("my-profile-btn").disabled = false;
}

$("search-form").onsubmit = (ev) => {
  ev.preventDefault();
  searchProfile($("search-input").value);
};

$("my-profile-btn").onclick = () => {
  if (!lastSettings.player_tag) {
    $("search-error").textContent = "Set your player tag in Settings first.";
    $("search-error").classList.remove("hidden");
    return;
  }
  searchProfile(lastSettings.player_tag);
};

/* ============================ profile page ============================ */

let currentProfile = null;

function stat(label, value, sub = "", cls = "") {
  return `<div class="stat ${cls}"><div class="label">${esc(label)}</div>` +
    `<div class="value">${value}</div>${sub ? `<div class="sub">${sub}</div>` : ""}</div>`;
}

// Win-streak field names aren't documented, so look for them by name.
function streakOf(b) {
  let best = null;
  for (const [k, v] of Object.entries(b)) {
    if (typeof v === "number" && /streak/i.test(k) && /max|highest|best/i.test(k)) {
      best = best === null ? v : Math.max(best, v);
    }
  }
  return best;
}

function namesOf(list) {
  return Array.isArray(list) ? list.map((x) => (x && x.name) || x).filter(Boolean) : [];
}

function chips(list) {
  const names = namesOf(list);
  return names.length
    ? `<div class="chips">${names.map((n) => `<span class="chip">${esc(n)}</span>`).join("")}</div>`
    : `<span class="none">None</span>`;
}

function renderProfile(p) {
  currentProfile = { tag: normTag(p.tag), name: p.name || "" };
  const club = p.club && p.club.name ? `${esc(p.club.name)} <span class="muted">${esc(p.club.tag)}</span>` : "No club";
  const color = apiColor(p.nameColor);
  const initial = esc((p.name || "?").trim().charAt(0).toUpperCase());
  const isFav = favTags.has(currentProfile.tag);

  const header = `
    <div class="p-header">
      <div class="avatar">${initial}</div>
      <div>
        <div class="p-name" style="color:${color}">${esc(p.name)}</div>
        <div class="p-sub">
          <span>${esc(p.tag)}</span>
          <span>${club}</span>
          <span class="pill">Level ${num(p.expLevel)}</span>
          ${p.totalPrestigeLevel ? `<span class="pill gold">Prestige ${num(p.totalPrestigeLevel)}</span>` : ""}
          ${p.fameTierName ? `<span class="pill">${esc(p.fameTierName)}</span>` : ""}
        </div>
      </div>
      <div class="p-actions">
        <button class="btn fav ${isFav ? "on" : ""}" id="fav-btn">${isFav ? "★ Favorited" : "☆ Favorite"}</button>
        <button class="btn" id="recent-btn">⚔ Recent games</button>
        <button class="btn" id="stats-btn">📈 Stats</button>
      </div>
    </div>`;

  const brawlers = Array.isArray(p.brawlers) ? p.brawlers : [];
  let topStreak = null;
  for (const b of brawlers) {
    const s = streakOf(b);
    if (s !== null) topStreak = topStreak === null ? s : Math.max(topStreak, s);
  }

  const grid1 = `<div class="grid">
    ${stat("Trophies", num(p.trophies), `Highest ${num(p.highestTrophies)}`, "gold")}
    ${stat("Ranked", esc(p.rankedRankName || "—"), p.rankedElo != null ? `${num(p.rankedElo)} elo` : "", "purple")}
    ${stat("Ranked peak (all time)", esc(p.highestAllTimeRankedRankName || "—"), p.highestAllTimeRankedElo != null ? `${num(p.highestAllTimeRankedElo)} elo` : "", "purple")}
    ${stat("Ranked peak (this season)", esc(p.highestSeasonRankedRankName || "—"), p.highestSeasonRankedElo != null ? `${num(p.highestSeasonRankedElo)} elo` : "")}
  </div>`;
  const grid2 = `<div class="grid">
      ${stat("3v3 victories", num(p["3vs3Victories"]))}
      ${stat("Solo victories", num(p.soloVictories))}
      ${stat("Duo victories", num(p.duoVictories))}
      ${stat("Best win streak", topStreak === null ? "—" : num(topStreak), "across all brawlers")}
      ${stat("Brawlers owned", num(brawlers.length))}
      ${p.fame != null ? stat("Fame", num(p.fame), esc(p.fameTierName || "")) : ""}
    </div>`;

  $("profile-body").innerHTML = header + grid1 + grid2 + `
    <div class="section-head">
      <h2>Brawlers</h2>
      <input id="brawler-filter" type="text" placeholder="Filter brawlers..." autocomplete="off" />
    </div>
    <div class="table-wrap"><table id="brawler-table"></table></div>
    <details class="rawwrap"><summary>Raw player data</summary>
      <div class="raw">${esc(JSON.stringify(p, null, 2))}</div>
    </details>`;

  setupBrawlerTable(brawlers);

  $("fav-btn").onclick = async () => {
    const btn = $("fav-btn");
    btn.disabled = true;
    try {
      if (favTags.has(currentProfile.tag)) {
        await invoke("remove_favorite", { tag: currentProfile.tag });
        favTags.delete(currentProfile.tag);
        toast("Removed from favorites");
      } else {
        await invoke("add_favorite", { tag: currentProfile.tag, name: currentProfile.name });
        favTags.add(currentProfile.tag);
        toast("Added to favorites. It will be tracked from now on.");
      }
      const on = favTags.has(currentProfile.tag);
      btn.classList.toggle("on", on);
      btn.textContent = on ? "★ Favorited" : "☆ Favorite";
    } catch (e) { toast(errText(e), true); }
    btn.disabled = false;
  };
  $("recent-btn").onclick = () => openRecent(currentProfile.tag, "profile");
  $("stats-btn").onclick = () => { statsTag = currentProfile.tag; statsBackTo = "profile"; showPage("stats"); };
}

/* ---------- brawler table ---------- */

const COLUMNS = [
  { key: "name", label: "Brawler", get: (b) => (b.name || "").toLowerCase(), cell: (b) => `${brIcon(b.id, b.name, 24)}${esc(b.name)}`, cls: "name" },
  { key: "power", label: "Power", get: (b) => b.power ?? 0, cell: (b) => num(b.power), cls: "num" },
  { key: "rank", label: "Rank", get: (b) => b.rank ?? 0, cell: (b) => num(b.rank), cls: "num" },
  { key: "trophies", label: "Trophies", get: (b) => b.trophies ?? 0, cell: (b) => num(b.trophies), cls: "num" },
  { key: "highestTrophies", label: "Highest", get: (b) => b.highestTrophies ?? 0, cell: (b) => num(b.highestTrophies), cls: "num" },
  { key: "streak", label: "Best streak", get: (b) => streakOf(b) ?? -1, cell: (b) => { const s = streakOf(b); return s === null ? "—" : num(s); }, cls: "num" },
  { key: "gadgets", label: "Gadgets", get: (b) => (b.gadgets || []).length, cell: (b) => (b.gadgets || []).length, cls: "num" },
  { key: "starPowers", label: "Star Powers", get: (b) => (b.starPowers || []).length, cell: (b) => (b.starPowers || []).length, cls: "num" },
  { key: "gears", label: "Gears", get: (b) => (b.gears || []).length, cell: (b) => (b.gears || []).length, cls: "num" },
];

function setupBrawlerTable(brawlers) {
  const table = $("brawler-table");
  const filter = $("brawler-filter");
  let sortKey = "trophies";
  let sortDir = -1;
  const open = new Set();

  function draw() {
    const q = filter.value.trim().toLowerCase();
    const col = COLUMNS.find((c) => c.key === sortKey);
    const rows = brawlers
      .filter((b) => !q || (b.name || "").toLowerCase().includes(q))
      .sort((a, b) => {
        const x = col.get(a), y = col.get(b);
        return (x < y ? -1 : x > y ? 1 : 0) * sortDir;
      });

    const head = COLUMNS.map((c) =>
      `<th data-key="${c.key}">${esc(c.label)}${c.key === sortKey ? `<span class="arrow">${sortDir < 0 ? "▼" : "▲"}</span>` : ""}</th>`
    ).join("");

    const body = rows.map((b) => {
      const id = String(b.id);
      const cells = COLUMNS.map((c) => `<td class="${c.cls}">${c.cell(b)}</td>`).join("");
      let html = `<tr class="row" data-id="${esc(id)}">${cells}</tr>`;
      if (open.has(id)) {
        const streakFields = Object.entries(b)
          .filter(([k, v]) => /streak/i.test(k) && typeof v === "number")
          .map(([k, v]) => `<span class="chip">${esc(k)}: ${num(v)}</span>`).join("");
        const skin = b.skin && b.skin.name ? `<span class="chip">${esc(b.skin.name)}</span>` : '<span class="none">Not reported</span>';
        html += `<tr class="detail"><td colspan="${COLUMNS.length}">
          <div class="detail-grid">
            <div><h4>Gadgets</h4>${chips(b.gadgets)}</div>
            <div><h4>Star Powers</h4>${chips(b.starPowers)}</div>
            <div><h4>Gears</h4>${chips(b.gears)}</div>
            <div><h4>Hypercharges</h4>${chips(b.hyperCharges)}</div>
            <div><h4>Skin</h4>${skin}</div>
            <div><h4>Streaks</h4>${streakFields || '<span class="none">Not reported</span>'}</div>
          </div>
          <div class="raw">${esc(JSON.stringify(b, null, 2))}</div>
        </td></tr>`;
      }
      return html;
    }).join("");

    table.innerHTML = `<thead><tr>${head}</tr></thead><tbody>${body ||
      `<tr><td colspan="${COLUMNS.length}" class="none" style="padding:18px">No brawlers match.</td></tr>`}</tbody>`;
  }

  table.onclick = (ev) => {
    const th = ev.target.closest("th");
    if (th) {
      const key = th.dataset.key;
      if (key === sortKey) sortDir *= -1;
      else { sortKey = key; sortDir = key === "name" ? 1 : -1; }
      draw();
      return;
    }
    const row = ev.target.closest("tr.row");
    if (row) {
      const id = row.dataset.id;
      open.has(id) ? open.delete(id) : open.add(id);
      draw();
    }
  };
  filter.oninput = draw;
  draw();
}

/* ============================== favorites ============================== */

async function renderFavorites() {
  const box = $("fav-list");
  const favs = await refreshFavTags();
  if (currentPage !== "favorites") return;

  if (!favs.length) {
    box.innerHTML = `<div class="empty">No favorites yet.<br>Open a player's profile and click <b>☆ Favorite</b>.</div>`;
    return;
  }

  box.innerHTML = favs.map((f) => {
    const st = f.state && f.state.initialized ? f.state : null;
    const status = st ? statusOf(st.last_battle_time) : null;
    const pill = !st ? `<span class="pill idle">Checking...</span>`
      : status.active ? `<span class="pill live">Playing now</span>` : `<span class="pill idle">Idle</span>`;
    return `<div class="fav-card">
      <div class="top">
        <div><div class="nm">${esc(f.name)}</div><div class="tg">#${esc(f.tag)}</div></div>
        ${pill}
      </div>
      <div class="meta">
        <div><div class="l">Trophies</div><div class="v">${st ? num(st.trophies) : "—"}</div></div>
        <div><div class="l">Ranked</div><div class="v">${st && st.ranked_rank_name ? esc(st.ranked_rank_name) : "—"}</div></div>
        <div><div class="l">Elo</div><div class="v">${st && st.ranked_elo != null ? num(st.ranked_elo) : "—"}</div></div>
        <div><div class="l">Status</div><div class="v" style="font-weight:600;font-size:12.5px">${st ? esc(status.text) : "Waiting for first check"}</div></div>
      </div>
      <div class="actions">
        <button class="btn" data-act="profile" data-tag="${esc(f.tag)}">Profile</button>
        <button class="btn" data-act="recent" data-tag="${esc(f.tag)}">Recent games</button>
        <button class="btn ghost" data-act="remove" data-tag="${esc(f.tag)}">Remove</button>
      </div>
    </div>`;
  }).join("");
}

$("fav-list").onclick = async (ev) => {
  const btn = ev.target.closest("[data-act]");
  if (!btn) return;
  const tag = btn.dataset.tag;
  if (btn.dataset.act === "profile") openProfile(tag, "favorites");
  if (btn.dataset.act === "recent") openRecent(tag, "favorites");
  if (btn.dataset.act === "remove") {
    try { await invoke("remove_favorite", { tag }); toast("Removed from favorites"); renderFavorites(); }
    catch (e) { toast(errText(e), true); }
  }
};

/* ============================ recent games ============================ */

let recentTag = null;

function openRecent(tag) {
  recentTag = normTag(tag);
  showPage("recent");
}

async function renderRecentPage() {
  const favs = await refreshFavTags();
  const me = normTag(lastSettings.player_tag);
  const opts = [];
  if (me) opts.push({ tag: me, label: "Me" });
  favs.forEach((f) => { if (f.tag !== me) opts.push({ tag: f.tag, label: f.name || "#" + f.tag }); });
  if (recentTag && !opts.some((o) => o.tag === recentTag)) opts.push({ tag: recentTag, label: "#" + recentTag });
  if (!recentTag && opts.length) recentTag = opts[0].tag;

  $("recent-select").innerHTML = opts.map((o) =>
    `<option value="${esc(o.tag)}" ${o.tag === recentTag ? "selected" : ""}>${esc(o.label)}</option>`).join("")
    || `<option value="">No players yet</option>`;

  if (recentTag) loadRecent(recentTag);
  else $("recent-list").innerHTML = `<div class="notice">Set your player tag in Settings, or type a tag above.</div>`;
}

$("recent-select").onchange = (ev) => { recentTag = ev.target.value; loadRecent(recentTag); };
$("recent-form").onsubmit = (ev) => {
  ev.preventDefault();
  const t = normTag($("recent-input").value);
  if (!t) return;
  recentTag = t;
  $("recent-input").value = "";
  renderRecentPage();
};
$("recent-refresh").onclick = () => { if (recentTag) loadRecent(recentTag); };

let recentToken = 0;

async function loadRecent(tag) {
  const token = ++recentToken;
  $("recent-error").classList.add("hidden");
  showSkeleton($("recent-list"), skel.cards(4));
  try {
    const r = await invoke("get_matches", { tag });
    if (token !== recentToken) return;
    renderMatches(r);
  } catch (e) {
    if (token !== recentToken) return;
    $("recent-list").innerHTML = "";
    $("recent-error").textContent = errText(e);
    $("recent-error").classList.remove("hidden");
  } finally {
    if (token === recentToken) $("recent-loading").classList.add("hidden"); // (spinner no longer shown; skeleton rows are)
  }
}

function deltaBadge(m, tracked) {
  if (m.ranked) {
    if (typeof m.elo_change === "number") {
      const cls = m.elo_change > 0 ? "pos" : m.elo_change < 0 ? "neg" : "";
      const tip = m.elo_before != null && m.elo_after != null ? `${num(m.elo_before)} → ${num(m.elo_after)}` : "";
      return `<span class="delta ${cls}" title="${esc(tip)}">${sign(m.elo_change)} elo</span>`;
    }
    if (m.in_progress) return `<span class="delta na">in progress</span>`;
    if (!m.complete) return `<span class="delta na" title="Only part of this series is in the log.">partial</span>`;
    const why = tracked
      ? "Played before tracking started, or while the app was closed."
      : "Add this player to Favorites to start tracking elo.";
    return `<span class="delta na" title="${esc(why)}">elo —</span>`;
  }
  return "";
}

function playerBtn(p) {
  const bt = p.brawler ? `${brIcon(p.id, p.brawler, 20)}<span class="br">${esc(p.brawler)}</span>` : "";
  return `<button class="pl ${p.me ? "me" : ""}"
      data-ptag="${esc(normTag(p.tag))}" data-pname="${esc(p.name)}" data-brawler="${esc(p.brawler)}"
      data-power="${esc(p.power || "")}" data-btroph="${esc(p.trophies || "")}" data-star="${p.star ? "1" : ""}" data-bid="${esc(p.id || "")}">
      ${p.star ? '<span class="star" title="Star Player">★</span>' : ""}
      <span class="pn">${esc(p.name)}</span>${bt}
    </button>`;
}

function card_ranked(el) {
  const c = el.closest(".battle");
  return !!(c && c.dataset.ranked);
}

function fmtDur(secs) {
  return secs ? `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}` : "";
}

function matchDetail(m) {
  const when = new Date(m.end * 1000).toLocaleString([], { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
  const total = m.rounds.reduce((s, r) => s + (r.duration || 0), 0);
  const me = (m.teams[m.my_team] || []).find((p) => p.me);
  const fact = (l, v) => `<div class="fact"><div class="l">${esc(l)}</div><div class="v">${v}</div></div>`;
  const elo = m.ranked
    ? (typeof m.elo_change === "number"
        ? `${sign(m.elo_change)}${m.elo_before != null && m.elo_after != null ? ` <span class="muted small">(${num(m.elo_before)} → ${num(m.elo_after)})</span>` : ""}`
        : "—")
    : "";
  const rounds = m.rounds.length > 1 || m.ranked
    ? `<table class="rounds-tbl"><thead><tr><th>Round</th><th>Result</th><th>Length</th></tr></thead><tbody>${
        m.rounds.map((r, i) => `<tr><td>${i + 1}</td><td class="${r.result === "victory" ? "g" : r.result === "defeat" ? "b" : ""}">${esc(r.result || "—")}</td><td>${esc(fmtDur(r.duration) || "—")}</td></tr>`).join("")
      }</tbody></table>`
    : "";
  return `<div class="m-detail hidden">
    <div class="m-map">${mapImg(m.event_id, m.map)}<div class="m-map-name">${esc(m.map || "Unknown map")}</div></div>
    <div class="m-facts">
      ${fact("Mode", esc(modeName(m.mode)))}
      ${fact("Type", m.ranked ? "Ranked" : esc(modeName(m.kind || "game")))}
      ${fact("Played", esc(when))}
      ${fact("Result", esc(m.result || "—"))}
      ${m.ranked ? fact("Rounds", `${m.wins}–${m.losses}`) : ""}
      ${total ? fact("Time played", esc(fmtDur(total))) : ""}
      ${m.ranked ? fact("Elo change", elo) : ""}
      ${me ? fact("Your brawler", `${brIcon(me.id, me.brawler, 20)}${esc(me.brawler)} · power ${esc(me.power)}`) : ""}
      ${m.star ? fact("Star player", `★ ${esc(m.star.name)}${m.star.brawler ? ` <span class="muted small">(${esc(m.star.brawler)})</span>` : ""}`) : ""}
    </div>
    ${rounds}
  </div>`;
}

function matchCard(m, tracked) {
  const cls = m.result === "victory" ? "win" : m.result === "defeat" ? "loss" : "draw";
  const when = m.end * 1000;
  const label = m.ranked
    ? (m.in_progress || !m.complete ? "Series" : m.result === "victory" ? "Victory" : m.result === "defeat" ? "Defeat" : "Draw")
    : (m.result === "victory" ? "Victory" : m.result === "defeat" ? "Defeat" : m.result === "draw" ? "Draw" : "Played");
  const score = m.ranked ? `<span class="score" title="Rounds won – lost">${m.wins}–${m.losses}</span>` : "";
  const rounds = m.ranked && m.rounds.length
    ? `<span class="rounds">${m.rounds.map((r) => `<i class="rd ${r.result === "victory" ? "w" : r.result === "defeat" ? "l" : ""}" title="${esc(r.result)} · ${esc(fmtDur(r.duration))}"></i>`).join("")}</span>`
    : "";

  const teams = (m.teams || []).slice();
  const mine = m.my_team != null ? m.my_team : -1;
  const order = teams.map((t, i) => i).sort((x, y) => (x === mine ? -1 : y === mine ? 1 : 0));
  const teamsHtml = teams.length
    ? `<div class="teams">${order.map((i) => `
      <div class="team ${i === mine ? "mine" : ""}">
        <h5>${teams.length === 1 ? "Players" : i === mine ? "Team" : "Opponents"}</h5>
        ${teams[i].map(playerBtn).join("")}
      </div>`).join("")}</div>`
    : "";

  const me = mine >= 0 ? (teams[mine] || []).find((p) => p.me) : null;
  const brawler = me && me.brawler ? `<span class="b-br" title="${esc(me.brawler)}">${brIcon(me.id, me.brawler, 26)}</span>` : "";
  return `<div class="battle ${cls}" ${m.ranked ? 'data-ranked="1"' : ""}>
    <div class="b-head" data-toggle title="Click for full match details">
      ${brawler}<span class="b-res">${label}</span>${score}${rounds}
      ${deltaBadge(m, tracked)}
      <span class="b-mode">${m.ranked ? '<span class="pill gold">Ranked</span> ' : ""}${esc(modeName(m.mode))} · ${esc(m.map || "?")}</span>
      <span class="b-when">${esc(ago(when))} · ${esc(new Date(when).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }))}</span>
    </div>
    ${matchDetail(m)}
    ${teamsHtml}
    <div class="pl-detail hidden"></div>
  </div>`;
}

function renderMatches(r) {
  let html = r.tracked
    ? `<div class="notice">Elo changes are worked out from elo readings taken while this app is running, so games played before tracking started (or while the app was closed) show “elo —”.</div>`
    : `<div class="notice">The Brawl Stars API doesn't report elo changes. This app works them out by watching a player's elo, so it can only show them for tracked players. Add this player to <b>Favorites</b> (or set them as you in Settings) to start tracking.</div>`;
  html += (r.matches || []).length
    ? r.matches.map((m) => matchCard(m, r.tracked)).join("")
    : `<div class="empty" style="margin-top:16px">No recent games.</div>`;
  $("recent-list").innerHTML = html;
}

$("recent-list").onclick = (ev) => {
  const open = ev.target.closest("[data-open]");
  if (open) { openProfile(open.dataset.open, "recent"); return; }

  const head = ev.target.closest("[data-toggle]");
  if (head) {
    const panel = head.closest(".battle").querySelector(".m-detail");
    panel.classList.toggle("hidden");
    head.classList.toggle("open", !panel.classList.contains("hidden"));
    return;
  }

  const pl = ev.target.closest(".pl");
  if (!pl) return;
  const card = pl.closest(".battle");
  const box = card.querySelector(".pl-detail");
  const wasOpen = pl.classList.contains("open");
  card.querySelectorAll(".pl.open").forEach((x) => x.classList.remove("open"));
  if (wasOpen) { box.classList.add("hidden"); return; }

  pl.classList.add("open");
  const d = pl.dataset;
  box.innerHTML = `
    <strong>${esc(d.pname)}</strong><span class="muted">#${esc(d.ptag)}</span>
    <span>Brawler: <b>${esc(d.brawler)}</b></span>
    ${d.power ? `<span>Power <b>${esc(d.power)}</b></span>` : ""}
    ${d.btroph ? `<span>Brawler ${card_ranked(pl) ? "tier" : "trophies"} <b>${esc(d.btroph)}</b></span>` : ""}
    ${d.star ? '<span class="pill gold">★ Star Player</span>' : ""}
    <button class="btn" data-open="${esc(d.ptag)}">View profile</button>`;
  box.classList.remove("hidden");
};

/* ============================ stats & graphs ============================ */

let statsTag = null;
let statsBackTo = null;
let statsToken = 0;

async function playerOptions(selectedTag) {
  const favs = await refreshFavTags();
  const me = normTag(lastSettings.player_tag);
  const opts = [];
  if (me) opts.push({ tag: me, label: "Me" });
  favs.forEach((f) => { if (f.tag !== me) opts.push({ tag: f.tag, label: f.name || "#" + f.tag }); });
  if (selectedTag && !opts.some((o) => o.tag === selectedTag)) opts.push({ tag: selectedTag, label: "#" + selectedTag });
  return opts;
}

// Step chart: a rating holds its value until the next reading. A value of 0 breaks the line.
let chartGeom = null; // geometry of the chart on screen, for the hover crosshair

// Reading in force at time t (the graph is a step line).
function valueAt(pts, t) {
  let v = pts[0][1];
  for (const p of pts) { if (p[0] > t) break; v = p[1]; }
  return v;
}

function lineChart(points, { color, unit, metric, foot }) {
  const pts = points.filter((p) => p[1] > 0);
  if (pts.length < 2) { chartGeom = null; return `<div class="empty small">Not enough readings yet. Keep the app running while you play.</div>`; }
  const W = 760, H = 220, L = 54, R = 12, T = 12, B = 26;
  const t0 = pts[0][0], t1 = pts[pts.length - 1][0];
  let lo = Math.min(...pts.map((p) => p[1])), hi = Math.max(...pts.map((p) => p[1]));
  const last = pts[pts.length - 1];
  const fpts = (foot || pts).filter((p) => p[1] > 0); // what the low/high in the footer cover

  // The nearest unfinished goal for this metric draws a dotted target line, if it is within reach.
  const goal = (statsGoals || []).filter((g) => g.metric === metric && g.progress.status !== "reached" && g.target > last[1]).sort((a, b) => a.target - b.target)[0];
  const span0 = Math.max(1, hi - lo);
  const showGoal = goal && goal.target - hi <= span0 * 3;
  if (showGoal) hi = Math.max(hi, goal.target);
  if (hi === lo) { hi += 1; lo -= 1; }
  const pad = (hi - lo) * 0.08; lo -= pad; hi += pad;
  const x = (t) => L + ((t - t0) / Math.max(1, t1 - t0)) * (W - L - R);
  const y = (v) => T + (1 - (v - lo) / (hi - lo)) * (H - T - B);

  let d = "", prev = null;
  for (const [t, v] of points) {
    if (v <= 0) { prev = null; continue; }
    d += prev === null ? `M${x(t).toFixed(1)} ${y(v).toFixed(1)}` : `H${x(t).toFixed(1)}V${y(v).toFixed(1)}`;
    prev = v;
  }
  const ticks = [0, 0.5, 1].map((f) => lo + (hi - lo) * f);
  const grid = ticks.map((v) => `<line x1="${L}" x2="${W - R}" y1="${y(v)}" y2="${y(v)}" class="gl"/><text x="${L - 8}" y="${y(v) + 4}" text-anchor="end" class="gt">${Math.round(v).toLocaleString()}</text>`).join("");
  const fmt = (t) => statsRange === "today"
    ? new Date(t * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : new Date(t * 1000).toLocaleDateString([], { month: "short", day: "numeric" });

  // Win/loss dots where games finished (elo graph: ranked games; trophy graph: the others).
  const marks = ((statsData && statsData.marks) || []).filter((g) => g[1] !== 0 && g[0] >= t0 && g[0] <= t1 && (metric === "elo" ? g[2] === 1 : g[2] === 0));
  const dots = marks.length && marks.length <= 150
    ? marks.map((g) => `<circle class="mk ${g[1] > 0 ? "w" : "l"}" cx="${x(g[0]).toFixed(1)}" cy="${y(valueAt(pts, g[0])).toFixed(1)}" r="3"/>`).join("") : "";
  const goalSvg = showGoal
    ? `<line class="goal-line" x1="${L}" x2="${W - R}" y1="${y(goal.target)}" y2="${y(goal.target)}"/><text class="goal-tag" x="${W - R}" y="${y(goal.target) - 5}" text-anchor="end">Goal ${goal.target.toLocaleString()}</text>` : "";

  chartGeom = { W, L, R, t0, t1, pts, x, y, unit, fmt: (t) => new Date(t * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }) };
  return `<svg viewBox="0 0 ${W} ${H}" class="chart" role="img" aria-label="${esc(unit)} over time">
    ${grid}${goalSvg}
    <text x="${L}" y="${H - 6}" class="gt">${esc(fmt(t0))}</text>
    <text x="${W - R}" y="${H - 6}" text-anchor="end" class="gt">${esc(fmt(t1))}</text>
    <path class="line" pathLength="1" d="${d}" fill="none" style="stroke:${color}" stroke-width="2.2" stroke-linejoin="round"/>
    ${dots}
    <circle cx="${x(last[0])}" cy="${y(last[1])}" r="4" style="fill:${color}"/>
    <g class="xh" style="display:none"><line y1="${T}" y2="${H - B}"/><circle r="5" style="fill:${color}"/></g>
  </svg>
  <div class="chart-foot muted small">Latest ${num(last[1])} ${esc(unit)} · low ${num(Math.min(...fpts.map((p) => p[1])))} · high ${num(Math.max(...fpts.map((p) => p[1])))}${dots ? `<span class="chart-key"><span><i style="background:var(--good)"></i>win</span><span><i style="background:var(--bad)"></i>loss</span></span>` : ""}</div>`;
}

// Hover crosshair: snaps to the nearest reading and shows its time and value.
(() => {
  const tip = document.createElement("div");
  tip.className = "hm-tip"; tip.hidden = true; document.body.appendChild(tip);
  const body = $("stats-body");
  const hide = (svg) => { tip.hidden = true; const g = svg && svg.querySelector(".xh"); if (g) g.style.display = "none"; };
  body.addEventListener("mousemove", (ev) => {
    const svg = ev.target.closest && ev.target.closest("svg.chart");
    if (!svg || !chartGeom) return;
    const r = svg.getBoundingClientRect(), vx = ((ev.clientX - r.left) / r.width) * chartGeom.W;
    if (vx < chartGeom.L - 4 || vx > chartGeom.W - chartGeom.R + 4) { hide(svg); return; }
    const t = chartGeom.t0 + ((vx - chartGeom.L) / (chartGeom.W - chartGeom.L - chartGeom.R)) * (chartGeom.t1 - chartGeom.t0);
    const p = chartGeom.pts.reduce((b, q) => (Math.abs(q[0] - t) < Math.abs(b[0] - t) ? q : b));
    const cx = chartGeom.x(p[0]), cy = chartGeom.y(p[1]);
    const g = svg.querySelector(".xh");
    g.style.display = ""; g.querySelector("line").setAttribute("x1", cx); g.querySelector("line").setAttribute("x2", cx);
    g.querySelector("circle").setAttribute("cx", cx); g.querySelector("circle").setAttribute("cy", cy);
    tip.textContent = `${num(p[1])} ${chartGeom.unit} · ${chartGeom.fmt(p[0])}`;
    tip.hidden = false;
    tip.style.left = Math.min(window.innerWidth - tip.offsetWidth - 8, Math.max(8, ev.clientX - tip.offsetWidth / 2)) + "px";
    tip.style.top = r.top + (cy / 220) * r.height - tip.offsetHeight - 14 + "px";
  });
  body.addEventListener("mouseleave", () => hide(body.querySelector("svg.chart")));
  body.addEventListener("mouseover", (ev) => { if (!ev.target.closest || !ev.target.closest("svg.chart")) hide(body.querySelector("svg.chart")); });
})();

function rateTable(title, rows, withIcons) {
  if (!rows.length) return "";
  const body = rows.slice(0, 12).map((r) => {
    const n = r.wins + r.losses;
    const pct = n ? Math.round((r.wins / n) * 100) : 0;
    return `<tr><td class="name">${withIcons ? brIcon(r.id, r.name, 22) : ""}${esc(r.name)}</td>
      <td class="num">${r.wins}–${r.losses}</td>
      <td class="num"><span class="bar"><i style="width:${pct}%"></i></span> ${n ? pct + "%" : "—"}</td></tr>`;
  }).join("");
  return `<div><h2>${esc(title)}</h2><div class="table-wrap"><table><thead><tr><th>Name</th><th>W–L</th><th>Win rate</th></tr></thead><tbody>${body}</tbody></table></div></div>`;
}

function sessionRows(list) {
  if (!list.length) return `<div class="empty small">No sessions recorded yet.</div>`;
  return `<div class="activity">${list.map((s) => {
    const when = new Date(s.start * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
    const mins = Math.max(1, Math.round((s.end - s.start) / 60));
    const badge = typeof s.elo_net === "number"
      ? `<span class="delta ${s.elo_net > 0 ? "pos" : s.elo_net < 0 ? "neg" : ""}">${sign(s.elo_net)} elo</span>` : "";
    return `<div class="a-row"><span class="a-time">${esc(when)}</span><span class="a-dot ${s.wins >= s.losses ? "victory" : "defeat"}"></span>
      <span class="a-text">${s.matches} game${s.matches === 1 ? "" : "s"} · ${s.wins}W–${s.losses}L · ${mins} min</span>${badge}</div>`;
  }).join("")}</div>`;
}

/* ---- overview: tiles, daily heat map, trophy / elo graph ---- */
let statsData = null, statsMetric = "trophies", statsRange = "all", heatMode = "battles", clearArmed = false;
const startOfDay = (d) => { const x = new Date(d); x.setHours(0, 0, 0, 0); return x; };
let selDay = startOfDay(new Date()); // the day shown in the Day summary card
const DAY = 86400;
// Heat-map colours follow the theme: a theme colour at some opacity.
const mix = (v, a) => `color-mix(in srgb, var(${v}) ${Math.round(a * 100)}%, transparent)`;
const dayKey = (d) => `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;

// Last 40 weeks as columns (Sunday on top). Each day: battles, wins, losses.
function heatMap(days, mode) {
  const byDay = new Map();
  for (const [end, res] of days) {
    const k = dayKey(new Date(end * 1000));
    const c = byDay.get(k) || { n: 0, w: 0, l: 0 };
    c.n++; if (res > 0) c.w++; else if (res < 0) c.l++;
    byDay.set(k, c);
  }
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const WEEKS = 40;
  const start = new Date(today); start.setDate(start.getDate() - today.getDay() - (WEEKS - 1) * 7);
  const maxN = Math.max(1, ...[...byDay.values()].map((c) => c.n));
  let cols = "", months = "", lastMonth = -1;
  for (let w = 0; w < WEEKS; w++) {
    let cells = "";
    for (let d = 0; d < 7; d++) {
      const day = new Date(start); day.setDate(start.getDate() + w * 7 + d);
      if (day > today) { cells += `<i class="hm off"></i>`; continue; }
      const c = byDay.get(dayKey(day));
      const label = day.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
      let style = "", tip = `${label}: no battles`;
      if (c) {
        const rate = Math.round((c.w / Math.max(1, c.w + c.l)) * 100);
        const a = (0.25 + 0.75 * (c.n / maxN)).toFixed(2);
        if (mode === "battles") style = `background:${mix("--accent", a)}`;
        else style = c.w > c.l ? `background:${mix("--good", a)}` : c.w < c.l ? `background:${mix("--bad", a)}` : `background:${mix("--muted", a)}`;
        tip = `${label}: ${c.n} battle${c.n === 1 ? "" : "s"} · ${c.w}W–${c.l}L · ${rate}% win rate`;
      }
      const dk = dayKey(day);
      cells += `<i class="hm pick${dk === dayKey(selDay) ? " sel" : ""}" style="${style}" data-day="${dk}" data-tip="${esc(tip + " (click for the day's summary)")}"></i>`;
    }
    const m = new Date(start.getTime() + w * 7 * DAY * 1000).getMonth();
    months += `<span>${m !== lastMonth ? new Date(start.getTime() + w * 7 * DAY * 1000).toLocaleDateString([], { month: "short" }) : ""}</span>`;
    lastMonth = m;
    cols += `<div class="hm-col">${cells}</div>`;
  }
  const sw = (v, a) => `<i class="hm" style="background:${mix(v, a)}"></i>`;
  const legend = mode === "battles"
    ? `<span class="hm-key">${sw("--accent", 0.25)} few ${sw("--accent", 1)} many battles</span>`
    : `<span class="hm-key">${sw("--good", 0.8)} more wins ${sw("--muted", 0.8)} even ${sw("--bad", 0.8)} more losses</span>`;
  return `<div class="hm-wrap"><div class="hm-months">${months}</div><div class="hm-grid">${cols}</div></div><div class="chart-foot muted small">${legend}</div>`;
}

function statsTiles(s) {
  const dec = s.wins + s.losses;
  const rate = dec ? Math.round((s.wins / dec) * 100) + "%" : "—";
  const cs = s.current_streak;
  const streak = cs === 0 ? "—" : cs > 0 ? `${cs}W` : `${-cs}L`;
  const star = s.total ? Math.round((s.star_count / s.total) * 100) + "%" : "—";
  const since = s.since ? "since " + new Date(s.since * 1000).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" }) : "";
  const avg = s.avg_trophies == null ? "—" : (s.avg_trophies > 0 ? "+" : "") + s.avg_trophies.toFixed(1);
  const rk = s.ranked_wins + s.ranked_losses;
  return `<div class="grid">
    ${stat("Battles", num(s.total), since)}
    ${stat("Win rate", rate, `${num(s.wins)}W – ${num(s.losses)}L`, "gold")}
    ${stat("Current streak", streak, cs > 0 ? "winning" : cs < 0 ? "losing" : "no games yet", cs > 0 ? "green" : "")}
    ${stat("Best win streak", num(s.best_win_streak), "in saved games")}
    ${stat("Star player", star, `${num(s.star_count)} time${s.star_count === 1 ? "" : "s"}`)}
    ${stat("Avg trophies / game", avg, "non-ranked games", "gold")}
    ${stat("Ranked record", rk ? `${s.ranked_wins}–${s.ranked_losses}` : "—", rk ? Math.round((s.ranked_wins / rk) * 100) + "% win rate" : "no ranked games", "purple")}
    ${stat("Net elo", s.elo_net == null ? "—" : sign(s.elo_net), "from elo readings", "purple")}
  </div>`;
}

function rangeCutoff() {
  if (statsRange === "all") return 0;
  if (statsRange === "today") { const d = new Date(); d.setHours(0, 0, 0, 0); return Math.floor(d.getTime() / 1000); }
  return Math.floor(Date.now() / 1000) - Number(statsRange) * DAY;
}

// Readings inside the range. The reading just before the start is kept (moved to the start) so
// the line begins at the right height; "today" is also extended to now.
function seriesForRange(pts) {
  const cutoff = rangeCutoff();
  if (!cutoff || !pts.length) return pts;
  const valid = pts.filter((p) => p[1] > 0);
  const before = [...valid].reverse().find((p) => p[0] < cutoff);
  const inside = pts.filter((p) => p[0] >= cutoff);
  const out = before ? [[cutoff, before[1]], ...inside] : inside;
  if (statsRange === "today" && out.length) { const last = [...out].reverse().find((p) => p[1] > 0); if (last) out.push([Math.floor(Date.now() / 1000), last[1]]); }
  return out;
}

// "+25 elo gained today", from the first to the last reading in the range.
function gainLine(pts, unit, note = "") {
  const v = pts.filter((p) => p[1] > 0);
  if (v.length < 2) return "";
  const d = v[v.length - 1][1] - v[0][1];
  const word = { today: "today", "7": "in the last 7 days", "30": "in the last 30 days", all: "overall" }[statsRange];
  return `<div class="gain ${d > 0 ? "pos" : d < 0 ? "neg" : ""}"><b>${d > 0 ? "+" : ""}${num(d)}</b> ${esc(unit)} ${esc(word)}${note ? ` <span class="muted small">${esc(note)}</span>` : ""}</div>`;
}

// The elo series has a 0 marker wherever a ranked season reset breaks the line. This is everything after the last one.
function thisSeason(pts) {
  let i = pts.length;
  while (i > 0 && pts[i - 1][1] > 0) i--;
  return pts.slice(i);
}

function chartSection() {
  const r = statsData;
  const elo = statsMetric === "elo";
  let ranged = seriesForRange(elo ? r.series.elo : r.series.trophies);
  // A season reset moves everyone's elo, so a range that spans one must not count the drop as a loss or
  // take last season's final elo as the "high". Numbers (and, except for All time, the line) cover this season only.
  let scope = null, resetNote = "";
  if (elo) {
    const cur = thisSeason(ranged);
    if (cur.length && cur.length < ranged.filter((p) => p[1] > 0).length) {
      scope = cur;
      resetNote = "this season only";
      if (statsRange !== "all") ranged = cur;
    }
  }
  const seg = (cls, key, cur, items) => `<div class="seg" data-seg="${key}">${items.map(([v, l]) =>
    `<button class="${cur === v ? "on" : ""}" data-v="${v}">${l}</button>`).join("")}</div>`;
  return `<div class="section-head"><h2>${elo ? "Ranked elo" : "Trophies"}</h2><div class="seg-row">
      ${seg("", "metric", statsMetric, [["trophies", "Trophies"], ["elo", "Ranked elo"]])}
      ${seg("", "range", statsRange, [["today", "Today"], ["7", "7 days"], ["30", "30 days"], ["all", "All time"]])}</div></div>
    ${elo ? seasonBanner(r.seasons && r.seasons[0]) : ""}
    <div class="card">${gainLine(scope || ranged, elo ? "elo" : "trophies", resetNote)}${lineChart(ranged,
      elo ? { color: "var(--elo)", unit: "elo", metric: "elo", foot: scope } : { color: "var(--accent)", unit: "trophies", metric: "trophies" })}${scope ? `<div class="muted small">A new ranked season began ${esc(fmtDay(scope[0][0]))}. The change and low/high above only count this season.</div>` : ""}</div>`;
}

// Season-wide elo, whatever range the graph is showing: peak, now, how far below the peak, and totals.
function seasonBanner(sn) {
  if (!sn) return "";
  const fromPeak = sn.end_elo - sn.peak, net = sn.end_elo - sn.start_elo;
  const tile = (k, v, sub, cls = "") => `<div class="day-tile"><div class="k">${k}</div><div class="v ${cls}">${v}</div><div class="s muted small">${sub || "&nbsp;"}</div></div>`;
  const partial = sn.wins + sn.losses > sn.elo_games ? `from ${sn.elo_games} of ${sn.wins + sn.losses} ranked games` : `${sn.elo_games} ranked game${sn.elo_games === 1 ? "" : "s"}`;
  return `<div class="card season-banner"><div class="muted small" style="margin-bottom:8px">Season ${esc(sn.id)} · ${esc(fmtDay(sn.from))} – ${esc(fmtDay(sn.to))} (not affected by the range above). Won and lost add up each game's change, so they aren't your net (${partial}).</div>
    <div class="day-tiles">
      ${tile("Season peak", num(sn.peak), `now ${num(sn.end_elo)}`)}
      ${tile("From the peak", fromPeak === 0 ? "at peak" : signed(fromPeak), "now vs season high", tone(fromPeak))}
      ${tile("Since season start", signed(net), `started at ${num(sn.start_elo)}`, tone(net))}
      ${tile("Won in games", sn.elo_games ? "+" + num(sn.gained) : "—", "added up over wins", sn.elo_games ? "pos" : "")}
      ${tile("Lost in games", sn.elo_games ? "−" + num(sn.lost) : "—", "added up over losses", sn.elo_games ? "neg" : "")}
    </div></div>`;
}

/* ---- day summary ---- */

const signed = (n) => (n > 0 ? "+" : "") + num(n);
const tone = (n) => (n > 0 ? "pos" : n < 0 ? "neg" : "");
const sameDay = (a, b) => dayKey(a) === dayKey(b);

function daySection() {
  const today = startOfDay(new Date()), isToday = sameDay(selDay, today);
  const label = isToday ? "Today" : sameDay(selDay, new Date(today.getTime() - DAY * 1000 + 3600e3)) ? "Yesterday"
    : selDay.toLocaleDateString([], { weekday: "long", month: "long", day: "numeric", year: selDay.getFullYear() === today.getFullYear() ? undefined : "numeric" });
  const iso = `${selDay.getFullYear()}-${String(selDay.getMonth() + 1).padStart(2, "0")}-${String(selDay.getDate()).padStart(2, "0")}`;
  return `<div class="section-head"><h2>Day summary <span class="muted day-label">${esc(label)}</span></h2>
    <div class="seg-row day-nav">
      <button class="btn small" data-day-step="-1" title="Previous day" aria-label="Previous day">‹</button>
      <input type="date" id="day-pick" value="${iso}" max="${dayKey2iso(today)}" />
      <button class="btn small" data-day-step="1" title="Next day" aria-label="Next day" ${isToday ? "disabled" : ""}>›</button>
      <button class="btn small" data-day-step="0" ${isToday ? "disabled" : ""}>Today</button>
    </div></div>
    <div class="card" id="day-body"><div class="skel" style="height:120px"></div></div>`;
}
const dayKey2iso = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

let dayToken = 0;
async function loadDay() {
  const box = $("day-body");
  if (!box) return;
  const token = ++dayToken;
  const from = Math.floor(startOfDay(selDay).getTime() / 1000);
  const next = startOfDay(selDay); next.setDate(next.getDate() + 1); // calendar-aware, so daylight saving days come out right
  let d;
  try { d = await invoke("get_day_summary", { tag: statsTag, from, to: Math.floor(next.getTime() / 1000) }); }
  catch (e) { if (token === dayToken) box.innerHTML = `<div class="error">${esc(errText(e))}</div>`; return; }
  if (token !== dayToken || !$("day-body")) return;
  $("day-body").innerHTML = dayBody(d);
}

function dayBody(d) {
  if (!d) return "";
  const isToday = sameDay(selDay, new Date());
  if (!d.games) {
    return `<div class="empty small">${isToday ? "No games saved yet today." : "No games saved for this day."}<br><span class="muted">Only games this app has seen are counted. Pick another day or click one on the activity map.</span></div>`;
  }
  const dec = d.wins + d.losses, rate = dec ? Math.round((d.wins / dec) * 100) : null;
  const when = isToday ? "Today" : "That day";
  const parts = [];
  if (d.trophy_net != null) parts.push(`<b class="${tone(d.trophy_net)}">${signed(d.trophy_net)}</b> trophies`);
  if (d.elo_net != null) parts.push(`<b class="${tone(d.elo_net)}">${signed(d.elo_net)}</b> elo`);
  const net = parts.length ? ` Net <b>${parts.join(" and ")}</b>.` : "";
  const sentence = `${when} you played <b>${d.games}</b> game${d.games === 1 ? "" : "s"}: won <b>${d.wins}</b>, lost <b>${d.losses}</b>${rate != null ? ` (<b>${rate}%</b> win rate)` : ""}.${net}`;
  const tile = (k, v, sub, cls = "") => `<div class="day-tile"><div class="k">${k}</div><div class="v ${cls}">${v}</div><div class="s muted small">${sub || "&nbsp;"}</div></div>`;
  const clock = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const eloSub = d.ranked ? (d.elo_games < d.ranked ? `from ${d.elo_games} of ${d.ranked} ranked games` : `${d.ranked} ranked game${d.ranked === 1 ? "" : "s"}`) : "no ranked games";
  const troSub = d.trophy_net != null ? `${d.trophy_games} game${d.trophy_games === 1 ? "" : "s"} counted` : "none reported";
  const tiles = [
    tile("Games", d.games, d.first && d.last ? `${clock(d.first)} – ${clock(d.last)}` : ""),
    tile("Record", `${d.wins}W – ${d.losses}L`, d.best_streak > 1 ? `best streak ${d.best_streak}` : "", d.wins > d.losses ? "pos" : d.wins < d.losses ? "neg" : ""),
    tile("Win rate", rate == null ? "—" : rate + "%", d.stars ? `${d.stars} star player` : ""),
    tile("Trophies", d.trophy_net == null ? "—" : signed(d.trophy_net), troSub, d.trophy_net == null ? "" : tone(d.trophy_net)),
    tile("Elo", d.elo_net == null ? "—" : signed(d.elo_net), eloSub, d.elo_net == null ? "" : tone(d.elo_net)),
  ].join("");
  const rows = (list) => list.map((r) => { const n = r.wins + r.losses; const p = n ? Math.round((r.wins / n) * 100) : 0;
    return `<span class="day-chip">${r.id ? brIcon(r.id, r.name, 20) : ""}<b>${esc(r.name)}</b> ${r.wins}–${r.losses} <span class="muted">${p}%</span></span>`; }).join("");
  const need = lastSettings.tilt_losses || 3;
  const tilt = isToday && d.loss_streak >= need
    ? `<div class="tilt-note">You've lost your last <b>${d.loss_streak}</b> in a row. A short break (water, stretch, a few minutes) often helps more than another game.</div>` : "";
  return `${tilt}<div class="day-sentence">${sentence}</div><div class="day-tiles">${tiles}</div>
    ${d.by_brawler.length ? `<div class="day-chips"><span class="muted small">Brawlers</span>${rows(d.by_brawler)}</div>` : ""}
    ${d.by_mode.length ? `<div class="day-chips"><span class="muted small">Modes</span>${rows(d.by_mode)}</div>` : ""}`;
}

function seasonsSection(list) {
  if (!list || !list.length) return "";
  const rows = list.map((r, i) => {
    const n = r.wins + r.losses, pct = n ? Math.round((r.wins / n) * 100) : null;
    const span = `${fmtDay(r.from)} – ${fmtDay(r.to)}`;
    const d = r.end_elo - r.start_elo;
    return `<tr><td class="name">Season ${esc(r.id)}${i === 0 && Date.now() / 1000 - r.to < 3 * DAY ? ` <span class="pill">current</span>` : ""}<div class="muted small">${esc(span)}</div></td>
      <td class="num">${num(r.start_elo)} → <b>${num(r.end_elo)}</b><div class="small ${tone(d)}">${d ? signed(d) : "no change"}</div></td>
      <td class="num">${num(r.peak)}<div class="muted small">low ${num(r.low)}</div></td>
      <td class="num">${n ? `${r.wins}–${r.losses}` : "—"}${pct != null ? `<div class="muted small">${pct}%</div>` : ""}</td></tr>`;
  }).join("");
  return `<div class="section-head"><h2>Ranked seasons</h2></div>
    <div class="card"><div class="table-wrap"><table class="season-table"><thead><tr><th>Season</th><th>Elo (first → last)</th><th>Peak</th><th>Ranked W–L</th></tr></thead><tbody>${rows}</tbody></table></div>
    <div class="muted small" style="margin-top:8px">Elo starts from the first reading this app saw in each season, so a season you joined late shows its later start.</div></div>`;
}

function heatSection() {
  const seg = [["battles", "Battles"], ["winrate", "Win rate"]];
  return `<div class="section-head"><h2>Activity</h2><div class="seg-row"><div class="seg" data-seg="heat">${seg.map(([v, l]) =>
    `<button class="${heatMode === v ? "on" : ""}" data-v="${v}">${l}</button>`).join("")}</div></div></div>
    <div class="card">${heatMap(statsData.days || [], heatMode)}</div>`;
}

/* ------------------------------- goals ------------------------------- */

let statsGoals = [];
let goalDraft = { metric: "elo", target: "", date: "" };
const fmtDay = (sec) => new Date(sec * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
const perDay = (v) => `${v > 0 ? "+" : ""}${Math.round(v).toLocaleString()}/day`;

function goalNote(g) {
  const p = g.progress, unit = g.metric === "elo" ? "elo" : "trophies";
  switch (p.status) {
    case "reached": return `🎉 Reached${g.reached_at ? ` on ${fmtDay(g.reached_at)}` : ""}.`;
    case "missed": return `Deadline passed with ${num(p.remaining)} ${unit} to go.`;
    case "no_data": return "Not enough recent readings to estimate a pace yet. Keep playing with the app open.";
    case "stalled": return `No gain over the last two weeks (${perDay(p.per_day)}).`;
    default: {
      const pace = `At ${perDay(p.per_day)}`;
      if (g.deadline) {
        return p.status === "on_track"
          ? `${pace}, you'll make it by ${fmtDay(g.deadline)} (needs ${perDay(p.need_per_day)}).`
          : `${pace} is short of the ${perDay(p.need_per_day)} needed by ${fmtDay(g.deadline)}.`;
      }
      return `${pace}, you'd get there around ${fmtDay(p.eta)}.`;
    }
  }
}

function goalCard(g) {
  const p = g.progress, unit = g.metric === "elo" ? "elo" : "trophies";
  const pct = Math.round(p.pct * 100);
  return `<div class="goal ${esc(p.status)}">
    <div class="goal-top"><b>Reach ${num(g.target)} ${unit}</b>
      <span class="goal-state">${{ reached: "Reached", on_track: "On track", behind: "Behind", missed: "Missed", stalled: "Stalled", no_data: "Waiting for data" }[p.status]}</span>
      <button class="goal-x" data-goal-x="${g.id}" title="Remove goal" aria-label="Remove goal">✕</button></div>
    <div class="goal-bar"><i style="width:${pct}%"></i></div>
    <div class="goal-meta"><span>${p.current != null ? `${num(p.current)} now` : "—"} · started at ${num(g.start)}</span><span>${pct}%${p.remaining ? ` · ${num(p.remaining)} to go` : ""}</span></div>
    <div class="muted small">${esc(goalNote(g))}</div></div>`;
}

function goalsSection() {
  const cards = statsGoals.length ? statsGoals.map(goalCard).join("") : `<div class="muted small">No goals yet. Set a target below and this tracks your progress, your pace and when you'd get there.</div>`;
  return `<div class="section-head"><h2>Goals</h2></div>
    <div class="card goals">${cards}
      <div class="goal-form">
        <select id="goal-metric"><option value="elo" ${goalDraft.metric === "elo" ? "selected" : ""}>Ranked elo</option><option value="trophies" ${goalDraft.metric === "trophies" ? "selected" : ""}>Trophies</option></select>
        <input id="goal-target" type="number" min="1" inputmode="numeric" placeholder="Target" value="${esc(goalDraft.target)}" />
        <input id="goal-date" type="date" title="Optional deadline" value="${esc(goalDraft.date)}" />
        <button class="btn primary" id="goal-add">Add goal</button>
        <span class="inline-msg" id="goal-msg"></span>
      </div></div>`;
}

async function loadGoals() {
  try { statsGoals = (await invoke("get_goals", { tag: statsTag })).goals; } catch (e) { statsGoals = []; }
}

function paintStats() {
  const r = statsData, s = r.summary;
  $("stats-body").innerHTML = `
    <div class="st-tiles">${statsTiles(s)}</div>
    <div class="st-goals">${goalsSection()}</div>
    <div class="st-activity">${heatSection()}</div>
    <div class="st-chart">${chartSection()}</div>
    <div class="st-day">${daySection()}</div>
    <div class="st-seasons">${seasonsSection(r.seasons)}</div>
    <div class="st-rates">
      <div class="section-head"><h2>Win rates</h2></div>
      <div class="two-col">${rateTable("By brawler", s.by_brawler, true)}${rateTable("By mode", s.by_mode, false)}${rateTable("By map", s.by_map, false)}</div>
    </div>
    <div class="st-sessions">
      <div class="section-head"><h2>Recent sessions</h2></div>
      ${sessionRows(r.sessions)}
    </div>
    <div class="st-danger danger-row"><button class="btn ghost danger" id="stats-clear">${clearArmed ? "Click again to delete saved history" : "Clear history"}</button>
      <span class="muted small">Deletes this player's saved games and graphs from this computer only.</span></div>`;
  loadDay();
}

$("stats-body").addEventListener("input", (ev) => {
  if (ev.target.id === "goal-target") goalDraft.target = ev.target.value;
  else if (ev.target.id === "goal-date") goalDraft.date = ev.target.value;
  else if (ev.target.id === "goal-metric") goalDraft.metric = ev.target.value;
});

// Picking a day (arrows, date box, or a square on the activity map) refreshes just the day card.
function pickDay(d) {
  selDay = startOfDay(d);
  const el = document.querySelector(".st-day");
  if (el) { el.innerHTML = daySection(); loadDay(); }
  document.querySelectorAll(".hm.pick.sel").forEach((c) => c.classList.remove("sel"));
  document.querySelector(`.hm.pick[data-day="${dayKey(selDay)}"]`)?.classList.add("sel");
}
$("stats-body").addEventListener("change", (ev) => {
  if (ev.target.id !== "day-pick" || !ev.target.value) return;
  const [y, m, d] = ev.target.value.split("-").map(Number);
  pickDay(new Date(y, m - 1, d));
});

$("stats-body").addEventListener("click", async (ev) => {
  const step = ev.target.closest("[data-day-step]");
  if (step) {
    const n = Number(step.dataset.dayStep), d = n === 0 ? new Date() : new Date(selDay);
    if (n) d.setDate(d.getDate() + n);
    pickDay(d > new Date() ? new Date() : d);
    return;
  }
  const cell = ev.target.closest(".hm.pick");
  if (cell) {
    const [y, m, d] = cell.dataset.day.split("-").map(Number);
    pickDay(new Date(y, m - 1, d));
    document.querySelector(".st-day")?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    return;
  }
  const rm = ev.target.closest("[data-goal-x]");
  if (rm) {
    try { await invoke("remove_goal", { tag: statsTag, id: Number(rm.dataset.goalX) }); } catch (e) { toast(errText(e), true); }
    await loadGoals(); paintStats();
    return;
  }
  if (ev.target.id === "goal-add") {
    const target = Math.round(Number(goalDraft.target));
    const msg = $("goal-msg");
    if (!target || target <= 0) { msg.textContent = "Enter a target number."; return; }
    // A date picker gives "YYYY-MM-DD"; the goal ends at the end of that local day.
    const deadline = goalDraft.date ? Math.floor(new Date(`${goalDraft.date}T23:59:59`).getTime() / 1000) : null;
    try {
      await invoke("add_goal", { tag: statsTag, metric: goalDraft.metric, target, deadline });
      goalDraft.target = ""; goalDraft.date = "";
      await loadGoals(); paintStats();
    } catch (e) { msg.textContent = errText(e); }
    return;
  }
  const seg = ev.target.closest(".seg button");
  if (seg && statsData) {
    const key = seg.parentElement.dataset.seg, v = seg.dataset.v;
    if (key === "metric") statsMetric = v; else if (key === "range") statsRange = v; else if (key === "heat") heatMode = v;
    paintStats();
    return;
  }
  if (ev.target.id === "stats-clear") {
    if (!clearArmed) {
      clearArmed = true; paintStats();
      setTimeout(() => { if (clearArmed) { clearArmed = false; if (statsData) paintStats(); } }, 5000);
      return;
    }
    clearArmed = false;
    try { await invoke("clear_history", { tag: statsTag }); } catch (e) { toast(errText(e)); }
    renderStatsPage();
  }
});

// One shared tooltip for heat-map cells.
(() => {
  const tip = document.createElement("div");
  tip.className = "hm-tip"; tip.hidden = true; document.body.appendChild(tip);
  const body = $("stats-body");
  body.addEventListener("mouseover", (ev) => {
    const c = ev.target.closest("[data-tip]");
    if (!c) { tip.hidden = true; return; }
    tip.textContent = c.dataset.tip; tip.hidden = false;
    const r = c.getBoundingClientRect();
    tip.style.left = Math.min(window.innerWidth - tip.offsetWidth - 8, Math.max(8, r.left + r.width / 2 - tip.offsetWidth / 2)) + "px";
    tip.style.top = r.top - tip.offsetHeight - 8 + "px";
  });
  body.addEventListener("mouseleave", () => { tip.hidden = true; });
})();

async function renderStatsPage() {
  const me = normTag(lastSettings.player_tag);
  if (!statsTag) statsTag = me;
  const opts = await playerOptions(statsTag);
  if (!statsTag && opts.length) statsTag = opts[0].tag;
  $("stats-select").innerHTML = opts.map((o) =>
    `<option value="${esc(o.tag)}" ${o.tag === statsTag ? "selected" : ""}>${esc(o.label)}</option>`).join("")
    || `<option value="">No players yet</option>`;
  if (!statsTag) {
    $("stats-body").innerHTML = `<div class="notice">Set your player tag in Settings first.</div>`;
    return;
  }
  const token = ++statsToken;
  let r;
  showSkeleton($("stats-body"), skel.stats());
  try { r = await invoke("get_stats", { tag: statsTag }); }
  catch (e) { $("stats-body").innerHTML = `<div class="error">${esc(errText(e))}</div>`; return; }
  if (token !== statsToken) return;

  if (!r.tracked) {
    statsData = null;
    $("stats-body").innerHTML = `<div class="notice">Nothing saved for this player yet. Make them <b>you</b> in Settings or add them to <b>Favorites</b>, keep the app running, and graphs fill in as they play.</div>`;
    return;
  }
  statsData = r;
  clearArmed = false;
  await loadGoals();
  if (token !== statsToken) return;
  paintStats();
}

$("stats-csv").onclick = async () => {
  try { savedToast(await invoke("export_matches_csv", { tag: statsTag })); } catch (e) { toast(errText(e), true); }
};
$("stats-png").onclick = async () => {
  try {
    const bytes = await chartPng();
    savedToast(await invoke("save_png", { name: `brawl-${statsMetric}-${statsRange}-${new Date().toISOString().slice(0, 10)}`, bytes }));
  } catch (e) { toast(errText(e), true); }
};

$("data-backup").onclick = async () => {
  try { savedToast(await invoke("export_backup")); } catch (e) { toast(errText(e), true); }
};
$("data-restore").onclick = () => $("data-file").click();
$("data-file").onchange = async (ev) => {
  const f = ev.target.files[0]; ev.target.value = "";
  if (!f) return;
  if (f.size > 200 * 1024 * 1024) { toast("That file is too large to be a backup.", true); return; }
  try {
    const r = await invoke("restore_backup", { text: await f.text() });
    toast(`Restored ${r.files} file${r.files === 1 ? "" : "s"}${r.players ? ` (${r.players} player${r.players === 1 ? "" : "s"})` : ""}.`);
    statsTag = ""; goalDraft.target = "";
    await loadSettings();
  } catch (e) { toast(errText(e), true); }
};

$("stats-select").onchange = (ev) => { statsTag = ev.target.value; renderStatsPage(); };
$("stats-refresh").onclick = () => renderStatsPage();

/* ============================ ranked picks ============================ */

let recInfo = null;
let recBans = [];
let recSource = "diamond+";
try { recSource = localStorage.getItem("recSource") || "diamond+"; } catch (e) { /* ignore */ }
let recMap = "";
let recToken = 0;

try { recBans = JSON.parse(localStorage.getItem("recBans") || "[]"); } catch (e) { recBans = []; }
function saveBans() { try { localStorage.setItem("recBans", JSON.stringify(recBans)); } catch (e) { /* ignore */ } }

async function loadMeta(refresh) {
  recInfo = await invoke("get_meta", { rankedOnly: $("rec-ranked").checked, source: recSource, refresh, limit: Number($("rec-size").value) || 200, parallel: Number($("rec-speed").value) || 4 });
  const sel = $("rec-map");
  if (!recMap || !recInfo.maps.some((m) => m.map === recMap)) recMap = recInfo.maps[0] ? recInfo.maps[0].map : "";
  sel.innerHTML = recInfo.maps.map((m) =>
    `<option value="${esc(m.map)}" ${m.map === recMap ? "selected" : ""}>${esc(m.map)} · ${esc(modeName(m.mode))} (${m.games} rounds)</option>`).join("")
    || `<option value="">No data yet</option>`;
  $("rec-ban-add").innerHTML = `<option value="">Add a ban…</option>` +
    recInfo.brawlers.filter((b) => !recBans.includes(b.id)).map((b) => `<option value="${b.id}">${esc(b.name)}</option>`).join("");
  const when = recInfo.fetched_at ? ago(recInfo.fetched_at * 1000) : "never";
  $("rec-status").textContent = recInfo.fetched_at
    ? `Sample: ${recInfo.players} Diamond+ players (${Object.entries(recInfo.tier_players || {}).map(([t, n]) => `${n} ${t}`).join(", ") || "none"}), ${num(recInfo.rounds)} rounds, updated ${when}.`
    : "No sample yet. Click “Refresh sample” (takes a few minutes).";
  renderBans();
  renderMapBox();
}

function renderMapBox() {
  const m = recInfo && recInfo.maps.find((x) => x.map === recMap);
  $("rec-map-box").innerHTML = m
    ? `${mapImg(m.event_id, m.map)}<div class="m-map-name">${esc(m.map)}</div><div class="muted small">${esc(modeName(m.mode))}</div>`
    : "";
}

function renderBans() {
  const names = new Map(((recInfo && recInfo.brawlers) || []).map((b) => [b.id, b.name]));
  $("rec-bans").innerHTML = recBans.length
    ? recBans.map((id) => `<span class="chip ban" data-ban="${id}" title="Click to remove">${brIcon(id, "", 18)}${esc(names.get(id) || id)} ✕</span>`).join("")
    : `<span class="none">No bans</span>`;
}

async function runRecommend() {
  runTierList();
  const token = ++recToken;
  const box = $("rec-list"), err = $("rec-error");
  err.classList.add("hidden");
  if (!recMap) { $("rec-top5").innerHTML = ""; box.innerHTML = `<div class="notice">Refresh the sample to get picks.</div>`; return; }
  try {
    const r = await invoke("recommend", { map: recMap, rankedOnly: $("rec-ranked").checked, source: recSource, banned: recBans, onlyOwned: $("rec-owned").checked });
    if (token !== recToken) return;
    renderTop5(r.recs);
    if (!r.recs.length) { box.innerHTML = `<div class="empty">Not enough data for this map with these filters. Try unticking “Ranked games only”, or refresh the sample.</div>`; return; }
    box.innerHTML = `<div class="table-wrap"><table><thead><tr><th>#</th><th>Brawler</th><th>Win rate</th><th>Rounds</th><th>Your power</th></tr></thead><tbody>${
      r.recs.slice(0, 15).map((x, i) => `<tr class="${i === 0 ? "top-pick" : ""}"><td class="num">${i + 1}</td>
        <td class="name">${brIcon(x.id, x.brawler, 26)}${esc(x.brawler)}</td>
        <td class="num"><span class="bar"><i style="width:${Math.round(x.win_rate * 100)}%"></i></span> ${(x.win_rate * 100).toFixed(1)}%</td>
        <td class="num">${num(x.games)}</td><td class="num">${x.power != null ? x.power : "—"}</td></tr>`).join("")
    }</tbody></table></div>
    <p class="muted small">Ordered by a smoothed win rate, so a brawler with few rounds can't outrank a well-tested one. “Win rate” is the raw figure.</p>`;
  } catch (e) {
    if (token !== recToken) return;
    box.innerHTML = "";
    err.textContent = errText(e);
    err.classList.remove("hidden");
  }
}

let tlMode = "";
async function runTierList() {
  const box = $("tl-list");
  try {
    const r = await invoke("get_tier_list", { rankedOnly: $("rec-ranked").checked, source: recSource, mode: tlMode || null });
    if (tlMode && !r.modes.includes(tlMode)) { tlMode = ""; return runTierList(); }
    $("tl-mode").innerHTML = `<option value="">All modes</option>` +
      r.modes.map((m) => `<option value="${esc(m)}" ${m === tlMode ? "selected" : ""}>${esc(modeName(m))}</option>`).join("");
    if (!r.list.length) { box.innerHTML = `<div class="empty">Not enough data yet. Refresh the sample above.</div>`; return; }
    const avg = r.list.reduce((a, x) => a + x.pick_rate, 0) / r.list.length;
    box.innerHTML = `<div class="table-wrap"><table><thead><tr><th>#</th><th>Brawler</th><th>Win rate</th><th>Pick rate</th><th>Rounds</th></tr></thead><tbody>${
      r.list.slice(0, 10).map((x, i) => `<tr class="${i === 0 ? "top-pick" : ""}"><td class="num">${i + 1}</td>
        <td class="name">${brIcon(x.id, x.brawler, 26)}${esc(x.brawler)}</td>
        <td class="num"><span class="bar"><i style="width:${Math.round(x.win_rate * 100)}%"></i></span> ${(x.win_rate * 100).toFixed(1)}%</td>
        <td class="num">${(x.pick_rate * 100).toFixed(1)}%${x.pick_rate > avg * 1.5 ? " 🔥" : ""}</td>
        <td class="num">${num(x.games)}</td></tr>`).join("")
    }</tbody></table></div>
    <p class="muted small">Top 10 of ${r.list.length} brawlers across every map${tlMode ? ` in ${esc(modeName(tlMode))}` : ""}, by smoothed win rate. Pick rate is each brawler's share of all rounds. It uses the same sample and filters as above, and brawlers with under 30 rounds are left out.</p>`;
  } catch (e) { box.innerHTML = ""; }
}
$("tl-mode").onchange = (ev) => { tlMode = ev.target.value; runTierList(); };

function renderTop5(recs) {
  const top = recs.slice(0, 5);
  $("rec-top5").innerHTML = top.length
    ? `<div class="section-head"><h2>Top ${top.length} picks</h2><span class="muted small">Win rate, sample size and your power level, with bans removed</span></div>
       <div class="top5">${top.map((x, i) => `
        <div class="pick ${i === 0 ? "first" : ""}">
          <div class="rank">#${i + 1}</div>
          ${brIcon(x.id, x.brawler, 56)}
          <div class="pn">${esc(x.brawler)}</div>
          <div class="wr">${(x.win_rate * 100).toFixed(0)}% <span class="muted small">win</span></div>
          <div class="muted small">${num(x.games)} rounds${x.power != null ? ` · power ${x.power}` : ""}</div>
        </div>`).join("")}</div>`
    : "";
}

async function initRecommend() {
  try { await loadMeta(false); } catch (e) { /* no sample yet */ }
  runRecommend();
}

$("rec-source").value = recSource;
$("rec-source").onchange = async (ev) => {
  recSource = ev.target.value;
  try { localStorage.setItem("recSource", recSource); } catch (e) { /* ignore */ }
  try { await loadMeta(false); } catch (e) { /* ignore */ }
  runRecommend();
};
$("rec-map").onchange = (ev) => { recMap = ev.target.value; renderMapBox(); runRecommend(); };
$("rec-ranked").onchange = async () => { try { await loadMeta(false); } catch (e) { /* ignore */ } runRecommend(); };
$("rec-owned").onchange = runRecommend;
$("rec-ban-add").onchange = (ev) => {
  const id = Number(ev.target.value);
  if (id && !recBans.includes(id)) { recBans.push(id); saveBans(); loadMeta(false).then(runRecommend).catch(() => {}); }
};
$("rec-bans").onclick = (ev) => {
  const chip = ev.target.closest("[data-ban]");
  if (!chip) return;
  recBans = recBans.filter((id) => id !== Number(chip.dataset.ban));
  saveBans();
  loadMeta(false).then(runRecommend).catch(() => {});
};
$("rec-clear-bans").onclick = () => { recBans = []; saveBans(); loadMeta(false).then(runRecommend).catch(() => {}); };
$("rec-refresh").onclick = async () => {
  const btn = $("rec-refresh");
  btn.disabled = true;
  $("rec-progress").textContent = "Starting…";
  try { await loadMeta(true); $("rec-progress").textContent = "Done."; await runRecommend(); }
  catch (e) { $("rec-progress").textContent = errText(e); }
  btn.disabled = false;
};
listen("meta-progress", (e) => {
  const [done, total] = e.payload;
  $("rec-progress").textContent = `Sampled ${done} of ${total} players…`;
});

/* ============================ compare ============================ */

const CMP_ROWS = [
  ["Trophies", (p) => p.trophies],
  ["Highest trophies", (p) => p.highestTrophies],
  ["Ranked elo", (p) => p.rankedElo],
  ["Ranked peak (all time)", (p) => p.highestAllTimeRankedElo],
  ["3v3 victories", (p) => p["3vs3Victories"]],
  ["Solo victories", (p) => p.soloVictories],
  ["Duo victories", (p) => p.duoVictories],
  ["Brawlers owned", (p) => (p.brawlers || []).length],
  ["Best win streak", (p) => (p.brawlers || []).reduce((m, b) => { const s = streakOf(b); return s === null ? m : Math.max(m ?? 0, s); }, null)],
  ["Level", (p) => p.expLevel],
];

function renderCompare(a, b) {
  const head = (p) => `<th><div class="cmp-name" style="color:${apiColor(p.nameColor)}">${esc(p.name)}</div><div class="muted small">${esc(p.tag)}</div></th>`;
  const rows = CMP_ROWS.map(([label, get]) => {
    const x = get(a), y = get(b);
    const both = typeof x === "number" && typeof y === "number";
    const cx = both && x > y ? "lead" : "", cy = both && y > x ? "lead" : "";
    return `<tr><td class="num ${cx}">${num(x)}</td><td class="cmp-label">${esc(label)}</td><td class="num ${cy}">${num(y)}</td></tr>`;
  }).join("");
  const rankRow = `<tr><td class="num">${esc(a.rankedRankName || "—")}</td><td class="cmp-label">Ranked rank</td><td class="num">${esc(b.rankedRankName || "—")}</td></tr>`;

  // Brawlers both own, with the trophy gap.
  const mapB = new Map((b.brawlers || []).map((x) => [x.id, x]));
  const shared = (a.brawlers || []).filter((x) => mapB.has(x.id)).map((x) => ({ a: x, b: mapB.get(x.id) }))
    .sort((p, q) => Math.max(q.a.trophies, q.b.trophies) - Math.max(p.a.trophies, p.b.trophies));
  const sharedRows = shared.slice(0, 15).map(({ a: x, b: y }) =>
    `<tr><td class="num ${x.trophies > y.trophies ? "lead" : ""}">${num(x.trophies)}</td>
     <td class="cmp-label">${brIcon(x.id, x.name, 22)}${esc(x.name)}</td>
     <td class="num ${y.trophies > x.trophies ? "lead" : ""}">${num(y.trophies)}</td></tr>`).join("");

  $("compare-body").innerHTML = `
    <div class="table-wrap"><table class="cmp"><thead><tr>${head(a)}<th></th>${head(b)}</tr></thead><tbody>${rows}${rankRow}</tbody></table></div>
    ${shared.length ? `<div class="section-head"><h2>Brawlers you both own (${shared.length})</h2><span class="muted small">Top 15 by trophies</span></div>
    <div class="table-wrap"><table class="cmp"><tbody>${sharedRows}</tbody></table></div>` : ""}`;
}

$("compare-form").onsubmit = async (ev) => {
  ev.preventDefault();
  const ta = normTag($("cmp-a").value), tb = normTag($("cmp-b").value);
  const err = $("compare-error");
  err.classList.add("hidden");
  if (!ta || !tb) { err.textContent = "Enter two player tags."; err.classList.remove("hidden"); return; }
  try {
    const [a, b] = await Promise.all([invoke("fetch_player", { tag: ta }), invoke("fetch_player", { tag: tb })]);
    renderCompare(a, b);
  } catch (e) {
    $("compare-body").innerHTML = "";
    err.textContent = errText(e);
    err.classList.remove("hidden");
  }
};
$("cmp-me").onclick = () => { $("cmp-a").value = normTag(lastSettings.player_tag); };

/* ============================ activity log ============================ */

let activityFilter = "";
let trackerError = "";

async function renderActivity() {
  let status = [], entries = [];
  try {
    [status, entries] = await Promise.all([
      invoke("get_tracking_status"),
      invoke("get_activity", { tag: activityFilter || null }),
    ]);
  } catch (e) { toast(errText(e), true); return; }
  if (currentPage !== "activity") return;

  const errBox = $("tracker-error");
  errBox.textContent = trackerError ? `Last tracker problem: ${trackerError}` : "";
  errBox.classList.toggle("hidden", !trackerError);

  const sel = $("activity-filter");
  sel.innerHTML = `<option value="">All players</option>` + status.map((s) =>
    `<option value="${esc(s.tag)}" ${s.tag === activityFilter ? "selected" : ""}>${esc(s.name)}${s.is_self ? " (me)" : ""}</option>`).join("");

  $("activity-status").innerHTML = status.length ? status.map((s) => {
    const st = s.state && s.state.initialized ? s.state : null;
    const info = st ? statusOf(st.last_battle_time) : null;
    const pill = !st ? `<span class="pill idle">Checking...</span>`
      : info.active ? `<span class="pill live">Playing now</span>` : `<span class="pill idle">Idle</span>`;
    return `<div class="fav-card">
      <div class="top"><div><div class="nm">${esc(s.name)}${s.is_self ? ' <span class="pill gold">me</span>' : ""}</div><div class="tg">#${esc(s.tag)}</div></div>${pill}</div>
      <div class="meta">
        <div><div class="l">Last game</div><div class="v" style="font-size:13px">${st && parseBT(st.last_battle_time) ? esc(ago(parseBT(st.last_battle_time))) : "—"}</div></div>
        <div><div class="l">Ranked</div><div class="v" style="font-size:13px">${st && st.ranked_rank_name ? esc(st.ranked_rank_name) : "—"}${st && st.ranked_elo != null ? ` · ${num(st.ranked_elo)}` : ""}</div></div>
      </div>
    </div>`;
  }).join("") : `<div class="empty">Nobody is being tracked yet. Set your player tag in Settings and turn on background tracking, or add favorites.</div>`;

  $("activity-list").innerHTML = entries.length ? entries.map((e) => {
    const t = new Date(e.ts * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
    const dotCls = e.kind === "game" ? (e.result === "victory" ? "victory" : e.result === "defeat" ? "defeat" : "") : e.kind;
    let badge = "";
    if (typeof e.delta === "number") {
      const cls = e.delta > 0 ? "pos" : e.delta < 0 ? "neg" : "";
      badge = e.combined > 1
        ? `<span class="delta ${cls}" title="Shared by ${e.combined} games">net ${sign(e.delta)} ${esc(e.unit)}</span>`
        : `<span class="delta ${cls}">${sign(e.delta)} ${esc(e.unit)}</span>`;
    }
    const text = e.kind === "game" ? `<b>${esc(e.name)}</b>: ${esc(e.text)}` : esc(e.text);
    return `<div class="a-row"><span class="a-time">${esc(t)}</span><span class="a-dot ${dotCls}"></span><span class="a-text">${text}</span>${badge}</div>`;
  }).join("") : `<div class="a-empty">Nothing yet. New games show up here within about a minute of finishing.</div>`;
}

$("activity-filter").onchange = (ev) => { activityFilter = ev.target.value; renderActivity(); };
$("activity-clear").onclick = async () => {
  try { await invoke("clear_activity"); renderActivity(); } catch (e) { toast(errText(e), true); }
};

listen("tracker-update", () => {
  trackerError = "";
  if (currentPage === "activity") renderActivity();
  if (currentPage === "favorites") renderFavorites();
});
listen("tracker-error", (e) => {
  trackerError = e.payload;
  if (currentPage === "activity") renderActivity();
});

// Keep "x min ago" and "playing now" fresh while those pages are open.
setInterval(() => {
  if (currentPage === "activity") renderActivity();
  if (currentPage === "favorites") renderFavorites();
}, 30000);

/* ============================== settings ============================== */

function setMsg(text, kind = "") {
  const el = $("set-msg");
  el.textContent = text;
  el.className = "inline-msg " + kind;
}

async function loadSettings() {
  lastSettings = await invoke("get_settings");
  $("set-key").value = lastSettings.api_key || "";
  $("set-tag").value = lastSettings.player_tag || "";
  $("set-client").value = lastSettings.discord_client_id || "";
  $("set-track").checked = lastSettings.tracking_enabled !== false;
  $("set-tray").checked = lastSettings.close_to_tray !== false;
  $("set-login").checked = !!lastSettings.launch_at_login;
  $("set-notify").checked = lastSettings.notifications !== false;
  $("set-n-session").checked = lastSettings.notify_session !== false;
  $("set-n-rank").checked = lastSettings.notify_rank !== false;
  $("set-n-goal").checked = lastSettings.notify_goal !== false;
  $("set-n-tilt").checked = lastSettings.notify_tilt !== false;
  $("set-tilt-n").value = String(lastSettings.tilt_losses || 3);
  $("set-quiet").checked = !!lastSettings.quiet_enabled;
  $("set-quiet-start").value = lastSettings.quiet_start || "23:00";
  $("set-quiet-end").value = lastSettings.quiet_end || "08:00";
  syncNotifyUI();
  $("set-autopresence").checked = !!lastSettings.auto_start_presence;
}

// Sub-options follow the master switch; the time boxes follow "Quiet hours".
function syncNotifyUI() {
  $("notify-sub").classList.toggle("off", !$("set-notify").checked);
  $("quiet-times").classList.toggle("off", !$("set-notify").checked || !$("set-quiet").checked);
}
$("set-notify").onchange = syncNotifyUI;
$("set-quiet").onchange = syncNotifyUI;

// Quiet hours follow the local clock; the app tells the background tracker the current offset.
function reportTimezone() {
  invoke("set_tz_offset", { minutes: -new Date().getTimezoneOffset() }).catch(() => {});
}

async function saveSettings() {
  lastSettings = {
    ...lastSettings,
    api_key: $("set-key").value.trim(),
    player_tag: $("set-tag").value.trim(),
    discord_client_id: $("set-client").value.trim(),
    tracking_enabled: $("set-track").checked,
    close_to_tray: $("set-tray").checked,
    launch_at_login: $("set-login").checked,
    notifications: $("set-notify").checked,
    notify_session: $("set-n-session").checked,
    notify_rank: $("set-n-rank").checked,
    notify_goal: $("set-n-goal").checked,
    notify_tilt: $("set-n-tilt").checked,
    tilt_losses: Number($("set-tilt-n").value) || 3,
    quiet_enabled: $("set-quiet").checked,
    quiet_start: $("set-quiet-start").value || "23:00",
    quiet_end: $("set-quiet-end").value || "08:00",
    auto_start_presence: $("set-autopresence").checked,
  };
  await invoke("save_settings", { settings: lastSettings });
}

$("set-save").onclick = async () => {
  try { await saveSettings(); setMsg("Saved.", "ok"); }
  catch (e) { setMsg(errText(e), "err"); }
};

$("set-test").onclick = async () => {
  setMsg("Testing...");
  try {
    await saveSettings();
    const p = await invoke("fetch_player", { tag: lastSettings.player_tag });
    setMsg(`Connected! Found ${p.name}.`, "ok");
  } catch (e) { setMsg(errText(e), "err"); }
};

/* ============================== presence ============================== */

function setPresenceUI(on) {
  $("presence-dot").classList.toggle("on", on);
  $("presence-label").textContent = on ? "Presence on" : "Presence off";
  const btn = $("presence-toggle");
  btn.textContent = on ? "Stop" : "Start";
  btn.classList.toggle("primary", !on);
  btn.classList.toggle("danger", on);
}

function logLine(msg) {
  const box = $("presence-log");
  const t = new Date().toLocaleTimeString();
  const div = document.createElement("div");
  div.innerHTML = `<span class="t">[${esc(t)}]</span> ${esc(msg)}`;
  box.appendChild(div);
  while (box.children.length > 100) box.removeChild(box.firstChild);
  box.scrollTop = box.scrollHeight;
}

$("presence-toggle").onclick = async () => {
  try {
    const running = await invoke("presence_running");
    if (running) {
      await invoke("stop_presence");
    } else {
      await saveSettings();
      await invoke("start_presence");
    }
  } catch (e) {
    logLine(errText(e));
    setMsg(errText(e), "err");
  }
};

/* --- presence page: content toggles, custom text, extras, rotation preview --- */

const PC_TOGGLES = {
  "pc-latest": "presence_latest", "pc-victories": "presence_victories", "pc-trophies": "presence_trophies",
  "pc-record": "presence_record", "pc-ranked": "presence_ranked", "pc-club": "presence_club",
  "pc-streak": "presence_streak", "pc-session": "presence_session",
  "pc-finished": "presence_finished", "pc-idle": "presence_idle",
  "pc-custom": "presence_custom", "pc-images": "presence_images", "pc-button": "presence_button",
};
const VIEW_TITLES = {
  latest: "Latest match", session: "Current session", trophies: "Trophies", ranked: "Ranked",
  finished: "Just finished", idle: "Taking a break", streak: "Win streak", victories: "Victories", record: "Recent record", club: "Club", custom: "Custom text",
};
let pcData = null;      // { views, placeholders }
let pcCurrent = null;   // view id Discord is showing right now
let pcLastLine = "pc-line1";
let pcTimer = null;

function pcFill(text) {
  let out = text || "";
  for (const p of (pcData && pcData.placeholders) || []) out = out.split(`{${p.key}}`).join(p.value);
  return out;
}

function pcImg(url, cls) {
  return url ? `<img class="${cls}" src="${esc(url)}" alt="" onerror="this.style.display='none'" />` : "";
}

function pcRenderPreview() {
  const views = (pcData && pcData.views) || [];
  const custom = {
    details: pcFill($("pc-line1").value), state: pcFill($("pc-line2").value),
    large_image: (views[0] || {}).large_image, small_image: (views[0] || {}).small_image,
  };
  // The preview shows the custom text (this card is about it); the rotation list shows the rest.
  $("pc-preview").innerHTML = `
    <div class="d-label">PLAYING A GAME</div>
    <div class="d-row">
      <div class="d-art">${pcImg(custom.large_image, "d-big")}${pcImg(custom.small_image, "d-small")}</div>
      <div class="d-text"><b>Brawl Stars</b><div>${esc(custom.details)}</div><div>${esc(custom.state)}</div><div class="muted">00:00 elapsed</div></div>
    </div>
    ${lastSettings.presence_button ? `<div class="d-btn">View Brawl Stars stats</div>` : ""}`;
  $("pc-pin-custom").textContent = lastSettings.presence_pin === "custom" ? "Unpin custom text" : "Pin custom text";
}

function pcRenderRotation() {
  const views = (pcData && pcData.views) || [];
  const pin = lastSettings.presence_pin || "";
  $("pc-rotation").innerHTML = views.map((v, i) => `
    <div class="rot ${v.id === pcCurrent ? "now" : ""}">
      <span class="n">${i + 1}</span>
      <span class="rt"><b>${esc(v.details)}</b><span class="muted">${esc(v.state)}</span></span>
      <span class="muted small">${esc(VIEW_TITLES[v.id] || v.id)}</span>
      <button class="btn ${pin === v.id ? "primary" : ""}" data-pin="${esc(v.id)}">${pin === v.id ? "UNPIN" : "PIN"}</button>
    </div>`).join("") || `<div class="empty small">Nothing to show yet.</div>`;
}

function pcRenderPlaceholders() {
  $("pc-placeholders").innerHTML = ((pcData && pcData.placeholders) || []).map((p) =>
    `<button type="button" class="ph" data-ph="${esc(p.key)}"><code>{${esc(p.key)}}</code><span>${esc(p.label)}</span><em>${esc(p.value)}</em></button>`).join("");
}

async function pcSave(patch) {
  lastSettings = { ...lastSettings, ...patch };
  await invoke("save_settings", { settings: lastSettings });
  clearTimeout(pcTimer);
  pcTimer = setTimeout(pcReload, 350);
}

async function pcReload() {
  const err = $("presence-error");
  try {
    pcData = await invoke("get_presence_preview");
    err.classList.add("hidden");
  } catch (e) {
    err.textContent = errText(e);
    err.classList.remove("hidden");
  }
  pcRenderPlaceholders();
  pcRenderPreview();
  pcRenderRotation();
}

function renderPresencePage() {
  for (const [id, key] of Object.entries(PC_TOGGLES)) $(id).checked = key === "presence_images" || key === "presence_button" || key === "presence_custom" || key === "presence_club" || key === "presence_idle"
    ? !!lastSettings[key] : lastSettings[key] !== false;
  $("pc-line1").value = lastSettings.presence_line1 || "";
  $("pc-line2").value = lastSettings.presence_line2 || "";
  pcRenderPreview();
  pcReload();
}

for (const [id, key] of Object.entries(PC_TOGGLES)) {
  $(id).onchange = (ev) => pcSave({ [key]: ev.target.checked }).catch((e) => toast(errText(e)));
}
for (const id of ["pc-line1", "pc-line2"]) {
  $(id).onfocus = () => { pcLastLine = id; };
  $(id).oninput = () => {
    pcRenderPreview();
    pcSave({ presence_line1: $("pc-line1").value, presence_line2: $("pc-line2").value }).catch(() => {});
  };
}
$("pc-placeholders").onclick = (ev) => {
  const b = ev.target.closest("[data-ph]");
  if (!b) return;
  const input = $(pcLastLine);
  const tok = `{${b.dataset.ph}}`;
  const s = input.selectionStart ?? input.value.length, e = input.selectionEnd ?? s;
  input.value = input.value.slice(0, s) + tok + input.value.slice(e);
  input.focus();
  input.setSelectionRange(s + tok.length, s + tok.length);
  input.dispatchEvent(new Event("input"));
};
$("pc-rotation").onclick = (ev) => {
  const b = ev.target.closest("[data-pin]");
  if (!b) return;
  const id = b.dataset.pin;
  pcSave({ presence_pin: lastSettings.presence_pin === id ? "" : id }).then(() => { pcRenderRotation(); pcRenderPreview(); }).catch((e) => toast(errText(e)));
};
$("pc-pin-custom").onclick = () =>
  pcSave({ presence_pin: lastSettings.presence_pin === "custom" ? "" : "custom" }).then(() => { pcRenderRotation(); pcRenderPreview(); }).catch((e) => toast(errText(e)));

listen("presence-view", (e) => { pcCurrent = e.payload; if (currentPage === "presence") pcRenderRotation(); });
listen("presence-log", (e) => logLine(e.payload));
listen("presence-status", (e) => setPresenceUI(!!e.payload));

/* ============================== themes =============================== */

// [id, name, one-line description, [background, panel, accent] for the preview swatch]
const THEMES = [
  ["default",  "Brawl Night", "The original. Deep purple, gold accents, rounded cards.", ["#0f0d1c", "#1c1833", "#ffce00"]],
  ["glass",    "Glass",       "Frosted translucent panels over a colourful glow.",       ["#1b2a4a", "#4a5a8a", "#5ee7ff"]],
  ["brutal",   "Neo Brutal",  "Light, thick black outlines, hard offset shadows.",       ["#fff4d6", "#ffffff", "#ff5c8a"]],
  ["terminal", "Terminal",    "Green-on-black, monospace, sharp square corners.",        ["#050b07", "#0a140d", "#39ff88"]],
  ["paper",    "Paper",       "Warm light theme, soft shadows, pill shapes.",            ["#f6efe6", "#fffaf3", "#d9663a"]],
  ["ember",    "Ember",       "Angular cut corners, charcoal and red-orange.",           ["#111214", "#1c1e22", "#ff5a36"]],
  ["pastel",   "Midnight Pastel", "Cozy dark lavender with soft pastel colours.",         ["#1e1e2e", "#313244", "#cba6f7"]],
  ["synthwave","Synthwave",   "Neon pink and cyan glow over a retro sunset grid.",       ["#0d0221", "#2a0f55", "#ff2fb3"]],
  ["blueprint","Blueprint",   "Technical drawing: blue grid paper, dashed lines.",       ["#0b2a4a", "#134270", "#ffd34d"]],
  ["arcade",   "Brawl Arcade","Bold game-menu style: bright blue, chunky outlines.",     ["#2a6bff", "#101c4d", "#ffd21f"]],
  ["slate",    "Slate",       "Minimal and flat, like a developer tool.",                ["#0b0c0e", "#1b1e24", "#5b8cff"]],
  ["daylight", "Daylight",    "Clean and bright: white cards on soft grey, violet accent.", ["#f2f4f9", "#ffffff", "#5b4bdb"]],
];
const LIGHT_THEMES = ["brutal", "paper", "daylight"];

// Navigation and page layouts: [id, name, description]. Applied as data-nav / data-content on <html>.
const NAV_LAYOUTS = [
  ["sidebar", "Sidebar",       "Labels always visible on the left (current)."],
  ["rail",    "Icon rail",     "Slim icon strip; names appear on hover."],
  ["top",     "Top tabs",      "A tab bar across the top; the page gets full width."],
  ["dock",    "Floating dock", "Icon dock floating at the bottom of the window."],
  ["grouped", "Grouped",       "Left sidebar split into Players, Games and App."],
];
const CONTENT_LAYOUTS = [
  ["cards",   "Cards",      "Roomy stacked cards (current)."],
  ["compact", "Compact",    "Dense cards with players inline; fits more on screen."],
  ["list",    "List",       "Flat aligned rows like a table; click a row for details."],
  ["columns", "Two columns","Games and stats side by side."],
  ["focus",   "Focus",      "One narrow centred column, large type."],
];
// Tiny wireframes for the picker (84x54).
function wire(kind) {
  const r = (x, y, w, h, o = 0.55, rx = 2) => `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="currentColor" opacity="${o}"/>`;
  const body = {
    sidebar: r(4, 4, 20, 46, 0.25) + [10, 17, 24, 31].map((y) => r(8, y, 12, 4)).join("") + r(30, 8, 50, 8, 0.7) + r(30, 22, 50, 12, 0.3) + r(30, 38, 50, 12, 0.3),
    rail:    r(4, 4, 8, 46, 0.25) + [10, 18, 26, 34].map((y) => r(6, y, 4, 4, 0.8, 2)).join("") + r(18, 8, 62, 8, 0.7) + r(18, 22, 62, 12, 0.3) + r(18, 38, 62, 12, 0.3),
    top:     r(4, 4, 76, 8, 0.25) + [8, 22, 36, 50].map((x) => r(x, 6, 10, 4, 0.8)).join("") + r(8, 18, 68, 8, 0.7) + r(8, 30, 68, 20, 0.3),
    dock:    r(8, 6, 68, 8, 0.7) + r(8, 18, 68, 18, 0.3) + r(24, 41, 36, 9, 0.35, 4) + [29, 37, 45, 53].map((x) => r(x, 44, 3, 3, 0.9, 2)).join(""),
    grouped: r(4, 4, 24, 46, 0.25) + r(8, 8, 10, 2, 0.9) + r(8, 12, 16, 3) + r(8, 17, 16, 3) + r(8, 24, 10, 2, 0.9) + r(8, 28, 16, 3) + r(8, 33, 16, 3) + r(8, 38, 16, 3) + r(34, 8, 46, 8, 0.7) + r(34, 22, 46, 12, 0.3) + r(34, 38, 46, 12, 0.3),
    cards:   r(6, 6, 72, 13, 0.3, 4) + r(6, 22, 72, 13, 0.3, 4) + r(6, 38, 72, 11, 0.3, 4),
    compact: [6, 13, 20, 27, 34, 41].map((y) => r(6, y, 72, 5, 0.3, 2)).join("") + r(8, 7, 20, 3, 0.8),
    list:    [6, 16, 26, 36, 46].map((y) => r(6, y, 14, 4, 0.8) + r(24, y, 30, 4, 0.5) + r(60, y, 18, 4, 0.3) + r(6, y + 7, 72, 1, 0.25, 0)).join(""),
    columns: r(6, 6, 34, 13, 0.3, 3) + r(44, 6, 34, 13, 0.3, 3) + r(6, 22, 34, 13, 0.3, 3) + r(44, 22, 34, 13, 0.3, 3) + r(6, 38, 34, 11, 0.3, 3) + r(44, 38, 34, 11, 0.3, 3),
    focus:   r(26, 4, 32, 6, 0.8) + r(22, 14, 40, 12, 0.3, 4) + r(22, 30, 40, 12, 0.3, 4) + r(22, 46, 40, 5, 0.3, 4),
  }[kind];
  return `<svg viewBox="0 0 84 54" width="84" height="54" class="wire"><rect x="1" y="1" width="82" height="52" rx="5" fill="none" stroke="currentColor" opacity="0.35"/>${body}</svg>`;
}
const lsGet = (k, d) => { try { return localStorage.getItem(k) || d; } catch { return d; } };
function applyLayout(kind, id) { // kind: "nav" | "content"
  const def = kind === "nav" ? "sidebar" : "cards";
  if (id === def) delete document.documentElement.dataset[kind];
  else document.documentElement.dataset[kind] = id;
  try { localStorage.setItem("bsh-" + kind, id); } catch {}
  renderLayoutPickers();
}
function renderLayoutPickers() {
  const draw = (gridId, list, cur) => {
    $(gridId).innerHTML = list.map(([id, name, desc]) => `
      <button class="theme-tile layout-tile ${id === cur ? "on" : ""}" data-layout-id="${id}">
        ${wire(id)}<b>${esc(name)}</b><span class="muted small">${esc(desc)}</span>
      </button>`).join("");
  };
  draw("nav-grid", NAV_LAYOUTS, lsGet("bsh-nav", "sidebar"));
  draw("content-grid", CONTENT_LAYOUTS, lsGet("bsh-content", "cards"));
}
$("nav-grid").onclick = (ev) => { const t = ev.target.closest("[data-layout-id]"); if (t) applyLayout("nav", t.dataset.layoutId); };
$("content-grid").onclick = (ev) => { const t = ev.target.closest("[data-layout-id]"); if (t) applyLayout("content", t.dataset.layoutId); };
// Names for the icon-only layouts (hover tooltips / accessibility).
document.querySelectorAll(".nav-item").forEach((b) => {
  const label = [...b.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join("").trim();
  b.dataset.label = label; b.title = label;
});
renderLayoutPickers();

function currentTheme() {
  try { return localStorage.getItem("bsh-theme") || "default"; } catch { return "default"; }
}

// The theme actually shown: with "Follow Windows" on, a light system swaps a dark choice for Daylight.
function effectiveTheme(id) {
  const light = window.matchMedia && matchMedia("(prefers-color-scheme: light)").matches;
  return lsGet("bsh-follow", "") === "1" && light && !LIGHT_THEMES.includes(id) ? "daylight" : id;
}

function applyTheme(id) {
  if (!THEMES.some((t) => t[0] === id)) id = "default";
  try { localStorage.setItem("bsh-theme", id); } catch {}
  paintTheme();
  renderThemePicker();
}

function paintTheme() {
  const eff = effectiveTheme(currentTheme());
  if (eff === "default") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = eff;
}
if (window.matchMedia) matchMedia("(prefers-color-scheme: light)").addEventListener?.("change", paintTheme);

function renderThemePicker() {
  const cur = currentTheme();
  $("theme-grid").innerHTML = THEMES.map(([id, name, desc, [bg, panel, accent]]) => `
    <button class="theme-tile ${id === cur ? "on" : ""}" data-theme-id="${id}">
      <span class="theme-sw" style="background:${bg}"><i style="background:${panel}"></i><i style="background:${accent}"></i></span>
      <b>${esc(name)}</b><span class="muted small">${esc(desc)}</span>
    </button>`).join("");
}
$("theme-grid").onclick = (ev) => {
  const t = ev.target.closest("[data-theme-id]");
  if (t) applyTheme(t.dataset.themeId);
};
renderThemePicker();

/* ---- accent colour, brightness, size, spacing, animation ---- */

const ACCENTS = [["#ffce00", "Gold"], ["#4f8cff", "Blue"], ["#3ddc84", "Green"], ["#ff4fa3", "Pink"], ["#ff8a3d", "Orange"], ["#9b6dff", "Purple"], ["#ff5d6c", "Red"], ["#22d3ee", "Cyan"]];
const SCALES = [["90", "Small"], ["100", "Normal"], ["115", "Large"], ["130", "Larger"]];

// Black or white text, whichever reads better on this colour.
function onColour(hex) {
  const n = parseInt(hex.slice(1), 16), c = [n >> 16, (n >> 8) & 255, n & 255].map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2] > 0.4 ? "#111111" : "#ffffff";
}
const lsSet = (k, v) => { try { v === "" || v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* private mode */ } };

function applyAccent(hex, redraw = true) {
  const root = document.documentElement;
  if (hex && /^#[0-9a-f]{6}$/i.test(hex)) {
    root.style.setProperty("--accent", hex); root.style.setProperty("--on-accent", onColour(hex));
    lsSet("bsh-accent", hex); lsSet("bsh-on-accent", onColour(hex));
  } else {
    root.style.removeProperty("--accent"); root.style.removeProperty("--on-accent");
    lsSet("bsh-accent", ""); lsSet("bsh-on-accent", "");
  }
  if (redraw) renderLookExtra();
}
function applyScale(v) { document.documentElement.style.zoom = v === "100" ? "" : Number(v) / 100; lsSet("bsh-scale", v === "100" ? "" : v); renderLookExtra(); }
function applyDensity(v) { v === "compact" ? (document.documentElement.dataset.density = "compact") : delete document.documentElement.dataset.density; lsSet("bsh-density", v === "compact" ? "compact" : ""); renderLookExtra(); }
function applyMotion(v) { v === "reduced" ? (document.documentElement.dataset.motion = "reduced") : delete document.documentElement.dataset.motion; lsSet("bsh-motion", v === "reduced" ? "reduced" : ""); renderLookExtra(); }

function renderLookExtra() {
  const acc = lsGet("bsh-accent", ""), scale = lsGet("bsh-scale", "100"), dens = lsGet("bsh-density", "normal") === "compact" ? "compact" : "normal";
  const mot = lsGet("bsh-motion", "") === "reduced" ? "reduced" : "full", follow = lsGet("bsh-follow", "") === "1";
  const seg = (key, cur, items) => `<div class="seg" data-look="${key}">${items.map(([v, l]) => `<button class="${cur === v ? "on" : ""}" data-v="${v}">${l}</button>`).join("")}</div>`;
  $("look-extra").innerHTML = `
    <div class="look-row"><h3 class="ap-h">Accent colour</h3>
      <div class="swatches">
        <button class="swatch reset ${acc ? "" : "on"}" data-accent="" title="Use the theme's own colour">Theme</button>
        ${ACCENTS.map(([h, n]) => `<button class="swatch ${acc.toLowerCase() === h ? "on" : ""}" style="--sw:${h}" data-accent="${h}" title="${n}" aria-label="${n}"></button>`).join("")}
        <input type="color" class="swatch-pick" id="accent-pick" value="${/^#[0-9a-f]{6}$/i.test(acc) ? acc : "#ffce00"}" title="Pick any colour" aria-label="Custom accent colour" />
      </div></div>
    <div class="look-row"><h3 class="ap-h">Light and dark</h3>
      <label class="check"><input id="look-follow" type="checkbox" ${follow ? "checked" : ""} /><span>Follow Windows: switch to the Daylight theme when Windows is in light mode (dark themes only)</span></label></div>
    <div class="look-row"><h3 class="ap-h">Text and interface size</h3>${seg("scale", scale, SCALES)}</div>
    <div class="look-row"><h3 class="ap-h">Spacing</h3>${seg("density", dens, [["normal", "Comfortable"], ["compact", "Compact (small screens)"]])}</div>
    <div class="look-row"><h3 class="ap-h">Animations</h3>${seg("motion", mot, [["full", "On"], ["reduced", "Reduced"]])}</div>`;
}
$("look-extra").addEventListener("click", (ev) => {
  const sw = ev.target.closest("[data-accent]");
  if (sw) { applyAccent(sw.dataset.accent); return; }
  const b = ev.target.closest("[data-look] button");
  if (!b) return;
  const k = b.parentElement.dataset.look;
  if (k === "scale") applyScale(b.dataset.v); else if (k === "density") applyDensity(b.dataset.v); else if (k === "motion") applyMotion(b.dataset.v);
});
$("look-extra").addEventListener("change", (ev) => {
  if (ev.target.id === "look-follow") { lsSet("bsh-follow", ev.target.checked ? "1" : ""); paintTheme(); }
});
// While dragging in the colour picker, apply the colour without rebuilding the controls (that would drop the picker).
$("look-extra").addEventListener("input", (ev) => { if (ev.target.id === "accent-pick") applyAccent(ev.target.value, false); });
$("look-extra").addEventListener("change", (ev) => { if (ev.target.id === "accent-pick") renderLookExtra(); });
renderLookExtra();

/* ============================== updates ============================== */

// Tiny markdown for release notes: headings, bullets, **bold**, `code`. Input is escaped first.
function mdLite(src) {
  const inline = (t) => esc(t).replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/`(.+?)`/g, "<code>$1</code>");
  let out = "", list = false;
  for (const raw of String(src || "").split(/\r?\n/)) {
    const line = raw.trim();
    const li = /^[-*]\s+(.*)/.exec(line);
    if (li) { if (!list) { out += "<ul>"; list = true; } out += `<li>${inline(li[1])}</li>`; continue; }
    if (list) { out += "</ul>"; list = false; }
    const hd = /^#{1,6}\s+(.*)/.exec(line);
    if (hd) out += `<h3>${inline(hd[1])}</h3>`;
    else if (line) out += `<p>${inline(line)}</p>`;
  }
  return out + (list ? "</ul>" : "");
}

function showWhatsNew(title, notes, onClose) {
  $("wn-title").textContent = title;
  $("wn-body").innerHTML = notes && notes.trim()
    ? mdLite(notes)
    : `<p class="muted">No release notes were published for this version.</p>`;
  $("whatsnew").classList.remove("hidden");
  $("wn-close").onclick = () => { $("whatsnew").classList.add("hidden"); if (onClose) onClose(); };
}

let pendingUpdate = null, installing = false;

async function checkForUpdate(manual) {
  const st = $("upd-status");
  if (manual) st.textContent = "Checking…";
  try {
    const u = await invoke("check_update");
    if (u) {
      pendingUpdate = u;
      $("ub-version").textContent = "v" + u.version;
      $("update-banner").classList.remove("hidden");
      st.textContent = `· version ${u.version} is available`;
    } else if (manual) st.textContent = "· you're on the newest version";
  } catch (e) {
    if (manual) st.textContent = "· " + errText(e);
    else console.warn("update check:", e);
  }
}

$("upd-check").onclick = () => checkForUpdate(true);
$("ub-later").onclick = () => $("update-banner").classList.add("hidden");
$("ub-notes").onclick = () => pendingUpdate && showWhatsNew(`Version ${pendingUpdate.version}`, pendingUpdate.notes);
$("upd-notes").onclick = async () => {
  try {
    const w = await invoke("whats_new", { force: true });
    if (w) showWhatsNew(`Version ${w.version}`, w.notes);
  } catch (e) { toast(errText(e), true); }
};
$("ub-install").onclick = async () => {
  if (installing) return;
  installing = true;
  $("ub-install").disabled = true;
  $("ub-install").textContent = "Downloading…";
  document.querySelector("#update-banner .ub-bar").classList.remove("hidden");
  try { await invoke("install_update"); } // restarts the app when it finishes
  catch (e) {
    installing = false;
    $("ub-install").disabled = false;
    $("ub-install").textContent = "Install and restart";
    toast(errText(e), true);
  }
};
listen("update-progress", (e) => {
  const p = e.payload || {};
  if (p.total) $("ub-fill").style.width = Math.min(100, Math.round((p.got / p.total) * 100)) + "%";
  if (p.done) { $("ub-fill").style.width = "100%"; $("ub-install").textContent = "Installing…"; }
});

/* ============================ welcome back ============================ */

function showRecap(r) {
  const when = new Date(r.since * 1000).toLocaleString([], { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
  $("rc-since").textContent = `Since the app last saw your profile (${when})`;
  const tile = (label, change, now, cls) => change == null ? "" :
    `<div class="rc-tile ${change > 0 ? "pos" : "neg"} ${cls}"><div class="rc-label">${label}</div>
     <div class="rc-big">${sign(change)}</div><div class="rc-now muted">now ${num(now)}</div></div>`;
  $("rc-tiles").innerHTML = tile("Trophies", r.trophy_change, r.trophies_now, "") + tile("Ranked elo", r.elo_change, r.elo_now, "rc-elo");
  $("rc-games").textContent = r.games
    ? `About ${r.games} game${r.games === 1 ? "" : "s"} in that time: ${r.wins} won, ${r.losses} lost${r.games >= 25 ? " (the game log only goes back 25 battles)" : ""}.`
    : "";
  $("recap").classList.remove("hidden");
  $("rc-close").onclick = () => { $("recap").classList.add("hidden"); invoke("ack_recap").catch(() => {}); };
}

async function maybeRecap() {
  try {
    const r = await invoke("get_recap");
    if (r) showRecap(r);
  } catch (e) { console.warn("recap:", e); } // offline etc.: it stays pending for next time
}

/* ============================ first-run setup ============================ */

const WZ_STEPS = ["Welcome", "Connect", "All set"];
let wzStep = 0, wzOk = false;

const wzOpen = () => !$("welcome").classList.contains("hidden");

function wzStatus(msg, cls = "") { const el = $("wz-status"); if (el) { el.textContent = msg; el.className = "wz-status " + cls; } }

function wzRender() {
  $("wz-dots").innerHTML = WZ_STEPS.map((_, i) => `<i class="${i <= wzStep ? "on" : ""}"></i>`).join("");
  $("wz-title").textContent = ["Welcome to Brawl Stars Hub", "Connect your account", "You're all set"][wzStep];
  $("wz-back").style.visibility = wzStep ? "visible" : "hidden";
  $("wz-skip").style.visibility = wzStep < 2 ? "visible" : "hidden";
  $("wz-next").textContent = ["Get started", wzOk ? "Next" : "Connect", "Finish"][wzStep];
  $("wz-body").innerHTML = [
    `<p>This app tracks your games, elo and trophies while it runs, and shows them as graphs, goals and Discord presence. It needs a free <b>Brawl Stars API key</b> to read your public stats. It takes about two minutes.</p>
     <ol>
       <li>Open <b>developer.brawlstars.com</b> and sign in with your Supercell ID.</li>
       <li>Go to <b>My Account</b> and choose <b>Create New Key</b>.</li>
       <li>Give it any name, and in the IP address box enter your <b>public IP address</b> (search “what is my IP”).</li>
       <li>Copy the long key it shows you.</li>
     </ol>
     <div class="wz-link"><code>developer.brawlstars.com</code><button class="btn ghost" id="wz-copy">Copy address</button></div>
     <p class="muted small">Keys only work from the IP address you entered. If your internet address changes, the app will tell you, and you add the new one on the same page. Your key stays on this computer.</p>`,
    `<div class="wz-field"><label for="wz-key">API key</label><input id="wz-key" type="password" autocomplete="off" spellcheck="false" placeholder="Paste the key" value="${esc(lastSettings.api_key || "")}" /></div>
     <div class="wz-field"><label for="wz-tag">Your player tag</label><input id="wz-tag" type="text" autocomplete="off" spellcheck="false" placeholder="#2PP0YLQ" value="${esc(lastSettings.player_tag || "")}" /></div>
     <p class="muted small">Your tag is under your name on your in-game profile.</p>
     <div id="wz-status" class="wz-status"></div>`,
    `<p>${wzName ? `Connected as <b>${esc(wzName)}</b>. ` : ""}The app works in the background, so leave it running (it sits in the tray) and your history builds up as you play.</p>
     <ol>
       <li>Add friends under <b>Favorites</b> to track them too.</li>
       <li>Set a target on <b>Stats &amp; Graphs</b> to follow your progress.</li>
       <li>Change the look under <b>Settings → Appearance</b>.</li>
       <li>Press <kbd>?</kbd> any time to see the keyboard shortcuts.</li>
     </ol>`,
  ][wzStep];
  if (wzStep === 0) $("wz-copy").onclick = () => navigator.clipboard?.writeText("developer.brawlstars.com").then(() => toast("Copied."), () => toast("Couldn't copy. Type it into your browser.", true));
  if (wzStep === 1) { $("wz-key").oninput = $("wz-tag").oninput = () => { wzOk = false; $("wz-next").textContent = "Connect"; }; setTimeout(() => ($("wz-key").value ? $("wz-tag") : $("wz-key")).focus(), 30); }
}

let wzName = "";
function wzClose(done) {
  $("welcome").classList.add("hidden");
  try { if (done) localStorage.setItem("bsh-welcome-done", "1"); else sessionStorage.setItem("bsh-welcome-skip", "1"); } catch (e) { /* private mode */ }
  if (!lastSettings.api_key) showPage("settings");
}

async function wzNext() {
  if (wzStep === 1 && !wzOk) {
    const key = $("wz-key").value.trim(), tag = $("wz-tag").value.trim();
    if (!key || !tag) { wzStatus("Enter both your API key and your player tag.", "err"); return; }
    wzStatus("Checking…");
    $("wz-next").disabled = true;
    try {
      lastSettings = { ...lastSettings, api_key: key, player_tag: tag };
      await invoke("save_settings", { settings: lastSettings });
      const p = await invoke("fetch_player", { tag });
      wzName = p.name; wzOk = true;
      await loadSettings();
      wzStatus(`Connected! Found ${p.name}.`, "ok");
      $("wz-next").textContent = "Next";
    } catch (e) { wzStatus(errText(e), "err"); }
    $("wz-next").disabled = false;
    return;
  }
  if (wzStep === 2) { wzClose(true); return; }
  wzStep++; wzRender();
}

$("wz-next").onclick = wzNext;
$("wz-back").onclick = () => { if (wzStep) { wzStep--; wzRender(); } };
$("wz-skip").onclick = () => wzClose(false);

// Shown on first launch, or whenever the key or tag is missing (until finished or skipped this time).
function maybeWelcome() {
  let seen = false, skipped = false;
  try { seen = !!localStorage.getItem("bsh-welcome-done"); skipped = !!sessionStorage.getItem("bsh-welcome-skip"); } catch (e) { /* ignore */ }
  const needs = !lastSettings.api_key || !lastSettings.player_tag;
  if (!needs || skipped || (seen && lastSettings.api_key)) return false;
  wzStep = 0; wzOk = false; wzName = "";
  $("welcome").classList.remove("hidden");
  wzRender();
  return true;
}

/* ===================== quick search & keyboard shortcuts ===================== */

let palItems = [], palSel = 0, palFavs = [];
const palOpen = () => !$("palette").classList.contains("hidden");

function palBuild(q) {
  const query = q.trim().toLowerCase();
  const pages = [...document.querySelectorAll(".nav-item")].map((b) => ({ kind: "Page", label: b.textContent.replace(/^[^\p{L}\p{N}]+/u, "").trim(), run: () => showPage(b.dataset.page) }));
  const me = lastSettings.player_tag ? [{ kind: "You", label: "My profile", run: () => openProfile(lastSettings.player_tag, currentPage) }] : [];
  const favs = palFavs.map((f) => ({ kind: "Favorite", label: `${f.name || "#" + f.tag}  #${f.tag}`, run: () => openProfile(f.tag, currentPage) }));
  const all = [...pages, ...me, ...favs];
  let out = query ? all.filter((i) => i.label.toLowerCase().includes(query)) : all;
  const tag = normTag(q);
  if (tag.length >= 3 && /^[0289PYLQGRJCUV]+$/.test(tag)) out = [{ kind: "Player", label: `Open player #${tag}`, run: () => openProfile(tag, currentPage) }, ...out];
  return out.slice(0, 40);
}

function palRender() {
  $("pal-list").innerHTML = palItems.map((it, i) => `<div class="pal-item ${i === palSel ? "sel" : ""}" role="option" data-i="${i}"><span>${esc(it.label)}</span><span class="pi-kind">${esc(it.kind)}</span></div>`).join("")
    || `<div class="muted small" style="padding:10px 12px">Nothing matches. Type a player tag to look someone up.</div>`;
  $("pal-list").querySelector(".sel")?.scrollIntoView({ block: "nearest" });
}

async function openPalette() {
  $("palette").classList.remove("hidden");
  $("pal-input").value = ""; palSel = 0;
  palFavs = []; palItems = palBuild(""); palRender();
  $("pal-input").focus();
  try { palFavs = (await invoke("get_favorites")).map((f) => ({ tag: f.tag, name: f.name })); } catch (e) { /* none yet */ }
  if (palOpen()) { palItems = palBuild($("pal-input").value); palRender(); }
}
const closePalette = () => $("palette").classList.add("hidden");
function palPick(i) { const it = palItems[i]; if (!it) return; closePalette(); it.run(); }

$("pal-input").oninput = () => { palSel = 0; palItems = palBuild($("pal-input").value); palRender(); };
$("pal-input").onkeydown = (ev) => {
  if (ev.key === "ArrowDown") { ev.preventDefault(); palSel = Math.min(palItems.length - 1, palSel + 1); palRender(); }
  else if (ev.key === "ArrowUp") { ev.preventDefault(); palSel = Math.max(0, palSel - 1); palRender(); }
  else if (ev.key === "Enter") { ev.preventDefault(); palPick(palSel); }
};
$("pal-list").onclick = (ev) => { const r = ev.target.closest("[data-i]"); if (r) palPick(Number(r.dataset.i)); };
$("palette").onclick = (ev) => { if (ev.target === $("palette")) closePalette(); };
$("shortcuts").onclick = (ev) => { if (ev.target === $("shortcuts") || ev.target.id === "sc-close") $("shortcuts").classList.add("hidden"); };

document.addEventListener("keydown", (ev) => {
  const t = ev.target, typing = t && (/^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.isContentEditable);
  if (ev.key === "Escape") {
    for (const id of ["palette", "shortcuts"]) if (!$(id).classList.contains("hidden")) { $(id).classList.add("hidden"); ev.preventDefault(); return; }
    return;
  }
  if (wzOpen()) return;
  if ((ev.ctrlKey || ev.metaKey) && !ev.altKey && ev.key.toLowerCase() === "k") { ev.preventDefault(); palOpen() ? closePalette() : openPalette(); return; }
  if (typing || ev.ctrlKey || ev.metaKey || ev.altKey || document.querySelector(".modal-back:not(.hidden)")) return;
  if (ev.key === "?") { ev.preventDefault(); $("shortcuts").classList.remove("hidden"); return; }
  if (/^[1-9]$/.test(ev.key)) { const b = document.querySelectorAll(".nav-item")[Number(ev.key) - 1]; if (b) { ev.preventDefault(); b.click(); } return; }
  if (ev.key.toLowerCase() === "r") {
    const btn = document.querySelector(".page.active button[id$='refresh']");
    if (btn && !btn.disabled) { ev.preventDefault(); btn.click(); }
  }
});

/* =============================== boot =============================== */

(async () => {
  try {
    await loadSettings();
    await refreshFavTags();
    invoke("get_icons").then((m) => { icons = m || {}; }).catch(() => {});
    setPresenceUI(await invoke("presence_running"));
    reportTimezone(); setInterval(reportTimezone, 30 * 60 * 1000);
    if (!maybeWelcome() && !lastSettings.api_key) showPage("settings");
    invoke("app_version").then((v) => { $("upd-version").textContent = v; }).catch(() => {});
    // Just updated? Show what changed (once).
    invoke("whats_new", { force: false }).then((w) => {
      if (w) showWhatsNew(`Updated to version ${w.version}`, w.notes, () => { invoke("ack_whats_new").catch(() => {}); maybeRecap(); });
      else maybeRecap();
    }).catch(() => maybeRecap());
    setTimeout(() => checkForUpdate(false), 5000);
    setInterval(() => checkForUpdate(false), 6 * 3600 * 1000);
  } catch (e) {
    console.error(e);
  }
})();
