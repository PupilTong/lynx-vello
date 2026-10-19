import assert from 'node:assert/strict'
import test from 'node:test'

import type {
  EditCommand,
  KeyFields,
  TextInputFields,
} from '../js/protocol.d.ts'
import {
  CompositionTracker,
  TextInputProxy,
  beforeInputCommand,
  engineHandlesKey,
  inputModeFor,
  keyFields,
  proxyBox,
} from '../js/text-input.ts'

type Listener = (event: FakeEvent) => void

interface FakeEvent {
  data?: string | null
  dataTransfer?: { getData: (type: string) => string } | null
  inputType?: string
  isComposing?: boolean
  preventDefault: () => void
  [member: string]: unknown
}

class FakeTarget {
  #listeners = new Map<string, Set<Listener>>()

  addEventListener(name: string, listener: Listener): void {
    let listeners = this.#listeners.get(name)
    if (listeners === undefined) {
      listeners = new Set()
      this.#listeners.set(name, listeners)
    }
    listeners.add(listener)
  }

  removeEventListener(name: string, listener: Listener): void {
    this.#listeners.get(name)?.delete(listener)
  }

  listenerCount(): number {
    return [...this.#listeners.values()].reduce(
      (count, listeners) => count + listeners.size,
      0,
    )
  }

  /** Dispatches one event and answers whether a listener prevented it. */
  emit(name: string, fields: Record<string, unknown> = {}): boolean {
    let prevented = false
    const event: FakeEvent = {
      preventDefault: () => {
        prevented = true
      },
      type: name,
      ...fields,
    }
    for (const listener of this.#listeners.get(name) ?? []) {
      listener(event)
    }
    return prevented
  }
}

class FakeDocument extends FakeTarget {
  activeElement: unknown = null
}

class FakeTextarea extends FakeTarget {
  attributes = new Map<string, string>()
  focusCount = 0
  ownerDocument: FakeDocument
  removed = false
  selectionEnd = 0
  selectionStart = 0
  style: {
    cssText?: string
    display?: string
    height?: string
    left?: string
    lineHeight?: string
    top?: string
    width?: string
  } = {}
  value = ''

  constructor(document: FakeDocument) {
    super()
    this.ownerDocument = document
  }

  blur(): void {
    if (this.ownerDocument.activeElement === this) {
      this.ownerDocument.activeElement = null
      this.emit('blur')
    }
  }

  focus(): void {
    this.focusCount += 1
    this.ownerDocument.activeElement = this
  }

  remove(): void {
    this.removed = true
  }

  removeAttribute(name: string): void {
    this.attributes.delete(name)
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value)
  }

  setSelectionRange(start: number, end: number): void {
    this.selectionStart = start
    this.selectionEnd = end
  }
}

const KEY: Omit<
  Parameters<typeof keyFields>[0],
  'key' | 'type'
> = {
  altKey: false,
  code: '',
  ctrlKey: false,
  metaKey: false,
  repeat: false,
  shiftKey: false,
}

function state(fields: Partial<TextInputFields> = {}): TextInputFields {
  return {
    caret: { height: 20, width: 1, x: 40, y: 10 },
    composition: null,
    compositionBounds: [],
    editableBounds: { height: 40, width: 200, x: 0, y: 0 },
    enterKeyHint: '',
    host: true,
    inputMode: '',
    inputType: 'text',
    selectionEnd: 0,
    selectionStart: 0,
    value: '',
    ...fields,
  }
}

const NOTHING_FOCUSED = state({
  caret: { height: 0, width: 0, x: 0, y: 0 },
  host: false,
})

function proxy() {
  const document = new FakeDocument()
  const edits: EditCommand[] = []
  const keys: KeyFields[] = []
  let canvasFocused = 0
  let textarea: FakeTextarea | undefined
  const input = new TextInputProxy({
    canvasBounds: () => ({ height: 100, left: 10, top: 20, width: 200 }),
    createTextarea: () => {
      textarea = new FakeTextarea(document)
      return textarea as unknown as HTMLTextAreaElement
    },
    focusCanvas: () => {
      canvasFocused += 1
    },
    sendEdit: (command) => edits.push(command),
    sendKey: (fields) => keys.push(fields),
    viewport: () => ({ height: 200, width: 400 }),
  })
  return {
    canvasFocused: () => canvasFocused,
    document,
    edits,
    input,
    keys,
    textarea: () => {
      assert.ok(textarea !== undefined, 'the proxy made its textarea')
      return textarea
    },
  }
}

test('a key the input method is composing with crosses as Process', () => {
  assert.deepEqual(
    keyFields({ ...KEY, key: 'a', shiftKey: true, metaKey: true, type: 'keydown' }),
    { code: '', down: true, key: 'a', modifiers: 12, repeat: false },
  )
  assert.equal(
    keyFields({ ...KEY, isComposing: true, key: 'ArrowLeft', type: 'keydown' }).key,
    'Process',
  )
  assert.equal(
    keyFields({ ...KEY, key: 'Enter', keyCode: 229, type: 'keydown' }).key,
    'Process',
  )
  assert.equal(keyFields({ ...KEY, key: 'x', type: 'keyup' }).down, false)
})

test('the engine handles movement and select-all, the proxy everything that edits', () => {
  const fields = (key: string, modifiers = 0): KeyFields => ({
    code: '',
    down: true,
    key,
    modifiers,
    repeat: false,
  })
  for (const key of ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End']) {
    assert.equal(engineHandlesKey(fields(key)), true, key)
    assert.equal(engineHandlesKey(fields(key, 8)), true, `shift-${key}`)
  }
  assert.equal(engineHandlesKey(fields('a', 2)), true, 'ctrl-A')
  assert.equal(engineHandlesKey(fields('A', 4)), true, 'meta-A')
  for (const key of ['a', 'Backspace', 'Delete', 'Enter', 'Tab', 'Process', 'v']) {
    assert.equal(engineHandlesKey(fields(key)), false, key)
  }
  assert.equal(engineHandlesKey(fields('v', 4)), false, 'paste is the proxy’s')
})

test('each beforeinput outside a composition is one command or none', () => {
  assert.deepEqual(beforeInputCommand('insertText', 'x', false), {
    kind: 'insert',
    text: 'x',
  })
  assert.deepEqual(beforeInputCommand('insertFromPaste', 'a\nb', true), {
    kind: 'insert',
    text: 'a\nb',
  })
  assert.equal(beforeInputCommand('insertText', null, false), undefined)
  assert.equal(beforeInputCommand('insertFromPaste', '', false), undefined)
  for (const inputType of ['insertLineBreak', 'insertParagraph']) {
    assert.deepEqual(beforeInputCommand(inputType, null, false), {
      kind: 'insertLineBreak',
    })
  }
  for (const [inputType, kind] of [
    ['deleteContentBackward', 'deleteBackward'],
    ['deleteContentForward', 'deleteForward'],
    ['deleteWordBackward', 'deleteWordBackward'],
    ['deleteWordForward', 'deleteWordForward'],
  ] as const) {
    assert.deepEqual(beforeInputCommand(inputType, null, false), { kind })
  }
  assert.deepEqual(beforeInputCommand('deleteByCut', null, true), {
    kind: 'deleteBackward',
  })
  assert.equal(beforeInputCommand('deleteByCut', null, false), undefined)
  for (const inputType of [
    'historyUndo',
    'historyRedo',
    'insertReplacementText',
    'insertFromDrop',
    'deleteByDrag',
    'deleteSoftLineBackward',
    'insertTranspose',
    'insertCompositionText',
  ]) {
    assert.equal(beforeInputCommand(inputType, 'x', true), undefined, inputType)
  }
})

test('a composition is its updates with the selection the input event shows, then one commit', () => {
  const composition = new CompositionTracker()
  assert.equal(composition.input(0, 0), undefined, 'nothing before a start')
  composition.start(2)
  assert.equal(composition.composing, true)
  composition.update('k')
  assert.deepEqual(composition.input(3, 3), {
    kind: 'setComposition',
    selectionEnd: 1,
    selectionStart: 1,
    text: 'k',
  })
  composition.update('か')
  assert.deepEqual(composition.input(3, 3), {
    kind: 'setComposition',
    selectionEnd: 1,
    selectionStart: 1,
    text: 'か',
  })
  assert.equal(composition.input(3, 3), undefined, 'no update, nothing to send')
  composition.update('か')
  assert.equal(composition.input(3, 3), undefined, 'an equal update is not sent again')
  composition.update('かな')
  assert.deepEqual(
    composition.input(2, 9),
    {
      kind: 'setComposition',
      selectionEnd: 2,
      selectionStart: 0,
      text: 'かな',
    },
    'the selection is clamped into the composition',
  )
  assert.deepEqual(composition.end('仮名'), {
    kind: 'commitText',
    relativeCursor: 0,
    text: '仮名',
  }, 'a commit that differs from the last update')
  assert.equal(composition.composing, false)
  assert.equal(composition.end('x'), undefined, 'one end per composition')
  assert.equal(composition.blur(), undefined)

  // Chromium's commit: the final update and its input, then the end with
  // the same text, which the engine already holds.
  composition.start(0)
  composition.update('日本')
  composition.input(2, 2)
  assert.deepEqual(composition.end('日本'), {
    keepSelection: false,
    kind: 'finishComposition',
  })
  // An update whose input never came is committed.
  composition.start(0)
  composition.update('a')
  composition.input(1, 1)
  composition.update('ab')
  assert.deepEqual(composition.end('ab'), {
    kind: 'commitText',
    relativeCursor: 0,
    text: 'ab',
  })

  composition.start(0)
  composition.update('a')
  assert.deepEqual(composition.blur(), {
    keepSelection: false,
    kind: 'finishComposition',
  })
  composition.start(0)
  composition.forget()
  assert.equal(composition.end('a'), undefined, 'a forgotten composition commits nothing')
})

test('the proxy sits over the caret, mapped through the canvas box', () => {
  assert.deepEqual(
    proxyBox(state(), { height: 100, left: 10, top: 20, width: 200 }, 400, 200),
    { height: 10, left: 30, top: 25, width: 1 },
  )
  assert.equal(
    proxyBox(state(), { height: 0, left: 0, top: 0, width: 0 }, 400, 200),
    undefined,
  )
  assert.equal(inputModeFor(state({ inputMode: 'numeric' })), 'numeric')
  assert.equal(inputModeFor(state({ inputType: 'email' })), 'email')
  assert.equal(inputModeFor(state({ inputType: 'number' })), 'decimal')
  assert.equal(inputModeFor(state({ inputType: 'password' })), '')
  assert.equal(inputModeFor(state()), '')
})

test('a focused host shows, focuses and mirrors the proxy; a blur hides it', () => {
  const { canvasFocused, document, input, textarea } = proxy()
  assert.equal(input.textarea, undefined, 'no textarea before a host is focused')
  input.update(NOTHING_FOCUSED)
  assert.equal(input.textarea, undefined)

  input.update(
    state({
      enterKeyHint: 'send',
      inputMode: 'numeric',
      selectionEnd: 3,
      selectionStart: 1,
      value: 'abc',
    }),
  )
  const area = textarea()
  assert.equal(document.activeElement, area)
  assert.equal(area.value, 'abc')
  assert.deepEqual([area.selectionStart, area.selectionEnd], [1, 3])
  assert.equal(area.style.display, 'block')
  assert.equal(area.style.left, '30px')
  assert.equal(area.style.top, '25px')
  assert.equal(area.attributes.get('inputmode'), 'numeric')
  assert.equal(area.attributes.get('enterkeyhint'), 'send')
  assert.equal(area.attributes.get('spellcheck'), 'false')
  assert.equal(area.attributes.get('autocapitalize'), 'off')
  assert.equal(input.holdsFocus, true)

  input.update(state({ value: 'abc' }))
  assert.equal(area.attributes.has('inputmode'), false)
  assert.equal(area.attributes.has('enterkeyhint'), false)

  input.update(NOTHING_FOCUSED)
  assert.equal(document.activeElement, null)
  assert.equal(area.style.display, 'none')
  assert.equal(canvasFocused(), 1, 'focus goes back to the canvas')
  assert.equal(input.holdsFocus, false)

  input.dispose()
  assert.equal(area.removed, true)
  assert.equal(area.listenerCount(), 0)
  assert.equal(document.listenerCount(), 0)
})

test('the proxy prevents every beforeinput and sends what it stands for', () => {
  const { edits, input, textarea } = proxy()
  input.update(state({ selectionEnd: 1, selectionStart: 1, value: 'a' }))
  const area = textarea()
  assert.equal(area.emit('beforeinput', { data: 'b', inputType: 'insertText' }), true)
  assert.equal(
    area.emit('beforeinput', { data: null, inputType: 'deleteContentBackward' }),
    true,
  )
  assert.equal(area.emit('beforeinput', { data: null, inputType: 'historyUndo' }), true)
  assert.equal(
    area.emit('beforeinput', {
      data: null,
      dataTransfer: { getData: () => 'pasted' },
      inputType: 'insertFromPaste',
    }),
    true,
  )
  assert.deepEqual(edits, [
    { kind: 'insert', text: 'b' },
    { kind: 'deleteBackward' },
    { kind: 'insert', text: 'pasted' },
  ])
  assert.equal(area.value, 'a', 'the textarea holds only what the engine published')
})

test('a composition reaches the engine as updates and a commit, unmirrored while it lasts', () => {
  const { edits, input, textarea } = proxy()
  input.update(state({ selectionEnd: 2, selectionStart: 2, value: 'ab' }))
  const area = textarea()
  area.emit('compositionstart', { data: '' })
  area.emit('compositionupdate', { data: 'n' })
  assert.equal(
    area.emit('beforeinput', {
      data: 'n',
      inputType: 'insertCompositionText',
      isComposing: true,
    }),
    false,
    'a composition’s own input is the browser’s',
  )
  // What the browser did to the textarea meanwhile.
  area.value = 'abn'
  area.setSelectionRange(3, 3)
  area.emit('input', { inputType: 'insertCompositionText', isComposing: true })
  // A state for the composition arrives while it lasts: not mirrored.
  input.update(
    state({
      composition: { end: 3, start: 2 },
      selectionEnd: 3,
      selectionStart: 3,
      value: 'abn',
    }),
  )
  area.emit('compositionupdate', { data: 'に' })
  area.value = 'abに'
  area.emit('input', { inputType: 'insertCompositionText', isComposing: true })
  input.update(state({ selectionEnd: 0, selectionStart: 0, value: 'stale' }))
  assert.equal(area.value, 'abに', 'nothing is written into a composition')
  area.emit('compositionend', { data: '二' })
  assert.deepEqual(edits, [
    { kind: 'setComposition', selectionEnd: 1, selectionStart: 1, text: 'n' },
    { kind: 'setComposition', selectionEnd: 1, selectionStart: 1, text: 'に' },
    { kind: 'commitText', relativeCursor: 0, text: '二' },
  ])
  input.update(state({ selectionEnd: 3, selectionStart: 3, value: 'ab二' }))
  assert.equal(area.value, 'ab二', 'mirrored again once it ended')
})

test('keys go to the engine, and the ones it moves the selection for are prevented here', () => {
  const { input, keys, textarea } = proxy()
  input.update(state({ value: 'ab' }))
  const area = textarea()
  assert.equal(area.emit('keydown', { ...KEY, key: 'ArrowLeft' }), true)
  assert.equal(area.emit('keydown', { ...KEY, key: 'Backspace' }), false)
  assert.equal(
    area.emit('keydown', { ...KEY, isComposing: true, key: 'ArrowLeft' }),
    false,
    'a composition’s arrow is the input method’s',
  )
  assert.equal(area.emit('keyup', { ...KEY, key: 'ArrowLeft' }), false)
  assert.deepEqual(
    keys.map(({ down, key }) => `${down ? 'down' : 'up'}:${key}`),
    ['down:ArrowLeft', 'down:Backspace', 'down:Process', 'up:ArrowLeft'],
  )
})

test('leaving the proxy finishes an open composition and keeps the engine’s focus', () => {
  const { document, edits, input, textarea } = proxy()
  input.update(state())
  const area = textarea()
  area.emit('compositionstart', { data: '' })
  area.blur()
  assert.deepEqual(edits, [{ keepSelection: false, kind: 'finishComposition' }])
  assert.equal(document.activeElement, null)
  assert.equal(input.holdsFocus, false)

  // A later state does not take focus back; a press on the canvas does.
  input.update(state({ value: 'x' }))
  assert.equal(document.activeElement, null)
  input.press()
  assert.equal(document.activeElement, area)
})

test('the engine’s blur forgets a composition the browser still holds', () => {
  const { edits, input, textarea } = proxy()
  input.update(state())
  const area = textarea()
  area.emit('compositionstart', { data: '' })
  input.update(NOTHING_FOCUSED)
  area.emit('compositionend', { data: 'late' })
  assert.deepEqual(edits, [], 'nothing is committed into the next host')
})

test('a press while the proxy holds focus focuses nothing again', () => {
  const { input, textarea } = proxy()
  input.update(state())
  const area = textarea()
  assert.equal(area.focusCount, 1)
  input.press()
  input.press()
  assert.equal(area.focusCount, 1)
})
