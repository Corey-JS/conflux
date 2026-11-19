// Conflux: search box -> /api/search, tap a result -> the provider's own page.
"use strict";

const $ = (id) => document.getElementById(id);
const els = {
  form: $("search-form"), q: $("q"), clear: $("clear"), providers: $("providers"),
  results: $("results"), heading: $("heading"), note: $("note"),
};

const state = { q: "", kind: "all", provider: "all" };
const names = {}; // provider id -> display name
let inflight = null;
let timer = null;

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
};

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

function initial(title) {
  return (title || "?").trim().charAt(0).toUpperCase();
}

function skeleton(n = 6) {
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
  els.results.replaceChildren(...items.map(row).filter(Boolean));
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

  try {
    if (!q) {
      if (state.kind === "movie" || state.kind === "series") {
        els.heading.textContent = state.kind === "movie" ? "Movies" : "Shows";
        els.results.replaceChildren();
        note("Type a title to search free movies and shows.");
        return;
      }
      els.heading.textContent = "Live channels";
      skeleton();
      const p = params({ limit: "400" });
      p.delete("kind");
      const data = await getJSON("/api/live?" + p, signal);
      render(data.items, "No live channels loaded yet. Try again in a minute.");
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

async function loadProviders() {
  let list = [];
  try {
    list = (await getJSON("/api/providers")).providers || [];
  } catch { /* chips are optional */ }
  const chip = (id, label, down) => {
    const b = el("button", { type: "button", "data-provider": id, "aria-pressed": String(state.provider === id), class: down ? "down" : null });
    if (id !== "all") b.append(el("i", { class: "dot " + id }));
    b.append(label);
    return b;
  };
  const chips = [chip("all", "All providers")];
  for (const p of list) {
    names[p.id] = p.name;
    const down = Boolean(p.catalog_error && !p.catalog_items) || Boolean(p.search_error && !p.mode.includes("catalog"));
    chips.push(chip(p.id, p.name, down));
  }
  if (state.provider !== "all" && !names[state.provider]) state.provider = "all";
  els.providers.replaceChildren(...chips);
  setPressed(els.providers, "provider", state.provider);
}

function init() {
  state.kind = store.get("kind") || "all";
  state.provider = store.get("provider") || "all";
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

  loadProviders().then(() => update());

  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.register("/sw.js").catch(() => {});
  }
}

init();
