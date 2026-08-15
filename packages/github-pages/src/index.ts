import './styles.css';

const RELOAD_MARKER = `bobcat-coi-reload:${new URL('.', document.baseURI).pathname}`;
const RELOAD_PARAMETER = 'bobcat-coi-reload';

type IndicatorState = 'pending' | 'ok' | 'error';

interface Indicator {
  readonly value: HTMLElement;
  readonly root: HTMLElement;
}

interface Shell {
  readonly canvas: HTMLCanvasElement;
  readonly canvasSize: HTMLElement;
  readonly isolation: Indicator;
  readonly message: HTMLElement;
  readonly renderer: Indicator;
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
            A Worker-owned wasm-bindgen engine spans a nested DOM worker and
            OffscreenCanvas WebGPU, isolated by COOP/COEP.
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
        <p id="runtime-message" role="status" aria-live="polite">
          Preparing cross-origin isolation…
        </p>
      </footer>
    </main>
  `;

  return {
    canvas: requiredElement<HTMLCanvasElement>(root, '#bobcat-canvas'),
    canvasSize: requiredElement<HTMLElement>(root, '#canvas-size'),
    isolation: indicator(root, 'isolation'),
    message: requiredElement<HTMLElement>(root, '#runtime-message'),
    renderer: indicator(root, 'renderer'),
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

const DEMO_SCRIPT = `
  globalThis.renderPage = function () {
    const page = __CreatePage('github-pages-demo', 0);
    const text = __CreateText(0);
    __AppendElement(text, __CreateRawText('Bobcat WebAssembly VM is running'));
    __AppendElement(page, text);
  };
`;

async function executeDemoScript(canvas: {
  executeScript(url: string | URL): Promise<void>;
}): Promise<void> {
  const script = new Blob([DEMO_SCRIPT], { type: 'text/javascript' });
  const url = URL.createObjectURL(script);
  try {
    await canvas.executeScript(url);
  } finally {
    URL.revokeObjectURL(url);
  }
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
  const bobcatModuleUrl = new URL(
    'bobcat-wasm/facade.js',
    document.baseURI,
  ).href;
  const { BobcatCanvas, default: init } = (await import(
    /* webpackIgnore: true */ bobcatModuleUrl
  )) as typeof import('bobcat-wasm');

  await init();

  // Let CSS establish the canvas size before transferring it to the Render
  // Worker and allocating the worker-owned WebGPU surface.
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  const initial = canvasMetrics(shell.canvas);
  const bobcat = await BobcatCanvas.create(
    shell.canvas,
    initial.width,
    initial.height,
    initial.dpr,
    {
      defaultDisplayLinear: true,
      defaultOverflowVisible: true,
      enableCSSSelector: true,
    },
  );
  bobcat.onerror = (error): void => {
    setIndicator(shell.renderer, 'Failed', 'error');
    shell.message.textContent = error.message;
  };
  setIndicator(shell.renderer, 'Offscreen WebGPU', 'ok');
  await executeDemoScript(bobcat);

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
        void bobcat
          .resize(next.width, next.height, next.dpr)
          .catch((error: unknown) => {
            setIndicator(shell.renderer, 'Failed', 'error');
            shell.message.textContent = errorMessage(error);
          });
      }
      shell.canvasSize.textContent = `${String(next.width)} × ${String(next.height)} · ${next.dpr.toFixed(2)}×`;
    });
  };

  const resizeObserver = new ResizeObserver(resize);
  resizeObserver.observe(shell.canvas);
  window.addEventListener('resize', resize, { passive: true });
  resize();

  shell.message.textContent =
    'Bobcat is running: the OffscreenCanvas embedder owns resources and pixels while core keeps its view, VM, and tree private.';
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
  console.error(error);
});
