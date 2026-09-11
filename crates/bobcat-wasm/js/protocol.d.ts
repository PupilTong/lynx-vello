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
  workerUrl: string
  width: number
}

/** Pointer input. The Worker answers nothing. */
export interface PointerMessage extends PointerFields {
  type: 'bobcat-pointer'
}

/** What each operation carries besides `type`, `operation` and `request`. */
export interface RequestFields {
  load: {
    styleSheetUrls: string[]
    url: string
  }
  loadLynxXml: {
    url: string
  }
  loadTemplate: {
    url: string
  }
  loadZip: {
    bytes: Uint8Array
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

export type FacadeMessage = InitMessage | PointerMessage | RequestMessage

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

export type RenderWorkerMessage = ReadyMessage | ErrorMessage | ResponseMessage
