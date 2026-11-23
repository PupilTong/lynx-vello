import assert from 'node:assert/strict'
import test, { type TestContext } from 'node:test'

import type { ExplorerRequest } from '../js/facade.ts'
import type { ExplorerMessage } from '../js/protocol.d.ts'

// The fields these assertions read from a message the facade posted.
interface PostedMessage {
  type: string
  config?: unknown
  request?: number
}

type FakeMessageListener = (event: { data: unknown }) => void

class FakeCanvas {
  declare style: { touchAction: string }

  constructor() {
    this.style = { touchAction: '' }
  }

  addEventListener(): void {}
  removeEventListener(): void {}

  getBoundingClientRect(): {
    height: number
    left: number
    top: number
    width: number
  } {
    return { height: 100, left: 0, top: 0, width: 100 }
  }

  hasPointerCapture(): boolean {
    return false
  }

  releasePointerCapture(): void {}
  setPointerCapture(): void {}

  transferControlToOffscreen(): { fake: string } {
    return { fake: 'offscreen-canvas' }
  }
}

class FakeWorker {
  static instances: FakeWorker[] = []

  #listeners = new Map<string, Set<FakeMessageListener>>()
  declare messages: PostedMessage[]
  declare terminated: boolean

  constructor() {
    this.messages = []
    this.terminated = false
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
      queueMicrotask(() => this.emit({ type: 'bobcat-ready' }))
    } else if (message.type === 'bobcat-request') {
      queueMicrotask(() =>
        this.emit({
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

  emit(data: unknown): void {
    for (const listener of this.#listeners.get('message') ?? []) {
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

async function createCanvas(
  t: TestContext,
  onExplorerRequest?: (request: ExplorerRequest) => void,
): Promise<{
  canvas: Awaited<ReturnType<typeof BobcatCanvas.create>>
  worker: FakeWorker
}> {
  FakeWorker.instances.length = 0
  const canvas = await BobcatCanvas.create(
    new FakeCanvas() as unknown as HTMLCanvasElement,
    100,
    100,
    1,
    LYNX_XML_PAGE_CONFIG,
    onExplorerRequest === undefined ? {} : { onExplorerRequest },
  )
  t.after(() => canvas.dispose())
  return { canvas, worker: FakeWorker.instances[0]! }
}

/** Runs `body` with `console[level]` recording its calls instead. */
function recordConsole(
  level: 'error' | 'warn',
  body: () => void,
): unknown[][] {
  const calls: unknown[][] = []
  const original = console[level]
  console[level] = (...values: unknown[]): void => {
    calls.push(values)
  }
  try {
    body()
  } finally {
    console[level] = original
  }
  return calls
}

test('each Explorer message reaches the page as its request', async (t) => {
  const requests: ExplorerRequest[] = []
  const { canvas, worker } = await createCanvas(t, (request) => {
    requests.push(request)
  })

  worker.emit({
    schema: 'file://lynx?local://showcase/menu/main.lynx.bundle',
    type: 'bobcat-explorer-open-schema',
  } satisfies ExplorerMessage)
  worker.emit({
    key: 'preferredTheme',
    type: 'bobcat-explorer-store',
    value: 'Dark',
  } satisfies ExplorerMessage)
  worker.emit({
    notice: 'noScanner',
    type: 'bobcat-explorer-notice',
  } satisfies ExplorerMessage)
  worker.emit({
    notice: 'emptySchema',
    type: 'bobcat-explorer-notice',
  } satisfies ExplorerMessage)

  assert.deepEqual(requests, [
    {
      kind: 'openSchema',
      schema: 'file://lynx?local://showcase/menu/main.lynx.bundle',
    },
    { key: 'preferredTheme', kind: 'store', value: 'Dark' },
    { kind: 'notice', notice: 'noScanner' },
    { kind: 'notice', notice: 'emptySchema' },
  ])

  await canvas.dispose()
})

test('an Explorer message whose fields are not the protocol\'s is dropped with a warning', async (t) => {
  const requests: ExplorerRequest[] = []
  const { canvas, worker } = await createCanvas(t, (request) => {
    requests.push(request)
  })

  const warnings = recordConsole('warn', () => {
    worker.emit({ schema: 7, type: 'bobcat-explorer-open-schema' })
    worker.emit({ key: 'theme', type: 'bobcat-explorer-store' })
    worker.emit({ notice: 'camera', type: 'bobcat-explorer-notice' })
  })

  assert.deepEqual(requests, [])
  assert.equal(warnings.length, 3)
  for (const warning of warnings) {
    assert.match(String(warning[0]), /unreadable Explorer message/u)
  }

  await canvas.dispose()
})

test('a request callback that throws is reported and the canvas goes on', async (t) => {
  let calls = 0
  const { canvas, worker } = await createCanvas(t, () => {
    calls += 1
    throw new Error('page failed')
  })

  const errors = recordConsole('error', () => {
    worker.emit({ schema: 'a.web.bundle', type: 'bobcat-explorer-open-schema' })
    worker.emit({ schema: 'b.web.bundle', type: 'bobcat-explorer-open-schema' })
  })

  assert.equal(calls, 2)
  assert.equal(errors.length, 2)
  assert.match(String(errors[0]![0]), /Explorer request callback threw/u)
  assert.equal(canvas.error, undefined)
  assert.equal(worker.terminated, false)

  await canvas.dispose()
})

test('a canvas without the callback drops Explorer messages', async (t) => {
  const { canvas, worker } = await createCanvas(t)
  const warnings = recordConsole('warn', () => {
    worker.emit({ schema: 'a.web.bundle', type: 'bobcat-explorer-open-schema' })
  })
  assert.deepEqual(warnings, [])
  assert.equal(canvas.error, undefined)
  await canvas.dispose()
})

test('the init message carries no native module table', async (t) => {
  const { canvas, worker } = await createCanvas(t, () => {})
  const init = worker.messages.find(({ type }) => type === 'bobcat-init')!
  assert.equal(Object.hasOwn(init, 'nativeModules'), false)
  await canvas.dispose()
})

test('a load carries globalProps as JSON text, and refuses what JSON cannot take', async (t) => {
  const { canvas, worker } = await createCanvas(t)

  await canvas.loadTemplate('card.web.bundle', {
    globalProps: { screenWidth: 390, theme: 'Dark' },
  })
  await canvas.loadTemplate('plain.web.bundle')

  assert.deepEqual(
    worker.messages
      .filter(({ type }) => type === 'bobcat-request')
      .map((message) => ({
        globalProps: (message as { globalProps?: string }).globalProps,
        hasField: Object.hasOwn(message, 'globalProps'),
      })),
    [
      { globalProps: '{"screenWidth":390,"theme":"Dark"}', hasField: true },
      { globalProps: undefined, hasField: false },
    ],
  )

  const cycle: Record<string, unknown> = {}
  cycle['self'] = cycle
  await assert.rejects(
    canvas.loadTemplate('card.web.bundle', { globalProps: cycle }),
    TypeError,
  )

  await canvas.dispose()
})
