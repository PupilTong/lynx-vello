// The GitHub Pages Lynx Explorer: the page shell, the canvas it hands to a
// `BobcatCanvas`, the Explorer homepage's `NativeModules`, and the Lynx XML
// editor, all on the page's main thread.
//
// The Lynx performance section, below the source in the same scrolling
// column, reads the same entries a native LynxView hands its host's
// `onPerformanceEvent`. The page passes `onPerformanceEntry` to
// `BobcatCanvas.create`, which is what has the Render Worker tell each view
// to observe; the engine writes each entry as Lynx's JSON on that Worker, the
// facade parses it, and `PerformanceTimeline` (from bobcat-wasm) files it
// under the load it belongs to. Every load the page
// starts — the Load button, a navigation, an Explorer `openSchema`, a Lynx
// XML render — begins a group and passes `openTime`, the instant it was asked
// for, so the load entry reports `totalFcp`; the facade adds the canvas's
// `containerInit` marks and the Render Worker the `prepareTemplate` marks
// around its fetch.

import './styles.css';

import type {
  BobcatCanvas,
  FcpBreakdown,
  LynxPaintEntry,
  LynxPerformanceEntry,
  LynxPerformanceMetric,
  LynxPipelineEntry,
  LynxThread,
  NativeModules,
  PageConfig,
  PerformanceLoad,
  PerformanceTimeline,
  Waterfall,
} from 'bobcat-wasm';

const MAX_LYNX_XML_BYTES = 16 * 1024 * 1024;
const RELOAD_MARKER = `bobcat-coi-reload:${new URL('.', document.baseURI).pathname}`;
const RELOAD_PARAMETER = 'bobcat-coi-reload';
const TAB_PARAMETER = 'tab';
// The Lynx Explorer homepage, built by `@explorer/homepage` and copied beside
// the page; the Canvas tab loads it first.
const HOMEPAGE_TEMPLATE = 'explorer-homepage/main.web.bundle';
// `ExplorerModule.navigateTo` builds this scheme around a bundle path that is
// relative to the Explorer's local template root, which here is the page.
const LOCAL_SCHEMA_PREFIX = 'file://lynx?local://';
// What a native Explorer opens, and what Bobcat opens instead.
const LYNX_BUNDLE_SUFFIX = '.lynx.bundle';
const WEB_BUNDLE_SUFFIX = '.web.bundle';
// One namespace for everything the demo's ExplorerModule stores, so the page
// shares no key with anything else served from this origin.
const STORAGE_PREFIX = 'bobcat-explorer:';
// Whether the Lynx performance panel is open, under that same namespace.
const PERFORMANCE_PANEL_KEY = `${STORAGE_PREFIX}performance-panel`;
// The pipeline log keeps this many rows per load; older ones are dropped and
// counted.
const PERFORMANCE_ROW_CAP = 200;

type IndicatorState = 'pending' | 'ok' | 'error';
type SourceState = 'idle' | 'pending' | 'ok' | 'error';
type WorkspaceTab = 'canvas' | 'lynx-xml';

interface Indicator {
  readonly value: HTMLElement;
  readonly root: HTMLElement;
}

type BobcatCanvasFactory = Pick<typeof BobcatCanvas, 'create'>;

/** The parts of bobcat-wasm the performance panel draws with. */
type PerformanceKit = Pick<
  typeof import('bobcat-wasm'),
  | 'PerformanceTimeline'
  | 'fcpBreakdown'
  | 'pipelineDuration'
  | 'pipelineWaterfall'
  | 'stageThread'
>;

interface PerformanceShell {
  readonly panel: HTMLDetailsElement;
  readonly counters: HTMLElement;
  readonly fcp: HTMLElement;
  readonly lynxFcp: HTMLElement;
  readonly totalFcp: HTMLElement;
  readonly breakdown: HTMLElement;
  readonly waterfall: HTMLElement;
  readonly loadRaw: HTMLElement;
  readonly filter: HTMLInputElement;
  readonly log: HTMLTableSectionElement;
  readonly loads: HTMLSelectElement;
}

interface Shell {
  readonly sourcePanel: HTMLElement;
  readonly frameRate: HTMLElement;
  readonly performance: PerformanceShell;
  readonly navigation: HTMLSelectElement;
  readonly navigationPosition: HTMLElement;
  readonly backButton: HTMLButtonElement;
  readonly forwardButton: HTMLButtonElement;
  readonly uploadPanel: HTMLFormElement;
  readonly entryInput: HTMLInputElement;
  readonly loadButton: HTMLButtonElement;
  readonly uploadStatus: HTMLElement;
  readonly zipInput: HTMLInputElement;
  readonly zipStatus: HTMLElement;
  readonly expandButton: HTMLButtonElement;
  readonly canvasHost: HTMLElement;
  readonly canvasSize: HTMLElement;
  readonly editor: HTMLTextAreaElement;
  readonly editorForm: HTMLFormElement;
  readonly isolation: Indicator;
  readonly message: HTMLElement;
  readonly previewTitle: HTMLElement;
  readonly renderButton: HTMLButtonElement;
  readonly renderButtonLabel: HTMLElement;
  readonly renderer: Indicator;
  readonly sourceMetrics: HTMLElement;
  readonly sourceStatus: HTMLElement;
  readonly tabLinks: readonly HTMLAnchorElement[];
  readonly workspace: HTMLElement;
}

interface TabRouter {
  refresh(): void;
  subscribe(listener: () => void): () => void;
}

interface CanvasSize {
  readonly dpr: number;
  readonly height: number;
  readonly width: number;
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
            OffscreenCanvas WebGPU. Inspect the live frame or edit a Lynx XML
            card and send it through the same isolated runtime.
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

      <section class="workspace-shell" aria-labelledby="workspace-title">
        <div class="workspace-bar">
          <div>
            <p class="panel-kicker">PLAYGROUND</p>
            <h2 id="workspace-title">Bobcat workspace</h2>
          </div>
          <nav class="workspace-tabs" aria-label="Workspace views">
            <a class="workspace-tab" data-workspace-tab="canvas" href="?tab=canvas">Canvas</a>
            <a class="workspace-tab" data-workspace-tab="lynx-xml" href="?tab=lynx-xml">Lynx XML</a>
          </nav>
        </div>

        <div class="workspace-grid" id="renderer-workspace" data-active-tab="canvas">
          <div class="source-panel" id="source-panel">
            <section class="canvas-tools" aria-label="Canvas monitoring and navigation">
              <div class="frame-rate-row">
                <span class="status-label" title="Sampled with requestAnimationFrame on the Painter thread; not GPU presents">Canvas FPS <span class="frame-rate-method">/ rAF</span></span>
                <span class="frame-rate-value" id="frame-rate" aria-live="off">—</span>
              </div>
              <div class="navigation-heading">
                <label for="navigation-stack" class="status-label">Navigation stack</label>
                <span id="navigation-position" class="field-help">0 / 0</span>
              </div>
              <div class="navigation-controls">
                <button id="navigate-back" type="button" aria-label="Go back" title="Go back" disabled>←</button>
                <button id="navigate-forward" type="button" aria-label="Go forward" title="Go forward" disabled>→</button>
                <select id="navigation-stack" aria-describedby="navigation-position" disabled>
                  <option>No pages yet</option>
                </select>
              </div>
            </section>
            <form class="upload-panel" id="canvas-source-panel" aria-labelledby="canvas-source-title">
              <div class="pane-heading">
                <div>
                  <p class="panel-kicker">SOURCE</p>
                  <h3 id="canvas-source-title">Canvas source</h3>
                </div>
                <button id="load-template" class="primary-button" type="submit" disabled>Load template</button>
              </div>
              <div class="upload-fields">
                <div class="source-field">
                  <label for="canvas-zip">Local ZIP file (optional)</label>
                  <p id="zip-help" class="field-help">Optionally choose a ZIP archive to provide local templates and resources.</p>
                  <input id="canvas-zip" type="file" accept=".zip,application/zip,application/x-zip-compressed" aria-describedby="zip-help zip-status">
                  <output id="zip-status" class="field-help" aria-live="polite">No ZIP selected</output>
                </div>
                <div class="source-field">
                  <label for="entry-template-url">Entry template URL</label>
                  <input id="entry-template-url" type="text" required value="${HOMEPAGE_TEMPLATE}" placeholder="https://example.com/main.web.bundle" autocomplete="off" autocapitalize="off" spellcheck="false" aria-describedby="template-loading-note">
                </div>
                <p id="template-loading-note" class="field-help">Enter a template URL to load directly. With a ZIP selected, use zip:///dist/main.web.bundle, a path from the ZIP root, or a full URL whose pathname matches an archive entry. Supports .lynx.xml, binary .web.bundle, and source-based .lynx.bundle templates.</p>
              </div>
              <div class="editor-footer">
                <output id="upload-status" class="field-help" aria-live="polite">Enter a template URL. A local ZIP is optional.</output>
              </div>
            </form>

            <form class="editor-panel" id="lynx-xml-panel" hidden>
              <div class="pane-heading editor-heading">
                <div>
                  <p class="panel-kicker">SOURCE</p>
                  <h3>Lynx XML editor</h3>
                </div>
                <button id="render-xml" type="submit" disabled>
                  <span id="render-xml-label">Submit XML</span>
                  <span class="button-arrow" aria-hidden="true">↗</span>
                </button>
              </div>
              <textarea
                id="lynx-xml-editor"
                aria-describedby="source-status source-metrics"
                aria-label="Lynx XML source"
                autocomplete="off"
                autocapitalize="off"
                placeholder="Loading demo.lynx.xml…"
                spellcheck="false"
                wrap="off"
              ></textarea>
              <div class="editor-footer">
                <output id="source-status" data-state="idle" aria-live="polite">Loading demo…</output>
                <span id="source-metrics">—</span>
              </div>
            </form>

            <section class="performance-tools" aria-label="Lynx performance">
              <details class="performance-panel" id="performance-panel" open>
                <summary class="performance-heading">
                  <span class="status-label" title="The entries a native LynxView hands its host's onPerformanceEvent">Lynx performance</span>
                  <span id="performance-counters" class="performance-counters">No load yet</span>
                </summary>
                <div class="performance-body">
                  <div class="performance-load-row">
                    <label for="performance-loads" class="status-label">Load</label>
                    <select id="performance-loads" title="The load the Lynx performance panel shows" disabled>
                      <option>No loads yet</option>
                    </select>
                  </div>
                  <dl class="performance-figures">
                    <div class="performance-figure"><dt>fcp</dt><dd id="performance-fcp">—</dd></div>
                    <div class="performance-figure"><dt>lynxFcp</dt><dd id="performance-lynx-fcp">—</dd></div>
                    <div class="performance-figure"><dt>totalFcp</dt><dd id="performance-total-fcp">—</dd></div>
                  </dl>
                  <div id="performance-breakdown"></div>
                  <div id="performance-waterfall" class="performance-waterfall-host"></div>
                  <div id="performance-load-raw"></div>
                  <div class="performance-log-heading">
                    <span class="status-label">Pipeline log</span>
                    <input id="performance-filter" type="search" placeholder="Filter name or identifier" aria-label="Filter the pipeline log by name or identifier" autocomplete="off" spellcheck="false">
                  </div>
                  <div class="performance-log" tabindex="0" aria-label="Pipeline log">
                    <table>
                      <colgroup>
                        <col class="performance-col-name">
                        <col class="performance-col-identifier">
                        <col class="performance-col-origin">
                        <col class="performance-col-duration">
                        <col class="performance-col-expand">
                      </colgroup>
                      <thead>
                        <tr>
                          <th scope="col">name</th>
                          <th scope="col">identifier</th>
                          <th scope="col" title="Lynx reports a pipeline's origin as its name; this is the framework's frameworkRenderingTiming.dsl and stage">dsl · stage</th>
                          <th scope="col" class="performance-numeric" title="paintEnd − pipelineStart">duration</th>
                          <th scope="col"><span class="visually-hidden">Stages</span></th>
                        </tr>
                      </thead>
                      <tbody id="performance-log"></tbody>
                    </table>
                  </div>
                </div>
              </details>
            </section>

          </div>
          <section class="preview-panel" aria-labelledby="canvas-title">
            <div class="pane-heading preview-heading">
              <div>
                <p class="panel-kicker">LIVE FRAME</p>
                <h3 id="canvas-title">Bobcat canvas</h3>
              </div>
              <div class="preview-actions">
                <output id="canvas-size">—</output>
                <button id="expand-preview" type="button" aria-controls="source-panel" aria-pressed="false" title="Expand canvas and hide source panel">
                  <span class="expand-icon" aria-hidden="true">⤢</span>
                  <span id="expand-preview-label">Expand</span>
                </button>
              </div>
            </div>
            <div class="canvas-frame">
              <div class="canvas-host" id="canvas-host">
                <canvas class="bobcat-canvas" aria-label="A geometric layout rendered by Bobcat"></canvas>
              </div>
            </div>
          </section>
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
    sourcePanel: requiredElement<HTMLElement>(root, '#source-panel'),
    frameRate: requiredElement<HTMLElement>(root, '#frame-rate'),
    performance: {
      panel: requiredElement<HTMLDetailsElement>(root, '#performance-panel'),
      counters: requiredElement<HTMLElement>(root, '#performance-counters'),
      fcp: requiredElement<HTMLElement>(root, '#performance-fcp'),
      lynxFcp: requiredElement<HTMLElement>(root, '#performance-lynx-fcp'),
      totalFcp: requiredElement<HTMLElement>(root, '#performance-total-fcp'),
      breakdown: requiredElement<HTMLElement>(root, '#performance-breakdown'),
      waterfall: requiredElement<HTMLElement>(root, '#performance-waterfall'),
      loadRaw: requiredElement<HTMLElement>(root, '#performance-load-raw'),
      filter: requiredElement<HTMLInputElement>(root, '#performance-filter'),
      log: requiredElement<HTMLTableSectionElement>(root, '#performance-log'),
      loads: requiredElement<HTMLSelectElement>(root, '#performance-loads'),
    },
    navigation: requiredElement<HTMLSelectElement>(root, '#navigation-stack'),
    navigationPosition: requiredElement<HTMLElement>(root, '#navigation-position'),
    backButton: requiredElement<HTMLButtonElement>(root, '#navigate-back'),
    forwardButton: requiredElement<HTMLButtonElement>(root, '#navigate-forward'),
    uploadPanel: requiredElement<HTMLFormElement>(root, '#canvas-source-panel'),
    entryInput: requiredElement<HTMLInputElement>(root, '#entry-template-url'),
    loadButton: requiredElement<HTMLButtonElement>(root, '#load-template'),
    uploadStatus: requiredElement<HTMLElement>(root, '#upload-status'),
    zipInput: requiredElement<HTMLInputElement>(root, '#canvas-zip'),
    zipStatus: requiredElement<HTMLElement>(root, '#zip-status'),
    expandButton: requiredElement<HTMLButtonElement>(root, '#expand-preview'),
    canvasHost: requiredElement<HTMLElement>(root, '#canvas-host'),
    canvasSize: requiredElement<HTMLElement>(root, '#canvas-size'),
    editor: requiredElement<HTMLTextAreaElement>(root, '#lynx-xml-editor'),
    editorForm: requiredElement<HTMLFormElement>(root, '#lynx-xml-panel'),
    isolation: indicator(root, 'isolation'),
    message: requiredElement<HTMLElement>(root, '#runtime-message'),
    previewTitle: requiredElement<HTMLElement>(root, '#canvas-title'),
    renderButton: requiredElement<HTMLButtonElement>(root, '#render-xml'),
    renderButtonLabel: requiredElement<HTMLElement>(root, '#render-xml-label'),
    renderer: indicator(root, 'renderer'),
    sourceMetrics: requiredElement<HTMLElement>(root, '#source-metrics'),
    sourceStatus: requiredElement<HTMLElement>(root, '#source-status'),
    tabLinks: Array.from(root.querySelectorAll<HTMLAnchorElement>('[data-workspace-tab]')),
    workspace: requiredElement<HTMLElement>(root, '#renderer-workspace'),
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

function workspaceTab(value: string | null | undefined): WorkspaceTab {
  return value === 'lynx-xml' ? 'lynx-xml' : 'canvas';
}

function tabUrl(tab: WorkspaceTab): URL {
  const url = new URL(window.location.href);
  url.searchParams.set(TAB_PARAMETER, tab);
  return url;
}

function createTabRouter(shell: Shell): TabRouter {
  const listeners = new Set<() => void>();
  let expanded = false;

  const apply = (): void => {
    const active = workspaceTab(
      new URL(window.location.href).searchParams.get(TAB_PARAMETER),
    );
    shell.workspace.dataset['activeTab'] = active;
    shell.workspace.dataset['expanded'] = String(expanded);
    shell.sourcePanel.hidden = expanded;
    shell.editorForm.hidden = expanded || active !== 'lynx-xml';
    shell.uploadPanel.hidden = expanded || active !== 'canvas';
    shell.expandButton.setAttribute('aria-pressed', String(expanded));
    shell.expandButton.title = expanded
      ? 'Restore canvas and show source panel'
      : 'Expand canvas and hide source panel';
    requiredElement<HTMLElement>(shell.expandButton, '#expand-preview-label').textContent =
      expanded ? 'Restore' : 'Expand';
    requiredElement<HTMLElement>(shell.expandButton, '.expand-icon').textContent =
      expanded ? '⤡' : '⤢';
    shell.previewTitle.textContent =
      active === 'lynx-xml' ? 'Rendered output' : 'Bobcat canvas';
    document.title =
      active === 'lynx-xml'
        ? 'Lynx XML · Bobcat'
        : 'Bobcat · Rust on the web';

    for (const link of shell.tabLinks) {
      const tab = workspaceTab(link.dataset['workspaceTab']);
      const selected = tab === active;
      link.href = tabUrl(tab).href;
      link.dataset['active'] = String(selected);
      if (selected) {
        link.setAttribute('aria-current', 'page');
      } else {
        link.removeAttribute('aria-current');
      }
    }

    for (const listener of listeners) {
      listener();
    }
  };

  shell.expandButton.addEventListener('click', () => {
    expanded = !expanded;
    apply();
  });
  shell.workspace.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && expanded) {
      expanded = false;
      apply();
      shell.expandButton.focus();
    }
  });
  shell.zipInput.addEventListener('change', () => {
    const file = shell.zipInput.files?.[0];
    shell.zipStatus.textContent = file === undefined
      ? 'No ZIP selected'
      : `${file.name} · ${formattedBytes(file.size)}`;
  });

  for (const link of shell.tabLinks) {
    link.addEventListener('click', (event) => {
      if (
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
      ) {
        return;
      }
      event.preventDefault();
      const tab = workspaceTab(link.dataset['workspaceTab']);
      const current = new URL(window.location.href);
      const rawTab = current.searchParams.get(TAB_PARAMETER);
      if (rawTab === tab) {
        return;
      }
      const nextUrl = tabUrl(tab);
      if (workspaceTab(rawTab) === tab) {
        window.history.replaceState(null, '', nextUrl);
      } else {
        window.history.pushState(null, '', nextUrl);
      }
      apply();
    });
  }
  window.addEventListener('popstate', apply);
  apply();

  return {
    refresh: apply,
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
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

  return new Promise((resolve) => {
    navigator.serviceWorker.addEventListener(
      'controllerchange',
      () => resolve(),
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

async function requiredResponse(url: URL, label: string): Promise<Response> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(
      `Could not load ${label}: ${String(response.status)} ${response.statusText}`,
    );
  }
  return response;
}

async function loadDemoSource(): Promise<string> {
  const sourceUrl = new URL('demo.lynx.xml', document.baseURI);
  return (await requiredResponse(sourceUrl, 'demo Lynx XML')).text();
}

async function loadDemoFont(): Promise<Uint8Array> {
  const fontUrl = new URL('Roboto-Regular.ttf', document.baseURI);
  const response = await requiredResponse(fontUrl, 'demo font');
  return new Uint8Array(await response.arrayBuffer());
}

// The Local Font Access API, which TypeScript's DOM library does not declare.
interface LocalFontData {
  readonly family: string;
  blob(): Promise<Blob>;
}

type LocalFontWindow = Window & {
  queryLocalFonts?(): Promise<LocalFontData[]>;
};

// The families read from the system for Chinese, the first one installed
// winning: macOS, Windows, then the usual Linux and Android ones.
const SYSTEM_FALLBACK_FAMILIES = [
  'PingFang SC',
  'Hiragino Sans GB',
  'Microsoft YaHei',
  'Noto Sans CJK SC',
  'Noto Sans SC',
  'Source Han Sans SC',
  'WenQuanYi Micro Hei',
  'Heiti SC',
  'SimSun',
  'Droid Sans Fallback',
];

async function localFontPermission(): Promise<PermissionState> {
  try {
    const status = await navigator.permissions.query({
      name: 'local-fonts' as PermissionName,
    });
    return status.state;
  } catch {
    // A browser that does not know the permission still prompts on a query.
    return 'prompt';
  }
}

// Asks before the first read: a query made outside a click cannot prompt.
function systemFontConsent(shell: Shell): Promise<void> {
  return new Promise((resolve) => {
    const panel = document.createElement('div');
    panel.className = 'font-consent';
    const note = document.createElement('p');
    note.className = 'field-help';
    note.textContent =
      'The browser gives the engine no system fonts. Allow access to local fonts to render Chinese text; without it only Roboto is available.';
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'primary-button';
    button.textContent = 'Use system fonts';
    button.addEventListener('click', () => resolve(), { once: true });
    panel.append(note, button);
    shell.canvasHost.replaceChildren(panel);
  });
}

// The files of the first installed fallback family. A collection file holds
// every face of its family, so faces that are the same file are read once.
async function readSystemFallbackFonts(
  query: () => Promise<LocalFontData[]>,
): Promise<Uint8Array[]> {
  const installed = await query();
  const family = SYSTEM_FALLBACK_FAMILIES.find((name) =>
    installed.some((font) => font.family === name),
  );
  const files = new Map<number, Blob>();
  for (const font of installed) {
    if (font.family === family) {
      const blob = await font.blob();
      files.set(blob.size, blob);
    }
  }
  return Promise.all(
    Array.from(files.values(), async (blob) => new Uint8Array(await blob.arrayBuffer())),
  );
}

/**
 * The system fonts every script falls back to, read once the user has agreed.
 * Empty where the browser has no Local Font Access API or the user refused,
 * which leaves the page with Roboto alone.
 */
async function loadSystemFallbackFonts(shell: Shell): Promise<Uint8Array[]> {
  const query = (window as LocalFontWindow).queryLocalFonts;
  if (typeof query !== 'function') {
    return [];
  }
  const permission = await localFontPermission();
  if (permission === 'denied') {
    return [];
  }
  if (permission === 'prompt') {
    shell.message.textContent = 'Waiting for access to system fonts…';
    await systemFontConsent(shell);
  }
  try {
    return await readSystemFallbackFonts(() => query.call(window));
  } catch (error) {
    console.warn('Could not read system fonts', error);
    return [];
  }
}

function canvasMetrics(canvas: HTMLCanvasElement): CanvasSize {
  const bounds = canvas.getBoundingClientRect();
  return {
    width: Math.max(1, Math.round(bounds.width)),
    height: Math.max(1, Math.round(bounds.height)),
    dpr: Math.max(1, window.devicePixelRatio || 1),
  };
}

function createCanvas(): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.className = 'bobcat-canvas';
  canvas.setAttribute(
    'aria-label',
    'Lynx template rendered by Bobcat',
  );
  return canvas;
}

function nextAnimationFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/**
 * How `ExplorerModule.openSchema` reaches the page's template loader. It is a
 * holder rather than the function because the module is built before the
 * canvas exists and the loader before anything can be rendered.
 */
interface TemplateLoader {
  load?: (label: string, template?: boolean) => Promise<void>;
}

/**
 * Resolves what the Explorer homepage asks to open into the URLs this demo
 * should try, in order.
 *
 * An absolute URL or a relative path is taken as it is. The
 * `file://lynx?local://<path>[?query]` scheme `navigateTo` builds resolves its
 * path — query and all — against this page, where the showcase menus and the
 * `@lynx-example` demos are published under `showcase/`. Those paths name
 * `.lynx.bundle`, because that is what a native Explorer loads; Bobcat
 * consumes the `.web.bundle` beside it, so the web sibling is tried first and
 * the named file second. The retry is what serves the demos that ship only a
 * source-based `.lynx.bundle`; one carrying real bytecode fails both, and the
 * upload status reports it.
 */
function explorerTemplateUrls(raw: string): string[] {
  const value = raw.trim();
  if (!value.startsWith(LOCAL_SCHEMA_PREFIX)) {
    return [new URL(value, document.baseURI).href];
  }
  const url = new URL(
    value.slice(LOCAL_SCHEMA_PREFIX.length),
    document.baseURI,
  );
  if (!url.pathname.endsWith(LYNX_BUNDLE_SUFFIX)) {
    return [url.href];
  }
  const web = new URL(url.href);
  web.pathname = `${url.pathname.slice(0, -LYNX_BUNDLE_SUFFIX.length)}${WEB_BUNDLE_SUFFIX}`;
  return [web.href, url.href];
}

function storeExplorerValue(key: string, value: unknown): void {
  try {
    localStorage.setItem(`${STORAGE_PREFIX}${key}`, String(value));
  } catch (error) {
    console.warn(`Could not store the Explorer preference ${key}`, error);
  }
}

/**
 * The host capabilities the Lynx Explorer homepage reaches through
 * `NativeModules.ExplorerModule`, as far as a browser demo can answer them.
 *
 * Every handler runs on this thread, after the page's call has already
 * returned `undefined` to it, so nothing here can answer synchronously:
 * `readFromLocalStorage` and `getSettingInfo` exist only so the homepage's
 * `typeof` checks pass, and read back as nothing. `saveThemePreferences` and
 * `saveToLocalStorage` really do write, but nothing reads them back yet: this
 * demo passes its pages no `globalProps` at all, so the homepage sees no
 * `preferredTheme` and lays itself out in its portrait default.
 */
function createExplorerModule(
  shell: Shell,
  loader: TemplateLoader,
): NativeModules {
  return {
    ExplorerModule: {
      openSchema(...args: unknown[]): void {
        const requested = args[0];
        if (typeof requested !== 'string' || requested.trim() === '') {
          shell.uploadStatus.textContent = 'The page asked to open an empty URL';
          return;
        }
        let urls: string[];
        try {
          urls = explorerTemplateUrls(requested);
        } catch (error) {
          shell.uploadStatus.textContent = `Could not open ${requested}: ${errorMessage(error)}`;
          return;
        }
        const load = loader.load;
        if (load === undefined) {
          console.warn('Bobcat is not ready to open', requested);
          return;
        }
        shell.zipInput.value = '';
        shell.zipStatus.textContent = 'No ZIP selected';
        void (async (): Promise<void> => {
          for (const [index, url] of urls.entries()) {
            // The same load the "Load template" button runs, from the same
            // fields, so its status, busy state and error text are the ones
            // the page already has.
            shell.entryInput.value = url;
            try {
              await load(url, true);
              return;
            } catch (error) {
              // The last candidate's failure is the one the page keeps.
              if (index === urls.length - 1) {
                throw error;
              }
              console.info(`Retrying ${requested} as ${urls[index + 1]!}`);
            }
          }
        })().catch((error: unknown) => {
          console.error(error);
        });
      },
      openScan(): void {
        shell.uploadStatus.textContent =
          'QR scanning is unavailable in this browser demo; paste a template URL instead';
        console.warn(
          'ExplorerModule.openScan: this demo has no camera scanner',
        );
      },
      setThreadMode(): void {
        console.info(
          'ExplorerModule.setThreadMode: Bobcat has one thread strategy',
        );
      },
      openDevtoolSwitchPage(): void {
        console.info(
          'ExplorerModule.openDevtoolSwitchPage: this demo ships no DevTool switches',
        );
      },
      saveThemePreferences(...args: unknown[]): void {
        storeExplorerValue(String(args[0]), args[1]);
      },
      saveToLocalStorage(...args: unknown[]): void {
        storeExplorerValue(String(args[0]), args[1]);
      },
      // Present for the page's feature detection, and nothing more: a module
      // method runs off the page's JavaScript thread, so it has no return
      // value to give. The homepage treats both as absent.
      readFromLocalStorage(): void {},
      getSettingInfo(): void {},
    },
  };
}

// Every bar is coloured by the thread its marks are taken on, and the
// legend names them.
const THREAD_LABELS: Readonly<Record<LynxThread, string>> = {
  page: 'Page',
  render: 'Render Worker',
  main: 'Lynx main thread',
  background: 'Background thread (BTS)',
};

// The fcp breakdown's segments, and the marks each runs between.
const SEGMENT_LABELS: Readonly<Record<string, { label: string; marks: string }>> = {
  open: { label: 'open → fetch', marks: 'openTime → prepareTemplateStart' },
  fetchBundle: { label: 'fetchBundle', marks: 'prepareTemplateStart → prepareTemplateEnd: the template fetch' },
  prepare: { label: 'decode + group', marks: 'prepareTemplateEnd → createLynxStart: the bundle decode and the view\'s group' },
  createLynx: { label: 'createLynx', marks: 'createLynxStart → createLynxEnd' },
  viewBoot: { label: 'view boot', marks: 'createLynxEnd → loadBundleStart: the realm boot up to the entry\'s evaluation' },
  lynxFcp: { label: 'lynxFcp', marks: 'loadBundleStart → paintEnd' },
};

function performanceMetric(entry: LynxPipelineEntry | undefined, key: string): LynxPerformanceMetric | undefined {
  const value = entry?.[key];
  return typeof value === 'object' && value !== null && typeof (value as { duration?: unknown }).duration === 'number'
    ? (value as LynxPerformanceMetric)
    : undefined;
}

function millis(value: number): string {
  return `${value.toFixed(1)} ms`;
}

function frameworkStage(entry: LynxPipelineEntry): string {
  const timing = entry.frameworkRenderingTiming;
  return [timing?.['dsl'], timing?.['stage']]
    .filter((part): part is string => typeof part === 'string' && part !== '')
    .join(' · ');
}

function readPanelOpen(): boolean {
  try {
    return localStorage.getItem(PERFORMANCE_PANEL_KEY) !== 'closed';
  } catch {
    return true;
  }
}

function storePanelOpen(open: boolean): void {
  try {
    localStorage.setItem(PERFORMANCE_PANEL_KEY, open ? 'open' : 'closed');
  } catch {
    // The panel opens by default where storage is blocked.
  }
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className !== undefined) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/**
 * The entry as received, pretty-printed in a `<details>` with a Copy button:
 * the shape is Lynx native's, and the point is comparing it with a native
 * LynxView's `onPerformanceEvent` output. `open` keeps a details the user
 * opened open across redraws.
 */
function rawEntry(entry: LynxPerformanceEntry, open: WeakSet<object>): HTMLDetailsElement {
  const details = element('details', 'performance-raw');
  details.open = open.has(entry);
  details.addEventListener('toggle', () => {
    if (details.open) open.add(entry);
    else open.delete(entry);
  });
  const summary = element('summary', undefined, 'Raw entry');
  const json = JSON.stringify(entry, null, 2);
  const copy = element('button', 'performance-copy', 'Copy');
  copy.type = 'button';
  const text = element('pre', 'performance-json', json);
  copy.addEventListener('click', () => {
    void navigator.clipboard.writeText(json).then(
      () => {
        copy.textContent = 'Copied';
      },
      () => {
        // Without clipboard access the text is selected for the user to copy.
        const selection = window.getSelection();
        selection?.selectAllChildren(text);
        copy.textContent = 'Selected';
      },
    );
    window.setTimeout(() => {
      copy.textContent = 'Copy';
    }, 1500);
  });
  details.append(summary, copy, text);
  return details;
}

/**
 * One stage waterfall: a row per stage pair, its bar placed on the shared
 * axis, the stage name at the left and its duration at the right, and the
 * `paint` marks as vertical lines across every row.
 */
function waterfallElement(
  waterfall: Waterfall,
  label: string,
  threadOf: (stage: string) => LynxThread,
): HTMLElement {
  const root = element('div', 'performance-waterfall');
  root.setAttribute('role', 'group');
  root.setAttribute('aria-label', label);
  const { axis } = waterfall;
  const offset = (time: number): string => `+${(time - axis.start).toFixed(1)} ms`;
  for (const bar of waterfall.bars) {
    const row = element('div', 'waterfall-row');
    const thread = threadOf(bar.name);
    row.title = `${bar.name} · ${THREAD_LABELS[thread]}\nstart ${bar.start.toFixed(3)} (${offset(bar.start)})\nend ${bar.end.toFixed(3)} (${offset(bar.end)})\nduration ${millis(bar.duration)}`;
    const name = element('span', 'waterfall-name', bar.name);
    const track = element('span', 'waterfall-track');
    const fill = element('span', 'waterfall-bar');
    fill.dataset['thread'] = thread;
    fill.style.left = `${bar.left}%`;
    fill.style.width = `${bar.width}%`;
    track.append(fill);
    row.append(name, track, element('span', 'waterfall-duration', millis(bar.duration)));
    root.append(row);
  }
  const markers = element('div', 'waterfall-markers');
  markers.setAttribute('aria-hidden', 'true');
  for (const [index, marker] of waterfall.markers.entries()) {
    const line = element('span', 'waterfall-marker');
    line.style.left = `${marker.left}%`;
    line.title = `${marker.name}\nstartTime ${marker.time.toFixed(3)} (${offset(marker.time)})`;
    const name = element('span', 'waterfall-marker-name', marker.name);
    // Marks that fall together stack their labels instead of overlapping;
    // past the middle a label reads leftwards from its line.
    name.style.setProperty('--marker-row', String(index));
    name.dataset['side'] = marker.left > 50 ? 'end' : 'start';
    line.append(name);
    markers.append(line);
  }
  markers.style.setProperty('--marker-rows', String(waterfall.markers.length));
  const scale = element('div', 'waterfall-scale');
  scale.style.setProperty('--marker-rows', String(waterfall.markers.length));
  scale.append(
    element('span', undefined, '0'),
    element('span', undefined, millis(axis.end - axis.start)),
  );
  root.append(markers, scale);
  const threads = new Set(waterfall.bars.map((bar) => threadOf(bar.name)));
  root.append(threadLegend([...threads]));
  return root;
}

/** Swatches naming the threads `threads` lists, in the order given. */
function threadLegend(threads: readonly LynxThread[]): HTMLElement {
  const legend = element('ul', 'thread-legend');
  legend.setAttribute('aria-label', 'Threads');
  for (const thread of threads) {
    const item = element('li');
    const swatch = element('span', 'thread-swatch');
    swatch.dataset['thread'] = thread;
    item.append(swatch, document.createTextNode(THREAD_LABELS[thread]));
    legend.append(item);
  }
  return legend;
}

/**
 * The load's `totalFcp` as one bar cut at each host and engine mark, with
 * `totalFcp`, `fcp` and `lynxFcp` as spans under it and each part's time
 * listed below, so the three figures read as parts of one span.
 */
function breakdownElement(breakdown: FcpBreakdown): HTMLElement {
  const root = element('div', 'performance-waterfall fcp-breakdown');
  root.setAttribute('role', 'group');
  root.setAttribute('aria-label', 'fcp breakdown');
  const bar = element('div', 'waterfall-row');
  const track = element('span', 'waterfall-track fcp-track');
  for (const segment of breakdown.segments) {
    const part = element('span', 'fcp-segment');
    part.dataset['thread'] = segment.thread;
    part.style.left = `${segment.left}%`;
    part.style.width = `${segment.width}%`;
    const labels = SEGMENT_LABELS[segment.name];
    part.title = `${labels?.label ?? segment.name}: ${millis(segment.duration)}\n${labels?.marks ?? ''}\n${THREAD_LABELS[segment.thread]}`;
    track.append(part);
  }
  bar.append(
    element('span', 'waterfall-name', 'parts'),
    track,
    element('span', 'waterfall-duration', millis(breakdown.axis.end - breakdown.axis.start)),
  );
  root.append(bar);
  for (const span of breakdown.spans) {
    const row = element('div', 'waterfall-row');
    const spanTrack = element('span', 'waterfall-track fcp-span-track');
    const line = element('span', 'fcp-span');
    line.style.left = `${span.left}%`;
    line.style.width = `${span.width}%`;
    spanTrack.append(line);
    row.append(
      element('span', 'waterfall-name', span.name),
      spanTrack,
      element('span', 'waterfall-duration', millis(span.duration)),
    );
    root.append(row);
  }
  const parts = element('ul', 'fcp-parts');
  for (const segment of breakdown.segments) {
    const item = element('li');
    const labels = SEGMENT_LABELS[segment.name];
    item.title = `${labels?.marks ?? ''}\n${THREAD_LABELS[segment.thread]}`;
    const swatch = element('span', 'thread-swatch');
    swatch.dataset['thread'] = segment.thread;
    item.append(
      swatch,
      document.createTextNode(labels?.label ?? segment.name),
      element('strong', undefined, millis(segment.duration)),
    );
    parts.append(item);
  }
  root.append(parts);
  return root;
}

/**
 * The Lynx performance section: the current load's `fcp`, `lynxFcp` and
 * `totalFcp` with the breakdown of the span they cover (the template fetch,
 * `fetchBundle`, among its parts), its engine stages from `loadBundleStart`
 * to `paintEnd` with the `paint` marks, coloured by thread, and a log of
 * every pipeline entry.
 * Earlier loads stay selectable in the section's own Load picker.
 *
 * Draws at most once per animation frame however many entries arrive, and
 * not at all while the section is collapsed.
 */
class PerformancePanel {
  readonly #shell: PerformanceShell;
  readonly #kit: PerformanceKit;
  readonly #timeline: PerformanceTimeline;
  /** The load shown, or `undefined` to follow the newest. */
  #viewing: PerformanceLoad | undefined;
  readonly #expanded = new WeakSet<object>();
  readonly #rawOpen = new WeakSet<object>();
  /** What the summary last drew, so a log-only change leaves it alone. */
  #summaryDrawn: { load: PerformanceLoad | undefined; entry: unknown; paints: number } | undefined;
  #frame = 0;

  constructor(shell: PerformanceShell, kit: PerformanceKit) {
    this.#shell = shell;
    this.#kit = kit;
    this.#timeline = new kit.PerformanceTimeline(PERFORMANCE_ROW_CAP);
    shell.panel.open = readPanelOpen();
    shell.panel.addEventListener('toggle', () => {
      storePanelOpen(shell.panel.open);
      this.#schedule();
    });
    shell.filter.addEventListener('input', () => this.#schedule());
    shell.loads.addEventListener('change', () => {
      const loads = this.#timeline.loads;
      const index = Number(shell.loads.value);
      this.#viewing = index >= loads.length - 1 ? undefined : loads[index];
      this.#schedule();
    });
    this.#draw();
  }

  /** Starts the group a page load's entries are filed under. */
  begin(label: string): void {
    const current = this.#timeline.current;
    // Following the newest load stays on it; a user looking back at an older
    // one is left there.
    this.#timeline.begin(label);
    if (this.#viewing === current) this.#viewing = undefined;
    this.#schedule();
  }

  add(entry: LynxPerformanceEntry): void {
    this.#timeline.add(entry);
    this.#schedule();
  }

  #schedule(): void {
    if (this.#frame !== 0) return;
    this.#frame = window.requestAnimationFrame(() => {
      this.#frame = 0;
      this.#draw();
    });
  }

  #draw(): void {
    this.#drawLoads();
    const load = this.#viewing ?? this.#timeline.current;
    const shell = this.#shell;
    shell.counters.textContent = load === undefined
      ? 'No load yet'
      : `${String(load.pipelineCount)} pipeline${load.pipelineCount === 1 ? '' : 's'} · ${String(load.dropped)} dropped`;
    if (!shell.panel.open) {
      // Collapsed sections draw nothing; the summary is redrawn on opening.
      this.#summaryDrawn = undefined;
      return;
    }
    this.#drawSummary(load);
    this.#drawLog(load);
  }

  #drawLoads(): void {
    const loads = this.#timeline.loads;
    const select = this.#shell.loads;
    if (loads.length === 0) {
      select.replaceChildren(new Option('No loads yet'));
      select.disabled = true;
      return;
    }
    const shown = this.#viewing ?? loads.at(-1);
    select.replaceChildren(...loads.map((load, index) => {
      const lynxFcp = performanceMetric(load.load, 'lynxFcp');
      const label = load.label.startsWith(document.baseURI) ? load.label.slice(document.baseURI.length) : load.label;
      const live = index === loads.length - 1 ? ' (live)' : '';
      const figure = lynxFcp === undefined ? '' : ` · ${millis(lynxFcp.duration)}`;
      const option = new Option(`Load ${String(index + 1)}${live} · ${label}${figure}`, String(index));
      option.selected = load === shown;
      return option;
    }));
    select.disabled = false;
    select.title = shown?.label ?? '';
  }

  #drawSummary(load: PerformanceLoad | undefined): void {
    const shell = this.#shell;
    const entry = load?.load;
    const drawn = this.#summaryDrawn;
    if (drawn?.load === load && drawn?.entry === entry && drawn?.paints === load?.paints.length) {
      return;
    }
    this.#summaryDrawn = { load, entry, paints: load?.paints.length ?? 0 };
    for (const [target, key] of [
      [shell.fcp, 'fcp'],
      [shell.lynxFcp, 'lynxFcp'],
      [shell.totalFcp, 'totalFcp'],
    ] as const) {
      const metric = performanceMetric(entry, key);
      target.textContent = metric === undefined ? '—' : millis(metric.duration);
      target.title = metric === undefined
        ? `${key}: not reported for this load`
        : `${key}: ${metric.endTimestampName} − ${metric.startTimestampName}`;
    }
    if (entry === undefined) {
      const note = element(
        'p',
        'field-help',
        load === undefined ? 'Load a page to see its stages.' : 'Waiting for the load entry…',
      );
      shell.breakdown.replaceChildren();
      shell.waterfall.replaceChildren(note);
      shell.loadRaw.replaceChildren();
      return;
    }
    const paints: LynxPaintEntry[] = load?.paints ?? [];
    const breakdown = this.#kit.fcpBreakdown(entry);
    shell.breakdown.replaceChildren(
      ...(breakdown === undefined
        ? []
        : [element('p', 'performance-subhead', 'fcp breakdown'), breakdownElement(breakdown)]),
    );
    shell.waterfall.replaceChildren(
      element('p', 'performance-subhead', `${entry.name === 'loadBundle' ? 'lynxFcp' : entry.name} stages`),
      waterfallElement(
        this.#kit.pipelineWaterfall(entry, paints),
        `${entry.name} stages`,
        (stage) => this.#kit.stageThread(stage),
      ),
    );
    shell.loadRaw.replaceChildren(rawEntry(entry, this.#rawOpen));
  }

  #drawLog(load: PerformanceLoad | undefined): void {
    const body = this.#shell.log;
    const filter = this.#shell.filter.value.trim().toLowerCase();
    const rows: HTMLTableRowElement[] = [];
    const pipelines = load?.pipelines ?? [];
    for (let index = pipelines.length - 1; index >= 0; index -= 1) {
      const entry = pipelines[index]!;
      if (
        filter !== '' &&
        !entry.name.toLowerCase().includes(filter) &&
        !entry.identifier.toLowerCase().includes(filter)
      ) {
        continue;
      }
      rows.push(...this.#logRows(entry));
    }
    if (rows.length === 0) {
      const row = element('tr', 'performance-empty');
      const cell = element('td', undefined, pipelines.length === 0 ? 'No pipelines yet' : 'No pipeline matches the filter');
      cell.colSpan = 5;
      row.append(cell);
      rows.push(row);
    }
    body.replaceChildren(...rows);
  }

  #logRows(entry: LynxPipelineEntry): HTMLTableRowElement[] {
    const row = element('tr');
    const duration = this.#kit.pipelineDuration(entry);
    const stage = frameworkStage(entry);
    const cells = [
      element('td', 'performance-mono', entry.name),
      element('td', 'performance-mono', entry.identifier),
      element('td', undefined, stage),
      element('td', 'performance-numeric', millis(duration)),
    ];
    for (const cell of cells) cell.title = cell.textContent ?? '';
    const toggleCell = element('td');
    const toggle = element('button', 'performance-expand', '▸');
    toggle.type = 'button';
    const open = this.#expanded.has(entry);
    toggle.setAttribute('aria-expanded', String(open));
    toggle.setAttribute('aria-label', `Stages of ${entry.name}${entry.identifier === '' ? '' : ` ${entry.identifier}`}`);
    toggle.addEventListener('click', () => {
      if (this.#expanded.has(entry)) this.#expanded.delete(entry);
      else this.#expanded.add(entry);
      this.#schedule();
    });
    toggleCell.append(toggle);
    row.append(...cells, toggleCell);
    if (!open) return [row];
    const detail = element('tr', 'performance-detail');
    const cell = element('td');
    cell.colSpan = 5;
    cell.append(
      waterfallElement(
        this.#kit.pipelineWaterfall(entry, [], entry.pipelineStart),
        `${entry.name} stages`,
        (stage) => this.#kit.stageThread(stage),
      ),
      rawEntry(entry, this.#rawOpen),
    );
    detail.append(cell);
    return [row, detail];
  }
}

class PreviewRenderer {
  readonly #config: PageConfig;
  readonly #factory: BobcatCanvasFactory;
  readonly #fallbackFonts: readonly Uint8Array[];
  readonly #fontBytes: Uint8Array;
  readonly #nativeModules: NativeModules;
  readonly #onPerformanceEntry: (entry: LynxPerformanceEntry) => void;
  readonly #reportFatal: (error: Error) => void;
  readonly #resizeObserver: ResizeObserver;
  readonly #shell: Shell;
  readonly #windowResize: () => void;
  #canvas: HTMLCanvasElement | undefined;
  #generation = 0;
  #lastSize: CanvasSize | undefined;
  #resizeFrame = 0;
  #view: BobcatCanvas | undefined;

  constructor(
    shell: Shell,
    factory: BobcatCanvasFactory,
    config: PageConfig,
    fontBytes: Uint8Array,
    fallbackFonts: readonly Uint8Array[],
    nativeModules: NativeModules,
    onPerformanceEntry: (entry: LynxPerformanceEntry) => void,
    reportFatal: (error: Error) => void,
  ) {
    this.#shell = shell;
    this.#onPerformanceEntry = onPerformanceEntry;
    this.#factory = factory;
    this.#config = config;
    this.#fontBytes = fontBytes;
    this.#fallbackFonts = fallbackFonts;
    this.#nativeModules = nativeModules;
    this.#reportFatal = reportFatal;
    this.#resizeObserver = new ResizeObserver(() => this.scheduleResize());
    this.#resizeObserver.observe(shell.canvasHost);
    this.#windowResize = () => this.scheduleResize();
    window.addEventListener('resize', this.#windowResize, { passive: true });
  }

  /** `openTime` is Lynx's: when the render was asked for (`Date.now()`). */
  async render(source: string, openTime: number): Promise<void> {
    if (source.trim().length === 0) {
      throw new Error('Lynx XML source cannot be empty');
    }
    const sourceBlob = new Blob([source], {
      type: 'application/xml;charset=utf-8',
    });
    if (sourceBlob.size > MAX_LYNX_XML_BYTES) {
      throw new Error('Lynx XML source exceeds the 16 MiB browser limit');
    }

    const sourceUrl = URL.createObjectURL(sourceBlob);
    try {
      await this.#load((view) => view.loadLynxXml(sourceUrl, { openTime }));
    } finally {
      URL.revokeObjectURL(sourceUrl);
    }
  }

  async renderTemplate(entry: string, file: File | undefined, openTime: number): Promise<void> {
    if (entry.trim() === '') {
      throw new Error('Enter a template URL');
    }
    const entryUrl = new URL(
      entry.trim(),
      file === undefined ? document.baseURI : 'zip:///',
    );
    if (file === undefined) {
      if (entryUrl.protocol === 'zip:') {
        throw new Error('Choose a ZIP file to load a zip:/// template URL');
      }
      await this.#load((view) => view.loadTemplate(entryUrl, { openTime }));
      return;
    }
    if (file.size > 64 * 1024 * 1024) {
      throw new Error('ZIP exceeds the 64 MiB upload limit');
    }
    const bytes = await file.arrayBuffer();
    await this.#load((view) => view.loadZip(bytes, entryUrl, { openTime }));
  }

  // This demo hands its pages no `globalProps`; see `createExplorerModule`.
  async #load(load: (view: BobcatCanvas) => Promise<void>): Promise<void> {
    const generation = ++this.#generation;
    let view = this.#view;
    if (view === undefined) {
      const canvas = createCanvas();
      this.#shell.canvasHost.replaceChildren(canvas);
      this.#canvas = canvas;
      this.#lastSize = undefined;

      await nextAnimationFrame();
      const initial = canvasMetrics(canvas);
      this.#updateCanvasSize(initial);

      try {
        view = await this.#factory.create(
          canvas,
          initial.width,
          initial.height,
          initial.dpr,
          this.#config,
          // Retained for every page this canvas loads, like the fonts.
          {
            nativeModules: this.#nativeModules,
            onFrameRate: (fps) => {
              if (this.#canvas === canvas) {
                this.#shell.frameRate.textContent = fps === null ? 'Unavailable' : fps.toFixed(1);
              }
            },
            // Passed even while the performance section is collapsed: it is
            // what turns observation on, and a page whose load entry was not
            // observed cannot report it later. The engine's cost of
            // assembling entries nobody looks at is accepted on this demo.
            onPerformanceEntry: (entry) => {
              if (this.#canvas === canvas) {
                this.#onPerformanceEntry(entry);
              }
            },
          },
        );
      } catch (error) {
        if (this.#canvas === canvas) {
          this.#canvas = undefined;
        }
        throw error;
      }
      if (generation !== this.#generation) {
        await view.dispose();
        return;
      }
      this.#view = view;
      view.onerror = (error): void => {
        if (this.#view === view) {
          this.#shell.frameRate.textContent = '—';
          this.#reportFatal(error);
        }
      };

      try {
        // Retained by the canvas and applied to every page it loads; a
        // container without the Roboto family makes the first load reject.
        await view.registerFonts(this.#fontBytes);
        await view.setDefaultFontFamily('Roboto');
        for (const font of this.#fallbackFonts) {
          await view.registerFallbackFonts(font);
        }
      } catch (error) {
        if (this.#view === view) {
          this.#view = undefined;
        }
        if (this.#canvas === canvas) {
          this.#canvas = undefined;
        }
        view.onerror = null;
        try {
          await view.dispose();
        } catch (disposeError) {
          console.warn('Could not dispose a failed Bobcat renderer', disposeError);
        }
        throw error;
      }
    }

    await load(view);
    if (generation !== this.#generation) {
      return;
    }
    this.scheduleResize();
  }

  scheduleResize(): void {
    window.cancelAnimationFrame(this.#resizeFrame);
    this.#resizeFrame = window.requestAnimationFrame(() => {
      this.#resizeFrame = 0;
      const canvas = this.#canvas;
      const view = this.#view;
      if (canvas === undefined || view === undefined) {
        return;
      }
      const next = canvasMetrics(canvas);
      if (
        this.#lastSize?.width === next.width &&
        this.#lastSize.height === next.height &&
        this.#lastSize.dpr === next.dpr
      ) {
        return;
      }
      this.#updateCanvasSize(next);
      void view.resize(next.width, next.height, next.dpr).catch((error: unknown) => {
        if (this.#view === view) {
          this.#reportFatal(
            error instanceof Error ? error : new Error(String(error)),
          );
        }
      });
    });
  }

  async dispose(): Promise<void> {
    ++this.#generation;
    window.cancelAnimationFrame(this.#resizeFrame);
    this.#resizeObserver.disconnect();
    window.removeEventListener('resize', this.#windowResize);
    await this.#releaseView();
  }

  async #releaseView(): Promise<void> {
    this.#shell.frameRate.textContent = '—';
    const view = this.#view;
    this.#view = undefined;
    if (view === undefined) {
      return;
    }
    view.onerror = null;
    try {
      await view.dispose();
    } catch (error) {
      console.warn('Could not dispose the Bobcat renderer', error);
    }
  }

  #updateCanvasSize(size: CanvasSize): void {
    this.#lastSize = size;
    this.#shell.canvasSize.textContent = `${String(size.width)} × ${String(size.height)} · ${size.dpr.toFixed(2)}×`;
  }
}

function lineCount(source: string): number {
  return source.length === 0 ? 0 : source.split(/\r\n?|\n/u).length;
}

function formattedBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${String(bytes)} B`;
  }
  return `${(bytes / 1024).toFixed(1)} KiB`;
}

function updateSourceMetrics(shell: Shell): void {
  const source = shell.editor.value;
  const bytes = new Blob([source]).size;
  shell.sourceMetrics.textContent = `${String(lineCount(source))} lines · ${formattedBytes(bytes)}`;
}

function setSourceStatus(
  shell: Shell,
  value: string,
  state: SourceState,
): void {
  shell.sourceStatus.textContent = value;
  shell.sourceStatus.dataset['state'] = state;
}

function installSources(
  shell: Shell,
  renderer: PreviewRenderer,
  performancePanel: PerformancePanel,
): (label: string, template?: boolean) => Promise<void> {
  let rendering = false;
  const entries: string[] = [];
  let position = -1;

  const updateNavigation = (): void => {
    shell.navigation.replaceChildren(...(entries.length === 0
      ? [new Option('No pages yet')]
      : entries.map((url, index) => {
        const label = url.startsWith(document.baseURI) ? url.slice(document.baseURI.length) : url;
        return new Option(`${index + 1}. ${label}`, String(index));
      })));
    shell.backButton.disabled = rendering || position <= 0;
    shell.forwardButton.disabled = rendering || position >= entries.length - 1;
    shell.navigation.disabled = rendering || entries.length === 0;
    shell.navigation.selectedIndex = Math.max(0, position);
    shell.navigation.title = entries[position] ?? '';
    shell.navigationPosition.textContent = `${position + 1} / ${entries.length}`;
  };

  const renderSource = async (label: string, template = false, target?: number): Promise<void> => {
    if (rendering) {
      return;
    }
    // Lynx's `openTime`: the instant the page was asked for, before anything
    // of the load has run.
    const openTime = Date.now();
    rendering = true;
    updateNavigation();
    const entry = shell.entryInput.value.trim();
    const status = template ? shell.uploadStatus : shell.sourceStatus;
    const setStatus = (value: string, state: SourceState): void => {
      status.textContent = value;
      status.dataset['state'] = state;
    };
    shell.editor.readOnly = true;
    shell.editorForm.setAttribute('aria-busy', 'true');
    shell.renderButton.disabled = true;
    shell.loadButton.disabled = true;
    shell.loadButton.textContent = template ? 'Loading…' : 'Load template';
    shell.zipInput.disabled = true;
    shell.entryInput.disabled = true;
    shell.uploadPanel.setAttribute('aria-busy', 'true');
    shell.renderButtonLabel.textContent = 'Rendering…';
    setStatus(`Rendering ${label}…`, 'pending');
    setIndicator(shell.renderer, 'Rendering…', 'pending');
    shell.message.textContent = `Preparing the native Lynx view for ${label}…`;

    performancePanel.begin(template ? entry : label);
    try {
      if (template) {
        await renderer.renderTemplate(entry, shell.zipInput.files?.[0], openTime);
        if (target === undefined) {
          entries.splice(position + 1, entries.length, entry);
          position = entries.length - 1;
        } else {
          position = target;
        }
      } else {
        await renderer.render(shell.editor.value, openTime);
        // Submitted XML has no reloadable URL.
        entries.length = 0;
        position = -1;
      }
      setStatus(`Rendered ${label}`, 'ok');
      setIndicator(shell.renderer, 'Offscreen WebGPU', 'ok');
      shell.message.textContent =
        'Render complete. The Render Worker, canvas, and Wasm instance stay warm for the next submission.';
    } catch (error) {
      const message = errorMessage(error);
      setStatus(`Render failed: ${message}`, 'error');
      setIndicator(shell.renderer, 'Source error', 'error');
      shell.message.textContent = `Unable to render ${label}: ${message}`;
      throw error;
    } finally {
      rendering = false;
      updateNavigation();
      shell.editor.readOnly = false;
      shell.editorForm.removeAttribute('aria-busy');
      shell.renderButton.disabled = false;
      shell.loadButton.disabled = false;
      shell.loadButton.textContent = 'Load template';
      shell.zipInput.disabled = false;
      shell.entryInput.disabled = false;
      shell.uploadPanel.removeAttribute('aria-busy');
      shell.renderButtonLabel.textContent = 'Submit XML';
    }
  };

  const navigate = (target: number): void => {
    const url = entries[target];
    if (url === undefined || target === position || rendering) return;
    shell.entryInput.value = url;
    shell.tabLinks.find((link) => link.dataset['workspaceTab'] === 'canvas')?.click();
    void renderSource(url, true, target).catch((error: unknown) => console.error(error));
  };
  shell.backButton.addEventListener('click', () => navigate(position - 1));
  shell.forwardButton.addEventListener('click', () => navigate(position + 1));
  shell.navigation.addEventListener('change', () => navigate(Number(shell.navigation.value)));

  shell.editor.addEventListener('input', () => {
    updateSourceMetrics(shell);
    if (!rendering) {
      setSourceStatus(shell, 'Changes ready to submit', 'idle');
    }
  });
  shell.editor.addEventListener('keydown', (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      shell.editorForm.requestSubmit();
    }
  });
  shell.editorForm.addEventListener('submit', (event) => {
    event.preventDefault();
    void renderSource('submitted XML').catch((error: unknown) => {
      console.error(error);
    });
  });

  shell.uploadPanel.addEventListener('submit', (event) => {
    event.preventDefault();
    void renderSource(shell.entryInput.value, true).catch((error: unknown) => {
      console.error(error);
    });
  });

  return renderSource;
}

async function start(shell: Shell, router: TabRouter): Promise<void> {
  shell.message.textContent = 'Loading the demo source…';
  const source = await loadDemoSource();
  shell.editor.value = source;
  updateSourceMetrics(shell);
  setSourceStatus(shell, 'Demo source loaded', 'idle');

  await ensureCrossOriginIsolation(shell);
  router.refresh();

  if (typeof SharedArrayBuffer === 'undefined') {
    throw new Error('SharedArrayBuffer is unavailable despite cross-origin isolation');
  }

  const webGpuNavigator = navigator as Navigator & { readonly gpu?: unknown };
  if (webGpuNavigator.gpu === undefined) {
    setIndicator(shell.renderer, 'Unavailable', 'error');
    throw new Error('WebGPU is required to render the Bobcat canvas');
  }
  setIndicator(shell.renderer, 'Initializing…', 'pending');

  shell.message.textContent = 'Loading the demo font and threaded Rust module…';
  const fontPromise = loadDemoFont();
  const bobcatModuleUrl = new URL(
    'bobcat-wasm/dist/facade.js',
    document.baseURI,
  ).href;
  const [fontBytes, bobcatModule] = await Promise.all([
    fontPromise,
    import(/* webpackIgnore: true */ bobcatModuleUrl),
  ]);
  const kit = bobcatModule as typeof import('bobcat-wasm');
  const { BobcatCanvas, LYNX_XML_PAGE_CONFIG, default: init } = kit;

  await init();

  // The module is loaded and instantiated by now; only the Rust renderer,
  // which takes its fonts at construction, waits for the user's answer.
  const fallbackFonts = await loadSystemFallbackFonts(shell);
  shell.message.textContent =
    fallbackFonts.length === 0
      ? 'No system fonts: text Roboto does not cover is not rendered.'
      : 'System fonts loaded.';

  // The Explorer homepage opens templates through its own native module; the
  // loader it uses is the page's, and exists only once `installSources` has
  // built it below.
  const loader: TemplateLoader = {};
  const performancePanel = new PerformancePanel(shell.performance, kit);
  const renderer = new PreviewRenderer(
    shell,
    BobcatCanvas,
    LYNX_XML_PAGE_CONFIG,
    fontBytes,
    fallbackFonts,
    createExplorerModule(shell, loader),
    (entry) => performancePanel.add(entry),
    (error) => {
      setIndicator(shell.renderer, 'Failed', 'error');
      setSourceStatus(shell, `Renderer failed: ${error.message}`, 'error');
      shell.message.textContent = error.message;
    },
  );
  window.addEventListener(
    'beforeunload',
    () => {
      void renderer.dispose();
    },
    { once: true },
  );
  router.subscribe(() => renderer.scheduleResize());
  const renderSource = installSources(shell, renderer, performancePanel);
  loader.load = renderSource;
  const initialTab = workspaceTab(
    new URL(window.location.href).searchParams.get(TAB_PARAMETER),
  );
  if (initialTab === 'lynx-xml') {
    await renderSource('demo.lynx.xml');
  } else {
    await renderSource(HOMEPAGE_TEMPLATE, true);
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const shell = mountShell();
const router = createTabRouter(shell);
void start(shell, router).catch((error: unknown) => {
  const message = errorMessage(error);
  shell.message.textContent = `Unable to start: ${message}`;

  if (!globalThis.crossOriginIsolated) {
    setIndicator(shell.isolation, 'Not isolated', 'error');
  }
  if ((navigator as Navigator & { readonly gpu?: unknown }).gpu === undefined) {
    setIndicator(shell.renderer, 'Unavailable', 'error');
  } else if (shell.renderer.root.dataset['state'] === 'pending') {
    setIndicator(shell.renderer, 'Not started', 'error');
  }
  console.error(error);
});
