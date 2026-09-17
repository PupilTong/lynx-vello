import assert from 'node:assert/strict'
import test from 'node:test'

interface Bounds {
  height: number
  left: number
  top: number
  width: number
}

interface FakePointerEvent {
  button: number
  clientX: number
  clientY: number
  defaultPrevented: boolean
  pointerId: number
  pointerType: string
}

interface FakeWheelEvent {
  clientX: number
  clientY: number
  ctrlKey: boolean
  defaultPrevented: boolean
  deltaMode: number
  deltaX: number
  deltaY: number
  preventDefault: () => void
}

type FakeEvent = FakePointerEvent | FakeWheelEvent
type FakeListener = (event: FakeEvent) => void

// The fields the assertions read from a message the facade posted.
interface PostedMessage {
  type: string
  bytes?: Uint8Array
  config?: unknown
  defaultPrevented?: boolean
  deltaX?: number
  deltaY?: number
  device?: number
  operation?: string
  phase?: number
  pointerId?: number
  request?: number
  url?: string
  x?: number
  y?: number
}

type FakeMessageListener = (event: { data: unknown }) => void

class FakeCanvas {
  #captured = new Set<number>()
  #listeners = new Map<string, Set<FakeListener>>()
  declare bounds: Bounds
  declare listenerOptions: Map<string, AddEventListenerOptions | undefined>
  declare released: number[]
  declare style: { touchAction: string }

  constructor(bounds: Bounds) {
    this.bounds = bounds
    this.listenerOptions = new Map()
    this.released = []
    this.style = { touchAction: 'pan-y' }
  }

  addEventListener(
    name: string,
    listener: FakeListener,
    options?: AddEventListenerOptions,
  ): void {
    let listeners = this.#listeners.get(name)
    if (listeners === undefined) {
      listeners = new Set()
      this.#listeners.set(name, listeners)
    }
    listeners.add(listener)
    this.listenerOptions.set(name, options)
  }

  removeEventListener(name: string, listener: FakeListener): void {
    this.#listeners.get(name)?.delete(listener)
  }

  emit(name: string, values: Partial<FakePointerEvent> = {}): void {
    const event = {
      button: 0,
      clientX: 0,
      clientY: 0,
      defaultPrevented: false,
      pointerId: 1,
      pointerType: 'mouse',
      ...values,
    }
    for (const listener of this.#listeners.get(name) ?? []) {
      listener(event)
    }
  }

  /** Dispatches one `wheel` and reports whether it was prevented. */
  emitWheel(values: Partial<FakeWheelEvent> = {}): boolean {
    let prevented = false
    const event: FakeWheelEvent = {
      clientX: 0,
      clientY: 0,
      ctrlKey: false,
      defaultPrevented: false,
      deltaMode: 0,
      deltaX: 0,
      deltaY: 0,
      preventDefault: () => {
        prevented = true
      },
      ...values,
    }
    for (const listener of this.#listeners.get('wheel') ?? []) {
      listener(event)
    }
    return prevented
  }

  getBoundingClientRect(): Bounds {
    return this.bounds
  }

  hasPointerCapture(pointerId: number): boolean {
    return this.#captured.has(pointerId)
  }

  listenerCount(): number {
    return [...this.#listeners.values()].reduce(
      (count, listeners) => count + listeners.size,
      0,
    )
  }

  releasePointerCapture(pointerId: number): void {
    this.#captured.delete(pointerId)
    this.released.push(pointerId)
  }

  setPointerCapture(pointerId: number): void {
    this.#captured.add(pointerId)
  }

  transferControlToOffscreen(): { fake: string } {
    return { fake: 'offscreen-canvas' }
  }
}

class FakeWorker {
  static instances: FakeWorker[] = []

  #listeners = new Map<string, Set<FakeMessageListener>>()
  declare messages: PostedMessage[]
  declare options: WorkerOptions | undefined
  declare terminated: boolean
  declare url: string | URL

  constructor(url: string | URL, options?: WorkerOptions) {
    this.messages = []
    this.options = options
    this.terminated = false
    this.url = url
    FakeWorker.instances.push(this)
  }

  addEventListener(name: string, listener: FakeMessageListener): void {
    let listeners = this.#listeners.get(name)
    if (listeners === undefined) {
      listeners = new Set()
      this.#listeners.set(name, listeners)
    }
    listeners.add(listener)
  }

  removeEventListener(name: string, listener: FakeMessageListener): void {
    this.#listeners.get(name)?.delete(listener)
  }

  postMessage(message: PostedMessage): void {
    this.messages.push(message)
    if (message.type === 'bobcat-init') {
      queueMicrotask(() => this.#emit('message', { type: 'bobcat-ready' }))
    } else if (message.type === 'bobcat-request') {
      queueMicrotask(() =>
        this.#emit('message', {
          ok: true,
          request: message.request,
          type: 'bobcat-response',
        }),
      )
    }
  }

  terminate(): void {
    this.terminated = true
  }

  #emit(name: string, data: unknown): void {
    for (const listener of this.#listeners.get(name) ?? []) {
      listener({ data })
    }
  }
}

const replacedGlobals = new Map<string, PropertyDescriptor | undefined>()

function replaceGlobal(name: string, value: unknown): void {
  replacedGlobals.set(name, Object.getOwnPropertyDescriptor(globalThis, name))
  Object.defineProperty(globalThis, name, {
    configurable: true,
    value,
    writable: true,
  })
}

replaceGlobal('HTMLCanvasElement', FakeCanvas)
replaceGlobal('Worker', FakeWorker)
replaceGlobal('crossOriginIsolated', true)
replaceGlobal('document', { baseURI: 'https://example.test/app/' })

const { BobcatCanvas, LYNX_XML_PAGE_CONFIG } = await import('../js/facade.ts')

test.after(() => {
  for (const [name, descriptor] of replacedGlobals) {
    if (descriptor === undefined) {
      delete (globalThis as Record<string, unknown>)[name]
    } else {
      Object.defineProperty(globalThis, name, descriptor)
    }
  }
})

test('forwards captured pointer sequences in viewport CSS pixels', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({
    height: 100,
    left: 10,
    top: 20,
    width: 200,
  })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    400,
    200,
    2,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!

  assert.equal(canvas.style.touchAction, 'none')
  canvas.emit('pointerdown', {
    clientX: 110,
    clientY: 45,
    defaultPrevented: true,
    pointerId: 7,
    pointerType: 'touch',
  })
  canvas.emit('pointermove', {
    clientX: 210,
    clientY: 120,
    pointerId: 7,
    pointerType: 'touch',
  })
  canvas.emit('pointerup', {
    clientX: 230,
    clientY: 120,
    pointerId: 7,
    pointerType: 'touch',
  })

  assert.deepEqual(
    worker.messages.filter(({ type }) => type === 'bobcat-pointer'),
    [
      {
        defaultPrevented: true,
        device: 1,
        phase: 0,
        pointerId: 7,
        type: 'bobcat-pointer',
        x: 200,
        y: 50,
      },
      {
        defaultPrevented: false,
        device: 1,
        phase: 1,
        pointerId: 7,
        type: 'bobcat-pointer',
        x: 400,
        y: 200,
      },
      {
        defaultPrevented: false,
        device: 1,
        phase: 2,
        pointerId: 7,
        type: 'bobcat-pointer',
        x: 440,
        y: 200,
      },
    ],
  )
  assert.deepEqual(canvas.released, [7])

  const messageCount = worker.messages.length
  canvas.emit('pointermove', { pointerId: 7, pointerType: 'touch' })
  assert.equal(worker.messages.length, messageCount, 'hover moves stay local')

  await view.dispose()
  assert.equal(canvas.listenerCount(), 0)
  assert.equal(canvas.style.touchAction, 'pan-y')
  assert.equal(worker.terminated, true)
})

test('forwards wheel deltas in viewport CSS pixels and prevents the page scroll', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({
    height: 100,
    left: 10,
    top: 20,
    width: 200,
  })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    400,
    200,
    2,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!
  const wheelMessages = () =>
    worker.messages.filter(({ type }) => type === 'bobcat-wheel')

  // Not passive: the listener prevents every wheel it forwards.
  assert.deepEqual(canvas.listenerOptions.get('wheel'), { passive: false })

  // Pixel mode: page CSS px, scaled by the canvas box like the position is.
  assert.equal(
    canvas.emitWheel({ clientX: 110, clientY: 45, deltaX: 3, deltaY: 10 }),
    true,
  )
  // Line mode is this embedder's 40 CSS px per line, with no box scaling.
  canvas.emitWheel({ clientX: 10, clientY: 20, deltaMode: 1, deltaY: 2 })
  // Page mode is the viewport itself.
  canvas.emitWheel({
    clientX: 10,
    clientY: 20,
    deltaMode: 2,
    deltaX: 0.5,
    deltaY: 1,
  })
  // What another handler already claimed crosses as claimed.
  canvas.emitWheel({
    clientX: 10,
    clientY: 20,
    defaultPrevented: true,
    deltaY: 1,
  })
  // A wheel that moves nothing is not an input.
  canvas.emitWheel({ clientX: 10, clientY: 20 })

  assert.deepEqual(wheelMessages(), [
    {
      defaultPrevented: false,
      deltaX: 6,
      deltaY: 20,
      type: 'bobcat-wheel',
      x: 200,
      y: 50,
    },
    {
      defaultPrevented: false,
      deltaX: 0,
      deltaY: 80,
      type: 'bobcat-wheel',
      x: 0,
      y: 0,
    },
    {
      defaultPrevented: false,
      deltaX: 200,
      deltaY: 200,
      type: 'bobcat-wheel',
      x: 0,
      y: 0,
    },
    {
      defaultPrevented: true,
      deltaX: 0,
      deltaY: 2,
      type: 'bobcat-wheel',
      x: 0,
      y: 0,
    },
  ])

  await view.dispose()
})

test('leaves a ctrl-held wheel to the browser, and every wheel after disposal', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 100, left: 0, top: 0, width: 100 })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    100,
    100,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!
  const wheelMessages = () =>
    worker.messages.filter(({ type }) => type === 'bobcat-wheel')

  // The browser's zoom gesture stays the page's, unforwarded and unprevented.
  assert.equal(
    canvas.emitWheel({ clientX: 5, clientY: 5, ctrlKey: true, deltaY: 4 }),
    false,
  )
  assert.deepEqual(wheelMessages(), [])

  await view.dispose()
  assert.equal(canvas.emitWheel({ clientX: 5, clientY: 5, deltaY: 4 }), false)
  assert.deepEqual(wheelMessages(), [])
  assert.equal(canvas.listenerCount(), 0)
})

test('ignores secondary mouse buttons and cancels lost capture', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 100, left: 0, top: 0, width: 100 })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    100,
    100,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!

  canvas.emit('pointerdown', {
    button: 2,
    clientX: 10,
    clientY: 10,
    pointerId: 4,
  })
  canvas.emit('pointerdown', {
    clientX: 20,
    clientY: 30,
    pointerId: 9,
    pointerType: 'pen',
  })
  canvas.emit('lostpointercapture', {
    clientX: Number.NaN,
    clientY: Number.NaN,
    pointerId: 9,
    pointerType: 'pen',
  })

  assert.deepEqual(
    worker.messages
      .filter(({ type }) => type === 'bobcat-pointer')
      .map(({ device, phase, pointerId, x, y }) => ({
        device,
        phase,
        pointerId,
        x,
        y,
      })),
    [
      { device: 2, phase: 0, pointerId: 9, x: 20, y: 30 },
      { device: 2, phase: 3, pointerId: 9, x: 20, y: 30 },
    ],
  )

  await view.dispose()
})

test('a load releases active pointers before replacing the native view', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 100, left: 0, top: 0, width: 100 })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    100,
    100,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!

  canvas.emit('pointerdown', {
    clientX: 50,
    clientY: 50,
    pointerId: 12,
  })
  await view.load('https://example.test/main.js')
  const pointerMessages = () =>
    worker.messages.filter(({ type }) => type === 'bobcat-pointer')
  // The sequence is ended, not dropped: a load that fails leaves the previous
  // page running, and a pointer it never saw released would wedge its router.
  assert.equal(pointerMessages().at(-1)!.phase, 3)
  const beforeUp = pointerMessages().length
  canvas.emit('pointerup', {
    clientX: 50,
    clientY: 50,
    pointerId: 12,
  })

  assert.deepEqual(canvas.released, [12])
  assert.equal(
    pointerMessages().length,
    beforeUp,
  )

  await view.dispose()
})

test('a Lynx XML load preserves the host-selected page configuration', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 60, left: 0, top: 0, width: 80 })
  const hostConfig = {
    defaultDisplayLinear: true,
    defaultOverflowVisible: true,
    enableCSSSelector: false,
  }
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    80,
    60,
    1,
    hostConfig,
  )
  const worker = FakeWorker.instances[0]!

  await view.loadLynxXml('../card.lynx.xml')

  const init = worker.messages.find(({ type }) => type === 'bobcat-init')!
  const load = worker.messages.find(
    ({ operation }) => operation === 'loadLynxXml',
  )!
  assert.deepEqual(init.config, hostConfig)
  assert.deepEqual(
    { operation: load.operation, url: load.url },
    {
      operation: 'loadLynxXml',
      url: 'https://example.test/card.lynx.xml',
    },
  )
  assert.equal(Object.hasOwn(load, 'config'), false)

  await view.dispose()
})

test('a template load resolves the URL and releases active pointer capture', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 60, left: 0, top: 0, width: 80 })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    80,
    60,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!
  canvas.emit('pointerdown', { clientX: 20, clientY: 20, pointerId: 7 })
  await view.loadTemplate('../archive/main.web.bundle')
  const request = worker.messages.find(({ operation }) => operation === 'loadTemplate')!
  assert.equal(request.url, 'https://example.test/archive/main.web.bundle')
  assert.equal(worker.messages.filter(({ type }) => type === 'bobcat-pointer').at(-1)!.phase, 3)
  assert.deepEqual(canvas.released, [7])
  await view.dispose()
})


test('a ZIP load forwards owned bytes and the archive URL without document resolution', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 60, left: 0, top: 0, width: 80 })
  const view = await BobcatCanvas.create(
    canvas as unknown as HTMLCanvasElement,
    80,
    60,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]!
  const bytes = new Uint8Array([80, 75, 3, 4])
  canvas.emit('pointerdown', { clientX: 20, clientY: 20, pointerId: 7 })
  await view.loadZip(bytes, 'bobcat-memory://archive/dist/main.web.bundle')
  bytes.fill(0)
  const request = worker.messages.find(({ operation }) => operation === 'loadZip')!
  assert.equal(request.url, 'bobcat-memory://archive/dist/main.web.bundle')
  assert.deepEqual(Array.from(request.bytes!), [80, 75, 3, 4])
  assert.deepEqual(canvas.released, [7])
  // A plain-JavaScript caller can pass anything; the facade rejects it.
  await assert.rejects(
    view.loadZip('bad' as unknown as Uint8Array, 'https://example.test/main'),
    TypeError,
  )
  await view.dispose()
})
