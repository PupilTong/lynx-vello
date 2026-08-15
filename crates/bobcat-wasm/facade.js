const MAX_THREADS = 6
const REQUEST_TIMEOUT_MS = 30_000
const RENDER_WORKER_URL = new URL('./render-worker.js', import.meta.url)
const DOM_WORKER_URL = new URL('./dom-worker.js', import.meta.url).href

let initialization

function preferredThreadCount() {
  const hardwareThreads = Math.max(
    1,
    globalThis.navigator?.hardwareConcurrency ?? 1,
  )
  return Math.max(1, Math.min(MAX_THREADS, hardwareThreads - 1 || 1))
}

function asError(error) {
  return error instanceof Error ? error : new Error(String(error))
}

function errorMessage(error) {
  return asError(error).message
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
      const timeout = setTimeout(() => {
        this.#fail(
          new Error(
            `Bobcat Render Worker did not initialize within ${String(REQUEST_TIMEOUT_MS)} ms`,
          ),
        )
      }, REQUEST_TIMEOUT_MS)
      this.#resolveReady = () => {
        clearTimeout(timeout)
        resolve()
      }
      this.#rejectReady = (error) => {
        clearTimeout(timeout)
        reject(error)
      }
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
    for (const listener of this.#fatalListeners) {
      listener(this.#fatalError)
    }
    this.#worker.terminate()
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

  static async create(canvas, width, height, devicePixelRatio) {
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
          devicePixelRatio,
          domWorkerUrl: DOM_WORKER_URL,
          height,
          threadCount: preferredThreadCount(),
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

  async addAuthorStylesheet(css) {
    await this.#request('addAuthorStylesheet', { css })
  }

  appendElement(parent, child) {
    return this.#request('appendElement', { child, parent })
  }

  createPage(_componentId, _componentCssId) {
    return this.#request('createPage')
  }

  createView(_parentComponentUniqueId) {
    return this.#request('createView')
  }

  async dropElement(element) {
    await this.#request('dropElement', { element })
  }

  async flushElementTree() {
    await this.#request('flushElementTree')
  }

  registerFonts(bytes) {
    return this.#request('registerFonts', { bytes })
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
