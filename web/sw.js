// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

// meshctl's service worker: there so that it can be installed as an app,
// and so that its files load at once, as Rucio's.
//
// meshctl is of no use without its daemon, so this does not make it work
// offline: the API and its WebSocket always go to the network, and the page
// itself is never taken from here, which keeps a proxy's HTTP authentication
// in front working (a 401 to a request from here never brings up the
// browser's dialog, only one to the page's own load does). Only the files
// the page loads are kept, and given at once while they are looked for
// again: those trunk names by their hash never change, and the rest come
// back fresh the next time.
const CACHE = 'meshctl-v1';

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    // Those of earlier versions, gone.
    const keys = await caches.keys();
    await Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key)));
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET' || request.mode === 'navigate') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.startsWith('/api/')) return;

  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const kept = await cache.match(request);
    const fresh = fetch(request)
      .then((response) => {
        if (response && response.ok) cache.put(request, response.clone());
        return response;
      })
      .catch(() => kept);
    return kept || fresh;
  })());
});
