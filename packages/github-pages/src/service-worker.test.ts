import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { expect, test, vi } from 'vitest';

const source = readFileSync(new URL('../public/coi-service-worker.js', import.meta.url), 'utf8');

function worker(cached?: Response) {
  const handlers = new Map<string, (event: unknown) => void>();
  const match = vi.fn().mockResolvedValue(cached);
  const fetch = vi.fn().mockResolvedValue(new Response('network'));
  vm.runInNewContext(source, {
    self: {
      registration: { scope: 'https://pages.test/project/' },
      addEventListener: (name: string, listener: (event: unknown) => void) => handlers.set(name, listener),
    },
    URL, Headers, Response, caches: { match }, fetch,
  });
  const request = async (url: string): Promise<Response> => {
    let response: Promise<Response> | undefined;
    handlers.get('fetch')!({
      request: new Request(url),
      respondWith: (value: Promise<Response>) => { response = value; },
    });
    return response!;
  };
  return { match, fetch, request };
}

test('serves archive assets with isolation headers and ignores resource query strings', async () => {
  const sw = worker(new Response(new Uint8Array([0, 255]), { headers: { 'Content-Type': 'image/png' } }));
  const response = await sw.request('https://pages.test/project/.bobcat-archives/test-id/dist/image.png?v=2');
  expect(sw.match).toHaveBeenCalledWith(expect.any(Request), {
    cacheName: 'bobcat-archive:test-id', ignoreSearch: true,
  });
  expect(sw.fetch).not.toHaveBeenCalled();
  expect(response.headers.get('Content-Type')).toBe('image/png');
  expect(response.headers.get('Cross-Origin-Embedder-Policy')).toBe('require-corp');
  expect(new Uint8Array(await response.arrayBuffer())).toEqual(new Uint8Array([0, 255]));
});

test('a missing ZIP asset fails locally without a network fallback', async () => {
  const sw = worker();
  const response = await sw.request('https://pages.test/project/.bobcat-archives/gone/missing.png');
  expect(response.status).toBe(404);
  expect(sw.fetch).not.toHaveBeenCalled();
});

test('ordinary and remote requests retain the existing network path', async () => {
  const sw = worker();
  await sw.request('https://pages.test/project/demo.lynx.xml');
  await sw.request('https://cdn.test/project/.bobcat-archives/a/image.png');
  expect(sw.fetch).toHaveBeenCalledTimes(2);
  expect(sw.match).not.toHaveBeenCalled();
});
