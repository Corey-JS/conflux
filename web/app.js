// Conflux: search box -> /api/search, tap a result -> the provider's own page.
"use strict";

const $ = (id) => document.getElementById(id);
const els = {
  form: $("search-form"), q: $("q"), clear: $("clear"), providers: $("providers"),
  results: $("results"), heading: $("heading"), note: $("note"),
  edit: $("edit"), editHint: $("edit-hint"), profileBtn: $("profile-btn"), sheet: $("profiles"),
  plist: $("plist"), hlist: $("hlist"), toast: $("toast"), genres: $("genres"), groups: $("groups"),
};

const state = { q: "", kind: "all", provider: "all", genre: "all" };
const names = {}; // provider id -> display name
let inflight = null;
let timer = null;

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
};

// ---- Genres ----
// Each provider names its genres its own way (Plex "Kids & Family", Pluto
// "Kids", Roku "Children-Music"), about 110 labels in all. Each label goes in
// the first group whose pattern it matches; anything left over is "Other".
const GENRES = [
  ["News", /news|weather|opinion/i],
  ["En Español", /espa[nñ]ol|spanish|latino/i],
  ["Kids & Family", /kid|famil|child|animated|cartoon/i],
  ["Anime", /anime/i],
  ["Sports", /sport|ball|soccer|boxing|wrestl|racing|olympic|hockey|bowling|billiard|martial|fishing|bik|bicycle|bmx|poker|outdoor|^auto$/i],
  ["Music", /music/i],
  ["Crime & Mystery", /crime|law|mystery|suspense|thriller|paranormal/i],
  ["Sci-Fi & Action", /sci-fi|science fiction|action|adventure|fantasy|horror|adrenaline/i],
  ["Comedy", /comed|sitcom/i],
  ["Drama", /drama|romance/i],
  ["Movies", /movie/i],
  ["Classic TV", /classic|western/i],
  ["Reality", /reality|bingeworthy|auction|medical/i],
  ["Game Shows & Talk", /game show|daytime|talk|card game/i],
  ["Food & Home", /food|home|cook|house|garden|travel|fashion|shopping/i],
  ["Nature & Science", /nature|animal|science|history|documentar|educat|explore|environment/i],
  ["Global", /global|international/i],
  ["Faith", /faith|relig/i],
  ["Entertainment", /entertain|special|season/i],
];
const OTHER = "Other";
const genreOrder = new Map([...GENRES.map(([g]) => g), OTHER].map((g, i) => [g, i]));

function genreOf(label) {
  const hit = GENRES.find(([, re]) => re.test(label));
  return hit ? hit[0] : OTHER;
}

// The genre groups a channel belongs to (cached on the item).
function genresOf(item) {
  if (!item._g) {
    const set = new Set((item.genres || []).map(genreOf));
    if (!set.size) set.add(OTHER);
    item._g = [...set];
  }
  return item._g;
}

// ---- Profiles: who's watching, and the channels each person has hidden ----
// Saved on this device only, as { current, list: [{ id, name, hidden }] }.
// `hidden` maps a channel's id to its title and provider, so the list of
// hidden channels can be shown even when that provider is down.
const profiles = {
  list: [],
  current: null,
  load() {
    try {
      const d = JSON.parse(store.get("profiles") || "null");
      if (d && Array.isArray(d.list)) {
        this.list = d.list.filter((p) => p && p.id && typeof p.name === "string")
          .map((p) => ({
            id: p.id, name: p.name,
            hidden: p.hidden && typeof p.hidden === "object" ? p.hidden : {},
            hiddenGenres: Array.isArray(p.hiddenGenres) ? p.hiddenGenres : [],
            hiddenProviders: Array.isArray(p.hiddenProviders) ? p.hiddenProviders : [],
          }));
        this.current = d.current;
      }
    } catch { /* start fresh */ }
    if (!this.list.length) this.list.push(this.make("Me"));
    if (!this.list.some((p) => p.id === this.current)) this.current = this.list[0].id;
  },
  save() { store.set("profiles", JSON.stringify({ current: this.current, list: this.list })); },
  make(name) {
    return { id: "p" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6), name, hidden: {}, hiddenGenres: [], hiddenProviders: [] };
  },
  get me() { return this.list.find((p) => p.id === this.current); },
  // A hidden provider hides everything from it; hidden genres and single
  // channels apply to live channels only (movies and shows use other genre names).
  isHidden(item) {
    const me = this.me;
    if (me.hiddenProviders.includes(item.provider)) return true;
    if (item.kind !== "live") return false;
    return Object.hasOwn(me.hidden, item.id) || genresOf(item).some((g) => me.hiddenGenres.includes(g));
  },
  providerHidden(id) { return this.me.hiddenProviders.includes(id); },
  toggle(listName, value) {
    const list = this.me[listName];
    const i = list.indexOf(value);
    if (i < 0) list.push(value); else list.splice(i, 1);
    this.save();
  },
  hide(item) { this.me.hidden[item.id] = { title: item.title, provider: item.provider }; this.save(); },
  unhide(id) { delete this.me.hidden[id]; this.save(); },
};

const AVATAR_COLORS = ["#5ee6c9", "#ffd23f", "#ff7a3d", "#a77bfa", "#5b8cff", "#ff6b9a"];
function avatar(p) {
  let h = 0;
  for (const c of p.id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return el("span", { class: "avatar", style: `background:${AVATAR_COLORS[h % AVATAR_COLORS.length]}`, "aria-hidden": "true" },
    (p.name.trim().charAt(0) || "?").toUpperCase());
}

function el(tag, attrs = {}, ...children) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") n.className = v;
    else n.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children) if (c != null) n.append(c);
  return n;
}

function svg(path) {
  const ns = "http://www.w3.org/2000/svg";
  const s = document.createElementNS(ns, "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("class", "go");
  s.setAttribute("aria-hidden", "true");
  const p = document.createElementNS(ns, "path");
  p.setAttribute("d", path);
  s.append(p);
  return s;
}

const KIND_LABEL = { movie: "Movie", series: "Show", live: "Live" };

function row(item) {
  if (!/^https:\/\//.test(item.watch_url)) return null;
  const isLive = item.kind === "live";
  const thumb = el("div", { class: "thumb " + (isLive ? "logo" : "poster") });
  if (item.image) {
    const img = el("img", { src: item.image, alt: "", loading: "lazy", decoding: "async", referrerpolicy: "no-referrer" });
    img.addEventListener("error", () => { img.remove(); thumb.textContent = initial(item.title); }, { once: true });
    thumb.append(img);
  } else {
    thumb.textContent = initial(item.title);
  }

  const meta = el("div", { class: "meta" }, el("i", { class: "dot " + item.provider }));
  const bits = [names[item.provider] || item.provider];
  if (isLive) {
    meta.append(el("b", { class: "live-badge" }, "LIVE"));
    if (item.channel_number) bits.push("Ch " + item.channel_number);
  } else {
    bits.push(KIND_LABEL[item.kind] || "");
    if (item.year) bits.push(String(item.year));
  }
  if (item.genres && item.genres[0]) bits.push(item.genres[0]);
  meta.append(el("span", {}, bits.filter(Boolean).join(" · ")));

  const label = `${item.title}, ${names[item.provider] || item.provider}`;
  const a = el("a", { href: item.watch_url, target: "_blank", rel: "noopener noreferrer", "aria-label": label },
    thumb,
    el("div", { class: "info" }, el("div", { class: "title" }, item.title), meta),
    svg("M7 17L17 7M9 7h8v8"));
  return el("li", {}, a);
}

// ---- TV guide (live channels with what's on now and next) ----
const fmtTime = new Intl.DateTimeFormat([], { hour: "numeric", minute: "2-digit" });
const live = { items: [], skew: 0, fetchedAt: 0 }; // skew: server clock minus ours, in ms
const expanded = new Set();
let guideRows = []; // what renderGuide drew: [{ item, key, li, bar, start, stop }]
let view = null; // "guide" or "results" once drawn, so refreshes can skip the skeleton
const serverNow = () => Date.now() + live.skew;
const clock = (sec) => fmtTime.format(new Date(sec * 1000));
const progress = (start, stop, t) => Math.min(100, Math.max(0, ((t - start) / (stop - start)) * 100));

// Which program a row shows and whether it has started. A row only needs
// rebuilding when this changes; otherwise just its progress bar moves.
function guideKey(item, t) {
  const p = (item.programs || []).find((x) => x.stop > t);
  return p ? `${p.start}${p.start <= t ? "+" : "-"}` : "";
}

function guideRow(item, t) {
  const progs = (item.programs || []).filter((p) => p.stop > t);
  if (!progs.length) return { li: row(item) };
  const [now, ...later] = progs;
  const airing = now.start <= t;
  const pct = airing ? progress(now.start, now.stop, t) : 0;

  // Art for the show that's on, like album art; the channel logo is the fallback.
  const art = now.image || item.image;
  const thumb = el("div", { class: "thumb " + (now.image ? "poster" : "logo") });
  if (art) {
    const img = el("img", { src: art, alt: "", loading: "lazy", decoding: "async", referrerpolicy: "no-referrer" });
    img.addEventListener("error", () => {
      img.remove();
      if (item.image && art !== item.image) {
        thumb.className = "thumb logo";
        thumb.append(el("img", { src: item.image, alt: "", referrerpolicy: "no-referrer" }));
      } else thumb.textContent = initial(item.title);
    }, { once: true });
    thumb.append(img);
  } else thumb.textContent = initial(item.title);

  const chan = el("div", { class: "meta" }, el("i", { class: "dot " + item.provider }),
    el("span", {}, [item.channel_number ? "Ch " + item.channel_number : null, item.title].filter(Boolean).join(" · ")));
  const when = airing ? `${clock(now.start)} – ${clock(now.stop)}` : `Starts ${clock(now.start)}`;
  const sub = [now.subtitle, when, now.rating].filter(Boolean).join(" · ");
  const info = el("div", { class: "info" }, chan,
    el("div", { class: "title" }, now.title),
    el("div", { class: "g-sub" }, sub));
  const bar = airing ? el("i", { style: `width:${pct.toFixed(1)}%` }) : null;
  if (bar) info.append(el("div", { class: "bar", "aria-hidden": "true" }, bar));
  if (later[0]) info.append(el("div", { class: "g-next" }, el("b", {}, "Next "), `${clock(later[0].start)}  ${later[0].title}`));

  const watch = el("a", { class: "watch", href: item.watch_url, target: "_blank", rel: "noopener noreferrer", "aria-label": `Watch ${item.title}` },
    svg("M7 17L17 7M9 7h8v8"));
  const open = expanded.has(item.id);
  const main = el("div", { class: "g-main", role: "button", tabindex: "0", "aria-expanded": String(open) }, thumb, info, watch);
  const li = el("li", { class: "guide" }, main);

  // Most rows are never opened, so the full schedule is built on first open.
  let sched = null;
  const show = (o) => {
    if (o && !sched) li.append(sched = schedList(progs));
    if (sched) sched.hidden = !o;
  };
  show(open);

  const toggle = () => {
    const o = main.getAttribute("aria-expanded") !== "true";
    main.setAttribute("aria-expanded", String(o));
    show(o);
    if (o) expanded.add(item.id); else expanded.delete(item.id);
  };
  main.addEventListener("click", (e) => { if (!e.target.closest("a")) toggle(); });
  main.addEventListener("keydown", (e) => {
    if ((e.key === "Enter" || e.key === " ") && e.target === main) { e.preventDefault(); toggle(); }
  });
  return { li, bar, start: now.start, stop: now.stop };
}

// Each guide row carries a hide button, shown only while editing.
function guideEntry(item, t) {
  const r = guideRow(item, t);
  if (!r.li) return r;
  r.li.classList.add("hideable");
  r.li.append(el("button", { type: "button", class: "hide", "data-hide": item.id, "aria-label": `Hide ${item.title}` }, "\u2212"));
  return r;
}

function schedList(progs) {
  return el("ol", { class: "g-sched" }, ...progs.map((p) =>
    el("li", {}, el("time", {}, clock(p.start)),
      el("div", {}, el("b", {}, p.title), p.subtitle ? el("span", {}, " · " + p.subtitle) : null,
        p.description ? el("p", {}, p.description) : null))));
}

// The guide has 1,400+ channels; building them all at once froze phones for
// a moment on every refresh. Rows are built a batch at a time, and the next
// batch is added when the end of the list comes within a few screens.
const GUIDE_BATCH = 50;
let guideNext = 0; // index in live.items of the next row to build
let guideFor = null; // the provider and genre filters the guide was drawn for
const guideEnd = el("li", { class: "guide-end", "aria-hidden": "true" });
const guideMore = "IntersectionObserver" in window
  ? new IntersectionObserver((entries) => {
    if (view === "guide" && entries.some((e) => e.isIntersecting)) appendGuideRows(GUIDE_BATCH);
  }, { rootMargin: "0px 0px 2000px 0px" })
  : null;

function renderGuide() {
  // A refresh of the same list rebuilds as many rows as were showing, so
  // the scroll position holds. A different filter starts from the top batch.
  const drawnFor = state.provider + "|" + state.genre;
  const keep = view === "guide" && guideFor === drawnFor ? guideRows.length : 0;
  guideFor = drawnFor;
  guideRows = [];
  guideNext = 0;
  // Rows added while scrolling, or rebuilt as shows change, aren't news to
  // announce; search results are.
  els.results.setAttribute("aria-live", "off");
  els.results.replaceChildren();
  view = "guide";
  // The header stays on screen, so a filter can change from deep in the
  // list; the new list starts at its top.
  if (!keep) window.scrollTo(0, 0);
  appendGuideRows(Math.max(GUIDE_BATCH, keep));
  if (!live.items.length) note("No live channels loaded yet. Try again in a minute.");
  else if (!guideRows.length) {
    note(state.genre !== "all" ? `No ${state.genre} channels here.` : "Every channel here is hidden. Open your profile to bring some back.");
  }
}

function appendGuideRows(n) {
  const t = serverNow() / 1000;
  const lis = [];
  while (lis.length < n && guideNext < live.items.length) {
    const item = live.items[guideNext++];
    if (profiles.isHidden(item)) continue;
    if (state.genre !== "all" && !genresOf(item).includes(state.genre)) continue;
    const r = guideEntry(item, t);
    if (!r.li) continue;
    guideRows.push({ item, key: guideKey(item, t), ...r });
    lis.push(r.li);
  }
  guideEnd.remove();
  els.results.append(...lis);
  if (guideNext >= live.items.length) return;
  if (!guideMore) { appendGuideRows(Infinity); return; }
  els.results.append(guideEnd);
  // Watching afresh always reports once, so a batch too short to push the
  // end out of range is followed by another.
  guideMore.unobserve(guideEnd);
  guideMore.observe(guideEnd);
}

// Move progress bars, and rebuild only the rows whose show has changed.
// Rebuilding every row every 30s would drop focus and redo every image.
function tickGuide() {
  const t = serverNow() / 1000;
  for (const r of guideRows) {
    const key = guideKey(r.item, t);
    if (key !== r.key) {
      const fresh = guideEntry(r.item, t);
      if (!fresh.li) continue;
      r.li.replaceWith(fresh.li);
      Object.assign(r, { bar: null }, fresh, { key });
    } else if (r.bar) {
      r.bar.style.width = `${progress(r.start, r.stop, t).toFixed(1)}%`;
    }
  }
}

function initial(title) {
  return (title || "?").trim().charAt(0).toUpperCase();
}

function skeleton(n = 6) {
  view = null;
  els.results.replaceChildren(...Array.from({ length: n }, () =>
    el("li", { class: "skeleton" }, el("a", {},
      el("div", { class: "thumb" }), el("div", { class: "info" },
        el("div", { class: "title" }, "Loading title"), el("div", { class: "meta" }, "Loading")), el("span")))));
}

function note(text) {
  els.note.hidden = !text;
  els.note.textContent = text || "";
}

function render(items, emptyText) {
  els.results.setAttribute("aria-live", "polite");
  els.results.replaceChildren(...items.filter((i) => !profiles.isHidden(i)).map(row).filter(Boolean));
  view = "results";
  if (!items.length) note(emptyText);
}

async function getJSON(url, signal) {
  const r = await fetch(url, { signal, headers: { Accept: "application/json" } });
  if (!r.ok) throw new Error(r.status === 503 ? "offline" : `HTTP ${r.status}`);
  return r.json();
}

function params(extra) {
  const p = new URLSearchParams(extra);
  if (state.kind !== "all") p.set("kind", state.kind);
  if (state.provider !== "all") p.set("provider", state.provider);
  return p;
}

async function update() {
  if (inflight) inflight.abort();
  inflight = new AbortController();
  const signal = inflight.signal;
  note("");
  const q = state.q.trim();
  els.clear.hidden = !q;
  const url = new URL(location.href);
  if (q) url.searchParams.set("q", q); else url.searchParams.delete("q");
  history.replaceState(null, "", url);

  els.edit.hidden = true;
  els.genres.hidden = true;
  if (q) setEditing(false);
  try {
    if (!q) {
      if (state.kind === "movie" || state.kind === "series") {
        els.heading.textContent = state.kind === "movie" ? "Movies" : "Shows";
        view = null;
        els.results.replaceChildren();
        note("Type a title to search free movies and shows.");
        return;
      }
      els.heading.textContent = "Live channels";
      els.edit.hidden = false;
      // A background refresh redraws in place instead of flashing the skeleton.
      if (view !== "guide") skeleton();
      const p = params({}); // no limit: the server sends whole lineups
      p.delete("kind");
      const data = await getJSON("/api/live?" + p, signal);
      live.items = data.items;
      live.skew = data.now ? data.now * 1000 - Date.now() : 0;
      live.fetchedAt = Date.now();
      renderGenres();
      renderGuide();
      return;
    }
    els.heading.textContent = "Results";
    skeleton(4);
    const data = await getJSON("/api/search?" + params({ q }), signal);
    render(data.items, `Nothing free found for “${q}”.`);
    if (data.unavailable && data.unavailable.length) {
      const msg = `${data.unavailable.join(" and ")} didn't answer this time.`;
      note(data.items.length ? msg : `Nothing found for “${q}”. ${msg}`);
    }
  } catch (e) {
    if (e.name === "AbortError") return;
    view = null;
    els.results.replaceChildren();
    note(navigator.onLine === false || e.message === "offline"
      ? "You're offline. Search needs a connection."
      : "Couldn't reach the Conflux server. Try again.");
  }
}

function schedule(delay = 250) {
  clearTimeout(timer);
  timer = setTimeout(update, delay);
}

function setPressed(container, attr, value) {
  for (const b of container.querySelectorAll("button")) {
    b.setAttribute("aria-pressed", String(b.dataset[attr] === value));
  }
}

let providerList = []; // from /api/providers

async function loadProviders() {
  try {
    providerList = (await getJSON("/api/providers")).providers || [];
  } catch { /* chips are optional */ }
  for (const p of providerList) names[p.id] = p.name;
  renderProviders();
}

// Provider chips, minus the ones this profile has hidden.
function renderProviders() {
  const chip = (id, label, down) => {
    const b = el("button", { type: "button", "data-provider": id, "aria-pressed": String(state.provider === id), class: down ? "down" : null });
    if (id !== "all") b.append(el("i", { class: "dot " + id }));
    b.append(label);
    return b;
  };
  const shown = providerList.filter((p) => !profiles.providerHidden(p.id));
  const chips = [chip("all", "All providers")];
  for (const p of shown) {
    const down = Boolean(p.catalog_error && !p.catalog_items) || Boolean(p.search_error && !p.mode.includes("catalog"));
    chips.push(chip(p.id, p.name, down));
  }
  if (state.provider !== "all" && !shown.some((p) => p.id === state.provider)) {
    state.provider = "all";
    store.set("provider", "all");
  }
  els.providers.replaceChildren(...chips);
}

// Genre chips for the guide: the groups in the loaded lineup that this
// profile hasn't hidden, busiest first. The chosen one stays even when the
// current provider has none, so it can be switched off.
function renderGenres() {
  const count = new Map();
  for (const item of live.items) {
    if (profiles.isHidden(item)) continue;
    for (const g of genresOf(item)) count.set(g, (count.get(g) || 0) + 1);
  }
  if (state.genre !== "all" && profiles.me.hiddenGenres.includes(state.genre)) {
    state.genre = "all";
    store.set("genre", "all");
  }
  if (state.genre !== "all" && !count.has(state.genre)) count.set(state.genre, 0);
  const genres = [...count.keys()].sort((a, b) =>
    (a === OTHER) - (b === OTHER) || count.get(b) - count.get(a) || genreOrder.get(a) - genreOrder.get(b));
  const chip = (g, label) => el("button", { type: "button", "data-genre": g, "aria-pressed": String(state.genre === g) }, label);
  els.genres.replaceChildren(chip("all", "All genres"), ...genres.map((g) => chip(g, g)));
  els.genres.hidden = genres.length < 2;
}

// ---- Editing the guide: hide channels with an undo ----
function setEditing(on) {
  document.body.classList.toggle("editing", on);
  els.edit.setAttribute("aria-pressed", String(on));
  els.edit.textContent = on ? "Done" : "Edit";
  els.editHint.hidden = !on;
}

function hideChannel(id) {
  const i = guideRows.findIndex((r) => r.item.id === id);
  if (i < 0) return;
  const [r] = guideRows.splice(i, 1);
  profiles.hide(r.item);
  r.li.remove();
  expanded.delete(id);
  renderProfileBtn();
  toast(`Hid ${r.item.title}`, () => { profiles.unhide(id); renderProfileBtn(); renderGuide(); });
  // Keep the list topped up so hiding a run of channels never empties the screen.
  if (guideRows.length < GUIDE_BATCH && guideNext < live.items.length) appendGuideRows(GUIDE_BATCH);
}

let toastTimer = null;
let toastUndo = null;
function toast(text, undo) {
  $("toast-text").textContent = text;
  toastUndo = undo;
  els.toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { els.toast.hidden = true; toastUndo = null; }, 5000);
}

// ---- Profile sheet ----
function renderProfileBtn() {
  const me = profiles.me;
  els.profileBtn.replaceChildren(avatar(me), el("span", { class: "pname" }, me.name));
  els.profileBtn.setAttribute("aria-label", `Profile: ${me.name}. Change profile or hidden channels`);
}

function renderSheet() {
  els.plist.replaceChildren(...profiles.list.map((p) => {
    const n = Object.keys(p.hidden).length;
    const on = p.id === profiles.current;
    return el("li", {}, el("button", { type: "button", "data-profile": p.id, "aria-pressed": String(on) },
      avatar(p), el("span", { class: "pl-name" }, p.name),
      el("span", { class: "pl-count" }, n ? `${n} hidden` : ""), on ? el("span", { class: "check", "aria-hidden": "true" }, "✓") : null));
  }));
  $("p-delete").disabled = profiles.list.length < 2;
  renderGroups();

  const hidden = Object.entries(profiles.me.hidden)
    .sort((a, b) => a[1].title.localeCompare(b[1].title));
  $("hidden-title").textContent = `Hidden channels${hidden.length ? ` (${hidden.length})` : ""} for ${profiles.me.name}`;
  $("hidden-empty").hidden = hidden.length > 0;
  els.hlist.replaceChildren(...hidden.map(([id, h]) => el("li", {},
    el("i", { class: "dot " + h.provider }),
    el("span", { class: "hl-title" }, h.title, el("small", {}, names[h.provider] || h.provider)),
    el("button", { type: "button", "data-unhide": id, "aria-label": `Show ${h.title} again` }, "Show"))));
}

// Chips to hide whole providers and genres. Pressed means hidden.
function renderGroups() {
  const me = profiles.me;
  const genres = [...genreOrder.keys()];
  const chip = (attr, value, label, hidden, dot) => el("button", { type: "button", [attr]: value, "aria-pressed": String(hidden) },
    dot ? el("i", { class: "dot " + value }) : null, label);
  els.groups.replaceChildren(
    el("p", { class: "group-label" }, "Providers"),
    el("div", { class: "chips" }, ...providerList.map((p) => chip("data-hide-provider", p.id, p.name, me.hiddenProviders.includes(p.id), true))),
    el("p", { class: "group-label" }, "Genres (live channels)"),
    el("div", { class: "chips" }, ...genres.map((g) => chip("data-hide-genre", g, g, me.hiddenGenres.includes(g)))));
}

function openSheet() {
  renderSheet();
  els.sheet.showModal();
}

// Profile changes redraw the guide (or results) for whoever is watching now.
function profileChanged() {
  profiles.save();
  renderProfileBtn();
  renderProviders();
  renderSheet();
  view = null; // a new person's list starts at the top
  schedule(0);
}

function askName(title, current = "") {
  const name = (prompt(title, current) || "").trim().slice(0, 24);
  return name || null;
}

function initProfiles() {
  profiles.load();
  profiles.save();
  renderProfileBtn();

  els.profileBtn.addEventListener("click", openSheet);
  $("profiles-close").addEventListener("click", () => els.sheet.close());
  // A tap on the dimmed backdrop closes the sheet.
  els.sheet.addEventListener("click", (e) => { if (e.target === els.sheet) els.sheet.close(); });

  els.plist.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-profile]");
    if (!b || b.dataset.profile === profiles.current) return;
    profiles.current = b.dataset.profile;
    setEditing(false);
    profileChanged();
    els.sheet.close();
  });
  $("p-add").addEventListener("click", () => {
    const name = askName("Name for the new profile");
    if (!name) return;
    const p = profiles.make(name);
    profiles.list.push(p);
    profiles.current = p.id;
    profileChanged();
  });
  $("p-rename").addEventListener("click", () => {
    const name = askName("Rename profile", profiles.me.name);
    if (!name) return;
    profiles.me.name = name;
    profiles.save();
    renderProfileBtn();
    renderSheet();
  });
  $("p-delete").addEventListener("click", () => {
    if (profiles.list.length < 2) return;
    if (!confirm(`Delete the profile “${profiles.me.name}” and its hidden channels?`)) return;
    profiles.list = profiles.list.filter((p) => p.id !== profiles.current);
    profiles.current = profiles.list[0].id;
    profileChanged();
  });
  els.groups.addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    if (b.dataset.hideProvider) profiles.toggle("hiddenProviders", b.dataset.hideProvider);
    else if (b.dataset.hideGenre) profiles.toggle("hiddenGenres", b.dataset.hideGenre);
    else return;
    profileChanged();
  });
  els.hlist.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-unhide]");
    if (!b) return;
    profiles.unhide(b.dataset.unhide);
    profileChanged();
  });

  els.edit.addEventListener("click", () => setEditing(!document.body.classList.contains("editing")));
  els.results.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-hide]");
    if (b) { e.stopPropagation(); hideChannel(b.dataset.hide); }
  }, true);
  $("toast-undo").addEventListener("click", () => {
    const undo = toastUndo;
    els.toast.hidden = true;
    toastUndo = null;
    if (undo) undo();
  });
}

function init() {
  initProfiles();
  state.kind = store.get("kind") || "all";
  state.provider = store.get("provider") || "all";
  state.genre = store.get("genre") || "all";
  state.q = new URL(location.href).searchParams.get("q") || "";
  els.q.value = state.q;
  setPressed(document.querySelector(".kinds"), "kind", state.kind);

  els.q.addEventListener("input", () => { state.q = els.q.value; schedule(); });
  els.form.addEventListener("submit", (e) => { e.preventDefault(); els.q.blur(); schedule(0); });
  els.clear.addEventListener("click", () => { els.q.value = state.q = ""; els.q.focus(); schedule(0); });

  document.querySelector(".kinds").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.kind = b.dataset.kind;
    store.set("kind", state.kind);
    setPressed(e.currentTarget, "kind", state.kind);
    schedule(0);
  });
  els.providers.addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.provider = b.dataset.provider;
    store.set("provider", state.provider);
    setPressed(els.providers, "provider", state.provider);
    schedule(0);
  });

  els.genres.addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.genre = b.dataset.genre;
    store.set("genre", state.genre);
    setPressed(els.genres, "genre", state.genre);
    renderGuide();
  });

  loadProviders().then(() => update());

  // Keep "now" honest: move progress every 30s, refetch schedules every 5 min.
  setInterval(() => {
    if (document.hidden || view !== "guide") return;
    if (Date.now() - live.fetchedAt > 5 * 60 * 1000) update(); else tickGuide();
  }, 30 * 1000);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) schedule(0); });

  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.register("/sw.js").catch(() => {});
  }
}

init();
