import { Unzip, UnzipInflate } from 'fflate';

const MAX_ZIP_BYTES = 64 * 1024 * 1024;
const MAX_EXTRACTED_BYTES = 128 * 1024 * 1024;
const MAX_FILES = 4096;

export interface MountedArchive {
  readonly entryUrl: URL;
  dispose(): Promise<void>;
}

function archivePath(path: string): string {
  const segments = path.split('/');
  if (
    path.startsWith('/') || path.includes('\\') || path.includes('\0') ||
    segments.some((segment) => segment === '..' || segment === '.' || segment === '')
  ) {
    throw new Error(`Invalid ZIP path: ${path}`);
  }
  return path;
}

export function entryPath(input: string): string {
  if (input.trim() === '') {
    throw new Error('Enter the template path inside the ZIP');
  }
  const url = new URL(input.trim(), 'https://zip.invalid/');
  if (url.protocol !== 'https:' && url.protocol !== 'http:') {
    throw new Error('Use a ZIP-relative path or an HTTP(S) entry template URL');
  }
  return archivePath(decodeURIComponent(url.pathname.slice(1)));
}

// Streaming output is counted as it arrives: a forged ZIP size cannot bypass
// the memory bound. Yield between compressed chunks so the form stays usable.
export async function extractArchive(file: Blob): Promise<Map<string, Blob>> {
  if (file.size > MAX_ZIP_BYTES) {
    throw new Error('ZIP exceeds the 64 MiB upload limit');
  }
  const files = new Map<string, Blob>();
  const names = new Set<string>();
  let extractedBytes = 0;
  let pending = 0;
  let count = 0;
  const unzip = new Unzip((entry) => {
    if (++count > MAX_FILES) {
      throw new Error('ZIP exceeds the 4096 entry limit');
    }
    if (entry.name.endsWith('/')) {
      archivePath(entry.name.slice(0, -1));
      return;
    }
    const name = archivePath(entry.name);
    if (names.has(name)) {
      throw new Error(`Duplicate ZIP path: ${name}`);
    }
    names.add(name);
    if ((entry.originalSize ?? 0) > MAX_EXTRACTED_BYTES - extractedBytes) {
      throw new Error('ZIP exceeds the 128 MiB extracted size limit');
    }
    const chunks: BlobPart[] = [];
    ++pending;
    entry.ondata = (error, data, final) => {
      if (error !== null) throw error;
      extractedBytes += data.byteLength;
      if (extractedBytes > MAX_EXTRACTED_BYTES) {
        throw new Error('ZIP exceeds the 128 MiB extracted size limit');
      }
      chunks.push(new Uint8Array(data));
      if (final) {
        files.set(name, new Blob(chunks, { type: mediaType(name) }));
        --pending;
      }
    };
    entry.start();
  });
  unzip.register(UnzipInflate);
  const chunkSize = 64 * 1024;
  for (let offset = 0; offset < file.size; offset += chunkSize) {
    const end = Math.min(offset + chunkSize, file.size);
    unzip.push(new Uint8Array(await file.slice(offset, end).arrayBuffer()), end === file.size);
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
  if (pending !== 0 || files.size === 0) {
    throw new Error('ZIP is empty or incomplete');
  }
  return files;
}

function mediaType(path: string): string {
  switch (path.split('.').pop()?.toLowerCase()) {
    case 'xml': return 'application/xml';
    case 'js': case 'mjs': return 'text/javascript';
    case 'css': return 'text/css';
    case 'json': return 'application/json';
    case 'png': return 'image/png';
    case 'jpg': case 'jpeg': return 'image/jpeg';
    case 'webp': return 'image/webp';
    case 'gif': return 'image/gif';
    case 'svg': return 'image/svg+xml';
    case 'ttf': return 'font/ttf';
    case 'otf': return 'font/otf';
    case 'woff': return 'font/woff';
    case 'woff2': return 'font/woff2';
    default: return 'application/octet-stream';
  }
}

export async function mountArchive(file: Blob, input: string): Promise<MountedArchive> {
  const path = entryPath(input);
  const files = await extractArchive(file);
  if (!files.has(path)) {
    throw new Error(`Entry template is not in the ZIP: ${path}`);
  }
  // Even hosts that supply COOP/COEP themselves need this worker to serve ZIP
  // resources to the Render Worker and its later image requests.
  const registration = await navigator.serviceWorker.register(new URL('coi-service-worker.js', document.baseURI), {
    scope: new URL('.', document.baseURI).pathname,
    updateViaCache: 'none',
  });
  const incoming = registration.installing ?? registration.waiting;
  if (incoming !== null && incoming !== undefined && incoming.state !== 'activated') {
    await new Promise<void>((resolve, reject) => {
      incoming.addEventListener('statechange', () => {
        if (incoming.state === 'activated') resolve();
        if (incoming.state === 'redundant') reject(new Error('ZIP resource worker could not activate'));
      });
    });
  }
  await navigator.serviceWorker.ready;
  if (navigator.serviceWorker.controller === null) {
    await new Promise<void>((resolve) => {
      navigator.serviceWorker.addEventListener('controllerchange', () => resolve(), { once: true });
    });
  }
  const id = crypto.randomUUID();
  const cacheName = `bobcat-archive:${id}`;
  const base = new URL(`.bobcat-archives/${id}/`, document.baseURI);
  const resourceUrl = (name: string): URL =>
    new URL(name.split('/').map(encodeURIComponent).join('/'), base);
  const dispose = async (): Promise<void> => { await caches.delete(cacheName); };
  try {
    const cache = await caches.open(cacheName);
    for (const [name, blob] of files) {
      await cache.put(resourceUrl(name), new Response(blob));
    }
    return { entryUrl: resourceUrl(path), dispose };
  } catch (error) {
    await dispose();
    throw error;
  }
}
