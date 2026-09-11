import { ImageDecoder } from './image-decoder.ts'
import type {
  InitMessage,
  Operation,
  PointerDevice,
  PointerFields,
  PointerMessage,
  PointerPhase,
  RenderWorkerMessage,
  RequestFields,
} from './protocol.d.ts'

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

let initialization: Promise<void> | undefined

export interface PageConfig {
  defaultDisplayLinear: boolean
  defaultOverflowVisible: boolean
  enableCSSSelector: boolean
}

/** web-core raw-loader defaults; callers may spread this object to override. */
export const LYNX_XML_PAGE_CONFIG: Readonly<PageConfig> = Object.freeze({
  defaultDisplayLinear: false,
  defaultOverflowVisible: false,
  enableCSSSelector: true,
})

// The machine's parallelism, raw. Core turns it into a pool with the same
// heuristic and the same cap a native view gets, so this side does no
// arithmetic of its own, and the cap is not restated here.
function hardwareConcurrency(): number {
  return Math.max(1, globalThis.navigator?.hardwareConcurrency ?? 1)
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error))
}

function errorMessage(error: unknown): string {
  return asError(error).message
}

function documentUrl(input: string | URL): string {
  return new URL(String(input), document.baseURI).href
}

function fontBytes(data: ArrayBuffer | Uint8Array): Uint8Array {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data)
  }
  if (data instanceof Uint8Array) {
    return new Uint8Array(data)
  }
  throw new TypeError('BobcatCanvas.registerFonts requires an ArrayBuffer or Uint8Array')
}

function validateMetrics(
  width: number,
  height: number,
  devicePixelRatio: number,
): void {
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

function pointerDevice(pointerType: string): PointerDevice {
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

interface ActivePointer {
  device: PointerDevice
  x: number
  y: number
}

/** Owns the DOM EventTarget half of the browser input bridge. */
class CanvasPointerInput {
  #active = new Map<number, ActivePointer>()
  #canvas: HTMLCanvasElement
  #disposed = false
  #height: number
  #previousTouchAction: string
  #send: (values: PointerFields) => void
  #width: number

  constructor(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    send: (values: PointerFields) => void,
  ) {
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

  resize(width: number, height: number): void {
    this.#width = width
    this.#height = height
  }

  /** Ends every live sequence, so a page that survives keeps no stale one. */
  reset(): void {
    for (const [pointerId, active] of this.#active) {
      this.#releaseCapture(pointerId)
      if (!this.#disposed) {
        this.#send({
          defaultPrevented: false,
          device: active.device,
          phase: POINTER_PHASE_CANCEL,
          pointerId,
          x: active.x,
          y: active.y,
        })
      }
    }
    this.#active.clear()
  }

  dispose(): void {
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

  #onPointerDown = (event: PointerEvent) => {
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

  #onPointerMove = (event: PointerEvent) => {
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

  #onPointerUp = (event: PointerEvent) => {
    this.#finish(event, POINTER_PHASE_UP)
  }

  #onPointerCancel = (event: PointerEvent) => {
    this.#finish(event, POINTER_PHASE_CANCEL)
  }

  #onLostPointerCapture = (event: PointerEvent) => {
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

  #finish(event: PointerEvent, phase: PointerPhase): void {
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

  #message(
    event: PointerEvent,
    device: PointerDevice,
    phase: PointerPhase,
    fallback?: ActivePointer,
  ): PointerFields | undefined {
    const bounds = this.#canvas.getBoundingClientRect()
    let x: number
    let y: number
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

  #releaseCapture(pointerId: number): void {
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

  #validPointerId(pointerId: number): boolean {
    return (
      Number.isInteger(pointerId) &&
      pointerId >= 0 &&
      pointerId <= 0xffff_ffff
    )
  }
}

interface PendingRequest {
  reject: (reason: unknown) => void
  resolve: () => void
}

class RenderWorkerClient {
  #fatalError: Error | undefined
  #fatalListeners = new Set<(error: Error) => void>()
  #nextRequest = 1
  #pending = new Map<number, PendingRequest>()
  #ready: Promise<void>
  #rejectReady!: (error: Error) => void
  #resolveReady!: () => void
  #readySettled = false
  #worker: Worker

  constructor(worker: Worker) {
    this.#worker = worker
    this.#ready = new Promise((resolve, reject) => {
      this.#resolveReady = resolve
      this.#rejectReady = reject
    })

    worker.addEventListener('message', this.#onMessage)
    worker.addEventListener('messageerror', this.#onMessageError)
    worker.addEventListener('error', this.#onError)
  }

  get error(): Error | undefined {
    return this.#fatalError
  }

  get ready(): Promise<void> {
    return this.#ready
  }

  #fail(error: unknown): void {
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

  #onError = (event: ErrorEvent) => {
    this.#fail(event.error ?? new Error(event.message))
  }

  #onMessageError = () => {
    this.#fail(new Error('Bobcat Render Worker sent an unreadable message'))
  }

  #onMessage = (event: MessageEvent) => {
    const message = event.data as RenderWorkerMessage
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
      pending.resolve()
    } else {
      pending.reject(new Error(message.error))
    }
  }

  // `dispose` is the one operation without fields, so it alone may omit them.
  request(operation: 'dispose'): Promise<void>
  request<O extends Operation>(
    operation: O,
    values: RequestFields[O],
  ): Promise<void>
  request(
    operation: Operation,
    values: RequestFields[Operation] = {},
  ): Promise<void> {
    if (this.#fatalError !== undefined) {
      return Promise.reject(this.#fatalError)
    }
    const request = this.#nextRequest
    this.#nextRequest = request === 0xffff_ffff ? 1 : request + 1
    const result = new Promise<void>((resolve, reject) => {
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
      // The Promise executor above stored this entry synchronously.
      const pending = this.#pending.get(request)!
      this.#pending.delete(request)
      pending.reject(error)
    }
    return result
  }

  dispatchPointer(values: PointerFields): void {
    if (this.#fatalError !== undefined) {
      return
    }
    try {
      this.#worker.postMessage({
        type: 'bobcat-pointer',
        ...values,
      } satisfies PointerMessage)
    } catch (error) {
      this.#fail(error)
    }
  }

  subscribeFatal(listener: (error: Error) => void): () => boolean {
    this.#fatalListeners.add(listener)
    if (this.#fatalError !== undefined) {
      listener(this.#fatalError)
    }
    return () => this.#fatalListeners.delete(listener)
  }

  close(): void {
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

export default function init(): Promise<void> {
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

/**
 * A Worker-owned Bobcat view with automatic canvas pointer forwarding.
 *
 * A Worker-owned Bobcat view attached to one HTML canvas. Active
 * `pointerdown`/`pointermove`/`pointerup`/`pointercancel` sequences on the
 * canvas are captured and forwarded to the native input router automatically.
 */
export class BobcatCanvas {
  #client: RenderWorkerClient
  #decoder: ImageDecoder
  #disposed = false
  #fatalError: Error | undefined
  #pointerInput: CanvasPointerInput
  #unsubscribeFatal: () => void

  onerror: ((error: Error) => void) | null = null

  private constructor(
    client: RenderWorkerClient,
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    decoder: ImageDecoder,
  ) {
    this.#client = client
    this.#decoder = decoder
    this.#pointerInput = new CanvasPointerInput(
      canvas,
      width,
      height,
      (values) => client.dispatchPointer(values),
    )
    this.#unsubscribeFatal = client.subscribeFatal((error) => {
      this.#fatalError = error
      this.#pointerInput.dispose()
      this.#decoder.close()
      if (typeof this.onerror === 'function') {
        this.onerror(error)
      }
    })
  }

  static async create(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    devicePixelRatio: number,
    pageConfig: PageConfig,
  ): Promise<BobcatCanvas> {
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
    // The Render Worker fetches a page's images and this thread decodes
    // them, over a channel of their own.
    const images = new MessageChannel()
    const decoder = new ImageDecoder(images.port1)
    let client: RenderWorkerClient | undefined
    let worker: Worker | undefined
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
          imagePort: images.port2,
          hardwareConcurrency: hardwareConcurrency(),
          workerUrl: THREAD_WORKER_URL,
          width,
        } satisfies InitMessage,
        [offscreen, images.port2],
      )
      await client.ready
    } catch (error) {
      decoder.close()
      client?.close()
      worker?.terminate()
      throw new Error(
        `Bobcat Canvas initialization failed; replace the canvas before retrying: ${errorMessage(error)}`,
        { cause: error },
      )
    }
    return new BobcatCanvas(client, canvas, width, height, decoder)
  }

  get error(): Error | undefined {
    return this.#fatalError
  }

  #request<O extends Operation>(
    operation: O,
    values: RequestFields[O],
  ): Promise<void> {
    if (this.#disposed) {
      return Promise.reject(new Error('This BobcatCanvas has been disposed'))
    }
    if (this.#fatalError !== undefined) {
      return Promise.reject(this.#fatalError)
    }
    return this.#client.request(operation, values)
  }

  /**
   * Fetch a page's author stylesheets and its main-thread entry script, then
   * show it.
   *
   * A native view is its page, so this builds a fresh one and drops the view
   * before it. Stylesheets cascade in the order given and all mount before the
   * entry script runs. The Promise resolves after Bobcat's boot sequence and
   * rejects on fetch, VM initialization, or evaluation failure, leaving the
   * previous page running if the fetch was what failed. Relative URLs resolve
   * against this document's base URL. Nothing here imposes a deadline.
   */
  async load(
    url: string | URL,
    styleSheetUrls: readonly (string | URL)[] = [],
  ): Promise<void> {
    if (!Array.isArray(styleSheetUrls)) {
      throw new TypeError('BobcatCanvas.load styleSheetUrls must be an array')
    }
    this.#pointerInput.reset()
    await this.#request('load', {
      styleSheetUrls: styleSheetUrls.map(documentUrl),
      url: documentUrl(url),
    })
  }

  /**
   * Fetch, parse, and show a single-file Lynx XML source envelope: the same
   * load as `load()`, with the envelope's sections as the sources. Its
   * optional background script runs as a module in the page's BTS worker
   * after the main-thread entry loads, with `lynx.getCoreContext()`
   * available. Page configuration remains the host's `BobcatCanvas.create`
   * choice; `LYNX_XML_PAGE_CONFIG` supplies web-core's raw-loader defaults.
   */
  async loadLynxXml(url: string | URL): Promise<void> {
    this.#pointerInput.reset()
    await this.#request('loadLynxXml', { url: documentUrl(url) })
  }

  /**
   * Fetch and show a binary web or source-based native template container.
   *
   * Fetch and decode a binary web or source-based native bundle (root entry).
   * Uses the container's page configuration and resolves relative resources
   * against its response URL. Native bytecode is rejected by the shared parser.
   */
  async loadTemplate(url: string | URL): Promise<void> {
    this.#pointerInput.reset()
    await this.#request('loadTemplate', { url: documentUrl(url) })
  }

  /**
   * Decode local ZIP bytes with bobcat-source and load the selected entry.
   *
   * Decode a ZIP and its selected template using bobcat-source. entryUrl must
   * be absolute: its decoded pathname selects the member and its origin maps
   * the ZIP resources. XML is strict UTF-8; binary bundles require root.
   * The shared loader limits input to 64 MiB, output to 128 MiB, and 4096 entries.
   */
  async loadZip(
    data: ArrayBuffer | Uint8Array,
    entryUrl: string | URL,
  ): Promise<void> {
    if (!(data instanceof ArrayBuffer) && !(data instanceof Uint8Array)) {
      throw new TypeError('BobcatCanvas.loadZip requires an ArrayBuffer or Uint8Array')
    }
    if (data.byteLength > 64 * 1024 * 1024) {
      throw new RangeError('ZIP exceeds the 64 MiB compressed size limit')
    }
    this.#pointerInput.reset()
    await this.#request('loadZip', {
      bytes: new Uint8Array(data),
      url: String(entryUrl),
    })
  }

  /** Retain font faces for every page this canvas loads. Call before a load. */
  async registerFonts(data: ArrayBuffer | Uint8Array): Promise<void> {
    await this.#request('registerFonts', { bytes: fontBytes(data) })
  }

  /**
   * Map CSS system-ui, sans-serif, and serif to a family for every page this
   * canvas loads. A name nothing provides makes the next load reject.
   */
  async setDefaultFontFamily(family: string): Promise<void> {
    if (typeof family !== 'string' || family.trim() === '') {
      throw new TypeError(
        'BobcatCanvas.setDefaultFontFamily requires a non-empty family name',
      )
    }
    await this.#request('setDefaultFontFamily', {
      family: family.trim(),
    })
  }

  async resize(
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<void> {
    validateMetrics(width, height, devicePixelRatio)
    // Messages from one Window reach the Worker in order: updating the input
    // map before posting resize means a following pointer is expressed in the
    // viewport the Worker will install before it dispatches that pointer.
    this.#pointerInput.resize(width, height)
    await this.#request('resize', { devicePixelRatio, height, width })
  }

  /** Releases pointer capture/listeners and terminates the Render Worker. */
  async dispose(): Promise<void> {
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
      this.#decoder.close()
    }
  }
}
