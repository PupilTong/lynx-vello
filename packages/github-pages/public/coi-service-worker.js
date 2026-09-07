/*
 * GitHub Pages cannot configure response headers. Once this worker controls
 * the page, it adds the two headers required for SharedArrayBuffer and Wasm
 * threads to every same-origin response, including the top-level document.
 */

const COOP = 'Cross-Origin-Opener-Policy';
const COEP = 'Cross-Origin-Embedder-Policy';
const CORP = 'Cross-Origin-Resource-Policy';

self.addEventListener('install', (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

async function resourceResponse(request) {
  const url = new URL(request.url);
  const archiveRoot = new URL('.bobcat-archives/', self.registration.scope);
  if (url.origin === archiveRoot.origin && url.pathname.startsWith(archiveRoot.pathname)) {
    const id = url.pathname.slice(archiveRoot.pathname.length).split('/')[0];
    const response = await caches.match(request, {
      cacheName: `bobcat-archive:${id}`,
      ignoreSearch: true,
    });
    return response ?? new Response('Resource is not in the ZIP', { status: 404 });
  }
  return fetch(request);
}

self.addEventListener('fetch', (event) => {
  const { request } = event;

  // Chromium rejects these requests if a Service Worker handles them.
  if (request.cache === 'only-if-cached' && request.mode !== 'same-origin') {
    return;
  }

  event.respondWith(
    resourceResponse(request).then((response) => {
      // Opaque cross-origin responses cannot be reconstructed or given headers.
      if (response.type === 'opaque' || response.status === 0) {
        return response;
      }

      const headers = new Headers(response.headers);
      headers.set(COOP, 'same-origin');
      headers.set(COEP, 'require-corp');
      headers.set(CORP, 'same-origin');

      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      });
    }),
  );
});
