// The messages the facade (`facade.ts`, on the page's main thread) and the
// Render Worker (`render-worker.ts`) post to each other. Both programs check
// against this one declaration. It is types only: where a message arrives,
// the event's data is cast to the sender's message type.

/**
 * The page configuration the facade validated. The Worker passes its fields
 * to `BobcatRenderer.create`.
 */
export interface PageConfigFields {
  defaultDisplayLinear: boolean
  defaultOverflowVisible: boolean
  enableCSSSelector: boolean
}

/** `0` mouse, `1` touch, `2` pen. */
export type PointerDevice = 0 | 1 | 2

/** `0` down, `1` move, `2` up, `3` cancel. */
export type PointerPhase = 0 | 1 | 2 | 3

/** One pointer event, in viewport CSS px. */
export interface PointerFields {
  defaultPrevented: boolean
  device: PointerDevice
  phase: PointerPhase
  pointerId: number
  x: number
  y: number
}

/**
 * One wheel event: position and scroll delta both in viewport CSS px. The
 * facade converts the browser's line and page units, so nothing downstream
 * knows what `deltaMode` the event carried.
 */
export interface WheelFields {
  defaultPrevented: boolean
  deltaX: number
  deltaY: number
  x: number
  y: number
}

// Facade → Render Worker.

/**
 * The first message. It transfers the canvas and the Worker's end of the
 * image decoder's channel.
 */
export interface InitMessage {
  type: 'bobcat-init'
  canvas: OffscreenCanvas
  config: PageConfigFields
  devicePixelRatio: number
  height: number
  imagePort: MessagePort
  hardwareConcurrency: number
  /**
   * The host's `NativeModules` surface, by module name: the methods it
   * declared, in declaration order. Only the names cross — the handlers
   * themselves stay on the page's main thread, where `localStorage` and
   * navigation are — and they are retained for every page the canvas loads,
   * like the fonts.
   */
  nativeModules: Record<string, string[]>
  measureFrameRate: boolean
  /**
   * The screen `SystemInfo` reports, in physical pixels: `screen.availWidth`
   * and `screen.availHeight` multiplied by `devicePixelRatio`, which is
   * web-core's own algorithm. Measured on the page's main thread, the one
   * thread `screen` exists on, and `NaN` where it does not — the engine then
   * derives each view's numbers from its own metrics instead.
   */
  screenPixelWidth: number
  screenPixelHeight: number
  workerUrl: string
  width: number
}

/**
 * The facade's answer to one function argument of one native-module call.
 *
 * Single-shot: the wrapper the facade put in that argument slot posts this
 * the first time the page's handler calls it, and does nothing afterwards.
 */
export interface NativeModuleCallbackMessage {
  type: 'bobcat-native-module-callback'
  /** The call number the Worker sent. */
  call: number
  /** Which argument of that call the answered function was. */
  index: number
  /** The answer's arguments as JSON array text, which the realm spreads. */
  args: string
}

/** Pointer input. The Worker answers nothing. */
export interface PointerMessage extends PointerFields {
  type: 'bobcat-pointer'
}

/** Wheel input. The Worker answers nothing. */
export interface WheelMessage extends WheelFields {
  type: 'bobcat-wheel'
}

/**
 * What each operation carries besides `type`, `operation` and `request`.
 *
 * Every load carries an optional `globalProps`: the JSON text the facade
 * serialized for `lynx.__globalProps`, which Rust hands to the page unread.
 */
export interface RequestFields {
  load: {
    globalProps?: string
    styleSheetUrls: string[]
    url: string
  }
  loadLynxXml: {
    globalProps?: string
    url: string
  }
  loadTemplate: {
    globalProps?: string
    url: string
  }
  loadZip: {
    bytes: Uint8Array
    globalProps?: string
    url: string
  }
  registerFonts: {
    bytes: Uint8Array
  }
  setDefaultFontFamily: {
    family: string
  }
  resize: {
    devicePixelRatio: number
    height: number
    width: number
  }
  dispose: Record<never, never>
}

export type Operation = keyof RequestFields

/**
 * One facade operation. The `bobcat-response` that carries the same `request`
 * number settles it.
 */
export type RequestMessage<O extends Operation = Operation> = {
  [P in O]: {
    type: 'bobcat-request'
    operation: P
    request: number
  } & RequestFields[P]
}[O]

export type FacadeMessage =
  | InitMessage
  | NativeModuleCallbackMessage
  | PointerMessage
  | RequestMessage
  | WheelMessage

// Render Worker → facade.

/** `BobcatRenderer.create` resolved. */
export interface ReadyMessage {
  type: 'bobcat-ready'
}

/**
 * A fatal failure. The facade rejects every pending operation and terminates
 * the Worker.
 */
export interface ErrorMessage {
  type: 'bobcat-error'
  message: string
}

/**
 * One `NativeModules.<module>.<method>(...)` the page's BTS realm made,
 * on its way to the handler the host gave `BobcatCanvas.create`.
 *
 * Nothing is awaited: the method answered `undefined` in the realm already.
 * `args` is the whole argument list as JSON array text, with each function
 * argument `null` and its index named in `callbacks`; the facade puts a
 * single-shot wrapper back in each of those slots before calling the handler.
 */
export interface NativeModuleCallMessage {
  type: 'bobcat-native-module'
  /** This call's number, which a callback answer quotes back. */
  call: number
  /** The `NativeModules` key. */
  module: string
  /** The method name, always one the host declared. */
  method: string
  /** The arguments as JSON array text. */
  args: string
  /** The argument indices that were functions. */
  callbacks: number[]
}

/** The outcome of the request with the same number. */
export type ResponseMessage =
  | {
      type: 'bobcat-response'
      ok: true
      request: number
    }
  | {
      type: 'bobcat-response'
      error: string
      ok: false
      request: number
    }

export type RenderWorkerMessage =
  | { type: 'bobcat-frame-rate'; fps: number | null }
  | ErrorMessage
  | NativeModuleCallMessage
  | ReadyMessage
  | ResponseMessage
