import { ImageDecoder } from './image-decoder.ts'
import type {
  InitMessage,
  NativeModuleCallMessage,
  NativeModuleCallbackMessage,
  Operation,
  PointerDevice,
  PointerFields,
  PointerMessage,
  PointerPhase,
  RenderWorkerMessage,
  RequestFields,
  WheelFields,
  WheelMessage,
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
const WHEEL_DELTA_MODE_LINE = 1
const WHEEL_DELTA_MODE_PAGE = 2
// This embedder's policy for the browser's abstract line unit, matching the
// native reference host's `WHEEL_LINE_CSS_PX`. Core accepts CSS pixels only.
const WHEEL_LINE_CSS_PX = 40

let initialization: Promise<void> | undefined

export interface PageConfig {
  defaultDisplayLinear: boolean
  defaultOverflowVisible: boolean
  enableCSSSelector: boolean
}

/**
 * The host capabilities a page reaches as `NativeModules.<name>.<method>()`
 * from its background thread.
 *
 * Every handler runs here, on the page's main thread, because that is where
 * `localStorage`, navigation and the rest of the DOM are — and it runs after
 * the calling script has moved on, so there is nothing to return. A handler
 * that has an answer takes a function argument and calls it: the page passes
 * a function, the engine hands this side a single-shot wrapper in that
 * argument's place, and calling it delivers the answer to the realm.
 */
export type NativeModules = Record<
  string,
  Record<string, (...args: unknown[]) => void>
>

/** What a canvas keeps for every page it loads, beyond its metrics. */
export interface BobcatCanvasOptions {
  nativeModules?: NativeModules
}

/** What one load hands the page it builds. */
export interface LoadOptions {
  /**
   * `lynx.__globalProps` for this page: any JSON-serializable value, which
   * the engine carries as text and never reads.
   */
  globalProps?: unknown
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

/**
 * The screen `SystemInfo` reports, in physical pixels — web-core's own
 * algorithm: the available screen size multiplied by `devicePixelRatio`.
 *
 * This thread is the page's main thread, the only one `screen` exists on, so
 * it is measured here and sent to the Render Worker rather than read there.
 * Read once, like web-core's. A browser that answers for neither gets `NaN`s,
 * which the engine refuses in favour of each view's own metrics.
 */
function screenPixels(): { height: number; width: number } {
  const screen = globalThis.screen
  const ratio = globalThis.devicePixelRatio
  if (!screen || typeof ratio !== 'number') {
    return { height: Number.NaN, width: Number.NaN }
  }
  return {
    height: screen.availHeight * ratio,
    width: screen.availWidth * ratio,
  }
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

/**
 * Reads the host's modules into the name/method table the Worker declares to
 * the realm, rejecting anything that is not a function: the table says what
 * exists on the page's `NativeModules` object, so a member that could not be
 * called must not appear in it.
 */
function nativeModuleTable(
  modules: NativeModules | undefined,
): Record<string, string[]> {
  if (modules === undefined) {
    return {}
  }
  if (modules === null || typeof modules !== 'object') {
    throw new TypeError('BobcatCanvas.create nativeModules must be an object')
  }
  const table: Record<string, string[]> = {}
  for (const [name, module] of Object.entries(modules)) {
    if (module === null || typeof module !== 'object') {
      throw new TypeError(`Bobcat nativeModules.${name} must be an object`)
    }
    const methods: string[] = []
    for (const [method, handler] of Object.entries(module)) {
      if (typeof handler !== 'function') {
        throw new TypeError(
          `Bobcat nativeModules.${name}.${method} must be a function`,
        )
      }
      methods.push(method)
    }
    table[name] = methods
  }
  return table
}

/**
 * Serializes one load's `globalProps` into the text the engine carries.
 * `undefined` means the page gets none; anything JSON refuses is the caller's
 * mistake, reported before the load crosses the Worker boundary.
 */
function globalPropsText(value: unknown): string | undefined {
  if (value === undefined) {
    return undefined
  }
  let text: string | undefined
  try {
    text = JSON.stringify(value)
  } catch (error) {
    throw new TypeError(
      `Bobcat globalProps must be JSON-serializable: ${errorMessage(error)}`,
    )
  }
  if (text === undefined) {
    throw new TypeError('Bobcat globalProps must be JSON-serializable')
  }
  return text
}

/**
 * The `globalProps` field a load message carries, or no field at all: a page
 * given none and a page given `undefined` are the same page.
 */
function globalPropsFields(options: LoadOptions): { globalProps?: string } {
  if (options === null || typeof options !== 'object') {
    throw new TypeError('Bobcat load options must be an object')
  }
  const globalProps = globalPropsText(options.globalProps)
  return globalProps === undefined ? {} : { globalProps }
}

/**
 * Runs one `bobcat-native-module` call against the host's handlers.
 *
 * The arguments arrive as JSON array text with each function argument `null`;
 * every index the Worker named gets a wrapper that posts the page's answer
 * back, the first time it is called and only then. The handler's own return
 * value is nothing — the realm's method already answered `undefined` — so a
 * handler that throws is reported here rather than propagated: the call is the
 * page's, not the Worker's, and nothing upstream is waiting on it.
 */
export function deliverNativeModuleCall(
  modules: NativeModules,
  message: NativeModuleCallMessage,
  post: (answer: NativeModuleCallbackMessage) => void,
): void {
  const handler = modules[message.module]?.[message.method]
  if (typeof handler !== 'function') {
    return
  }
  let args: unknown[]
  try {
    const parsed: unknown = JSON.parse(message.args)
    args = Array.isArray(parsed) ? parsed : []
  } catch (error) {
    console.error(
      `Bobcat could not read the arguments of NativeModules.${message.module}.${message.method}`,
      error,
    )
    return
  }
  for (const index of message.callbacks) {
    let answered = false
    args[index] = (...answer: unknown[]): void => {
      if (answered) {
        return
      }
      answered = true
      post({
        args: JSON.stringify(answer),
        call: message.call,
        index,
        type: 'bobcat-native-module-callback',
      })
    }
  }
  try {
    handler(...args)
  } catch (error) {
    console.error(
      `NativeModules.${message.module}.${message.method} threw`,
      error,
    )
  }
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

/**
 * Where a client point lands in viewport CSS px, and the scale that carries a
 * client-space length there with it.
 */
interface ViewportMapping {
  scaleX: number
  scaleY: number
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
  #sendWheel: (values: WheelFields) => void
  #width: number

  constructor(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    send: (values: PointerFields) => void,
    sendWheel: (values: WheelFields) => void,
  ) {
    this.#canvas = canvas
    this.#height = height
    this.#previousTouchAction = canvas.style.touchAction
    this.#send = send
    this.#sendWheel = sendWheel
    this.#width = width

    // Transferring drawing control does not transfer the canvas's DOM events.
    // The embedder owns touch panning so the engine can arbitrate tap vs scroll.
    canvas.style.touchAction = 'none'
    canvas.addEventListener('pointerdown', this.#onPointerDown)
    canvas.addEventListener('pointermove', this.#onPointerMove)
    canvas.addEventListener('pointerup', this.#onPointerUp)
    canvas.addEventListener('pointercancel', this.#onPointerCancel)
    canvas.addEventListener('lostpointercapture', this.#onLostPointerCapture)
    // Not passive: a forwarded wheel is prevented, so the listener has to be
    // allowed to prevent it.
    canvas.addEventListener('wheel', this.#onWheel, { passive: false })
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
    this.#canvas.removeEventListener('wheel', this.#onWheel)
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

  #onWheel = (event: WheelEvent) => {
    if (this.#disposed) {
      return
    }
    // A ctrl-held wheel is the browser's own zoom gesture. It stays the page's:
    // nothing is forwarded, and nothing is prevented.
    if (event.ctrlKey) {
      return
    }
    // A wheel has no active sequence to fall back on, so a canvas box that
    // cannot be mapped drops the event.
    const mapped = this.#mapClient(event.clientX, event.clientY)
    if (mapped === undefined) {
      return
    }
    let deltaX: number
    let deltaY: number
    switch (event.deltaMode) {
      case WHEEL_DELTA_MODE_LINE:
        deltaX = event.deltaX * WHEEL_LINE_CSS_PX
        deltaY = event.deltaY * WHEEL_LINE_CSS_PX
        break
      case WHEEL_DELTA_MODE_PAGE:
        deltaX = event.deltaX * this.#width
        deltaY = event.deltaY * this.#height
        break
      default:
        // Pixel mode: page CSS px, which the canvas's own box carries into the
        // viewport exactly as it carries a position.
        deltaX = event.deltaX * mapped.scaleX
        deltaY = event.deltaY * mapped.scaleY
        break
    }
    if (!Number.isFinite(deltaX) || !Number.isFinite(deltaY)) {
      return
    }
    if (deltaX === 0 && deltaY === 0) {
      return
    }
    // The browser's positive delta already means "scroll offset increases",
    // which is the engine's convention too, so no sign is flipped here.
    const defaultPrevented = event.defaultPrevented === true
    // Read above, before this call: the Worker answers nothing, so the facade
    // cannot learn synchronously whether the engine consumed the scroll. The
    // canvas owns wheel scrolling outright, the way `touch-action: none`
    // makes it own touch panning.
    event.preventDefault()
    this.#sendWheel({
      defaultPrevented,
      deltaX,
      deltaY,
      x: mapped.x,
      y: mapped.y,
    })
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
    const mapped = this.#mapClient(event.clientX, event.clientY)
    const position = mapped ?? fallback
    if (position === undefined) {
      return undefined
    }
    return {
      defaultPrevented: event.defaultPrevented === true,
      device,
      phase,
      pointerId: event.pointerId,
      x: position.x,
      y: position.y,
    }
  }

  /**
   * Maps a client point through the canvas bounds into viewport CSS px, and
   * reports the scale that mapping applies. Nothing when the element has no
   * finite, non-empty box, or when the result is not finite.
   */
  #mapClient(clientX: number, clientY: number): ViewportMapping | undefined {
    const bounds = this.#canvas.getBoundingClientRect()
    if (
      !Number.isFinite(clientX) ||
      !Number.isFinite(clientY) ||
      !Number.isFinite(bounds.left) ||
      !Number.isFinite(bounds.top) ||
      !Number.isFinite(bounds.width) ||
      !Number.isFinite(bounds.height) ||
      bounds.width <= 0 ||
      bounds.height <= 0
    ) {
      return undefined
    }
    const scaleX = this.#width / bounds.width
    const scaleY = this.#height / bounds.height
    const x = (clientX - bounds.left) * scaleX
    const y = (clientY - bounds.top) * scaleY
    if (!Number.isFinite(x) || !Number.isFinite(y)) {
      return undefined
    }
    return { scaleX, scaleY, x, y }
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
  #modules: NativeModules
  #nextRequest = 1
  #pending = new Map<number, PendingRequest>()
  #ready: Promise<void>
  #rejectReady!: (error: Error) => void
  #resolveReady!: () => void
  #readySettled = false
  #worker: Worker

  constructor(worker: Worker, modules: NativeModules) {
    this.#modules = modules
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
    if (message?.type === 'bobcat-native-module') {
      // The handlers are the page's; a failure of theirs is theirs to hear
      // about, so nothing here touches the fatal path.
      deliverNativeModuleCall(this.#modules, message, (answer) => {
        if (this.#fatalError !== undefined) {
          return
        }
        try {
          this.#worker.postMessage(answer)
        } catch (error) {
          console.error('Bobcat could not deliver a native module answer', error)
        }
      })
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

  dispatchWheel(values: WheelFields): void {
    if (this.#fatalError !== undefined) {
      return
    }
    try {
      this.#worker.postMessage({
        type: 'bobcat-wheel',
        ...values,
      } satisfies WheelMessage)
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
 * A Worker-owned Bobcat view attached to one HTML canvas. Active
 * `pointerdown`/`pointermove`/`pointerup`/`pointercancel` sequences on the
 * canvas, and its `wheel` events, are captured and forwarded to the native
 * input router automatically.
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
      (values) => client.dispatchWheel(values),
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
    options: BobcatCanvasOptions = {},
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
    if (options === null || typeof options !== 'object') {
      throw new TypeError('BobcatCanvas.create options must be an object')
    }
    // Checked before the Worker exists: a module table the page cannot
    // answer is the caller's mistake, not a runtime failure of the canvas.
    const modules = options.nativeModules ?? {}
    const nativeModules = nativeModuleTable(options.nativeModules)
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

    const screen = screenPixels()

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
      client = new RenderWorkerClient(worker, modules)
      worker.postMessage(
        {
          type: 'bobcat-init',
          canvas: offscreen,
          config,
          devicePixelRatio,
          height,
          imagePort: images.port2,
          hardwareConcurrency: hardwareConcurrency(),
          nativeModules,
          screenPixelHeight: screen.height,
          screenPixelWidth: screen.width,
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
   *
   * `options.globalProps` becomes the page's `lynx.__globalProps`.
   */
  async load(
    url: string | URL,
    styleSheetUrls: readonly (string | URL)[] = [],
    options: LoadOptions = {},
  ): Promise<void> {
    if (!Array.isArray(styleSheetUrls)) {
      throw new TypeError('BobcatCanvas.load styleSheetUrls must be an array')
    }
    this.#pointerInput.reset()
    await this.#request('load', {
      ...globalPropsFields(options),
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
  async loadLynxXml(url: string | URL, options: LoadOptions = {}): Promise<void> {
    this.#pointerInput.reset()
    await this.#request('loadLynxXml', {
      ...globalPropsFields(options),
      url: documentUrl(url),
    })
  }

  /**
   * Fetch, decode, and show a binary web or source-based native bundle (root
   * entry). Uses the container's page configuration and resolves relative
   * resources against its response URL. Native bytecode is rejected by the
   * shared parser.
   */
  async loadTemplate(url: string | URL, options: LoadOptions = {}): Promise<void> {
    this.#pointerInput.reset()
    await this.#request('loadTemplate', {
      ...globalPropsFields(options),
      url: documentUrl(url),
    })
  }

  /**
   * Decode local ZIP bytes with bobcat-source and load the selected entry.
   * `entryUrl` must be absolute: its decoded pathname selects the member and
   * its origin maps the ZIP resources. XML is strict UTF-8; binary bundles
   * require root. The shared loader limits input to 64 MiB, output to 128 MiB,
   * and 4096 entries.
   */
  async loadZip(
    data: ArrayBuffer | Uint8Array,
    entryUrl: string | URL,
    options: LoadOptions = {},
  ): Promise<void> {
    if (!(data instanceof ArrayBuffer) && !(data instanceof Uint8Array)) {
      throw new TypeError('BobcatCanvas.loadZip requires an ArrayBuffer or Uint8Array')
    }
    if (data.byteLength > 64 * 1024 * 1024) {
      throw new RangeError('ZIP exceeds the 64 MiB compressed size limit')
    }
    this.#pointerInput.reset()
    await this.#request('loadZip', {
      ...globalPropsFields(options),
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
