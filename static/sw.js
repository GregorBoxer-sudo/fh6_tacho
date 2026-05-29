// IMPORTANT: bump this version whenever any file in ASSETS changes.
// Failure to do so means PWA users keep serving stale cached assets.
const CACHE_NAME = "forza-tacho-v93";
const ASSETS = [
  "/",
  "/index.html",
  "/analytics.html",
  "/analytics.css",
  "/analytics.js",
  "/map.jpg",
  "/style.css",
  "/app.js",
  "/manifest.webmanifest",
  "/icon.svg"
];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE_NAME).then((cache) => cache.addAll(ASSETS)));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(keys.filter((key) => key !== CACHE_NAME).map((key) => caches.delete(key)))
    )
  );
  self.clients.claim();
});

self.addEventListener("fetch", (event) => {
  if (event.request.url.includes("/events") || event.request.url.includes("/api/")) {
    return;
  }

  event.respondWith(
    fetch(event.request).catch(() => caches.match(event.request).then((cached) => cached || caches.match("/")))
  );
});
