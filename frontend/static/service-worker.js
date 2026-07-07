/*
 * OgreNotes service worker (Phase 5 M-P3 piece A).
 *
 * Scope: caches the app shell (HTML, CSS, WASM, JS glue, fonts) so a
 * cold reload after a network drop still loads the UI. Does NOT cache
 * API or WebSocket traffic — those are deliberately network-only so
 * a stale session never serves a user a doc from yesterday's cache.
 *
 * Cache lifecycle: SHELL_CACHE name embeds a version stamp. On
 * activation, any cache whose name doesn't match the current
 * SHELL_CACHE is deleted. To bust a deployed worker, bump the
 * `SHELL_VERSION` constant below; on next activation, old caches are
 * removed and the new build's shell is re-fetched on demand.
 */

const SHELL_VERSION = 'v2';
const SHELL_CACHE = `ogrenotes-shell-${SHELL_VERSION}`;

// Pre-cache only the entrypoints. WASM + JS glue have hashed
// filenames per Trunk build, so they're fetched on first navigation
// and cached then; no need to enumerate them statically.
const PRECACHE_URLS = [
  '/',
  '/index.html',
  '/manifest.webmanifest',
  '/icon.svg',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then((cache) => cache.addAll(PRECACHE_URLS))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(
          names
            .filter((name) => name.startsWith('ogrenotes-shell-') && name !== SHELL_CACHE)
            .map((name) => caches.delete(name)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const req = event.request;

  if (req.method !== 'GET') {
    return; // mutations always go to network
  }

  const url = new URL(req.url);

  // Same-origin only. Cross-origin assets (analytics, font CDNs) are
  // network-passthrough.
  if (url.origin !== self.location.origin) {
    return;
  }

  // API and WebSocket — never cache. Stale doc data is worse than an
  // offline error.
  if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/ws')) {
    return;
  }

  // Navigations (the app shell / index.html) are NETWORK-FIRST. Each
  // deploy ships a fresh index.html that references new content-hashed
  // asset filenames; serving a stale cached shell strands the client on
  // asset URLs that no longer exist, which then fail Subresource
  // Integrity and brick the app. Fall back to cache only when the
  // network is unreachable, preserving the offline-cold-reload goal.
  if (req.mode === 'navigate') {
    event.respondWith(
      fetch(req)
        .then((response) => {
          if (response && response.status === 200 && response.type === 'basic') {
            const clone = response.clone();
            caches.open(SHELL_CACHE).then((cache) => cache.put(req, clone));
          }
          return response;
        })
        .catch(() =>
          caches.match(req).then((cached) => cached || caches.match('/index.html')),
        ),
    );
    return;
  }

  // Sub-resources (content-hashed JS/WASM/CSS, fonts) are immutable per
  // filename: stale-while-revalidate — serve cached if present (fast),
  // revalidate in the background (fresh next time).
  event.respondWith(
    caches.match(req).then((cached) => {
      const fetchPromise = fetch(req)
        .then((response) => {
          if (response && response.status === 200 && response.type === 'basic') {
            const clone = response.clone();
            caches.open(SHELL_CACHE).then((cache) => cache.put(req, clone));
          }
          return response;
        })
        .catch(() => cached);

      return cached || fetchPromise;
    }),
  );
});
