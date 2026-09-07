import { afterEach, expect, test, vi } from 'vitest';
import { strToU8, zipSync } from 'fflate';
import { entryPath, extractArchive, mountArchive } from './archive';

function zip(files: Record<string, Uint8Array>): Blob {
  return new Blob([new Uint8Array(zipSync(files))]);
}

afterEach(() => vi.unstubAllGlobals());

test('entry URLs select ZIP paths, including escaped names and query strings', () => {
  expect(entryPath('dist/main.web.bundle')).toBe('dist/main.web.bundle');
  expect(entryPath('https://cdn.test/dist/my%20card.lynx.xml?v=2#main')).toBe('dist/my card.lynx.xml');
  expect(() => entryPath('file:///tmp/card')).toThrow('HTTP(S)');
  expect(() => entryPath('')).toThrow('Enter');
  expect(() => entryPath('https://cdn.test/%2f../escape')).toThrow('Invalid ZIP path');
});

test('extracts nested compressed templates and resources without decoding binary bytes', async () => {
  const image = new Uint8Array([137, 80, 78, 71, 0, 255]);
  const files = await extractArchive(zip({
    'dist/': new Uint8Array(),
    'dist/card.lynx.xml': strToU8('<lynx></lynx>'),
    'dist/images/a.png': image,
  }));
  expect(await files.get('dist/card.lynx.xml')?.text()).toBe('<lynx></lynx>');
  expect(files.get('dist/images/a.png')?.type).toBe('image/png');
  expect(new Uint8Array(await files.get('dist/images/a.png')!.arrayBuffer())).toEqual(image);
});

test.each(['../escape', '/absolute', 'a/../../escape', 'a\\b'])('rejects unsafe ZIP path %s', async (path) => {
  await expect(extractArchive(zip({ [path]: strToU8('bad') }))).rejects.toThrow('Invalid ZIP path');
});

test('rejects incomplete archives and excessive declared output before allocation', async () => {
  const bytes = zipSync({ 'card.xml': strToU8('hello') });
  await expect(extractArchive(new Blob([bytes.slice(0, 33)]))).rejects.toThrow();
  const oversized = new Uint8Array(bytes);
  new DataView(oversized.buffer).setUint32(22, 129 * 1024 * 1024, true);
  await expect(extractArchive(new Blob([oversized]))).rejects.toThrow('128 MiB');
});

test('rejects a missing entry before installing the service worker', async () => {
  await expect(mountArchive(zip({ 'card.xml': strToU8('hello') }), 'missing.xml'))
    .rejects.toThrow('Entry template is not in the ZIP');
});

test('mounts escaped paths under one isolated archive and removes it on disposal', async () => {
  vi.stubGlobal('document', { baseURI: 'https://pages.test/project/' });
  const register = vi.fn().mockResolvedValue({});
  vi.stubGlobal('navigator', { serviceWorker: { register, ready: Promise.resolve(), controller: {} } });
  const entries = new Map<string, Response>();
  const put = vi.fn(async (url: URL, response: Response) => { entries.set(url.href, response); });
  const remove = vi.fn().mockResolvedValue(true);
  vi.stubGlobal('caches', { open: vi.fn().mockResolvedValue({ put }), delete: remove });
  const archive = await mountArchive(zip({
    'dist/my card.xml': strToU8('<lynx/>'),
    'dist/image.png': new Uint8Array([0, 255]),
  }), 'https://cdn.test/dist/my%20card.xml');
  expect(archive.entryUrl.href).toMatch(/^https:\/\/pages.test\/project\/\.bobcat-archives\/.+\/dist\/my%20card.xml$/u);
  expect(await entries.get(archive.entryUrl.href)?.text()).toBe('<lynx/>');
  expect(entries.has(new URL('image.png', archive.entryUrl).href)).toBe(true);
  await archive.dispose();
  expect(remove).toHaveBeenCalledOnce();
});

test('removes a partially populated archive when cache storage fails', async () => {
  vi.stubGlobal('document', { baseURI: 'https://pages.test/project/' });
  vi.stubGlobal('navigator', { serviceWorker: {
    register: vi.fn().mockResolvedValue({}), ready: Promise.resolve(), controller: {},
  } });
  const remove = vi.fn().mockResolvedValue(true);
  vi.stubGlobal('caches', {
    open: vi.fn().mockResolvedValue({ put: vi.fn().mockRejectedValue(new Error('Quota exceeded')) }),
    delete: remove,
  });
  await expect(mountArchive(zip({ 'card.xml': strToU8('hello') }), 'card.xml')).rejects.toThrow('Quota exceeded');
  expect(remove).toHaveBeenCalledOnce();
});
