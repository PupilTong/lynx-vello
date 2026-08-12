import './styles.css';

const RELOAD_MARKER = `bobcat-coi-reload:${new URL('.', document.baseURI).pathname}`;
const RELOAD_PARAMETER = 'bobcat-coi-reload';
const MAX_THREAD_COUNT = 8;
const CHECKSUM_BYTES = 4 * 1024 * 1024;

type IndicatorState = 'pending' | 'ok' | 'error';

interface Indicator {
  readonly value: HTMLElement;
  readonly root: HTMLElement;
}

interface Shell {
  readonly canvas: HTMLCanvasElement;
  readonly canvasSize: HTMLElement;
  readonly checksumButton: HTMLButtonElement;
  readonly checksum: Indicator;
  readonly isolation: Indicator;
  readonly message: HTMLElement;
  readonly renderer: Indicator;
  readonly threads: Indicator;
}

function requiredElement<T extends Element>(
  parent: ParentNode,
  selector: string,
): T {
  const element = parent.querySelector<T>(selector);
  if (element === null) {
    throw new Error(`Missing required page element: ${selector}`);
  }
  return element;
}

function indicator(root: ParentNode, name: string): Indicator {
  const element = requiredElement<HTMLElement>(root, `[data-status="${name}"]`);
  return {
    root: element,
    value: requiredElement<HTMLElement>(element, '.status-value'),
  };
}

function mountShell(): Shell {
  const root = document.querySelector<HTMLElement>('#root');
  if (root === null) {
    throw new Error('Rsbuild did not create the application root');
  }

  root.innerHTML = `
    <main class="app-shell">
      <header class="hero">
        <p class="eyebrow">BOBCAT / WEBASSEMBLY</p>
        <div class="hero-copy">
          <h1>Rust, threaded<br>in the browser.</h1>
          <p class="lede">
            One Bobcat WASI module drives a WebGPU Canvas and real Rust threads
            with atomics, isolated in the browser by COOP/COEP.
          </p>
        </div>
      </header>

      <section class="status-grid" aria-label="Runtime status">
        <article class="status-card" data-status="isolation" data-state="pending">
          <span class="status-dot" aria-hidden="true"></span>
          <span class="status-label">Isolation</span>
          <strong class="status-value">Checking…</strong>
        </article>
        <article class="status-card" data-status="renderer" data-state="pending">
          <span class="status-dot" aria-hidden="true"></span>
          <span class="status-label">Renderer</span>
          <strong class="status-value">Checking…</strong>
        </article>
        <article class="status-card" data-status="threads" data-state="pending">
          <span class="status-dot" aria-hidden="true"></span>
          <span class="status-label">Rust workers</span>
          <strong class="status-value">Waiting…</strong>
        </article>
      </section>

      <section class="demo-panel" aria-labelledby="canvas-title">
        <div class="panel-heading">
          <div>
            <p class="panel-kicker">LIVE FRAME</p>
            <h2 id="canvas-title">Bobcat canvas</h2>
          </div>
          <output id="canvas-size">—</output>
        </div>
        <div class="canvas-frame">
          <canvas id="bobcat-canvas" aria-label="A geometric layout rendered by Bobcat"></canvas>
        </div>
      </section>

      <footer class="runtime-footer">
        <div>
          <p id="runtime-message" role="status" aria-live="polite">
            Preparing cross-origin isolation…
          </p>
          <div class="checksum-row" data-status="checksum" data-state="pending">
            <span class="status-dot" aria-hidden="true"></span>
            <span>Parallel checksum</span>
            <strong class="status-value">Waiting…</strong>
          </div>
        </div>
        <button id="checksum-button" type="button" disabled>Run again</button>
      </footer>
    </main>
  `;

  return {
    canvas: requiredElement<HTMLCanvasElement>(root, '#bobcat-canvas'),
    canvasSize: requiredElement<HTMLElement>(root, '#canvas-size'),
    checksumButton: requiredElement<HTMLButtonElement>(root, '#checksum-button'),
    checksum: indicator(root, 'checksum'),
    isolation: indicator(root, 'isolation'),
    message: requiredElement<HTMLElement>(root, '#runtime-message'),
    renderer: indicator(root, 'renderer'),
    threads: indicator(root, 'threads'),
  };
}

function setIndicator(
  target: Indicator,
  value: string,
  state: IndicatorState,
): void {
  target.value.textContent = value;
  target.root.dataset['state'] = state;
}

function sessionMarker(): string | null {
  try {
    const stored = sessionStorage.getItem(RELOAD_MARKER);
    if (stored !== null) {
      return stored;
    }
  } catch {
    // Fall through to the URL marker used when storage is blocked.
  }

  return new URL(window.location.href).searchParams.get(RELOAD_PARAMETER);
}

function setSessionMarker(value: string): void {
  try {
    sessionStorage.setItem(RELOAD_MARKER, value);
  } catch {
    const url = new URL(window.location.href);
    url.searchParams.set(RELOAD_PARAMETER, value);
    window.history.replaceState(null, '', url);
  }
}

function clearSessionMarker(): void {
  try {
    sessionStorage.removeItem(RELOAD_MARKER);
  } catch {
    // Nothing else relies on storage being available.
  }

  const url = new URL(window.location.href);
  if (url.searchParams.has(RELOAD_PARAMETER)) {
    url.searchParams.delete(RELOAD_PARAMETER);
    window.history.replaceState(null, '', url);
  }
}

function waitForController(): Promise<void> {
  if (navigator.serviceWorker.controller !== null) {
    return Promise.resolve();
  }

  return new Promise((resolve, reject) => {
    const controllerChanged = (): void => {
      window.clearTimeout(timeout);
      resolve();
    };
    const timeout = window.setTimeout(() => {
      navigator.serviceWorker.removeEventListener(
        'controllerchange',
        controllerChanged,
      );
      reject(new Error('The COOP/COEP Service Worker did not take control'));
    }, 15_000);
    navigator.serviceWorker.addEventListener(
      'controllerchange',
      controllerChanged,
      { once: true },
    );
  });
}

async function reloadForIsolation(): Promise<never> {
  await waitForController();
  setSessionMarker('attempted');
  window.location.reload();

  // Keep the unisolated document from loading the threaded Wasm module while
  // navigation transfers control to the Service Worker.
  return new Promise<never>(() => undefined);
}

async function ensureCrossOriginIsolation(shell: Shell): Promise<void> {
  if (globalThis.crossOriginIsolated) {
    clearSessionMarker();
    setIndicator(shell.isolation, 'COOP + COEP', 'ok');
    return;
  }

  if (!globalThis.isSecureContext) {
    throw new Error('A secure HTTPS or localhost origin is required');
  }
  if (!('serviceWorker' in navigator)) {
    throw new Error('This browser does not support Service Workers');
  }
  if (sessionMarker() === 'attempted') {
    throw new Error(
      'crossOriginIsolated is still false after the Service Worker reload',
    );
  }

  shell.message.textContent = 'Installing the COOP/COEP Service Worker…';
  const serviceWorkerUrl = new URL('coi-service-worker.js', document.baseURI);
  const scopeUrl = new URL('.', document.baseURI);
  await navigator.serviceWorker.register(serviceWorkerUrl, {
    scope: scopeUrl.pathname,
    updateViaCache: 'none',
  });
  await navigator.serviceWorker.ready;

  shell.message.textContent = 'Reloading once under cross-origin isolation…';
  await reloadForIsolation();
}

function preferredThreadCount(): number {
  const available = Math.max(1, navigator.hardwareConcurrency || 1);
  return Math.max(1, Math.min(MAX_THREAD_COUNT, available - 1 || 1));
}

function checksumInput(): Uint8Array {
  const bytes = new Uint8Array(CHECKSUM_BYTES);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = (index * 31 + (index >>> 7)) & 0xff;
  }
  return bytes;
}

const DEMO_STYLES = `
  page {
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    width: 100%;
    height: 100%;
    padding: 28px;
    gap: 16px;
    background-color: #0d1017;
  }

  page > view {
    box-sizing: border-box;
    border-radius: 22px;
  }

  page > view:nth-child(1) {
    display: flex;
    flex-direction: row;
    align-items: flex-end;
    justify-content: space-between;
    height: 37%;
    padding: 24px;
    background-color: #c8f560;
  }

  page > view:nth-child(1) > view:nth-child(1) {
    width: 42%;
    height: 32%;
    border-radius: 999px;
    background-color: #17261d;
  }

  page > view:nth-child(1) > view:nth-child(2) {
    width: 54px;
    height: 54px;
    border-radius: 999px;
    background-color: #f8f1e7;
  }

  page > view:nth-child(2) {
    display: flex;
    flex: 1;
    flex-direction: row;
    gap: 16px;
  }

  page > view:nth-child(2) > view {
    flex: 1;
    border-radius: 18px;
  }

  page > view:nth-child(2) > view:nth-child(1) {
    background-color: #ff7d52;
  }

  page > view:nth-child(2) > view:nth-child(2) {
    background-color: #786cf5;
  }

  page > view:nth-child(2) > view:nth-child(3) {
    background-color: #f8f1e7;
  }

  page > view:nth-child(3) {
    height: 13%;
    background-color: #202631;
  }

  @media (max-width: 560px) {
    page {
      padding: 16px;
      gap: 10px;
    }

    page > view:nth-child(1) {
      height: 31%;
      padding: 16px;
    }

    page > view:nth-child(2) {
      flex-direction: column;
      gap: 10px;
    }

    page > view:nth-child(3) {
      height: 10%;
    }
  }
`;

function createDemoTree(canvas: {
  addAuthorStylesheet(css: string): void;
  appendElement(parent: number, child: number): number;
  createPage(componentId: string, cssId: number): number;
  createView(parentComponentUniqueId: number): number;
  flushElementTree(): void;
}): void {
  canvas.addAuthorStylesheet(DEMO_STYLES);

  const page = canvas.createPage('github-pages-demo', 0);
  const hero = canvas.createView(0);
  const heroBar = canvas.createView(0);
  const heroDot = canvas.createView(0);
  const grid = canvas.createView(0);
  const firstCard = canvas.createView(0);
  const secondCard = canvas.createView(0);
  const thirdCard = canvas.createView(0);
  const footer = canvas.createView(0);

  canvas.appendElement(hero, heroBar);
  canvas.appendElement(hero, heroDot);
  canvas.appendElement(grid, firstCard);
  canvas.appendElement(grid, secondCard);
  canvas.appendElement(grid, thirdCard);
  canvas.appendElement(page, hero);
  canvas.appendElement(page, grid);
  canvas.appendElement(page, footer);
  canvas.flushElementTree();
}

function canvasMetrics(canvas: HTMLCanvasElement): {
  readonly dpr: number;
  readonly height: number;
  readonly width: number;
} {
  const bounds = canvas.getBoundingClientRect();
  return {
    width: Math.max(1, Math.round(bounds.width)),
    height: Math.max(1, Math.round(bounds.height)),
    dpr: Math.max(1, window.devicePixelRatio || 1),
  };
}

async function start(shell: Shell): Promise<void> {
  await ensureCrossOriginIsolation(shell);

  if (typeof SharedArrayBuffer === 'undefined') {
    throw new Error('SharedArrayBuffer is unavailable despite cross-origin isolation');
  }

  const webGpuNavigator = navigator as Navigator & { readonly gpu?: unknown };
  if (webGpuNavigator.gpu === undefined) {
    setIndicator(shell.renderer, 'Unavailable', 'error');
    throw new Error('WebGPU is required to render the Bobcat canvas');
  }
  setIndicator(shell.renderer, 'Initializing…', 'pending');

  shell.message.textContent = 'Loading the threaded Rust module…';
  const { BobcatCanvas, default: init, parallelChecksum } = await import(
    'bobcat-wasm'
  );

  await init();
  const requestedThreads = preferredThreadCount();
  const input = checksumInput();

  const runChecksum = async (): Promise<void> => {
    shell.checksumButton.disabled = true;
    setIndicator(shell.checksum, 'Running…', 'pending');

    // Give the pending state one paint before NAPI-RS dispatches the AsyncTask.
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    try {
      const startedAt = performance.now();
      const report = await parallelChecksum(input, requestedThreads);
      const elapsed = performance.now() - startedAt;
      setIndicator(
        shell.threads,
        `${String(report.threads)} worker${report.threads === 1 ? '' : 's'}`,
        'ok',
      );
      setIndicator(
        shell.checksum,
        `${String(report.checksum)} · ${elapsed.toFixed(1)} ms`,
        'ok',
      );
    } catch (error) {
      setIndicator(shell.threads, 'Failed', 'error');
      setIndicator(shell.checksum, 'Failed', 'error');
      shell.message.textContent = errorMessage(error);
      throw error;
    } finally {
      shell.checksumButton.disabled = false;
    }
  };

  await runChecksum();

  // Let CSS establish the canvas size before allocating the WebGPU surface.
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  const initial = canvasMetrics(shell.canvas);
  const bobcat = await BobcatCanvas.create(
    shell.canvas,
    initial.width,
    initial.height,
    initial.dpr,
  );
  setIndicator(shell.renderer, 'WASI WebGPU', 'ok');
  createDemoTree(bobcat);

  const renderFrame = (): void => {
    try {
      bobcat.renderIfRequested();
    } catch (error) {
      setIndicator(shell.renderer, 'Failed', 'error');
      shell.message.textContent = errorMessage(error);
      return;
    }
    window.requestAnimationFrame(renderFrame);
  };
  window.requestAnimationFrame(renderFrame);

  let lastWidth = initial.width;
  let lastHeight = initial.height;
  let lastDpr = initial.dpr;
  let resizeFrame = 0;

  const resize = (): void => {
    window.cancelAnimationFrame(resizeFrame);
    resizeFrame = window.requestAnimationFrame(() => {
      const next = canvasMetrics(shell.canvas);
      if (
        next.width !== lastWidth ||
        next.height !== lastHeight ||
        next.dpr !== lastDpr
      ) {
        lastWidth = next.width;
        lastHeight = next.height;
        lastDpr = next.dpr;
        bobcat.resize(next.width, next.height, next.dpr);
      }
      shell.canvasSize.textContent = `${String(next.width)} × ${String(next.height)} · ${next.dpr.toFixed(2)}×`;
    });
  };

  const resizeObserver = new ResizeObserver(resize);
  resizeObserver.observe(shell.canvas);
  window.addEventListener('resize', resize, { passive: true });
  resize();

  shell.checksumButton.addEventListener('click', () => {
    void runChecksum().catch((error: unknown) => {
      console.error(error);
    });
  });
  shell.message.textContent =
    'Bobcat Canvas and the Rust thread probe are running in one WASI module.';
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const shell = mountShell();
void start(shell).catch((error: unknown) => {
  const message = errorMessage(error);
  shell.message.textContent = `Unable to start: ${message}`;

  if (!globalThis.crossOriginIsolated) {
    setIndicator(shell.isolation, 'Not isolated', 'error');
  }
  if ((navigator as Navigator & { readonly gpu?: unknown }).gpu === undefined) {
    setIndicator(shell.renderer, 'Unavailable', 'error');
  } else if (shell.renderer.root.dataset['state'] !== 'ok') {
    setIndicator(shell.renderer, 'Not started', 'error');
  }
  if (shell.threads.root.dataset['state'] !== 'ok') {
    setIndicator(shell.threads, 'Not started', 'error');
  }
  if (shell.checksum.root.dataset['state'] !== 'ok') {
    setIndicator(shell.checksum, 'Not run', 'error');
  }
  console.error(error);
});
