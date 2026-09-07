import './styles.css';
import { mountArchive, type MountedArchive } from './archive';

import type { BobcatCanvas, PageConfig } from 'bobcat-wasm';

const MAX_LYNX_XML_BYTES = 16 * 1024 * 1024;
const RELOAD_MARKER = `bobcat-coi-reload:${new URL('.', document.baseURI).pathname}`;
const RELOAD_PARAMETER = 'bobcat-coi-reload';
const TAB_PARAMETER = 'tab';

type IndicatorState = 'pending' | 'ok' | 'error';
type SourceState = 'idle' | 'pending' | 'ok' | 'error';
type WorkspaceTab = 'canvas' | 'lynx-xml';

interface Indicator {
  readonly value: HTMLElement;
  readonly root: HTMLElement;
}

type BobcatCanvasFactory = Pick<typeof BobcatCanvas, 'create'>;

interface Shell {
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
                <label for="canvas-zip">Local ZIP file</label>
                <p id="zip-help" class="field-help">Choose a ZIP archive containing your template and resources.</p>
                <input id="canvas-zip" type="file" required accept=".zip,application/zip,application/x-zip-compressed" aria-describedby="zip-help zip-status">
                <output id="zip-status" class="field-help" aria-live="polite">No ZIP selected</output>
              </div>
              <div class="source-field">
                <label for="entry-template-url">Entry template URL</label>
                <input id="entry-template-url" type="text" required placeholder="dist/main.web.bundle" autocomplete="off" autocapitalize="off" spellcheck="false" aria-describedby="template-loading-note">
              </div>
              <p id="template-loading-note" class="field-help">Enter a path from the ZIP root, or a full URL whose pathname matches it. Supports .lynx.xml, binary .web.bundle, and source-based .lynx.bundle templates.</p>
            </div>
            <div class="editor-footer">
              <output id="upload-status" class="field-help" aria-live="polite">Choose a ZIP and enter its template path.</output>
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

          <section class="preview-panel" aria-labelledby="canvas-title">
            <div class="pane-heading preview-heading">
              <div>
                <p class="panel-kicker">LIVE FRAME</p>
                <h3 id="canvas-title">Bobcat canvas</h3>
              </div>
              <div class="preview-actions">
                <output id="canvas-size">—</output>
                <button id="expand-preview" type="button" aria-controls="canvas-source-panel lynx-xml-panel" aria-pressed="false" title="Expand canvas and hide source panel">
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

class PreviewRenderer {
  readonly #config: PageConfig;
  readonly #factory: BobcatCanvasFactory;
  readonly #fontBytes: Uint8Array;
  readonly #reportFatal: (error: Error) => void;
  readonly #resizeObserver: ResizeObserver;
  readonly #shell: Shell;
  readonly #windowResize: () => void;
  #archive: MountedArchive | undefined;
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
    reportFatal: (error: Error) => void,
  ) {
    this.#shell = shell;
    this.#factory = factory;
    this.#config = config;
    this.#fontBytes = fontBytes;
    this.#reportFatal = reportFatal;
    this.#resizeObserver = new ResizeObserver(() => this.scheduleResize());
    this.#resizeObserver.observe(shell.canvasHost);
    this.#windowResize = () => this.scheduleResize();
    window.addEventListener('resize', this.#windowResize, { passive: true });
  }

  async render(source: string): Promise<void> {
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
      await this.#load((view) => view.loadLynxXml(sourceUrl));
      await this.#archive?.dispose();
      this.#archive = undefined;
    } finally {
      URL.revokeObjectURL(sourceUrl);
    }
  }

  async renderArchive(file: File, entry: string): Promise<void> {
    const archive = await mountArchive(file, entry);
    try {
      await this.#load((view) => archive.entryUrl.pathname.endsWith('.xml')
        ? view.loadLynxXml(archive.entryUrl)
        : view.loadTemplate(archive.entryUrl));
    } catch (error) {
      await archive.dispose();
      throw error;
    }
    await this.#archive?.dispose();
    this.#archive = archive;
  }

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
          this.#reportFatal(error);
        }
      };

      try {
        // Retained by the canvas and applied to every page it loads; a
        // container without the Roboto family makes the first load reject.
        await view.registerFonts(this.#fontBytes);
        await view.setDefaultFontFamily('Roboto');
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
    const archive = this.#archive;
    this.#archive = undefined;
    await Promise.all([this.#releaseView(), archive?.dispose()]);
  }

  async #releaseView(): Promise<void> {
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
): (label: string) => Promise<void> {
  let rendering = false;

  const renderSource = async (label: string, archive = false): Promise<void> => {
    if (rendering) {
      return;
    }
    rendering = true;
    const source = shell.editor.value;
    const status = archive ? shell.uploadStatus : shell.sourceStatus;
    const setStatus = (value: string, state: SourceState): void => {
      status.textContent = value;
      status.dataset['state'] = state;
    };
    shell.editor.readOnly = true;
    shell.editorForm.setAttribute('aria-busy', 'true');
    shell.renderButton.disabled = true;
    shell.loadButton.disabled = true;
    shell.loadButton.textContent = archive ? 'Loading…' : 'Load template';
    shell.zipInput.disabled = true;
    shell.entryInput.disabled = true;
    shell.uploadPanel.setAttribute('aria-busy', 'true');
    shell.renderButtonLabel.textContent = 'Rendering…';
    setStatus(`Rendering ${label}…`, 'pending');
    setIndicator(shell.renderer, 'Rendering…', 'pending');
    shell.message.textContent = `Preparing the native Lynx view for ${label}…`;

    try {
      if (archive) {
        const file = shell.zipInput.files?.[0];
        if (file === undefined) throw new Error('Choose a ZIP file first');
        await renderer.renderArchive(file, shell.entryInput.value);
      } else {
        await renderer.render(source);
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
    'bobcat-wasm/facade.js',
    document.baseURI,
  ).href;
  const [fontBytes, bobcatModule] = await Promise.all([
    fontPromise,
    import(/* webpackIgnore: true */ bobcatModuleUrl),
  ]);
  const { BobcatCanvas, LYNX_XML_PAGE_CONFIG, default: init } =
    bobcatModule as typeof import('bobcat-wasm');

  await init();

  const renderer = new PreviewRenderer(
    shell,
    BobcatCanvas,
    LYNX_XML_PAGE_CONFIG,
    fontBytes,
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
  const renderSource = installSources(shell, renderer);
  await renderSource('demo.lynx.xml');
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
