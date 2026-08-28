import assert from 'node:assert/strict'
import test from 'node:test'

class FakeCanvas {
  #captured = new Set()
  #listeners = new Map()

  constructor(bounds) {
    this.bounds = bounds
    this.released = []
    this.style = { touchAction: 'pan-y' }
  }

  addEventListener(name, listener) {
    let listeners = this.#listeners.get(name)
    if (listeners === undefined) {
      listeners = new Set()
      this.#listeners.set(name, listeners)
    }
    listeners.add(listener)
  }

  removeEventListener(name, listener) {
    this.#listeners.get(name)?.delete(listener)
  }

  emit(name, values = {}) {
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

  getBoundingClientRect() {
    return this.bounds
  }

  hasPointerCapture(pointerId) {
    return this.#captured.has(pointerId)
  }

  listenerCount() {
    return [...this.#listeners.values()].reduce(
      (count, listeners) => count + listeners.size,
      0,
    )
  }

  releasePointerCapture(pointerId) {
    this.#captured.delete(pointerId)
    this.released.push(pointerId)
  }

  setPointerCapture(pointerId) {
    this.#captured.add(pointerId)
  }

  transferControlToOffscreen() {
    return { fake: 'offscreen-canvas' }
  }
}

class FakeWorker {
  static instances = []

  #listeners = new Map()

  constructor(url, options) {
    this.messages = []
    this.options = options
    this.terminated = false
    this.url = url
    FakeWorker.instances.push(this)
  }

  addEventListener(name, listener) {
    let listeners = this.#listeners.get(name)
    if (listeners === undefined) {
      listeners = new Set()
      this.#listeners.set(name, listeners)
    }
    listeners.add(listener)
  }

  removeEventListener(name, listener) {
    this.#listeners.get(name)?.delete(listener)
  }

  postMessage(message) {
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

  terminate() {
    this.terminated = true
  }

  #emit(name, data) {
    for (const listener of this.#listeners.get(name) ?? []) {
      listener({ data })
    }
  }
}

const replacedGlobals = new Map()

function replaceGlobal(name, value) {
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

const { BobcatCanvas, LYNX_XML_PAGE_CONFIG } = await import('../facade.js')

test.after(() => {
  for (const [name, descriptor] of replacedGlobals) {
    if (descriptor === undefined) {
      delete globalThis[name]
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
    canvas,
    400,
    200,
    2,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]

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

test('ignores secondary mouse buttons and cancels lost capture', async () => {
  FakeWorker.instances.length = 0
  const canvas = new FakeCanvas({ height: 100, left: 0, top: 0, width: 100 })
  const view = await BobcatCanvas.create(
    canvas,
    100,
    100,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]

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
    canvas,
    100,
    100,
    1,
    LYNX_XML_PAGE_CONFIG,
  )
  const worker = FakeWorker.instances[0]

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
  assert.equal(pointerMessages().at(-1).phase, 3)
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
