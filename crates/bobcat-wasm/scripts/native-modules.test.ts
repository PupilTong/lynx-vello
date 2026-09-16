import assert from 'node:assert/strict'
import test from 'node:test'

import type {
  NativeModuleCallMessage,
  NativeModuleCallbackMessage,
} from '../js/protocol.d.ts'

// The fields these assertions read from a message the facade posted.
interface PostedMessage {
  type: string
  args?: string
  call?: number
  config?: unknown
  index?: number
  nativeModules?: Record<string, string[]>
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

const { BobcatCanvas, LYNX_XML_PAGE_CONFIG, deliverNativeModuleCall } =
  await import('../js/facade.ts')

test.after(() => {
  for (const [name, descriptor] of replacedGlobals) {
    if (descriptor === undefined) {
      delete (globalThis as Record<string, unknown>)[name]
    } else {
      Object.defineProperty(globalThis, name, descriptor)
    }
  }
})

function call(
  values: Partial<NativeModuleCallMessage> = {},
): NativeModuleCallMessage {
  return {
    args: '[]',
    call: 1,
    callbacks: [],
    method: 'ping',
    module: 'Echo',
    type: 'bobcat-native-module',
    ...values,
  }
}

async function createCanvas(
  nativeModules?: Record<string, Record<string, (...args: unknown[]) => void>>,
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
    nativeModules === undefined ? {} : { nativeModules },
  )
  return { canvas, worker: FakeWorker.instances[0]! }
}

test('a call arrives parsed, with a function back in every callback slot', () => {
  const seen: unknown[][] = []
  const posted: NativeModuleCallbackMessage[] = []
  deliverNativeModuleCall(
    {
      Storage: {
        read(...args: unknown[]): void {
          seen.push(args)
          const answer = args[1]
          assert.equal(typeof answer, 'function')
          ;(answer as (...values: unknown[]) => void)('stored', 7)
        },
      },
    },
    call({
      args: '["session",null,{"retry":true}]',
      call: 12,
      callbacks: [1],
      method: 'read',
      module: 'Storage',
    }),
    (answer) => posted.push(answer),
  )

  assert.equal(seen.length, 1)
  assert.deepEqual(
    [seen[0]![0], seen[0]![2]],
    ['session', { retry: true }],
    'non-function arguments arrive as the page sent them',
  )
  assert.deepEqual(posted, [
    {
      args: '["stored",7]',
      call: 12,
      index: 1,
      type: 'bobcat-native-module-callback',
    },
  ])
})

test('a callback wrapper answers once, whatever the handler does with it', () => {
  const posted: NativeModuleCallbackMessage[] = []
  deliverNativeModuleCall(
    {
      Echo: {
        ping(...args: unknown[]): void {
          const answer = args[0] as (...values: unknown[]) => void
          answer('first')
          answer('second')
          answer()
        },
      },
    },
    call({ args: '[null]', call: 3, callbacks: [0] }),
    (answer) => posted.push(answer),
  )

  assert.deepEqual(posted, [
    {
      args: '["first"]',
      call: 3,
      index: 0,
      type: 'bobcat-native-module-callback',
    },
  ])
})

test('two callbacks keep their own argument indices and answer independently', () => {
  const posted: NativeModuleCallbackMessage[] = []
  deliverNativeModuleCall(
    {
      Files: {
        read(...args: unknown[]): void {
          ;(args[2] as (...values: unknown[]) => void)('failed')
          ;(args[1] as (...values: unknown[]) => void)('ok')
        },
      },
    },
    call({
      args: '["name",null,null]',
      call: 9,
      callbacks: [1, 2],
      method: 'read',
      module: 'Files',
    }),
    (answer) => posted.push(answer),
  )

  assert.deepEqual(
    posted.map(({ args, index }) => ({ args, index })),
    [
      { args: '["failed"]', index: 2 },
      { args: '["ok"]', index: 1 },
    ],
  )
})

test('a handler that throws is reported rather than propagated', () => {
  const errors: unknown[][] = []
  const consoleError = console.error
  console.error = (...values: unknown[]): void => {
    errors.push(values)
  }
  try {
    deliverNativeModuleCall(
      {
        Echo: {
          ping(): void {
            throw new Error('handler failed')
          },
        },
      },
      call(),
      () => assert.fail('a throwing handler answers nothing'),
    )
  } finally {
    console.error = consoleError
  }
  assert.equal(errors.length, 1)
  assert.match(String(errors[0]![0]), /NativeModules\.Echo\.ping threw/u)
})

test('an unknown module or method is ignored', () => {
  const post = (): void => assert.fail('nothing answers a call nobody took')
  deliverNativeModuleCall({}, call({ callbacks: [0] }), post)
  deliverNativeModuleCall(
    { Echo: {} },
    call({ callbacks: [0], method: 'absent' }),
    post,
  )
  deliverNativeModuleCall(
    { Echo: { ping: undefined as unknown as () => void } },
    call({ callbacks: [0] }),
    post,
  )
})

test('the canvas declares its module table and routes calls to the handlers', async () => {
  const opened: unknown[] = []
  const { canvas, worker } = await createCanvas({
    ExplorerModule: {
      openSchema(...args: unknown[]): void {
        opened.push(args[0])
      },
      openScan(): void {},
    },
  })

  const init = worker.messages.find(({ type }) => type === 'bobcat-init')!
  assert.deepEqual(init.nativeModules, {
    ExplorerModule: ['openSchema', 'openScan'],
  })

  worker.emit({
    args: '["https://example.test/card.web.bundle"]',
    call: 5,
    callbacks: [],
    method: 'openSchema',
    module: 'ExplorerModule',
    type: 'bobcat-native-module',
  } satisfies NativeModuleCallMessage)
  assert.deepEqual(opened, ['https://example.test/card.web.bundle'])

  await canvas.dispose()
})

test('a canvas answers a callback on the Worker channel', async () => {
  const { canvas, worker } = await createCanvas({
    Storage: {
      read(...args: unknown[]): void {
        ;(args[0] as (...values: unknown[]) => void)('value')
      },
    },
  })

  worker.emit({
    args: '[null]',
    call: 2,
    callbacks: [0],
    method: 'read',
    module: 'Storage',
    type: 'bobcat-native-module',
  } satisfies NativeModuleCallMessage)

  assert.deepEqual(
    worker.messages.filter(
      ({ type }) => type === 'bobcat-native-module-callback',
    ),
    [
      {
        args: '["value"]',
        call: 2,
        index: 0,
        type: 'bobcat-native-module-callback',
      },
    ],
  )

  await canvas.dispose()
})

test('a canvas without native modules declares an empty table', async () => {
  const { canvas, worker } = await createCanvas()
  const init = worker.messages.find(({ type }) => type === 'bobcat-init')!
  assert.deepEqual(init.nativeModules, {})
  await canvas.dispose()
})

test('create refuses a module member that is not a function', async () => {
  FakeWorker.instances.length = 0
  await assert.rejects(
    BobcatCanvas.create(
      new FakeCanvas() as unknown as HTMLCanvasElement,
      100,
      100,
      1,
      LYNX_XML_PAGE_CONFIG,
      {
        nativeModules: {
          ExplorerModule: { openSchema: 'not a function' },
        } as unknown as Record<
          string,
          Record<string, (...args: unknown[]) => void>
        >,
      },
    ),
    (error: unknown) =>
      error instanceof TypeError &&
      /nativeModules\.ExplorerModule\.openSchema must be a function/u.test(
        error.message,
      ),
  )
  assert.equal(
    FakeWorker.instances.length,
    0,
    'the table is checked before the Worker exists',
  )
})

test('a load carries globalProps as JSON text, and refuses what JSON cannot take', async () => {
  const { canvas, worker } = await createCanvas()

  await canvas.loadTemplate('card.web.bundle', {
    globalProps: { screenWidth: 390, theme: 'Dark' },
  })
  await canvas.loadTemplate('plain.web.bundle')

  assert.deepEqual(
    worker.messages
      .filter(({ type }) => type === 'bobcat-request')
      .map(({ args, ...message }) => ({
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
