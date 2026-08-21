const MAX_THREADS = 6
const RENDER_WORKER_URL = new URL('./render-worker.js', import.meta.url)
const THREAD_WORKER_URL = new URL('./dom-worker.js', import.meta.url).href

let initialization

/** web-core raw-loader defaults; callers may spread this object to override. */
export const LYNX_XML_PAGE_CONFIG = Object.freeze({
  defaultDisplayLinear: false,
  defaultOverflowVisible: false,
  enableCSSSelector: true,
})

function preferredThreadCount() {
  const hardwareThreads = Math.max(
    1,
    globalThis.navigator?.hardwareConcurrency ?? 1,
  )
  return Math.max(2, Math.min(MAX_THREADS, hardwareThreads - 1 || 2))
}

function asError(error) {
  return error instanceof Error ? error : new Error(String(error))
}

function errorMessage(error) {
  return asError(error).message
}

function documentUrl(input) {
  return new URL(String(input), document.baseURI).href
}

function fontBytes(data) {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data)
  }
  if (data instanceof Uint8Array) {
    return new Uint8Array(data)
  }
  throw new TypeError('BobcatCanvas.registerFonts requires an ArrayBuffer or Uint8Array')
}

class RenderWorkerClient {
  #fatalError
  #fatalListeners = new Set()
  #nextRequest = 1
  #pending = new Map()
  #ready
  #rejectReady
  #resolveReady
  #readySettled = false
  #worker

  constructor(worker) {
    this.#worker = worker
    this.#ready = new Promise((resolve, reject) => {
      this.#resolveReady = resolve
      this.#rejectReady = reject
    })

    worker.addEventListener('message', this.#onMessage)
    worker.addEventListener('messageerror', this.#onMessageError)
    worker.addEventListener('error', this.#onError)
  }

  get error() {
    return this.#fatalError
  }

  get ready() {
    return this.#ready
  }

  #fail(error) {
    if (this.#fatalError !== undefined) {
      return
    }
    this.#fatalError = asError(error)
    if (!this.#readySettled) {
      this.#readySettled = true
      this.#rejectReady(this.#fatalError)
    }
    for (const pending of this.#pending.values()) {
      pending.reject(this.#fatalError)
    }
    this.#pending.clear()
    this.#worker.terminate()
    for (const listener of this.#fatalListeners) {
      listener(this.#fatalError)
    }
  }

  #onError = (event) => {
    this.#fail(event.error ?? new Error(event.message))
  }

  #onMessageError = () => {
    this.#fail(new Error('Bobcat Render Worker sent an unreadable message'))
  }

  #onMessage = (event) => {
    const message = event.data
    if (message?.type === 'bobcat-ready') {
      if (!this.#readySettled) {
        this.#readySettled = true
        this.#resolveReady()
      }
      return
    }
    if (message?.type === 'bobcat-error') {
      this.#fail(new Error(message.message))
      return
    }
    if (message?.type !== 'bobcat-response') {
      return
    }

    const pending = this.#pending.get(message.request)
    if (pending === undefined) {
      return
    }
    this.#pending.delete(message.request)
    if (message.ok) {
      pending.resolve(message.value)
    } else {
      pending.reject(new Error(message.error))
    }
  }

  request(operation, values = {}) {
    if (this.#fatalError !== undefined) {
      return Promise.reject(this.#fatalError)
    }
    const request = this.#nextRequest
    this.#nextRequest = request === 0xffff_ffff ? 1 : request + 1
    const result = new Promise((resolve, reject) => {
      this.#pending.set(request, { reject, resolve })
    })
    try {
      this.#worker.postMessage({
        type: 'bobcat-request',
        operation,
        request,
        ...values,
      })
    } catch (error) {
      const pending = this.#pending.get(request)
      this.#pending.delete(request)
      pending.reject(error)
    }
    return result
  }

  subscribeFatal(listener) {
    this.#fatalListeners.add(listener)
    if (this.#fatalError !== undefined) {
      listener(this.#fatalError)
    }
    return () => this.#fatalListeners.delete(listener)
  }

  close() {
    const closed = new Error('Bobcat Render Worker was closed')
    for (const pending of this.#pending.values()) {
      pending.reject(closed)
    }
    this.#pending.clear()
    this.#worker.removeEventListener('message', this.#onMessage)
    this.#worker.removeEventListener('messageerror', this.#onMessageError)
    this.#worker.removeEventListener('error', this.#onError)
    this.#worker.terminate()
  }
}

export default function init() {
  initialization ??= Promise.resolve().then(() => {
    if (!globalThis.crossOriginIsolated) {
      throw new Error(
        'Bobcat threads require a cross-origin-isolated browsing context',
      )
    }
    if (typeof globalThis.SharedArrayBuffer !== 'function') {
      throw new Error('Bobcat threads require SharedArrayBuffer support')
    }
  })
  return initialization
}

export class BobcatCanvas {
  #client
  #disposed = false
  #fatalError
  #unsubscribeFatal

  onerror = null

  constructor(client) {
    this.#client = client
    this.#unsubscribeFatal = client.subscribeFatal((error) => {
      this.#fatalError = error
      if (typeof this.onerror === 'function') {
        this.onerror(error)
      }
    })
  }

  static async create(
    canvas,
    width,
    height,
    devicePixelRatio,
    pageConfig,
  ) {
    await init()
    if (
      typeof globalThis.HTMLCanvasElement !== 'function' ||
      !(canvas instanceof globalThis.HTMLCanvasElement)
    ) {
      throw new TypeError('BobcatCanvas.create requires an HTMLCanvasElement')
    }
    if (typeof canvas.transferControlToOffscreen !== 'function') {
      throw new Error('This browser does not support OffscreenCanvas transfer')
    }
    if (pageConfig === null || typeof pageConfig !== 'object') {
      throw new TypeError('BobcatCanvas.create pageConfig must be an object')
    }
    const config = {
      defaultDisplayLinear: pageConfig.defaultDisplayLinear,
      defaultOverflowVisible: pageConfig.defaultOverflowVisible,
      enableCSSSelector: pageConfig.enableCSSSelector,
    }
    for (const [name, value] of Object.entries(config)) {
      if (typeof value !== 'boolean') {
        throw new TypeError(`Bobcat pageConfig.${name} must be a boolean`)
      }
    }

    const offscreen = canvas.transferControlToOffscreen()
    let client
    let worker
    try {
      worker = new Worker(RENDER_WORKER_URL, {
        name: 'bobcat-render',
        type: 'module',
      })
      client = new RenderWorkerClient(worker)
      worker.postMessage(
        {
          type: 'bobcat-init',
          canvas: offscreen,
          config,
          devicePixelRatio,
          height,
          threadCount: preferredThreadCount(),
          workerUrl: THREAD_WORKER_URL,
          width,
        },
        [offscreen],
      )
      await client.ready
    } catch (error) {
      client?.close()
      worker?.terminate()
      throw new Error(
        `Bobcat Canvas initialization failed; replace the canvas before retrying: ${errorMessage(error)}`,
        { cause: error },
      )
    }
    return new BobcatCanvas(client)
  }

  get error() {
    return this.#fatalError
  }

  #request(operation, values) {
    if (this.#disposed) {
      return Promise.reject(new Error('This BobcatCanvas has been disposed'))
    }
    if (this.#fatalError !== undefined) {
      return Promise.reject(this.#fatalError)
    }
    return this.#client.request(operation, values)
  }

  /**
   * Fetch and run the main-thread script at `url`.
   *
   * The Promise resolves after Bobcat's boot sequence and rejects on fetch,
   * VM initialization, or evaluation failure. Relative URLs are resolved
   * against this document's base URL before they cross the Worker boundary.
   * A Canvas accepts exactly one entry-script operation. Loading, VM startup,
   * and execution have no facade-imposed deadline.
   */
  async executeScript(url) {
    await this.#request('executeScript', { url: documentUrl(url) })
  }

  /** Fetch and mount an author stylesheet at `url`. */
  async loadStyleSheet(url) {
    await this.#request('loadStyleSheet', { url: documentUrl(url) })
  }

  /**
   * Fetch, parse, and run a single-file Lynx XML source envelope.
   *
   * A present stylesheet is mounted before its main-thread script starts. A
   * background-thread section is retained, but currently produces a console
   * warning because Bobcat does not execute it yet. Page configuration remains
   * the host's `BobcatCanvas.create` choice; `LYNX_XML_PAGE_CONFIG` supplies
   * web-core's raw-loader defaults when the host does not need overrides. This
   * is a one-shot entry-script operation; a repeated call rejects before fetch
   * or stylesheet mounting.
   */
  async loadLynxXml(url) {
    await this.#request('loadLynxXml', { url: documentUrl(url) })
  }

  /** Register one or more font faces from an OpenType font container. */
  async registerFonts(data) {
    return await this.#request('registerFonts', { bytes: fontBytes(data) })
  }

  /** Map CSS system-ui, sans-serif, and serif to a registered family. */
  async setDefaultFontFamily(family) {
    if (typeof family !== 'string' || family.trim() === '') {
      throw new TypeError(
        'BobcatCanvas.setDefaultFontFamily requires a non-empty family name',
      )
    }
    return await this.#request('setDefaultFontFamily', {
      family: family.trim(),
    })
  }

  async resize(width, height, devicePixelRatio) {
    await this.#request('resize', { devicePixelRatio, height, width })
  }

  async dispose() {
    if (this.#disposed) {
      return
    }
    this.#disposed = true
    try {
      if (this.#fatalError === undefined) {
        await this.#client.request('dispose')
      }
    } finally {
      this.#unsubscribeFatal()
      this.#client.close()
    }
  }
}
