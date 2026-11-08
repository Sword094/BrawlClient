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
function toast(msg, isErr = false) {
  const el = $("toast");
  el.textContent = msg;
  el.className = "toast" + (isErr ? " err" : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add("hidden"), isErr ? 5000 : 2200);
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
  return `<img class="bricon" src="${esc(list[0])}" data-alts="${esc(JSON.stringify(list.slice(1)))}" width="${size}" height="${size}" alt="" loading="lazy" />`;
}
document.addEventListener("error", (ev) => {
  const img = ev.target;
  if (!(img instanceof HTMLImageElement) || !img.classList.contains("bricon")) return;
  let alts = [];
  try { alts = JSON.parse(img.dataset.alts || "[]"); } catch (e) { /* ignore */ }
  const next = alts.shift();
  if (next) { img.dataset.alts = JSON.stringify(alts); img.src = next; }
  else { img.style.display = "none"; }
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
  $("recent-loading").classList.remove("hidden");
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
    if (token === recentToken) $("recent-loading").classList.add("hidden");
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

  return `<div class="battle ${cls}" ${m.ranked ? 'data-ranked="1"' : ""}>
    <div class="b-head" data-toggle title="Click for full match details">
      <span class="b-res">${label}</span>${score}${rounds}
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
function lineChart(points, { color, unit }) {
  const pts = points.filter((p) => p[1] > 0);
  if (pts.length < 2) return `<div class="empty small">Not enough readings yet. Keep the app running while you play.</div>`;
  const W = 760, H = 220, L = 54, R = 12, T = 12, B = 26;
  const t0 = pts[0][0], t1 = pts[pts.length - 1][0];
  let lo = Math.min(...pts.map((p) => p[1])), hi = Math.max(...pts.map((p) => p[1]));
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
  const last = pts[pts.length - 1];
  return `<svg viewBox="0 0 ${W} ${H}" class="chart" role="img" aria-label="${esc(unit)} over time">
    ${grid}
    <text x="${L}" y="${H - 6}" class="gt">${esc(fmt(t0))}</text>
    <text x="${W - R}" y="${H - 6}" text-anchor="end" class="gt">${esc(fmt(t1))}</text>
    <path d="${d}" fill="none" stroke="${color}" stroke-width="2.2" stroke-linejoin="round"/>
    <circle cx="${x(last[0])}" cy="${y(last[1])}" r="4" fill="${color}"/>
  </svg>
  <div class="chart-foot muted small">Latest ${num(last[1])} ${esc(unit)} · low ${num(Math.min(...pts.map((p) => p[1])))} · high ${num(Math.max(...pts.map((p) => p[1])))}</div>`;
}

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
const DAY = 86400;
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
        if (mode === "battles") style = `background:rgba(255,206,0,${a})`;
        else style = c.w > c.l ? `background:rgba(61,214,140,${a})` : c.w < c.l ? `background:rgba(255,92,92,${a})` : `background:rgba(150,150,170,${a})`;
        tip = `${label}: ${c.n} battle${c.n === 1 ? "" : "s"} · ${c.w}W–${c.l}L · ${rate}% win rate`;
      }
      cells += `<i class="hm" style="${style}" data-tip="${esc(tip)}"></i>`;
    }
    const m = new Date(start.getTime() + w * 7 * DAY * 1000).getMonth();
    months += `<span>${m !== lastMonth ? new Date(start.getTime() + w * 7 * DAY * 1000).toLocaleDateString([], { month: "short" }) : ""}</span>`;
    lastMonth = m;
    cols += `<div class="hm-col">${cells}</div>`;
  }
  const legend = mode === "battles"
    ? `<span class="hm-key"><i class="hm" style="background:rgba(255,206,0,.25)"></i> few <i class="hm" style="background:rgba(255,206,0,1)"></i> many battles</span>`
    : `<span class="hm-key"><i class="hm" style="background:rgba(61,214,140,.8)"></i> more wins <i class="hm" style="background:rgba(150,150,170,.8)"></i> even <i class="hm" style="background:rgba(255,92,92,.8)"></i> more losses</span>`;
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
function gainLine(pts, unit) {
  const v = pts.filter((p) => p[1] > 0);
  if (v.length < 2) return "";
  const d = v[v.length - 1][1] - v[0][1];
  const word = { today: "today", "7": "in the last 7 days", "30": "in the last 30 days", all: "overall" }[statsRange];
  return `<div class="gain ${d > 0 ? "pos" : d < 0 ? "neg" : ""}"><b>${d > 0 ? "+" : ""}${num(d)}</b> ${esc(unit)} ${esc(word)}</div>`;
}

function chartSection() {
  const r = statsData;
  const elo = statsMetric === "elo";
  const ranged = seriesForRange(elo ? r.series.elo : r.series.trophies);
  const seg = (cls, key, cur, items) => `<div class="seg" data-seg="${key}">${items.map(([v, l]) =>
    `<button class="${cur === v ? "on" : ""}" data-v="${v}">${l}</button>`).join("")}</div>`;
  return `<div class="section-head"><h2>${elo ? "Ranked elo" : "Trophies"}</h2><div class="seg-row">
      ${seg("", "metric", statsMetric, [["trophies", "Trophies"], ["elo", "Ranked elo"]])}
      ${seg("", "range", statsRange, [["today", "Today"], ["7", "7 days"], ["30", "30 days"], ["all", "All time"]])}</div></div>
    <div class="card">${gainLine(ranged, elo ? "elo" : "trophies")}${lineChart(ranged,
      elo ? { color: "#b79cff", unit: "elo" } : { color: "#ffce00", unit: "trophies" })}</div>`;
}

function heatSection() {
  const seg = [["battles", "Battles"], ["winrate", "Win rate"]];
  return `<div class="section-head"><h2>Activity</h2><div class="seg-row"><div class="seg" data-seg="heat">${seg.map(([v, l]) =>
    `<button class="${heatMode === v ? "on" : ""}" data-v="${v}">${l}</button>`).join("")}</div></div></div>
    <div class="card">${heatMap(statsData.days || [], heatMode)}</div>`;
}

function paintStats() {
  const r = statsData, s = r.summary;
  $("stats-body").innerHTML = `
    ${statsTiles(s)}
    ${heatSection()}
    ${chartSection()}
    <div class="section-head"><h2>Win rates</h2></div>
    <div class="two-col">${rateTable("By brawler", s.by_brawler, true)}${rateTable("By mode", s.by_mode, false)}${rateTable("By map", s.by_map, false)}</div>
    <div class="section-head"><h2>Recent sessions</h2></div>
    ${sessionRows(r.sessions)}
    <div class="danger-row"><button class="btn ghost danger" id="stats-clear">${clearArmed ? "Click again to delete saved history" : "Clear history"}</button>
      <span class="muted small">Deletes this player's saved games and graphs from this computer only.</span></div>`;
}

$("stats-body").addEventListener("click", async (ev) => {
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
  paintStats();
}

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
  $("set-autopresence").checked = !!lastSettings.auto_start_presence;
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
  "pc-custom": "presence_custom", "pc-images": "presence_images", "pc-button": "presence_button",
};
const VIEW_TITLES = {
  latest: "Latest match", session: "Current session", trophies: "Trophies", ranked: "Ranked",
  streak: "Win streak", victories: "Victories", record: "Recent record", club: "Club", custom: "Custom text",
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
  for (const [id, key] of Object.entries(PC_TOGGLES)) $(id).checked = key === "presence_images" || key === "presence_button" || key === "presence_custom" || key === "presence_club"
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

/* =============================== boot =============================== */

(async () => {
  try {
    await loadSettings();
    await refreshFavTags();
    invoke("get_icons").then((m) => { icons = m || {}; }).catch(() => {});
    setPresenceUI(await invoke("presence_running"));
    if (!lastSettings.api_key) showPage("settings");
  } catch (e) {
    console.error(e);
  }
})();
