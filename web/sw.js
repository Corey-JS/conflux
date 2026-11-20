// Offline shell: the app opens instantly from cache; searches always go to
// the network (they're useless stale).
const VERSION = "conflux-v7";
const SHELL = [
  "/", "/index.html", "/app.css", "/app.js", "/manifest.webmanifest",
  "/icons/favicon-32.png", "/icons/apple-touch-icon.png", "/icons/icon-192.png", "/icons/icon-512.png",
];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(VERSION).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== VERSION).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin) return;

  if (url.pathname.startsWith("/api/")) {
    e.respondWith(fetch(e.request).catch(() =>
      new Response(JSON.stringify({ error: "offline" }), { status: 503, headers: { "Content-Type": "application/json" } })));
    return;
  }

  // Stale-while-revalidate for the app shell.
  e.respondWith(
    caches.open(VERSION).then(async (cache) => {
      const key = url.pathname === "/" ? "/index.html" : e.request;
      const cached = await cache.match(key, { ignoreSearch: true });
      const fresh = fetch(e.request).then((r) => {
        if (r.ok && r.type === "basic") cache.put(key, r.clone());
        return r;
      }).catch(() => cached);
      return cached || fresh;
    }),
  );
});
