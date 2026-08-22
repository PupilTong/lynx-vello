const MAX_THREADS = 6
const MAX_RENDER_DIMENSION = 16_384
const RENDER_WORKER_URL = new URL('./render-worker.js', import.meta.url)
const THREAD_WORKER_URL = new URL('./dom-worker.js', import.meta.url).href

const POINTER_DEVICE_MOUSE = 0
const POINTER_DEVICE_TOUCH = 1
const POINTER_DEVICE_PEN = 2
const POINTER_PHASE_DOWN = 0
const POINTER_PHASE_MOVE = 1
const POINTER_PHASE_UP = 2
const POINTER_PHASE_CANCEL = 3

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

function validateMetrics(width, height, devicePixelRatio) {
  const physicalWidth = width * devicePixelRatio
  const physicalHeight = height * devicePixelRatio
  if (
    Number.isFinite(width) &&
    Number.isFinite(height) &&
    Number.isFinite(devicePixelRatio) &&
    width > 0 &&
    height > 0 &&
    devicePixelRatio > 0 &&
    physicalWidth <= MAX_RENDER_DIMENSION &&
    physicalHeight <= MAX_RENDER_DIMENSION
  ) {
    return
  }
  throw new TypeError(
    `Bobcat viewport metrics must be finite, positive, and no larger than ${String(MAX_RENDER_DIMENSION)} physical pixels per axis`,
  )
}

function pointerDevice(pointerType) {
  switch (pointerType) {
    case 'touch':
      return POINTER_DEVICE_TOUCH
    case 'pen':
      return POINTER_DEVICE_PEN
    case 'mouse':
    default:
      // Real browser input names one of the three values above. Treat an empty
      // value from a constructed PointerEvent as a non-scrolling mouse rather
      // than inventing another device kind at the native boundary.
      return POINTER_DEVICE_MOUSE
  }
}

/** Owns the DOM EventTarget half of the browser input bridge. */
class CanvasPointerInput {
  #active = new Map()
  #canvas
  #disposed = false
  #height
  #previousTouchAction
  #send
  #width

  constructor(canvas, width, height, send) {
    this.#canvas = canvas
    this.#height = height
    this.#previousTouchAction = canvas.style.touchAction
    this.#send = send
    this.#width = width

    // Transferring drawing control does not transfer the canvas's DOM events.
    // The embedder owns touch panning so the engine can arbitrate tap vs scroll.
    canvas.style.touchAction = 'none'
    canvas.addEventListener('pointerdown', this.#onPointerDown)
    canvas.addEventListener('pointermove', this.#onPointerMove)
    canvas.addEventListener('pointerup', this.#onPointerUp)
    canvas.addEventListener('pointercancel', this.#onPointerCancel)
    canvas.addEventListener('lostpointercapture', this.#onLostPointerCapture)
  }

  resize(width, height) {
    this.#width = width
    this.#height = height
  }

  reset() {
    for (const pointerId of this.#active.keys()) {
      this.#releaseCapture(pointerId)
    }
    this.#active.clear()
  }

  dispose() {
    if (this.#disposed) {
      return
    }
    this.#disposed = true
    this.#canvas.removeEventListener('pointerdown', this.#onPointerDown)
    this.#canvas.removeEventListener('pointermove', this.#onPointerMove)
    this.#canvas.removeEventListener('pointerup', this.#onPointerUp)
    this.#canvas.removeEventListener('pointercancel', this.#onPointerCancel)
    this.#canvas.removeEventListener(
      'lostpointercapture',
      this.#onLostPointerCapture,
    )
    this.reset()
    this.#canvas.style.touchAction = this.#previousTouchAction
  }

  #onPointerDown = (event) => {
    if (
      this.#disposed ||
      !this.#validPointerId(event.pointerId) ||
      this.#active.has(event.pointerId)
    ) {
      return
    }
    const device = pointerDevice(event.pointerType)
    if (device === POINTER_DEVICE_MOUSE && event.button !== 0) {
      return
    }
    const message = this.#message(event, device, POINTER_PHASE_DOWN)
    if (message === undefined) {
      return
    }
    this.#active.set(event.pointerId, {
      device,
      x: message.x,
      y: message.y,
    })
    try {
      this.#canvas.setPointerCapture(event.pointerId)
    } catch {
      // Capture is an interaction guarantee, not a reason to drop the down.
      // A detached or concurrently-cancelled pointer may reject the request.
    }
    this.#send(message)
  }

  #onPointerMove = (event) => {
    const active = this.#active.get(event.pointerId)
    if (this.#disposed || active === undefined) {
      return
    }
    const message = this.#message(
      event,
      active.device,
      POINTER_PHASE_MOVE,
      active,
    )
    if (message === undefined) {
      return
    }
    active.x = message.x
    active.y = message.y
    this.#send(message)
  }

  #onPointerUp = (event) => {
    this.#finish(event, POINTER_PHASE_UP)
  }

  #onPointerCancel = (event) => {
    this.#finish(event, POINTER_PHASE_CANCEL)
  }

  #onLostPointerCapture = (event) => {
    const active = this.#active.get(event.pointerId)
    if (this.#disposed || active === undefined) {
      return
    }
    const message = this.#message(
      event,
      active.device,
      POINTER_PHASE_CANCEL,
      active,
    )
    this.#active.delete(event.pointerId)
    if (message !== undefined) {
      this.#send(message)
    }
  }

  #finish(event, phase) {
    const active = this.#active.get(event.pointerId)
    if (this.#disposed || active === undefined) {
      return
    }
    const message = this.#message(event, active.device, phase, active)
    this.#active.delete(event.pointerId)
    this.#releaseCapture(event.pointerId)
    if (message !== undefined) {
      this.#send(message)
    }
  }

  #message(event, device, phase, fallback) {
    const bounds = this.#canvas.getBoundingClientRect()
    let x
    let y
    if (
      Number.isFinite(event.clientX) &&
      Number.isFinite(event.clientY) &&
      Number.isFinite(bounds.left) &&
      Number.isFinite(bounds.top) &&
      Number.isFinite(bounds.width) &&
      Number.isFinite(bounds.height) &&
      bounds.width > 0 &&
      bounds.height > 0
    ) {
      x = ((event.clientX - bounds.left) * this.#width) / bounds.width
      y = ((event.clientY - bounds.top) * this.#height) / bounds.height
    } else if (fallback !== undefined) {
      x = fallback.x
      y = fallback.y
    } else {
      return undefined
    }
    if (!Number.isFinite(x) || !Number.isFinite(y)) {
      return undefined
    }
    return {
      defaultPrevented: event.defaultPrevented === true,
      device,
      phase,
      pointerId: event.pointerId,
      x,
      y,
    }
  }

  #releaseCapture(pointerId) {
    try {
      if (
        typeof this.#canvas.hasPointerCapture !== 'function' ||
        this.#canvas.hasPointerCapture(pointerId)
      ) {
        this.#canvas.releasePointerCapture(pointerId)
      }
    } catch {
      // The browser may already have released capture while dispatching up or
      // cancel. The sequence is complete either way.
    }
  }

  #validPointerId(pointerId) {
    return (
      Number.isInteger(pointerId) &&
      pointerId >= 0 &&
      pointerId <= 0xffff_ffff
    )
  }
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

  dispatchPointer(values) {
    if (this.#fatalError !== undefined) {
      return
    }
    try {
      this.#worker.postMessage({ type: 'bobcat-pointer', ...values })
    } catch (error) {
      this.#fail(error)
    }
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

/** A Worker-owned Bobcat view with automatic canvas pointer forwarding. */
export class BobcatCanvas {
  #client
  #disposed = false
  #fatalError
  #pointerInput
  #unsubscribeFatal

  onerror = null

  constructor(client, canvas, width, height) {
    this.#client = client
    this.#pointerInput = new CanvasPointerInput(
      canvas,
      width,
      height,
      (values) => client.dispatchPointer(values),
    )
    this.#unsubscribeFatal = client.subscribeFatal((error) => {
      this.#fatalError = error
      this.#pointerInput.dispose()
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
    validateMetrics(width, height, devicePixelRatio)
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
    return new BobcatCanvas(client, canvas, width, height)
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
   * The current native view accepts exactly one entry-script operation;
   * `reset()` installs a fresh view. Loading, VM startup, and execution have
   * no facade-imposed deadline.
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
   * is a one-shot operation for the current native view; a repeated call
   * rejects before fetch or stylesheet mounting unless `reset()` ran first.
   */
  async loadLynxXml(url) {
    await this.#request('loadLynxXml', { url: documentUrl(url) })
  }

  /**
   * Replace the native Lynx view while retaining this Render Worker, its
   * transferred canvas, initialized Wasm instance, page configuration,
   * current device metrics, registered font containers, and selected default
   * font family.
   */
  async reset() {
    this.#pointerInput.reset()
    await this.#request('reset')
  }

  /** Register one or more font faces and restore them after each reset. */
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
    validateMetrics(width, height, devicePixelRatio)
    // Messages from one Window reach the Worker in order: updating the input
    // map before posting resize means a following pointer is expressed in the
    // viewport the Worker will install before it dispatches that pointer.
    this.#pointerInput.resize(width, height)
    await this.#request('resize', { devicePixelRatio, height, width })
  }

  async dispose() {
    if (this.#disposed) {
      return
    }
    this.#disposed = true
    this.#pointerInput.dispose()
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
